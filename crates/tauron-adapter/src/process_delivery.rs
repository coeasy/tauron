// 进程插件投递实现（0.4-A1：与 Js 投递一并打通的 Process 形态）。
//
// 与 Js 投递（`JsCallDelivery`，经事件总线 request 通道）不同，进程插件把调用写成
// JSON-RPC 行帧写进 sidecar 的 **stdin**；sidecar 处理后把回帧（含 `callId`）写回
// **stdout**，`CommandSpawner` 的读线程把回帧路由到 `ProcessFrameSinkImpl`，最终
// 调 `Registry::settle_call` 闭合链路。
//
// **运行期证据（轮 22 / V7-P1-05）**：`crates/tauron-test-sidecar` 提供仓内可执行的
// sidecar 夹具（`publish = false`，永不上架），其 `tests/sidecar_e2e.rs` 真起进程、
// 真走 stdio 帧回路，并且**走的就是本文件的投递/结算面**——"sidecar 真的收到帧 /
// 真的回帧 / 宿主真的据此结算"已不再是只有契约与格式级的推导。
// 那份 E2E 同时覆盖了真崩溃计数、超长帧、旧代际回帧、EOF 回收、pending 上限与
// 静默不回帧的 sidecar。

use std::sync::Arc;

use serde_json::Value;
use tauron_host::{
    call_delivery::{CallDelivery, CallOutcome, DeliveryKind, DeliveryReceipt},
    generation::Generation,
    registry::Registry,
    runtime::RuntimeHandle,
    ErrorCode, HostError, HostResult,
};
use tauron_proc::ProcessFrameSink;

use crate::ProcRuntime;

/// 进程插件投递器：把一次 pending call 写成 JSON-RPC 行帧写入 sidecar stdin。
///
/// 未运行（`live_pid_of` 拿不到 pid）= 没有运行时不投递，诚实返回"未投递"；
/// 上层 `deliver_call` 据此转成 `Unsupported`（`E_PLUGIN_TYPE_NO_RUNTIME` 的诚实语义）。
pub struct ProcessCallDelivery {
    proc_runtime: Arc<ProcRuntime>,
    registry: Arc<Registry>,
}

impl ProcessCallDelivery {
    pub fn new(proc_runtime: Arc<ProcRuntime>, registry: Arc<Registry>) -> Self {
        Self { proc_runtime, registry }
    }

    fn runtime_for_call(
        &self,
        call: &tauron_host::PendingCall,
    ) -> HostResult<Option<RuntimeHandle>> {
        let Some(expected) = call.runtime_generation else {
            return Ok(None);
        };
        let current = self.registry.live_runtime_handle_of(&call.target).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_LEASE_EXPIRED,
                format!(
                    "process call {} targets generation {} but no live runtime exists",
                    call.call_id, expected.0
                ),
            )
        })?;
        if current.generation != expected {
            return Err(stale_process_call(
                &call.call_id,
                expected,
                Some(&current),
                "runtime generation changed before delivery/settlement",
            ));
        }
        Ok(Some(current))
    }
}

fn stale_process_call(
    call_id: &str,
    expected: Generation,
    current: Option<&RuntimeHandle>,
    reason: &str,
) -> HostError {
    let current = current
        .map(|handle| format!("pid={} generation={}", handle.pid, handle.generation.0))
        .unwrap_or_else(|| "none".to_string());
    HostError::new(
        ErrorCode::E_LEASE_EXPIRED,
        format!(
            "process call {call_id} is bound to generation {} but current runtime is {current}: {reason}",
            expected.0
        ),
    )
}

