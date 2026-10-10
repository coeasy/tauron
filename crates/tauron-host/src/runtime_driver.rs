//! 统一运行时驱动契约（`RuntimeDriver`）—— FULL-AUDIT B-1 / 独立审计 R1「统一前提」/
//! 易用方案 INT-4 的公共 seam。
//!
//! ## 为什么需要这一层
//!
//! 现状里四种插件形态各走各的投递：JS 走事件总线 request 通道
//! （[`crate::call_delivery::JsCallDelivery`]）、Process 走 sidecar stdin/stdout 帧回路
//! （`tauron-adapter` 的 `ProcessCallDelivery`）、WASM/Rust 落到 `UnwiredDelivery`
//! 诚实失败。它们**没有共享的 prepare/start/invoke/cancel/health/stop 契约**
//! （FULL-AUDIT U-4 的原文缺陷），于是"四形态"在实现上是四种各写一遍的生命周期，
//! 认知负担与并行态都高。
//!
//! 本模块把这条生命周期收敛成**一个 trait**：任何运行形态只要实现它，就能被同一套
//! 一致性用例（见 `tests` 末尾）验证——这正是 FULL-AUDIT B-5「Runtime Conformance
//! Kit」的落点基座。
//!
//! ## 诚实边界（继承本仓"绝不伪造成功"纪律，一个字不美化）
//!
//! - **这是契约基座，不是迁移本身**：现有 Js/Process/Wasm 投递**尚未**改写成
//!   `RuntimeDriver` 实现。把热路径一次性重写到新 trait 属多轮、高回归风险工程，
//!   排期在 T-6（JS）/T-10（框架层）/T-11（WASM）/T-12（Process）。本模块只做**加法**：
//!   立契约、给一个可脚本化的 [`MockRuntimeDriver`]、用一致性用例钉死契约语义。
//! - **不支持的形态是结构化事实**：驱动对不支持的 [`PluginType`] 返回
//!   `E_PLUGIN_TYPE_NO_RUNTIME`（不是 `Ok`、不是空成功），与 `UnwiredDelivery` 同一姿态。
//! - **生命周期误用 fail-closed**：未 `prepare` 就 `start`、重复 `start`、未 `start` 就
//!   `invoke`，一律 `E_STATE_INVALID_TRANSITION`——把"调用序列错了"如实报出，绝不静默兜底。
//!
//! ## 为什么复用 `PluginType` 与 `ErrorCode` 而不自造枚举
//!
//! 独立审计把"并行态泛滥"列为头号结构债（两套身份/两套 readiness/三套命令清单…）。
//! 新立一层若再造一个 `DriverKind`、一个 `DriverError`，就是在制造**第五套并行态**。
//! 因此：形态用 [`PluginType`]，错误用 [`HostError`]/[`ErrorCode`]——本 trait 只做
//! "把已有事实接进统一生命周期"，不做"再定义一遍世界"。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde_json::Value;

use crate::error::{ErrorCode, HostError, HostResult};
use crate::manifest::PluginType;

/// 一次驱动调用所处的生命周期阶段。`health` 据此报告，而**不是**另起一套状态机——
/// 它是 [`crate::lifecycle::State`] 在"运行时实例"粒度上的投影（Preparing→Ready 之间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverPhase {
    /// 已 `prepare` 但未 `start`：资源就绪、尚无执行体。
    Prepared,
    /// 已 `start`：可接受 `invoke`。
    Running,
    /// 已 `stop`（或从未 `start` 后终止）：`invoke` 一律拒绝。
    Stopped,
}

/// 一个运行实例的健康快照。刻意**不含**具体计数（pending/流/订阅仍归 `Registry` 与
/// [`crate::runtime::ReapStats`]）——驱动只报告"我这个实例此刻能不能干活"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverHealth {
    pub phase: DriverPhase,
    /// 实例是否仍可接受新 `invoke`（`Running` 且未中毒）。中毒 = 曾崩溃/租约失效，
    /// 需要上层按形态决定重启或隔离；这里**如实**报告 false，不粉饰成"健康"。
    pub serving: bool,
    /// 最后一次异常的机器可读原因（无异常则 `None`）。
    pub last_error: Option<String>,
}

