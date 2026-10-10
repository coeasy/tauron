// 本模块是 Tauron 宿主适配器「插件调用 + 流式帧」两条命令族（call / stream）的按原名
// **逐字节纯搬移**（T-7 十三→十四片）。
//
// 搬运动机：把 `lib.rs` 里一整段连续区间（旧 5732–6017 行）的调用命令族与流式命令族
// 一并顶格迁出——`cmd_plugin_call`/`cmd_call_plugin`/`cmd_call_plugin_with_parent`/
// `cmd_call_result`/`cmd_call_take`/`cmd_call_end`/`cmd_cancel`（pending call 生命周期）
// + `cmd_stream_open`/`cmd_stream_write`/`cmd_stream_grant`/`cmd_stream_close`（流式帧
// R5/P0-1）两条族共 11 条 `pub fn`，连同它们的同族私助手 `call_owner_key`（A 口径只数
// pub，此私函数不入台账、随族搬）与两条返回结构体 `StreamOpened`/`StreamCredit`。
// lib.rs 通过文件尾 `pub use call_stream::{…}` 按原名再导出全部 pub，保 `crate::cmd_*`
// 由 tauri.rs 接线、`crate::StreamOpened`/`crate::StreamCredit` 由 tauri.rs 返回体解析、
// 以及 lib.rs 内联测试裸名调用**全部**不变。
//
// 两族刻意合并成**一个文件**：它们在 lib.rs 里本就物理相邻（调用族 5732–5931 紧接
// 流式帧 5933–6017），且语义同源——流式帧寄生在一次「已挂帧载体」的 pending call 上，
// `stream_open` 的 `E_CALL_NOT_FOUND` 判据直接依赖调用侧先带 channel 调过 `plugin_call`。
// 拆成两个文件反而要在 lib.rs 制造两处非相邻豁口，违背「每片一段连续纯搬移」的低风险纪律。
//
// 因此本片是**纯搬移，零语义变更**：可见性、函数体、doc 注释、区间内那条「流式帧」区块
// 注释之外的一切均逐字保留；`use super::*;` 让搬来的命令看到 crate 根私有/导入项
// （`PluginRuntimeState`/`Caller`/`PendingCall`/`CallOutcome`/`StreamFrame`/`StreamKind`
// 类型，`guard` 兜闭包、`ProviderResult`/`HostResult`/`HostError`/`ErrorCode`、以及
// `state.deliver_call`/`state.settle_call_for`/`state.registry.call_*`/`state.registry.stream_*`
// 等方法与字段），无需 `pub(crate)` 扩权，也无需改任何调用点。

use super::*;

/// `host_plugin_call`：插件调用自己的 C/D 后端（self 档）。
pub fn cmd_plugin_call(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    cmd: &str,
    args: serde_json::Value,
) -> HostResult<PendingCall> {
    guard("plugin_call", || {
        let id = tauron_host::authz::resolve_self_identity(webview_label, claimed_id)?;
        let call = state.registry.call_begin(&id, cmd, args)?;
        // A1 投递：把请求送到真正能执行它的一端（Js → 事件总线 request 通道）。
        // 未装配投递（Rust/Wasm 在 A4 之前、或 Process 在 A3 之前）落到 unwired，
        // 返回诚实的 `E_PLUGIN_TYPE_NO_RUNTIME` 而非假装成功。
        match state.deliver_call(&call)? {
            ProviderResult::Value(_) => Ok(call),
            ProviderResult::Unsupported(b) => {
                Err(HostError::new(ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME, b.reason))
            }
        }
    })?
}

/// `host_call_plugin`：跨主体调用（0.4-A1）。宿主主窗与插件→插件共用。
///
/// `caller` 是发起主体（`"main"` 或插件 id），**配额记在它名下**；`target` 是执行主体。
/// 返回 [`ProviderResult`]：`Value(call)` = 已投递待应答；`Unsupported` = 未装配投递
/// （无通路）——不假装成功，前端据此读到 `reason` 而不是拿到一个永不结束的 pending。
pub fn cmd_call_plugin(
    state: &PluginRuntimeState,
    caller: &str,
    target: &str,
    cmd: &str,
    args: serde_json::Value,
) -> HostResult<ProviderResult<PendingCall>> {
    cmd_call_plugin_with_parent(state, caller, target, cmd, args, None)
}