/// 把一次 stdin 写入失败映射成**调用方可分流**的错误码（轮 30）。
///
/// 写侧有四类完全不同的事实（见 `CommandSpawner::write_frame` 的四种如实失败），
/// 从前这里一律塌成 `E_STATE_INVALID_TRANSITION`，后果不是"码不好看"而是**可执行的
/// 误判**：
///
/// - 那个码的语义是「状态机没有匹配的规则」——而这里状态机**放行了**（调用已经进
///   pending 表），只是写侧落空。把它报给调用方，等于把人往"检查插件状态/改调用
///   序列"上支，而正确处置是"把在飞的调用取件腾额度"或"重新 spawn 运行期"；
/// - sidecar 已退出 / 管道已断（`NotFound`/`BrokenPipe`）本质是**租约没了**，与
///   [`stale_process_call`] 同一个事实，必须同一个码：`E_LEASE_EXPIRED` 的 retry
///   class 是 `AfterReconnect`，塌成非法状态迁移就把这条机器可读的"先重连再试"信号
///   整个丢掉了；
/// - 写队列满（`WouldBlock`）与 pending 表满对调用方是**同一件事**——"这个目标的受理
///   额度满了，退避后重投"，所以同码 `E_CALL_PENDING_FULL`。V4 刻意不给这两个码
///   自动重试权（`RetryClass::Never`：幂等性未被证明前不自动重放），这里不翻案。
///
/// 帧超过协议上界（`InvalidInput`）报 `E_INVALID_MANIFEST`：它是仓内既有的
/// 「入参不合协议」码（`host_updater_check` 的空 `current_version`、
/// `host_recover_report` 的表外 `outcome` 都用它），比"状态迁移非法"更贴近事实。
///
/// 两种容量失败的**出处**都留在 message 里（写队列上界与 pending 表上界是两个不同
/// 的界，排查时必须能分辨），码只承载"该怎么处置"。
fn stdin_write_failure(pid: u32, e: &std::io::Error) -> HostError {
    let kind = e.kind();
    let (code, advice) = match kind {
        std::io::ErrorKind::WouldBlock => (
            ErrorCode::E_CALL_PENDING_FULL,
            "sidecar 写队列已满：这次调用的额度已被占满，退避后重投即可（与 pending 表满同一处置）",
        ),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::BrokenPipe => (
            ErrorCode::E_LEASE_EXPIRED,
            "该 pid 的 stdin 通路已不存在：需要重新 host_runtime_spawn，而不是重投这一帧",
        ),
        _ => (
            ErrorCode::E_INVALID_MANIFEST,
            "这一帧无法成形投递（超过协议上界或入参不可序列化）：修正调用参数，重试无用",
        ),
    };
    HostError::new(
        code,
        format!("写入 sidecar stdin 失败（pid {pid}，kind {kind:?}）：{e}；处置：{advice}"),
    )
}

impl CallDelivery for ProcessCallDelivery {
    fn target_kind(&self) -> DeliveryKind {
        DeliveryKind::Process
    }

    fn deliver(&self, call: &tauron_host::PendingCall) -> HostResult<DeliveryReceipt> {
        // Process calls accepted before a runtime exists remain honestly undeliverable.
        let runtime = match self.runtime_for_call(call)? {
            Some(runtime) => runtime,
            None => {
                return Ok(DeliveryReceipt {
                    delivered: false,
                    reason: Some(format!(
                        "插件 `{}` 没有绑定可投递的 runtime generation",
                        call.target
                    )),
                })
            }
        };
        let pid = runtime.pid;

        // 构造 JSON-RPC 行帧：`callId` 内嵌，供回帧关联（无需 id↔call_id 映射表）；
        // `id` 用 `seq`（单调唯一）供 sidecar 自身做请求/响应配对；`caller`/`target`
        // 与 Js 投递载荷对齐（帧上自带权威字段，sidecar 日志/鉴权可直接用）。
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": call.seq,
            "method": call.cmd,
            "params": call.args,
            "callId": call.call_id,
            "caller": call.caller,
            "target": call.target,
            "runtimeGeneration": runtime.generation,
        });
        let bytes = serde_json::to_vec(&frame).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("序列化进程调用帧失败（这次调用无法成形）：{e}"),
            )
        })?;

        match self.proc_runtime.spawner().write_frame(pid, &bytes) {
            Ok(()) => Ok(DeliveryReceipt { delivered: true, reason: None }),
            // 启动器根本没有「写 sidecar stdin」这条通路：这不是失败，是**未装配投递**——
            // 与上面「没有 runtime 不投递」同一个形状，上层据此转成 `Unsupported`，
            // 让调用方看见「没有通路」而不是拿到一个误导性的错误码。
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Ok(DeliveryReceipt {
                delivered: false,
                reason: Some(format!("该启动器不支持写入 sidecar stdin（pid {pid}）：{e}")),
            }),
            Err(e) => Err(stdin_write_failure(pid, &e)),
        }
    }

    fn settle(&self, call_id: &str, outcome: CallOutcome) -> HostResult<tauron_host::PendingCall> {
        // Explicit settlement must prove the call still belongs to the active target generation.
        let call = self.registry.peek_call(call_id)?;
        let _ = self.runtime_for_call(&call)?;
        self.registry.settle_call(call_id, outcome)
    }
}