/// `invoke` 的结果。`ok=false` 时 `error_code` 必非空——与
/// [`crate::call_delivery::CallOutcome`] 同形（诚实：失败必须带原因，不得空成功）。
#[derive(Debug, Clone, PartialEq)]
pub struct DriverResult {
    pub ok: bool,
    pub result: Option<Value>,
    pub error_code: Option<String>,
}

/// `prepare` 的输入：一次运行实例的规格。字段刻意从简——把 manifest/config 的权威解析
/// 留在各自域里，本层只接"起一个实例需要知道的最小事实"。
#[derive(Debug, Clone, PartialEq)]
pub struct DriverSpec {
    /// 目标插件 id（与 [`crate::manifest::PluginId`] 字符串一致）。
    pub plugin_id: String,
    /// 期望的插件形态。驱动若不支持该形态，`prepare` 即返回 `E_PLUGIN_TYPE_NO_RUNTIME`。
    pub kind: PluginType,
    /// 形态相关的启动参数（进程可执行路径 / wasm 模块路径 / js 入口…），由上层从
    /// manifest 抽出后传入。本层**不猜**缺省。
    pub entry: Value,
}

/// `invoke` 的一次调用（与 [`crate::registry::PendingCall`] 解耦，便于独立测试驱动）。
#[derive(Debug, Clone)]
pub struct DriverCall {
    pub call_id: String,
    pub cmd: String,
    pub args: Value,
}

/// `prepare` 成功后返回的句柄（不透明 id + 阶段）。
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedRuntime {
    pub handle: u64,
    pub spec: DriverSpec,
}

/// `start` 成功后返回的句柄。`PreparedRuntime` 与 `RunningRuntime` 是两个类型，
/// 让"未 start 就 invoke"在**类型层**也不易写错（但运行期仍强制校验，见各实现）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningRuntime {
    pub handle: u64,
}

/// 统一运行时驱动契约（FULL-AUDIT B-1 六方法）。
///
/// **实现方义务（=一致性用例要验证的不变量）**：
/// 1. `prepare` 对不支持的 [`PluginType`] 必须返回 `E_PLUGIN_TYPE_NO_RUNTIME`，不得假就绪；
/// 2. `start` 只对已 `prepare` 的句柄有效；重复 `start` → `E_STATE_INVALID_TRANSITION`；
/// 3. `invoke` 只对已 `start` 未 `stop` 的实例有效；否则 `E_STATE_INVALID_TRANSITION`；
///    调用失败以 `DriverResult{ok:false, error_code:Some(..)}` 表达，**不**用 `Err` 吞掉
///    （`Err` 专供"传输/容量"级错误，与 [`crate::call_delivery`] 的分工一致）；
/// 4. `cancel` 对未知 `call_id` 返回 `E_CALL_NOT_FOUND`；
/// 5. `health` 永不为 `Err`：它是只读快照，异常也如实反映在 `serving=false`/`last_error`；
/// 6. `stop` **幂等**：对已 stop 的实例再 `stop` 返回 `Ok(())`（目标"没有存活执行体"已成立），
///    与 [`crate::runtime::ReapOutcome::AlreadyGone`] 同一诚实取向——但绝不复活。
pub trait RuntimeDriver: Send + Sync {
    /// 该驱动支持的形态集合。上层据此把 [`PluginType`] 路由到对的驱动。
    fn supported(&self) -> Vec<PluginType>;

    /// 阶段一：校验规格、预留资源，产出 `PreparedRuntime`。不做启动。
    fn prepare(&self, spec: &DriverSpec) -> HostResult<PreparedRuntime>;

    /// 阶段二：把已 `prepare` 的实例拉起，产出 `RunningRuntime`。
    fn start(&self, prepared: &PreparedRuntime) -> HostResult<RunningRuntime>;