/// V4 A77 contextual cross-principal call. Legacy callers omit parent_call_id and become roots.
pub fn cmd_call_plugin_with_parent(
    state: &PluginRuntimeState,
    caller: &str,
    target: &str,
    cmd: &str,
    args: serde_json::Value,
    parent_call_id: Option<&str>,
) -> HostResult<ProviderResult<PendingCall>> {
    guard("call_plugin", || {
        let call = state.registry.call_begin_cross_with_parent(
            caller,
            target,
            caller,
            cmd,
            args,
            parent_call_id,
        )?;
        match state.deliver_call(&call) {
            Ok(ProviderResult::Value(value)) => Ok(ProviderResult::Value(value)),
            Ok(ProviderResult::Unsupported(body)) => {
                let _ = state.registry.call_cancel(&call.call_id);
                Ok(ProviderResult::Unsupported(body))
            }
            Err(error) => {
                let _ = state.registry.call_cancel(&call.call_id);
                Err(error)
            }
        }
    })?
}

/// `host_call_result`：执行方回填一次调用的结果（0.4-A1 的结算入口）。
///
/// 只有执行方（call.target）能回填：身份必须 == target，否则伪造结果被拒。
/// 结算后条目**保留**，等待发起方 [`cmd_call_take`] 取走。
pub fn cmd_call_result(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    call_id: &str,
    ok: bool,
    result: Option<serde_json::Value>,
    error_code: Option<String>,
) -> HostResult<PendingCall> {
    guard("call_result", || {
        let id = tauron_host::authz::resolve_self_identity(webview_label, claimed_id)?;
        // 只有执行方能回填：身份必须 == 该 call 的 target。
        let target = state.registry.call_target(call_id)?;
        if target != id.as_str() {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "插件 `{id}` 不是调用 `{call_id}` 的执行方（target=`{target}`），不得回填结果"
                ),
            ));
        }
        state.settle_call_for(&target, call_id, CallOutcome { ok, result, error_code })
    })?
}

/// `host_call_take`：发起方取走一次已结算的结果（0.4-A1 的回执取件）。
///
/// 只有发起方（call.caller）能取：插件身份必须 == caller，或主窗取自己发起的调用。
/// `Settled` 取走即删；`Pending` 返回副本、保留（调用方据此知道「还没好」）。
pub fn cmd_call_take(
    state: &PluginRuntimeState,
    webview_label: &str,
    // `claimed_id` 故意不使用：self 档命令的身份只从 webview label 派生
    // （防冒充），与 `host_call_result` 不同——后者用 `claimed_id` 是因为
    // `resolve_self_identity` 同时需要 label 与 claimed 来做插件身份校验。
    // 这里 caller 既可能是插件也可能是主窗，直接用 `resolve_principal` 即可。
    _claimed_id: Option<&str>,
    call_id: &str,
) -> HostResult<PendingCall> {
    guard("call_take", || {
        let principal = tauron_host::authz::resolve_principal(webview_label);
        let caller = match &principal {
            tauron_host::authz::Principal::Plugin(id) => id.as_str().to_string(),
            tauron_host::authz::Principal::MainWindow => "main".to_string(),
            tauron_host::authz::Principal::Invalid(_) => {
                return Err(HostError::new(
                    ErrorCode::E_AUTH_DENIED,
                    "非法身份主体，不得取走调用结果",
                ));
            }
        };
        // 只有发起方能取：caller 必须 == 该 call 的 caller。
        let pending = state.registry.peek_call(call_id)?;
        if pending.caller != caller {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "主体 `{caller}` 不是调用 `{call_id}` 的发起方（caller=`{}`），不得取走结果",
                    pending.caller
                ),
            ));
        }
        state.registry.take_call(call_id)
    })?
}

/// 调用归属键：pending 条目的 `plugin_id`（= 配额归属者 = 发起主体）口径。
///
/// 与 `cmd_call_take` 里的 caller 映射同源：插件 → 自己的 id，主窗 → `"main"`。
/// self 档命令的身份一律从 webview label 派生，不认调用方传入的身份字段。
fn call_owner_key(caller: &Caller) -> &str {
    match caller {
        Caller::Plugin(id) => id.as_str(),
        Caller::MainWindow => "main",
    }
}