/// sidecar stdout 回帧接收器：解析 JSON-RPC 响应，提取 `callId` 并 `settle_call`。
///
/// 由 `cmd_runtime_spawn` 在真起进程后注册到该 pid；`CommandSpawner` 读线程每收到
/// 一行帧就调用一次。幂等：call 已结算时 `settle_call` 报 `E_CALL_ALREADY_SETTLED`，
/// 忽略即可（sidecar 可能因重传/超时而重复回帧）。
pub struct ProcessFrameSinkImpl {
    registry: Arc<Registry>,
}

impl ProcessFrameSinkImpl {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry }
    }
}

impl ProcessFrameSink for ProcessFrameSinkImpl {
    fn on_frame(&self, pid: u32, frame: &[u8]) {
        let v: Value = match serde_json::from_slice(frame) {
            Ok(v) => v,
            Err(_) => return, // 非 JSON / 污染行：跳过
        };
        let call_id = match v.get("callId").and_then(|c| c.as_str()) {
            Some(c) => c.to_string(),
            None => return, // 没有 callId：无法关联，丢弃
        };

        let call = match self.registry.peek_call(&call_id) {
            Ok(call) => call,
            Err(_) => return,
        };
        let Some(expected) = call.runtime_generation else {
            eprintln!("[tauron] sidecar 回帧拒绝：callId={call_id} 没有绑定 runtimeGeneration");
            return;
        };
        let current = self.registry.live_runtime_handle_of(&call.target);
        let valid_source = current
            .as_ref()
            .is_some_and(|handle| handle.pid == pid && handle.generation == expected);
        if !valid_source {
            let err = stale_process_call(
                &call_id,
                expected,
                current.as_ref(),
                "reply pid/generation does not match the call binding",
            );
            eprintln!("[tauron] sidecar 回帧拒绝：{} — {}", err.code, err.message);
            return;
        }

        // `error` 字段存在 = 调用失败（`ok = false`）。
        let error = v.get("error");
        let ok = error.is_none();
        let result = v.get("result").cloned();
        // error_code 优先取 `error.code`（数字转字符串），否则取 `error.message`。
        let error_code = error
            .and_then(|e| e.get("code").and_then(|c| c.as_i64()))
            .map(|c| c.to_string())
            .or_else(|| {
                error.and_then(|e| e.get("message")).and_then(|m| m.as_str()).map(|s| s.to_string())
            });

        // 结算。**幂等**：重复回帧（sidecar 重发 / 宿主已按 TTL 回收该 pending）
        // 返回 `E_CALL_NOT_FOUND` / `E_CALL_ALREADY_SETTLED`——那是预期路径，静默。
        //
        // 1.0-W7（P2-1）：其余失败**必须留痕**。此前这里是 `let _ =`，把
        // `settle_call` 的**全部**错误一并吞掉——包括非幂等的内部失败，
        // 结果通道出问题时既无计数也无日志，故障点被彻底淹没。
        if let Err(err) =
            self.registry.settle_call(&call_id, CallOutcome { ok, result, error_code })
        {
            if !matches!(err.code, ErrorCode::E_CALL_NOT_FOUND | ErrorCode::E_CALL_ALREADY_SETTLED)
            {
                eprintln!(
                    "[tauron] sidecar 回帧结算失败（callId={call_id}）：{} — {}",
                    err.code, err.message
                );
            }
        }
    }
}