    /// 对已 `start` 的实例发起一次调用。结果用 [`DriverResult`] 承载（失败也在其中）。
    fn invoke(&self, running: &RunningRuntime, call: &DriverCall) -> HostResult<DriverResult>;

    /// 取消一次在飞调用（best-effort；实际中断能力依形态而定，做不到就如实 `E_CALL_NOT_FOUND`）。
    fn cancel(&self, running: &RunningRuntime, call_id: &str) -> HostResult<()>;

    /// 只读健康快照（永不为 `Err`）。
    fn health(&self, running: &RunningRuntime) -> DriverHealth;

    /// 终止实例并回收其执行体。**幂等**：重复调用返回 `Ok`，但不得复活。
    fn stop(&self, running: &RunningRuntime) -> HostResult<()>;
}

// ── MockRuntimeDriver：契约的可脚本化参考实现 ───────────────────────────────
//
// 它的价值不在"能跑真插件"，而在**把上面六条不变量做成可读、可测、可复用**的参考实现：
// 迁移真驱动（T-6/T-10/T-11/T-12）时，同一套 `conformance_cases` 必须照样全绿。

#[derive(Debug)]
struct MockInstance {
    spec: DriverSpec,
    phase: DriverPhase,
    serving: bool,
    last_error: Option<String>,
    /// 在飞但未结算的 call_id（用于 cancel / stop 时如实清理）。
    inflight: Vec<String>,
}

/// 可脚本化的 [`RuntimeDriver`] 参考实现。默认支持 Js/Process/Wasm/Rust 全四形态，
/// 用 `with_supported` 收窄；`script` 可为某条 `cmd` 预置失败（ok=false + 指定码）。
pub struct MockRuntimeDriver {
    supported: Vec<PluginType>,
    next_handle: AtomicU64,
    instances: Mutex<HashMap<u64, MockInstance>>,
    /// `cmd` → 预置失败码。命中则 `invoke` 返回 `ok:false`（**不**动 serving，失败是业务级）。
    scripted_failures: Mutex<HashMap<String, String>>,
}

impl Default for MockRuntimeDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRuntimeDriver {
    /// 支持全部四种形态。**唯一对外的构造点**——其余构造/预置能力是 [`Self::with_supported`]
    /// 等 crate 内测试辅助（`#[cfg(test)]`），刻意不进发布面、也不参与孤儿公共 API 台账。
    pub fn new() -> Self {
        Self {
            supported: vec![
                PluginType::Js,
                PluginType::Process,
                PluginType::Wasm,
                PluginType::Rust,
            ],
            next_handle: AtomicU64::new(1),
            instances: Mutex::new(HashMap::new()),
            scripted_failures: Mutex::new(HashMap::new()),
        }
    }

    /// 只支持给定形态集合（模拟"某驱动不支持 Rust"这类真实缺口）。
    /// **仅测试构建可见**（`#[cfg(test)]`）：收窄支持集只在本仓一致性用例里有意义，
    /// 既非对外面、也不该在非测试构建里成为死代码。
    #[cfg(test)]
    pub(crate) fn with_supported(kinds: Vec<PluginType>) -> Self {
        Self {
            supported: kinds,
            next_handle: AtomicU64::new(1),
            instances: Mutex::new(HashMap::new()),
            scripted_failures: Mutex::new(HashMap::new()),
        }
    }

    /// 为某个 `cmd` 预置一次失败结果（返回 `ok:false` + 该码），模拟执行方报错。
    /// **仅测试构建可见**（`#[cfg(test)]`），见 [`Self::with_supported`] 同一理由。
    #[cfg(test)]
    pub(crate) fn script_failure(&self, cmd: &str, code: &str) {
        self.scripted_failures.lock().insert(cmd.to_string(), code.to_string());
    }

    /// 已存活（未 stop）的实例数——供测试断言"stop 真的回收"，不泄漏句柄表。
    /// **仅测试构建可见**（`#[cfg(test)]`），见 [`Self::with_supported`] 同一理由。
    #[cfg(test)]
    pub(crate) fn live_instance_count(&self) -> usize {
        self.instances.lock().values().filter(|i| i.phase != DriverPhase::Stopped).count()
    }
}