/// `host_call_end`：stream 终帧确认（self 档）。
///
/// **归属校验（本轮补）**：只有该调用的归属主体（`pending.plugin_id`，即发起方）
/// 能结束它。此前本命令只收 `callId`、不做任何身份判定——任何插件一旦拿到或
/// 猜到别人的 callId，就能终结对方的 pending call 及其名下全部流（跨插件越权 +
/// 拒绝服务）。这与 `COMMANDS` 表里 `host_call_end` 登记为 `self` 档（"只能作用于
/// 调用者自己"）的契约相矛盾，故补上与 [`cmd_call_take`] 同一形状的归属判定。
pub fn cmd_call_end(
    state: &PluginRuntimeState,
    caller: &Caller,
    call_id: &str,
) -> HostResult<PendingCall> {
    guard("call_end", || {
        let owner = call_owner_key(caller);
        let pending = state.registry.peek_call(call_id)?;
        if pending.plugin_id != owner {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "调用 `{call_id}` 的归属主体是 `{}`，`{owner}` 不得结束它",
                    pending.plugin_id
                ),
            ));
        }
        state.registry.call_end(call_id)
    })?
}

/// `host_cancel`：取消 pending call（self 档）。
///
/// **归属校验（本轮补）**：与 [`cmd_call_end`] 同一判据——只有发起方能取消自己的
/// 调用。缺此判定时 `host_cancel` 会成为"任意插件终止任意插件调用"的越权入口
/// （`E_AUTH_DENIED` 在**任何副作用之前**返回，被取消的调用与流纹丝不动）。
pub fn cmd_cancel(state: &PluginRuntimeState, caller: &Caller, call_id: &str) -> HostResult<()> {
    guard("cancel", || {
        let owner = call_owner_key(caller);
        let pending = state.registry.peek_call(call_id)?;
        if pending.plugin_id != owner {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "调用 `{call_id}` 的归属主体是 `{}`，`{owner}` 不得取消它",
                    pending.plugin_id
                ),
            ));
        }
        state.registry.call_cancel(call_id)
    })?
}

// ────────────────────────────────────────────────────────────────
// 流式帧（R5 / P0-1）
//
// 这三条命令是「流式」从**形状**变成**通路**的地方。在此之前：
// `plugin_call` 的 `kind: 'stream'` 会被校验、`call_end` 的注释也叫「终帧确认」，
// 但没有任何路径能把帧送出去——`channel` 只当不透明 JSON 收下，写完就丢，
// 前端表现为「回调永不触发、也不报错」。三命令 + `Registry::stream_*` 补上这条链。
// ────────────────────────────────────────────────────────────────

/// `host_stream_open` 的返回：新句柄 + 归属调用。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOpened {
    pub stream_id: String,
    pub call_id: String,
}

/// `host_stream_open`：为一次**已挂帧载体**的调用开流（self 档）。
///
/// 调用侧必须带 `channel` 调 `host_plugin_call`：没有载体的调用在这里以
/// `E_CALL_NOT_FOUND` 显式失败，而不是开一条「帧进虚空」的流（那是最难查的静默断链）。
pub fn cmd_stream_open(
    state: &PluginRuntimeState,
    subscriber: &str,
    call_id: &str,
) -> HostResult<StreamOpened> {
    guard("stream_open", || {
        let stream_id = state.registry.stream_open(call_id, subscriber)?;
        Ok(StreamOpened { stream_id, call_id: call_id.to_string() })
    })?
}

/// `host_stream_write`：写一帧 `data`（self 档）。
///
/// `seq` 由宿主铸（不接受前端传入，与 pending call 的 `seq` 同规矩）：前端自报
/// 序号就能伪造乱序/重复，接收方的去重假设随即失效。
pub fn cmd_stream_write(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    args_json: Option<serde_json::Value>,
    args_raw: Option<Vec<u8>>,
) -> HostResult<StreamFrame> {
    guard("stream_write", || {
        state.registry.stream_write(stream_id, subscriber, args_json, args_raw)
    })?
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCredit {
    pub stream_id: String,
    pub credit_bytes: usize,
}

pub fn cmd_stream_grant(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    bytes: usize,
) -> HostResult<StreamCredit> {
    guard("stream_grant", || {
        let credit_bytes = state.registry.stream_grant(stream_id, subscriber, bytes)?;
        Ok(StreamCredit { stream_id: stream_id.to_string(), credit_bytes })
    })?
}

/// `host_stream_close`：发终帧并使句柄失效（self 档）。
///
/// `kind` 只接受 `end` / `error`（闭集）：`data` 不是终帧，用它关流会让接收方
/// 以为流还在继续。
pub fn cmd_stream_close(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    kind: &str,
) -> HostResult<StreamFrame> {
    let parsed = StreamKind::parse(kind).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("未知帧种类 `{kind}`（合法值：data | end | error）"),
        )
    })?;
    guard("stream_close", || state.registry.stream_close(stream_id, subscriber, parsed))?
}