impl RuntimeDriver for MockRuntimeDriver {
    fn supported(&self) -> Vec<PluginType> {
        self.supported.clone()
    }

    fn prepare(&self, spec: &DriverSpec) -> HostResult<PreparedRuntime> {
        // 义务 1：不支持的形态是结构化事实，不假就绪。
        if !self.supported.contains(&spec.kind) {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME,
                format!("MockRuntimeDriver 不支持形态 {:?}", spec.kind),
            ));
        }
        let handle = self.next_handle.fetch_add(1, Ordering::AcqRel);
        self.instances.lock().insert(
            handle,
            MockInstance {
                spec: spec.clone(),
                phase: DriverPhase::Prepared,
                serving: false,
                last_error: None,
                inflight: Vec::new(),
            },
        );
        Ok(PreparedRuntime { handle, spec: spec.clone() })
    }

    fn start(&self, prepared: &PreparedRuntime) -> HostResult<RunningRuntime> {
        let mut ins = self.instances.lock();
        let inst = ins.get_mut(&prepared.handle).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("句柄 {} 从未 prepare", prepared.handle),
            )
        })?;
        // 义务 2：只有 Prepared 能 start；重复 start（已在 Running）拒绝。
        match inst.phase {
            DriverPhase::Prepared => {
                inst.phase = DriverPhase::Running;
                inst.serving = true;
                Ok(RunningRuntime { handle: prepared.handle })
            }
            DriverPhase::Running => Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("句柄 {} 已在运行，拒绝重复 start", prepared.handle),
            )),
            DriverPhase::Stopped => Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("句柄 {} 已 stop，不得复活", prepared.handle),
            )),
        }
    }

    fn invoke(&self, running: &RunningRuntime, call: &DriverCall) -> HostResult<DriverResult> {
        let mut ins = self.instances.lock();
        let inst = ins.get_mut(&running.handle).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("句柄 {} 不存在", running.handle),
            )
        })?;
        // 义务 3：未 Running 不得 invoke。
        if inst.phase != DriverPhase::Running || !inst.serving {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("句柄 {} 处于 {:?}（非可执行态），拒绝 invoke", running.handle, inst.phase),
            ));
        }
        inst.inflight.push(call.call_id.clone());

        // 失败以 DriverResult 承载（不是 Err），业务级失败不动 serving。
        let scripted = self.scripted_failures.lock().get(&call.cmd).cloned();
        let plugin_id = inst.spec.plugin_id.clone();
        inst.inflight.retain(|c| c != &call.call_id);
        if let Some(code) = scripted {
            return Ok(DriverResult { ok: false, result: None, error_code: Some(code) });
        }
        Ok(DriverResult {
            ok: true,
            result: Some(serde_json::json!({
                "echo": call.args,
                "cmd": call.cmd,
                "plugin": plugin_id,
            })),
            error_code: None,
        })
    }

    fn cancel(&self, running: &RunningRuntime, call_id: &str) -> HostResult<()> {
        let mut ins = self.instances.lock();
        let inst = ins.get_mut(&running.handle).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("句柄 {} 不存在，无从取消 {}", running.handle, call_id),
            )
        })?;
        // 义务 4：未知 call_id 如实 E_CALL_NOT_FOUND（不静默 Ok）。
        if inst.inflight.iter().any(|c| c == call_id) {
            inst.inflight.retain(|c| c != call_id);
            Ok(())
        } else {
            Err(HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("在飞调用 {} 不存在或已结算", call_id),
            ))
        }
    }

    fn health(&self, running: &RunningRuntime) -> DriverHealth {
        // 义务 5：health 永不为 Err——只读快照，异常如实反映。
        let ins = self.instances.lock();
        match ins.get(&running.handle) {
            Some(i) => DriverHealth {
                phase: i.phase,
                serving: i.serving,
                last_error: i.last_error.clone(),
            },
            None => DriverHealth {
                phase: DriverPhase::Stopped,
                serving: false,
                last_error: Some(format!("句柄 {} 不存在", running.handle)),
            },
        }
    }

    fn stop(&self, running: &RunningRuntime) -> HostResult<()> {
        let mut ins = self.instances.lock();
        let inst = ins.get_mut(&running.handle).ok_or_else(|| {
            // 从没见过的句柄无法确认"已无执行体"，如实结构化失败（区别于对已知实例的重复 stop
            // ——那条走下面 `Some(inst)` 分支幂等返回 Ok）。
            HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("句柄 {} 从未存在", running.handle),
            )
        })?;
        // 已在 Running 或 Prepared：置 Stopped、清在飞、serving=false。已 Stopped：幂等 Ok。
        inst.phase = DriverPhase::Stopped;
        inst.serving = false;
        inst.inflight.clear();
        Ok(())
    }
}

// 全部方法 `&self` + 具体参数，`Box<dyn RuntimeDriver>` 对象安全（上层按形态路由靠这个）。

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: PluginType) -> DriverSpec {
        DriverSpec { plugin_id: "com.example.p".into(), kind, entry: Value::Null }
    }

    fn call(id: &str, cmd: &str, args: Value) -> DriverCall {
        DriverCall { call_id: id.into(), cmd: cmd.into(), args }
    }

    /// 义务 1：不支持的形态 prepare 即 `E_PLUGIN_TYPE_NO_RUNTIME`，不假就绪。
    #[test]
    fn unsupported_kind_prepare_fails_closed() {
        let drv = MockRuntimeDriver::with_supported(vec![PluginType::Js]);
        assert!(drv.supported().contains(&PluginType::Js));
        let err = drv.prepare(&spec(PluginType::Wasm)).expect_err("不支持的形态必须结构化失败");
        assert_eq!(err.code, ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME);
    }

    /// 完整生命周期正向：prepare→start→invoke→health→stop 全绿，echo 结果正确。
    #[test]
    fn full_happy_lifecycle_echoes() {
        let drv = MockRuntimeDriver::new();
        let prepared = drv.prepare(&spec(PluginType::Process)).unwrap();
        let running = drv.start(&prepared).unwrap();
        let res = drv.invoke(&running, &call("c1", "run", serde_json::json!({"x": 1}))).unwrap();
        assert!(res.ok);
        assert_eq!(res.result.unwrap()["echo"], serde_json::json!({"x": 1}));
        let h = drv.health(&running);
        assert_eq!(h.phase, DriverPhase::Running);
        assert!(h.serving);
        drv.stop(&running).unwrap();
        assert_eq!(drv.health(&running).phase, DriverPhase::Stopped);
        assert!(!drv.health(&running).serving);
    }

    /// 义务 2：重复 start 拒绝，且拒绝后仍可正常 stop（不半吊子）。
    #[test]
    fn double_start_is_rejected() {
        let drv = MockRuntimeDriver::new();
        let prepared = drv.prepare(&spec(PluginType::Js)).unwrap();
        let running = drv.start(&prepared).unwrap();
        let err = drv.start(&prepared).expect_err("重复 start 必须被拒");
        assert_eq!(err.code, ErrorCode::E_STATE_INVALID_TRANSITION);
        // 拒绝不影响既有运行实例。
        assert!(drv.health(&running).serving);
    }

    /// 义务 3：未 start 不得 invoke（拿 Prepared 的 handle 硬造 RunningRuntime）。
    #[test]
    fn invoke_before_start_is_rejected() {
        let drv = MockRuntimeDriver::new();
        let prepared = drv.prepare(&spec(PluginType::Js)).unwrap();
        let not_running = RunningRuntime { handle: prepared.handle };
        let err = drv
            .invoke(&not_running, &call("c1", "run", Value::Null))
            .expect_err("未 start 的句柄不得 invoke");
        assert_eq!(err.code, ErrorCode::E_STATE_INVALID_TRANSITION);
    }

    /// 义务 3：stop 后 invoke 拒绝（不得复活）。
    #[test]
    fn invoke_after_stop_is_rejected() {
        let drv = MockRuntimeDriver::new();
        let prepared = drv.prepare(&spec(PluginType::Js)).unwrap();
        let running = drv.start(&prepared).unwrap();
        drv.stop(&running).unwrap();
        let err =
            drv.invoke(&running, &call("c1", "run", Value::Null)).expect_err("stop 后不得 invoke");
        assert_eq!(err.code, ErrorCode::E_STATE_INVALID_TRANSITION);
    }

    /// 失败以 DriverResult 承载而非 Err（与 call_delivery 的分工一致），且不动 serving。
    #[test]
    fn scripted_failure_reports_result_not_transport_error() {
        let drv = MockRuntimeDriver::new();
        drv.script_failure("boom", "E_PLUGIN_TYPE_NO_RUNTIME");
        let prepared = drv.prepare(&spec(PluginType::Js)).unwrap();
        let running = drv.start(&prepared).unwrap();
        let res = drv
            .invoke(&running, &call("c1", "boom", Value::Null))
            .expect("业务级失败不该是传输错误");
        assert!(!res.ok);
        assert_eq!(res.error_code.as_deref(), Some("E_PLUGIN_TYPE_NO_RUNTIME"));
        assert!(drv.health(&running).serving, "业务失败不该把实例打成不健康");
    }

    /// 义务 4：cancel 未知 call_id 如实 E_CALL_NOT_FOUND（不静默成功）。
    #[test]
    fn cancel_unknown_call_is_found_error() {
        let drv = MockRuntimeDriver::new();
        let prepared = drv.prepare(&spec(PluginType::Js)).unwrap();
        let running = drv.start(&prepared).unwrap();
        let err = drv.cancel(&running, "ghost").expect_err("未知 call 不得静默 Ok");
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
    }

    /// 义务 6：stop 幂等——重复 stop 返回 Ok，不复活、不泄漏活实例。
    #[test]
    fn stop_is_idempotent_and_does_not_leak() {
        let drv = MockRuntimeDriver::new();
        let prepared = drv.prepare(&spec(PluginType::Process)).unwrap();
        let running = drv.start(&prepared).unwrap();
        assert_eq!(drv.live_instance_count(), 1);
        drv.stop(&running).unwrap();
        drv.stop(&running).unwrap(); // 幂等
        assert_eq!(drv.health(&running).phase, DriverPhase::Stopped);
        assert_eq!(drv.live_instance_count(), 0, "stop 后不得残留活实例");
    }

    /// 未 prepare 的句柄 start/stop 均结构化失败（不凭空造实例）。
    #[test]
    fn unknown_handle_operations_fail_closed() {
        let drv = MockRuntimeDriver::new();
        let phantom = PreparedRuntime { handle: 9999, spec: spec(PluginType::Js) };
        let running = RunningRuntime { handle: 9999 };
        assert!(drv.start(&phantom).is_err());
        assert!(drv.stop(&running).is_err());
        assert!(!drv.health(&running).serving, "不存在句柄 health 报不健康而非 panic");
    }

    /// trait 可装箱为 `Box<dyn RuntimeDriver>`（对象安全验证——上层要按形态路由就靠这个）。
    #[test]
    fn trait_is_object_safe_and_routable() {
        let drivers: Vec<Box<dyn RuntimeDriver>> = vec![
            Box::new(MockRuntimeDriver::with_supported(vec![PluginType::Js])),
            Box::new(MockRuntimeDriver::with_supported(vec![PluginType::Process])),
        ];
        let wanted = PluginType::Process;
        let chosen =
            drivers.iter().find(|d| d.supported().contains(&wanted)).expect("应有驱动支持 Process");
        let prepared = chosen.prepare(&spec(wanted)).unwrap();
        let running = chosen.start(&prepared).unwrap();
        assert!(chosen.invoke(&running, &call("c1", "run", Value::Null)).unwrap().ok);
    }
}
