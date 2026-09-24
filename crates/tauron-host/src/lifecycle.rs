//! §4.3 生命周期状态机（`tauron-host/lifecycle`）。
//!
//! 职责：表驱动迁移 + `transition()` 单点收口 + 守卫 + 非法迁移计数。
//!
//! 关键约束（计划 §4.3）：
//! - **action 内禁止再调 transition**（`apply()` 只改计数器/标志位，从不反向迁移；
//!   CI 静态检查见门禁 §8-2）；
//! - **写入单一**：本模块是 `plugin.state` 的唯一写入者（计划 §4.3）；
//! - **状态机不接受来自事件载荷的前缀解析输入**（R8）——`Event` 是类型化枚举，
//!   `reason` 仅用于遥测与审计，从不参与分支判定；
//! - **链深 ≤ 2**（`MAX_CHAIN_DEPTH`），超限即拒绝；
//! - **无零耗环**：见 [`validate_table`]。
//!
//! 第四轮修订落地：
//! - **D24/D3**：ERRORED 拆两档 `ErroredRetryable` / `ErroredUserConfirm`；
//!   `InstallFailed` 为安装期终态（仅可卸载，**不自动重试**）；
//! - **D3 补全出边**：`RUNNING→ERRORED`、`ERRORED→{ENABLED,DISABLED,UNINSTALLED}`、
//!   `DISABLED→UNINSTALLED`；
//! - **D27**：用户显式重启用 = 计数清零（`Enable` 规则统一带 `ResetCounters`），
//!   防"启用即再熔断"死循环；
//! - **D25/D28**：安全模式三级降级；试验性启用（`TrialEnable`）**单独计失败**，
//!   1 次即回落 `disabled-by-safemode`，不累入应用级启动计数（该计数属 §4.14 进程级）。

use crate::error::{ErrorCode, HostError, HostResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 自动重试次数上限（超过即升级为需用户确认）。
pub const MAX_RETRY: usize = 3;
/// 连续失败熔断阈值。
pub const CIRCUIT_THRESHOLD: usize = 3;
/// 试验性启用（安全模式内单个试启）次数上限。
pub const MAX_TRIAL_ATTEMPTS: usize = 3;
/// 链式迁移深度上限（门禁 §8-2：链深 ≤2）。
pub const MAX_CHAIN_DEPTH: usize = 2;

// ──────────────────────────────────────────────────────────────────────────
// 状态
// ──────────────────────────────────────────────────────────────────────────

/// 插件生命周期状态（D24：ERRORED 拆两档；D3：新增 INSTALL_FAILED）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    Discovered,
    Installing,
    Installed,
    Enabled,
    Running,
    Disabled,
    ErroredRetryable,
    ErroredUserConfirm,
    /// 安装期终态：验签/hash/解包/range 任一失败。仅可卸载，不自动重试（D3）。
    InstallFailed,
    /// 终态。
    Uninstalled,
}

impl State {
    pub const ALL: [State; 10] = [
        State::Discovered,
        State::Installing,
        State::Installed,
        State::Enabled,
        State::Running,
        State::Disabled,
        State::ErroredRetryable,
        State::ErroredUserConfirm,
        State::InstallFailed,
        State::Uninstalled,
    ];

    /// 完全终态：无任何出边。
    pub const fn is_terminal(self) -> bool {
        matches!(self, State::Uninstalled)
    }

    /// 准终态：仅可卸载（D3：INSTALL_FAILED 不自动重试）。
    pub const fn is_quasi_terminal(self) -> bool {
        matches!(self, State::InstallFailed)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            State::Discovered => "DISCOVERED",
            State::Installing => "INSTALLING",
            State::Installed => "INSTALLED",
            State::Enabled => "ENABLED",
            State::Running => "RUNNING",
            State::Disabled => "DISABLED",
            State::ErroredRetryable => "ERRORED_RETRYABLE",
            State::ErroredUserConfirm => "ERRORED_USER_CONFIRM",
            State::InstallFailed => "INSTALL_FAILED",
            State::Uninstalled => "UNINSTALLED",
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 事件
// ──────────────────────────────────────────────────────────────────────────

/// 生命周期事件（类型化枚举，禁止字符串前缀解析——R8）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Event {
    InstallStart,
    InstallOk,
    InstallFail,
    Enable,
    Disable,
    Attach,
    Detach,
    ErrorRetryable,
    ErrorFatal,
    RetryOk,
    RetryExhausted,
    /// 安全模式下单个试启（D25/D28：试验性启用，单独计失败）。
    TrialEnable,
    SafemodeEnter,
    SafemodeExit,
    /// 5min 健康窗口 → 计数器衰减（架构 §4.4）。
    HealthOk,
    Uninstall,
    Purge,
    /// sidecar（进程插件）意外退出（P0-2：`host_runtime_health` 探测到不存活）。
    ///
    /// 与 `ErrorRetryable` 同形：崩溃重启是**有预算的自动重试**（`tauron-proc`
    /// 的 `CrashLimit`：缺省 3 次 / 5min），预算耗尽才升级为需用户确认。
    /// 在试验性启用（D28）中崩溃则按试验失败回落 `disabled-by-safemode`——
    /// 与 `ErrorFatal` 的试验分支同一条落点。
    RuntimeCrash,
}

impl Event {
    pub const ALL: [Event; 18] = [
        Event::InstallStart,
        Event::InstallOk,
        Event::InstallFail,
        Event::Enable,
        Event::Disable,
        Event::Attach,
        Event::Detach,
        Event::ErrorRetryable,
        Event::ErrorFatal,
        Event::RetryOk,
        Event::RetryExhausted,
        Event::TrialEnable,
        Event::SafemodeEnter,
        Event::SafemodeExit,
        Event::HealthOk,
        Event::Uninstall,
        Event::Purge,
        Event::RuntimeCrash,
    ];

    /// 是否可由用户 / UI 触发（false = 系统自动触发）。
    ///
    /// 用于"无零耗环"判定：全由系统事件构成的环若不含预算消耗动作，
    /// 就可能无限自转（例如自动重试永不终止）。
    pub const fn user_initiated(self) -> bool {
        match self {
            Event::InstallStart
            | Event::Enable
            | Event::Disable
            | Event::Attach
            | Event::Detach
            | Event::TrialEnable
            | Event::SafemodeEnter
            | Event::SafemodeExit
            | Event::Uninstall
            | Event::Purge => true,
            Event::InstallOk
            | Event::InstallFail
            | Event::ErrorRetryable
            | Event::ErrorFatal
            | Event::RetryOk
            | Event::RetryExhausted
            | Event::HealthOk
            | Event::RuntimeCrash => false,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Event::InstallStart => "INSTALL_START",
            Event::InstallOk => "INSTALL_OK",
            Event::InstallFail => "INSTALL_FAIL",
            Event::Enable => "ENABLE",
            Event::Disable => "DISABLE",
            Event::Attach => "ATTACH",
            Event::Detach => "DETACH",
            Event::ErrorRetryable => "ERROR_RETRYABLE",
            Event::ErrorFatal => "ERROR_FATAL",
            Event::RetryOk => "RETRY_OK",
            Event::RetryExhausted => "RETRY_EXHAUSTED",
            Event::TrialEnable => "TRIAL_ENABLE",
            Event::SafemodeEnter => "SAFEMODE_ENTER",
            Event::SafemodeExit => "SAFEMODE_EXIT",
            Event::HealthOk => "HEALTH_OK",
            Event::Uninstall => "UNINSTALL",
            Event::Purge => "PURGE",
            Event::RuntimeCrash => "RUNTIME_CRASH",
        }
    }
}

impl std::fmt::Display for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 守卫 / 动作
// ──────────────────────────────────────────────────────────────────────────

/// 迁移守卫（对当前上下文的布尔条件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Guard {
    None,
    RetryBudgetAvailable,
    RetryBudgetExhausted,
    CircuitOpen,
    CircuitClosed,
    InSafemode,
    /// 安全模式禁用 **且** 试验预算未耗尽（D28 两条件必须同时成立）。
    InSafemodeWithTrialBudget,
    TrialActive,
    TrialBudgetAvailable,
}

impl Guard {
    pub fn holds(&self, s: &PluginState) -> bool {
        match self {
            Guard::None => true,
            Guard::RetryBudgetAvailable => s.retry_count < MAX_RETRY,
            Guard::RetryBudgetExhausted => s.retry_count >= MAX_RETRY,
            Guard::CircuitOpen => s.circuit_open,
            Guard::CircuitClosed => !s.circuit_open,
            Guard::InSafemode => s.disabled_by_safemode,
            Guard::InSafemodeWithTrialBudget => {
                s.disabled_by_safemode && s.trial_failures < MAX_TRIAL_ATTEMPTS
            }
            Guard::TrialActive => s.trial_from_safemode,
            Guard::TrialBudgetAvailable => s.trial_failures < MAX_TRIAL_ATTEMPTS,
        }
    }
}

/// 迁移动作。
///
/// **`apply()` 内禁止再调 `transition()`**（计划 §4.3 CI 静态检查）：
/// 动作只改计数器与标志位，从不改变 `state`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Action {
    IncrementRetry,
    IncrementFailure,
    IncrementTrialFailure,
    /// 计数清零（D27：用户显式重启用 = 新观察窗口）。
    ResetCounters,
    /// 重置运行期计数（retry/failure/circuit），**不动 `trial_failures`**。
    ///
    /// D28：试验性启用的预算必须跨次累积，否则 `MAX_TRIAL_ATTEMPTS` 形同虚设
    /// ——`TrialEnable` / `SafemodeExit` 这类系统驱动的进入路径用它。
    ResetRuntimeCounters,
    /// 健康窗口衰减：retry_count 与 failure_count 各减 1（饱和于 0）；
    /// 两者同时归零即关断熔断（error-budget 式自动恢复）。
    DecayRetry,
    SetCircuitOpen,
    SetSafemodeDisabled,
    ClearSafemode,
    SetTrial,
    ClearTrial,
}

impl Action {
    /// 是否消耗有界预算（用于"无零耗环"判定）。
    pub const fn consumes_budget(self) -> bool {
        matches!(
            self,
            Action::IncrementRetry | Action::IncrementFailure | Action::IncrementTrialFailure
        )
    }
}

/// 应用单个动作（**绝不改变 `state`**）。
pub fn apply(s: &mut PluginState, action: Action) {
    match action {
        Action::IncrementRetry => s.retry_count += 1,
        Action::IncrementFailure => {
            s.failure_count += 1;
            if s.failure_count >= CIRCUIT_THRESHOLD {
                s.circuit_open = true;
            }
        }
        Action::IncrementTrialFailure => s.trial_failures += 1,
        Action::ResetCounters => {
            s.retry_count = 0;
            s.failure_count = 0;
            s.trial_failures = 0;
            s.circuit_open = false;
        }
        Action::ResetRuntimeCounters => {
            s.retry_count = 0;
            s.failure_count = 0;
            s.circuit_open = false;
        }
        Action::DecayRetry => {
            // 健康窗口：retry 与 failure 各衰减 1（饱和于 0）。
            // 两者归零即熔断关闭——error-budget 式自动恢复。
            s.retry_count = s.retry_count.saturating_sub(1);
            s.failure_count = s.failure_count.saturating_sub(1);
            if s.retry_count == 0 && s.failure_count == 0 {
                s.circuit_open = false;
            }
        }
        Action::SetCircuitOpen => s.circuit_open = true,
        Action::SetSafemodeDisabled => s.disabled_by_safemode = true,
        Action::ClearSafemode => s.disabled_by_safemode = false,
        Action::SetTrial => s.trial_from_safemode = true,
        Action::ClearTrial => s.trial_from_safemode = false,
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 迁移表
// ──────────────────────────────────────────────────────────────────────────

/// 单条迁移规则。
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub from: State,
    pub event: Event,
    pub guard: Guard,
    pub to: State,
    pub actions: &'static [Action],
    /// 链式后续事件（应用本规则后继续迁移）。深度受 `MAX_CHAIN_DEPTH` 限制。
    pub chain: Option<Event>,
}

macro_rules! rule {
    // (from, event, to)
    ($from:expr, $event:expr, $to:expr) => {
        Rule { from: $from, event: $event, guard: Guard::None, to: $to, actions: &[], chain: None }
    };
    // (from, event, to, actions)
    ($from:expr, $event:expr, $to:expr, $actions:expr) => {
        Rule { from: $from, event: $event, guard: Guard::None, to: $to, actions: $actions, chain: None }
    };
    // (from, event, guard, to, actions) —— 有守卫的规则必须带 actions（可为 &[]）
    ($from:expr, $event:expr, $guard:expr, $to:expr, $actions:expr) => {
        Rule { from: $from, event: $event, guard: $guard, to: $to, actions: $actions, chain: None }
    };
}

/// 状态×事件迁移表（数组常量，CI 遍历校验——门禁 §8-2）。
///
/// 注：**表内顺序即匹配优先级**，同 `(from, event)` 下先声明者先匹配。
/// 因此守卫较严格的规则（如 `TrialActive`）必须声明在宽泛规则之前。
pub static TRANSITIONS: &[Rule] = &[
    // ── DISCOVERED ──────────────────────────────────────────────
    rule!(State::Discovered, Event::InstallStart, State::Installing),

    // ── INSTALLING ──────────────────────────────────────────────
    rule!(State::Installing, Event::InstallOk, State::Installed),
    // D3：安装期失败落终态，不自动重试。
    rule!(State::Installing, Event::InstallFail, State::InstallFailed),

    // ── INSTALLED ───────────────────────────────────────────────
    rule!(State::Installed, Event::Enable, State::Enabled, &[Action::ResetCounters]),
    // 未启用过的插件也允许直接禁用（UI 常见操作；无害的补边）。
    rule!(State::Installed, Event::Disable, State::Disabled),
    rule!(State::Installed, Event::Uninstall, State::Uninstalled),
    rule!(State::Installed, Event::Purge, State::Uninstalled),
    rule!(State::Installed, Event::SafemodeEnter, State::Disabled, &[Action::SetSafemodeDisabled]),

    // ── ENABLED ─────────────────────────────────────────────────
    // D25/D28：试验性启用中的插件出错 → 回落 disabled-by-safemode，
    // 单独计失败，不累入应用级启动计数。必须先于宽泛错误规则。
    rule!(State::Enabled, Event::ErrorRetryable, Guard::TrialActive, State::Disabled, &[Action::IncrementTrialFailure, Action::SetSafemodeDisabled, Action::ClearTrial]),
    rule!(State::Enabled, Event::ErrorFatal, Guard::TrialActive, State::Disabled, &[Action::IncrementTrialFailure, Action::SetSafemodeDisabled, Action::ClearTrial]),
    // 自动重试预算内 → 可重试档；预算耗尽 → 直接升级为需用户确认。
    rule!(State::Enabled, Event::ErrorRetryable, Guard::RetryBudgetAvailable, State::ErroredRetryable, &[Action::IncrementRetry]),
    rule!(State::Enabled, Event::ErrorRetryable, Guard::RetryBudgetExhausted, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    rule!(State::Enabled, Event::ErrorFatal, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    // P0-2：进程插件（sidecar）意外退出。语义与 `ErrorRetryable` 一致——崩溃重启
    // 是**有预算的自动重试**（`CrashLimit`），预算耗尽才升级为需用户确认；
    // 试验启用期间则按 D28 试验失败回落（必须先于无守卫的规则）。
    rule!(State::Enabled, Event::RuntimeCrash, Guard::TrialActive, State::Disabled, &[Action::IncrementTrialFailure, Action::SetSafemodeDisabled, Action::ClearTrial]),
    rule!(State::Enabled, Event::RuntimeCrash, Guard::RetryBudgetAvailable, State::ErroredRetryable, &[Action::IncrementRetry]),
    rule!(State::Enabled, Event::RuntimeCrash, Guard::RetryBudgetExhausted, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    rule!(State::Enabled, Event::Attach, State::Running),
    rule!(State::Enabled, Event::Disable, State::Disabled),
    rule!(State::Enabled, Event::SafemodeEnter, State::Disabled, &[Action::SetSafemodeDisabled]),
    rule!(State::Enabled, Event::HealthOk, State::Enabled, &[Action::DecayRetry]),
    rule!(State::Enabled, Event::Uninstall, State::Uninstalled),
    rule!(State::Enabled, Event::Purge, State::Uninstalled),

    // ── RUNNING ─────────────────────────────────────────────────
    rule!(State::Running, Event::ErrorRetryable, Guard::TrialActive, State::Disabled, &[Action::IncrementTrialFailure, Action::SetSafemodeDisabled, Action::ClearTrial]),
    rule!(State::Running, Event::ErrorFatal, Guard::TrialActive, State::Disabled, &[Action::IncrementTrialFailure, Action::SetSafemodeDisabled, Action::ClearTrial]),
    rule!(State::Running, Event::ErrorRetryable, Guard::RetryBudgetAvailable, State::ErroredRetryable, &[Action::IncrementRetry]),
    rule!(State::Running, Event::ErrorRetryable, Guard::RetryBudgetExhausted, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    // D3：补全出边 RUNNING → ERRORED（sidecar 意外退出等运行期崩溃）。
    rule!(State::Running, Event::ErrorFatal, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    // P0-2：RUNNING 上的 sidecar 崩溃（与 ENABLED 同形；RUNNING 才是进程插件的常态）。
    rule!(State::Running, Event::RuntimeCrash, Guard::TrialActive, State::Disabled, &[Action::IncrementTrialFailure, Action::SetSafemodeDisabled, Action::ClearTrial]),
    rule!(State::Running, Event::RuntimeCrash, Guard::RetryBudgetAvailable, State::ErroredRetryable, &[Action::IncrementRetry]),
    rule!(State::Running, Event::RuntimeCrash, Guard::RetryBudgetExhausted, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    rule!(State::Running, Event::Detach, State::Enabled),
    rule!(State::Running, Event::Disable, State::Disabled),
    rule!(State::Running, Event::SafemodeEnter, State::Disabled, &[Action::SetSafemodeDisabled]),
    rule!(State::Running, Event::HealthOk, State::Running, &[Action::DecayRetry]),
    rule!(State::Running, Event::Uninstall, State::Uninstalled),
    rule!(State::Running, Event::Purge, State::Uninstalled),

    // ── DISABLED ────────────────────────────────────────────────
    // D28：试验性启用仅对 safemode 禁用者有效，且受独立预算限制
    //（两条件必须同时成立，否则 MAX_TRIAL_ATTEMPTS 形同虚设）。
    rule!(State::Disabled, Event::TrialEnable, Guard::InSafemodeWithTrialBudget, State::Enabled, &[Action::ClearSafemode, Action::SetTrial, Action::ResetRuntimeCounters]),
    // D27：任何 Enable 都清零计数（防"启用即再熔断"死循环）。
    // ClearSafemode 必须随行：DISABLED 可能来自 SafemodeEnter（`disabled_by_safemode`
    // 置位）。若只迁移状态不清标志，插件会停在「state=Enabled 但标志仍 true」的
    // 非法组合——`PluginSummary.disabledBySafemode` 从此永远说谎，而恢复对账
    // （`reconcile_recovery_phase`）补发的 `SafemodeExit` 又只认 DISABLED 态、
    // 会被记为非法迁移忽略掉，标志再无机会清零。显式启用是用户的权威放行指令；
    // 若恢复引擎仍判定该插件应禁用，对账会在下一拍重新 `SafemodeEnter`（从
    // ENABLED 态合法），安全模式权威不因此旁路。对标志本就为 false 的普通禁用，
    // 清除是无害幂等操作。
    rule!(State::Disabled, Event::Enable, State::Enabled, &[Action::ClearSafemode, Action::ResetCounters]),
    rule!(State::Disabled, Event::SafemodeExit, Guard::InSafemode, State::Enabled, &[Action::ClearSafemode, Action::ResetRuntimeCounters]),
    // D3：补全出边 DISABLED → UNINSTALLED。
    rule!(State::Disabled, Event::Uninstall, State::Uninstalled),
    rule!(State::Disabled, Event::Purge, State::Uninstalled),

    // ── ERRORED_RETRYABLE ───────────────────────────────────────
    // 自动重试：预算内可再试（不清零计数，保留预算消耗痕迹）。
    rule!(State::ErroredRetryable, Event::Enable, Guard::RetryBudgetAvailable, State::Enabled, &[]),
    // 预算耗尽时的启用尝试 → 升级需用户确认（而非静默忽略）。
    rule!(State::ErroredRetryable, Event::Enable, Guard::RetryBudgetExhausted, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    rule!(State::ErroredRetryable, Event::RetryOk, State::Enabled, &[Action::ResetCounters]),
    rule!(State::ErroredRetryable, Event::RetryExhausted, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    rule!(State::ErroredRetryable, Event::Disable, State::Disabled),
    // D3：补全出边 ERRORED → {ENABLED, DISABLED, UNINSTALLED}。
    rule!(State::ErroredRetryable, Event::Uninstall, State::Uninstalled),
    rule!(State::ErroredRetryable, Event::Purge, State::Uninstalled),

    // ── ERRORED_USER_CONFIRM ────────────────────────────────────
    // §7-4：不自动重试，需用户确认。
    rule!(State::ErroredUserConfirm, Event::Enable, State::Enabled, &[Action::ResetCounters]),
    // 等待确认期间再出错：留在原地但继续累计失败，喂熔断计数。
    rule!(State::ErroredUserConfirm, Event::ErrorFatal, State::ErroredUserConfirm, &[Action::IncrementFailure]),
    rule!(State::ErroredUserConfirm, Event::Disable, State::Disabled),
    rule!(State::ErroredUserConfirm, Event::Uninstall, State::Uninstalled),
    rule!(State::ErroredUserConfirm, Event::Purge, State::Uninstalled),

    // ── INSTALL_FAILED（准终态：仅可卸载，D3）────────────────────
    rule!(State::InstallFailed, Event::Uninstall, State::Uninstalled),
    rule!(State::InstallFailed, Event::Purge, State::Uninstalled),

    // ── UNINSTALLED（终态：无出边）──────────────────────────────
];

// ──────────────────────────────────────────────────────────────────────────
// 上下文
// ──────────────────────────────────────────────────────────────────────────

/// 插件状态机上下文（`plugin.state` 的唯一写入对象）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginState {
    pub state: State,
    pub retry_count: usize,
    pub failure_count: usize,
    pub trial_failures: usize,
    pub circuit_open: bool,
    pub disabled_by_safemode: bool,
    pub trial_from_safemode: bool,
    /// 非法迁移计数（门禁 §8-2 的运行时信号）。
    pub illegal_transitions: u64,
    /// 最近一次事件的 reason（仅遥测/审计，**从不参与分支判定**——R8）。
    pub last_reason: Option<String>,
    pub last_event: Option<Event>,
}

impl Default for PluginState {
    fn default() -> Self {
        Self {
            state: State::Discovered,
            retry_count: 0,
            failure_count: 0,
            trial_failures: 0,
            circuit_open: false,
            disabled_by_safemode: false,
            trial_from_safemode: false,
            illegal_transitions: 0,
            last_reason: None,
            last_event: None,
        }
    }
}

/// 一次迁移的结果。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionOutcome {
    pub event: Event,
    pub from: State,
    pub to: State,
    /// 实际迁移深度（含链式）。1 = 单次迁移。
    pub depth: usize,
    /// 是否未匹配到任何规则（此时 `from == to`）。
    pub illegal: bool,
    /// 按序执行的动作。
    pub actions: Vec<Action>,
}

/// 单点收口迁移（`TRANSITIONS` 表）。
pub fn transition(s: &mut PluginState, event: Event) -> TransitionOutcome {
    transition_with(TRANSITIONS, s, event, None)
}

/// 单点收口迁移（可注入表，便于单元测试链深上限）。
pub fn transition_with(
    table: &[Rule],
    s: &mut PluginState,
    event: Event,
    reason: Option<&str>,
) -> TransitionOutcome {
    step_with(table, s, event, reason, 1)
}

fn step_with(
    table: &[Rule],
    s: &mut PluginState,
    event: Event,
    reason: Option<&str>,
    depth: usize,
) -> TransitionOutcome {
    let from = s.state;

    // 链深超限：拒绝继续（不 panic，只记非法）。
    if depth > MAX_CHAIN_DEPTH {
        s.illegal_transitions += 1;
        return TransitionOutcome {
            event,
            from,
            to: s.state,
            depth,
            illegal: true,
            actions: Vec::new(),
        };
    }

    let rule = table
        .iter()
        .find(|r| r.from == s.state && r.event == event && r.guard.holds(s));

    let Some(rule) = rule else {
        // 未匹配 = 非法迁移。计数但绝不 panic（计划 §4.1）。
        s.illegal_transitions += 1;
        if let Some(r) = reason {
            s.last_reason = Some(r.to_string());
        }
        s.last_event = Some(event);
        return TransitionOutcome {
            event,
            from,
            to: s.state,
            depth,
            illegal: true,
            actions: Vec::new(),
        };
    };

    // 应用动作（绝不调用 transition —— 计划 §4.3 CI 静态检查）。
    let mut actions = Vec::new();
    for a in rule.actions {
        apply(s, *a);
        actions.push(*a);
    }
    s.state = rule.to;
    if let Some(r) = reason {
        s.last_reason = Some(r.to_string());
    }
    s.last_event = Some(event);

    // 链式迁移。
    //
    // 不设本地深度闸：一律递归，由函数顶部的 `depth > MAX_CHAIN_DEPTH`
    // 判定收口。否则会在递归返回后把子调用的 `illegal: true` 覆盖成 false，
    // 使"链深超限"这一 CI 门禁信号被吞掉。
    if let Some(next) = rule.chain {
        let mut r = step_with(table, s, next, reason, depth + 1);
        let mut merged = actions;
        merged.extend(std::mem::take(&mut r.actions));
        r.from = from;
        r.actions = merged;
        return r;
    }

    TransitionOutcome {
        event,
        from,
        to: s.state,
        depth,
        illegal: false,
        actions,
    }
}

/// 从当前状态可达的事件集合（按表推导）。
pub fn reachable_events(s: State) -> Vec<Event> {
    let mut out = Vec::new();
    for e in Event::ALL {
        if TRANSITIONS.iter().any(|r| r.from == s && r.event == e) {
            out.push(e);
        }
    }
    out
}

// ──────────────────────────────────────────────────────────────────────────
// 表校验（CI 门禁 §8-2）
// ──────────────────────────────────────────────────────────────────────────

/// 校验迁移表的结构性性质（CI 门禁 §8-2 的入口）。
///
/// 检查项：
/// 1. `(from, event, guard)` 键唯一；
/// 2. 每个非终态至少 1 条出边；
/// 3. `Uninstalled` 恰好 0 条出边；
/// 4. `InstallFailed` 仅 `Uninstall`/`Purge` 出边（D3：不自动重试）；
/// 5. **无零耗环**：每个大小 ≥2 的强连通分量内至少含一条"用户触发"规则
///    或含"消耗预算"动作的规则（自环豁免，见下）；
/// 6. 链深 ≤ `MAX_CHAIN_DEPTH`；
/// 7. D3 出边补齐：`RUNNING→ERRORED_USER_CONFIRM` 存在。
pub fn validate_table() -> HostResult<()> {
    // 1. 键唯一
    let mut seen: HashSet<RuleKey> = HashSet::new();
    for r in TRANSITIONS {
        let k = RuleKey(r.from, r.event, r.guard);
        if !seen.insert(k) {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!(
                    "迁移表存在重复键：{} + {} (guard={:?})",
                    r.from, r.event, r.guard
                ),
            ));
        }
    }

    // 2. 非终态至少一条出边
    for st in State::ALL {
        if st.is_terminal() {
            continue;
        }
        if !TRANSITIONS.iter().any(|r| r.from == st) {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("非终态 {} 没有任何出边", st),
            ));
        }
    }

    // 3. 终态恰好零出边
    if TRANSITIONS.iter().any(|r| r.from == State::Uninstalled) {
        return Err(HostError::new(
            ErrorCode::E_STATE_INVALID_TRANSITION,
            "终态 UNINSTALLED 不应有任何出边".to_string(),
        ));
    }

    // 4. INSTALL_FAILED 仅可卸载
    for r in TRANSITIONS {
        if r.from == State::InstallFailed
            && !matches!(r.event, Event::Uninstall | Event::Purge)
        {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!(
                    "INSTALL_FAILED 只允许 Uninstall/Purge，出现 {}（D3：安装期失败不自动重试）",
                    r.event
                ),
            ));
        }
    }

    // 5. 无零耗环
    if let Err(e) = check_no_zero_cost_cycle() {
        return Err(e);
    }

    // 6. 链深上限
    for r in TRANSITIONS {
        let d = chain_depth(r);
        if d > MAX_CHAIN_DEPTH {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("{} + {} 链深 {} 超过上限 {}", r.from, r.event, d, MAX_CHAIN_DEPTH),
            ));
        }
    }

    // 7. D3 出边补齐
    if !TRANSITIONS.iter().any(|r| {
        r.from == State::Running
            && r.event == Event::ErrorFatal
            && r.to == State::ErroredUserConfirm
    }) {
        return Err(HostError::new(
            ErrorCode::E_STATE_INVALID_TRANSITION,
            "缺少 D3 要求的出边 RUNNING → ERRORED_USER_CONFIRM".to_string(),
        ));
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RuleKey(State, Event, Guard);

fn chain_depth(rule: &Rule) -> usize {
    let mut d = 1;
    let mut cur = rule;
    while let Some(next_event) = cur.chain {
        d += 1;
        let next = match TRANSITIONS
            .iter()
            .find(|r| r.from == cur.to && r.event == next_event && r.guard == Guard::None)
        {
            Some(r) => r,
            None => break,
        };
        cur = next;
        if d > 64 {
            return d;
        }
    }
    d
}

/// 计算所有强连通分量（|states| 很小，用可达性矩阵判定即可）。
fn strongly_connected_components() -> Vec<Vec<State>> {
    let states = State::ALL;
    let edges: HashMap<State, Vec<State>> = TRANSITIONS
        .iter()
        .fold(HashMap::new(), |mut m, r| {
            m.entry(r.from).or_default().push(r.to);
            m
        });

    // 可达性矩阵。
    let mut reach = vec![vec![false; states.len()]; states.len()];
    for (i, s) in states.iter().enumerate() {
        let mut stack = std::collections::VecDeque::new();
        stack.push_back(*s);
        reach[i][i] = true;
        while let Some(cur) = stack.pop_back() {
            if let Some(ns) = edges.get(&cur) {
                for &n in ns {
                    if let Some(j) = states.iter().position(|x| *x == n) {
                        if !reach[i][j] {
                            reach[i][j] = true;
                            stack.push_back(n);
                        }
                    }
                }
            }
        }
    }

    // SCC：i,j 互相可达即同分量。
    let mut assigned = vec![false; states.len()];
    let mut sccs: Vec<Vec<State>> = Vec::new();
    for i in 0..states.len() {
        if assigned[i] {
            continue;
        }
        let mut comp = Vec::new();
        for j in 0..states.len() {
            if !assigned[j] && reach[i][j] && reach[j][i] {
                comp.push(states[j]);
                assigned[j] = true;
            }
        }
        // 去掉自环分量（size==1 且无自环动作）在调用处单独处理。
        sccs.push(comp);
    }
    sccs
}

fn check_no_zero_cost_cycle() -> HostResult<()> {
    for comp in strongly_connected_components() {
        if comp.len() < 2 {
            continue;
        }
        let comp_set: HashSet<State> = comp.iter().copied().collect();
        let ok = TRANSITIONS
            .iter()
            .filter(|r| comp_set.contains(&r.from) && comp_set.contains(&r.to))
            .any(|r| r.event.user_initiated() || r.actions.iter().any(|a| a.consumes_budget()));
        if !ok {
            let names: Vec<String> = comp.iter().map(|s| s.as_str().to_string()).collect();
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!(
                    "零耗环：{} 内没有用户触发规则，也没有消耗预算的动作——可能无限自转（门禁 §8-2）",
                    names.join(" ↔ ")
                ),
            ));
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// 安全模式三级降级（D25/D28）
// ──────────────────────────────────────────────────────────────────────────

/// 宿主（应用级）启动失败状态。**不属于 `PluginState`**——它是进程级计数器，
/// 由 §4.14 崩溃恢复模块持有；此处仅定义三档语义与判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BootTier {
    /// 正常启动。
    Normal,
    /// 安全模式：仅加载必需插件（宿主/设置/更新），其余标 `disabled-by-safemode`。
    Safemode,
    /// 修复模式：零插件最小壳 + 错误报告 + 重装/修复引导。
    Repair,
}

/// 判定启动档位：连续 2 次未正常完成启动 → 第 3 次安全模式；
/// 安全模式下再失败 → 修复模式（D25：堵住永久 boot loop）。
pub fn boot_tier(consecutive_failures: u32, already_in_safemode: bool) -> BootTier {
    if already_in_safemode {
        BootTier::Repair
    } else if consecutive_failures >= 2 {
        BootTier::Safemode
    } else {
        BootTier::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 表结构校验 ───────────────────────────────────────────────

    #[test]
    fn table_is_well_formed() {
        assert!(
            validate_table().is_ok(),
            "{}",
            validate_table().unwrap_err().message
        );
    }

    #[test]
    fn terminal_states_have_expected_shape() {
        assert!(State::Uninstalled.is_terminal());
        assert!(!State::Uninstalled.is_quasi_terminal());
        assert!(State::InstallFailed.is_quasi_terminal());
        assert!(!State::InstallFailed.is_terminal());
        assert!(reachable_events(State::Uninstalled).is_empty());
        assert_eq!(reachable_events(State::InstallFailed), vec![Event::Uninstall, Event::Purge]);
    }

    // ── 状态×事件全组合（计划 §4.3 测试要求）────────────────────

    #[test]
    fn full_state_event_matrix_never_panics() {
        for st in State::ALL {
            for ev in Event::ALL {
                let mut s = PluginState::default();
                s.state = st;
                let before_illegal = s.illegal_transitions;
                // 必须不 panic、不悬挂。
                let out = transition(&mut s, ev);
                assert!(out.depth <= MAX_CHAIN_DEPTH + 1);
                if out.illegal {
                    assert_eq!(out.from, out.to, "非法迁移不得改变状态：{st} + {ev}");
                    assert!(
                        s.illegal_transitions >= before_illegal + 1,
                        "非法迁移必须计数：{st} + {ev}"
                    );
                } else {
                    assert_ne!(out.depth, 0);
                    assert_eq!(s.state, out.to);
                    assert_eq!(
                        s.illegal_transitions,
                        before_illegal,
                        "合法迁移不得增加非法计数：{st} + {ev}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_nonterminal_state_reaches_a_legal_successor() {
        for st in State::ALL {
            if st.is_terminal() {
                continue;
            }
            assert!(
                !reachable_events(st).is_empty(),
                "{st} 无出边"
            );
        }
    }

    // ── 安装路径 ─────────────────────────────────────────────────

    #[test]
    fn install_success_path() {
        let mut s = PluginState::default();
        let o1 = transition(&mut s, Event::InstallStart);
        assert_eq!(s.state, State::Installing);
        assert!(!o1.illegal);
        let o2 = transition(&mut s, Event::InstallOk);
        assert_eq!(s.state, State::Installed);
        assert!(!o2.illegal);
    }

    #[test]
    fn install_failure_goes_to_terminal_and_never_auto_retries() {
        // D3：验签/hash/解包/range 任一失败 → INSTALL_FAILED，不自动重试。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        let o = transition(&mut s, Event::InstallFail);
        assert_eq!(s.state, State::InstallFailed);
        assert!(!o.illegal);
        assert_eq!(s.illegal_transitions, 0);

        // 重试类事件全部非法（这正是"不自动重试"的语义保证）。
        for ev in [
            Event::Enable,
            Event::RetryOk,
            Event::RetryExhausted,
            Event::InstallOk,
            Event::ErrorRetryable,
            Event::ErrorFatal,
            Event::HealthOk,
        ] {
            let before = s.state;
            let o = transition(&mut s, ev);
            assert!(o.illegal, "{ev} 在 INSTALL_FAILED 上应非法");
            assert_eq!(s.state, before);
        }

        // 仅可卸载。
        assert_eq!(transition(&mut s, Event::Uninstall).to, State::Uninstalled);
        assert_eq!(s.state, State::Uninstalled);
    }

    // ── ERRORED 两档（D24）──────────────────────────────────────

    #[test]
    fn retryable_error_enters_retryable_tier() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        let o = transition(&mut s, Event::ErrorRetryable);
        assert_eq!(s.state, State::ErroredRetryable);
        assert!(o.actions.contains(&Action::IncrementRetry));
        assert_eq!(s.retry_count, 1);
    }

    #[test]
    fn fatal_error_needs_user_confirmation() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        let o = transition(&mut s, Event::ErrorFatal);
        assert_eq!(s.state, State::ErroredUserConfirm);
        assert!(o.actions.contains(&Action::IncrementFailure));

        // §7-4：不自动重试。重试事件不应把它拉回 ENABLED。
        let before = s.state;
        let o = transition(&mut s, Event::RetryOk);
        assert!(o.illegal);
        assert_eq!(s.state, before);
    }

    /// P0-2：sidecar 崩溃与 `ErrorRetryable` 同形——**有预算的自动重试**。
    /// 预算内进可重试档；耗尽才升级为需用户确认。
    #[test]
    fn runtime_crash_uses_the_retry_budget_then_escalates() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        transition(&mut s, Event::Attach); // sidecar 跑起来的常态是 RUNNING

        let o = transition(&mut s, Event::RuntimeCrash);
        assert!(!o.illegal, "RUNNING 上的崩溃必须合法");
        assert_eq!(s.state, State::ErroredRetryable);
        assert!(o.actions.contains(&Action::IncrementRetry));
        assert_eq!(s.retry_count, 1);

        // 预算内重试（Enable → Attach）后再次崩溃：仍按预算走，不升级。
        transition(&mut s, Event::Enable);
        transition(&mut s, Event::Attach);
        let o = transition(&mut s, Event::RuntimeCrash);
        assert!(!o.illegal);
        assert_eq!(s.state, State::ErroredRetryable);
        assert_eq!(s.retry_count, 2);

        // 预算耗尽时再崩 → 需用户确认（不无限自动重启）。
        s.state = State::Enabled;
        s.retry_count = MAX_RETRY;
        let o = transition(&mut s, Event::RuntimeCrash);
        assert!(!o.illegal);
        assert_eq!(s.state, State::ErroredUserConfirm);
        assert!(o.actions.contains(&Action::IncrementFailure));
    }

    /// P0-2 + D28：试验性启用期间崩溃 → 回落 disabled-by-safemode，单计试验失败。
    #[test]
    fn runtime_crash_during_trial_falls_back_to_safemode_disabled() {
        let mut s = PluginState {
            state: State::Running,
            disabled_by_safemode: true,
            trial_from_safemode: true,
            ..Default::default()
        };
        let before = s.trial_failures;
        let o = transition(&mut s, Event::RuntimeCrash);
        assert!(!o.illegal);
        assert_eq!(s.state, State::Disabled);
        assert!(s.disabled_by_safemode, "试验中崩溃必须回落安全模式禁用");
        assert!(!s.trial_from_safemode, "试验标记必须清除");
        assert_eq!(s.trial_failures, before + 1, "必须计入试验失败预算");
        assert!(o.actions.contains(&Action::IncrementTrialFailure));
    }

    #[test]
    fn user_confirm_enable_resets_counters() {
        // D27：用户显式重启用 = 计数清零 + 新观察窗口。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        // 先耗尽重试预算，再致命失败：两类计数都被污染。
        transition(&mut s, Event::ErrorRetryable);
        transition(&mut s, Event::Enable);
        assert_eq!(s.retry_count, 1);
        transition(&mut s, Event::ErrorFatal);
        assert_eq!(s.failure_count, 1);
        assert!(!s.circuit_open, "单次失败不应熔断");

        let o = transition(&mut s, Event::Enable);
        assert!(!o.illegal);
        assert_eq!(s.state, State::Enabled);
        assert!(o.actions.contains(&Action::ResetCounters));
        assert_eq!(s.failure_count, 0);
        assert_eq!(s.retry_count, 0);
        assert!(!s.circuit_open);
    }

    // ── 自动重试上限与升级 ───────────────────────────────────────

    #[test]
    fn retry_budget_exhausts_then_escalates() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);

        // 三次重试耗尽预算；预算耗尽前的自动重试都还能回到 ENABLED。
        for i in 0..MAX_RETRY {
            let o = transition(&mut s, Event::ErrorRetryable);
            assert_eq!(s.state, State::ErroredRetryable, "第 {i} 次：预算内应进可重试档");
            assert!(o.actions.contains(&Action::IncrementRetry));
            if s.retry_count < MAX_RETRY {
                let o = transition(&mut s, Event::Enable);
                assert!(!o.illegal, "预算内重试不应非法");
                assert_eq!(s.state, State::Enabled);
            }
        }
        assert_eq!(s.retry_count, MAX_RETRY);
        assert_eq!(s.state, State::ErroredRetryable);

        // 预算耗尽后：自动重试不再回到 ENABLED，而是升级为需用户确认。
        let o = transition(&mut s, Event::Enable);
        assert!(o.actions.contains(&Action::IncrementFailure));
        assert_eq!(s.state, State::ErroredUserConfirm);
    }

    #[test]
    fn retry_ok_resets_counters() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        transition(&mut s, Event::ErrorRetryable);
        assert_eq!(s.retry_count, 1);

        let o = transition(&mut s, Event::RetryOk);
        assert_eq!(s.state, State::Enabled);
        assert!(o.actions.contains(&Action::ResetCounters));
        assert_eq!(s.retry_count, 0);
        assert!(!s.circuit_open);
    }

    #[test]
    fn retry_exhausted_escalates_to_user_confirm() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        transition(&mut s, Event::ErrorRetryable);
        let o = transition(&mut s, Event::RetryExhausted);
        assert_eq!(s.state, State::ErroredUserConfirm);
        assert!(o.actions.contains(&Action::IncrementFailure));
    }

    // ── D3 出边补齐 ──────────────────────────────────────────────

    #[test]
    fn running_to_errored_is_present() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        transition(&mut s, Event::Attach);
        assert_eq!(s.state, State::Running);

        let o = transition(&mut s, Event::ErrorFatal);
        assert_eq!(s.state, State::ErroredUserConfirm, "D3：RUNNING→ERRORED");
        assert!(!o.illegal);
    }

    #[test]
    fn disabled_can_uninstall() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Disable);
        assert_eq!(s.state, State::Disabled);
        assert_eq!(transition(&mut s, Event::Uninstall).to, State::Uninstalled);
    }

    #[test]
    fn errored_both_tiers_reach_all_three_outbound_targets() {
        for target in [State::Enabled, State::Disabled, State::Uninstalled] {
            // ERRORED_RETRYABLE → {ENABLED, DISABLED, UNINSTALLED}
            let mut s = PluginState::default();
            transition(&mut s, Event::InstallStart);
            transition(&mut s, Event::InstallOk);
            transition(&mut s, Event::Enable);
            transition(&mut s, Event::ErrorRetryable);
            assert_eq!(s.state, State::ErroredRetryable);
            let ev = match target {
                State::Enabled => Event::Enable,
                State::Disabled => Event::Disable,
                State::Uninstalled => Event::Uninstall,
                _ => unreachable!(),
            };
            let o = transition(&mut s, ev);
            assert_eq!(o.to, target, "ERRORED_RETRYABLE → {target}");

            // ERRORED_USER_CONFIRM → {ENABLED, DISABLED, UNINSTALLED}
            let mut s = PluginState::default();
            transition(&mut s, Event::InstallStart);
            transition(&mut s, Event::InstallOk);
            transition(&mut s, Event::Enable);
            transition(&mut s, Event::ErrorFatal);
            assert_eq!(s.state, State::ErroredUserConfirm);
            let o = transition(&mut s, ev);
            assert_eq!(o.to, target, "ERRORED_USER_CONFIRM → {target}");
        }
    }

    // ── 熔断 ─────────────────────────────────────────────────────

    #[test]
    fn circuit_opens_after_threshold_failures() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        for _ in 0..CIRCUIT_THRESHOLD {
            transition(&mut s, Event::ErrorFatal);
            // 被熔断后 Disable 到 DISABLED。
            if s.state == State::ErroredUserConfirm {
                transition(&mut s, Event::Disable);
                // 重新启用（计数已清零）。
                transition(&mut s, Event::Enable);
            }
        }
        // 熔断后重新启用 = 计数清零（D27）：不会持续处于熔断态。
        assert_eq!(s.failure_count, 0, "重新启用后计数应清零（D27）");
        assert!(!s.circuit_open, "重新启用后熔断应关闭（D27）");
        assert_eq!(s.state, State::Enabled);
        // 直接验证熔断阈值本身。
        let mut s2 = PluginState::default();
        transition(&mut s2, Event::InstallStart);
        transition(&mut s2, Event::InstallOk);
        transition(&mut s2, Event::Enable);
        for _ in 0..CIRCUIT_THRESHOLD {
            transition(&mut s2, Event::ErrorFatal);
        }
        assert!(s2.circuit_open, "连续 {CIRCUIT_THRESHOLD} 次失败应熔断");
        // 阈值 -1 次不应熔断。
        let mut s3 = PluginState::default();
        transition(&mut s3, Event::InstallStart);
        transition(&mut s3, Event::InstallOk);
        transition(&mut s3, Event::Enable);
        for _ in 0..(CIRCUIT_THRESHOLD - 1) {
            transition(&mut s3, Event::ErrorFatal);
        }
        assert!(!s3.circuit_open, "少于阈值不应熔断");
        assert_eq!(s3.failure_count, CIRCUIT_THRESHOLD - 1);
    }

    #[test]
    fn health_window_decays_counters() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        // 两次失败 + 两次自动重试 → 回到 ENABLED，预算已消耗 2。
        for _ in 0..2 {
            transition(&mut s, Event::ErrorRetryable);
            transition(&mut s, Event::Enable);
        }
        assert_eq!(s.state, State::Enabled);
        assert_eq!(s.retry_count, 2);

        let o = transition(&mut s, Event::HealthOk);
        assert!(o.actions.contains(&Action::DecayRetry));
        assert_eq!(s.retry_count, 1);
        let o = transition(&mut s, Event::HealthOk);
        assert!(o.actions.contains(&Action::DecayRetry));
        assert_eq!(s.retry_count, 0);
        // 饱和于 0，不越界（usize 下溢会 panic）。
        let o = transition(&mut s, Event::HealthOk);
        assert!(o.actions.contains(&Action::DecayRetry));
        assert_eq!(s.retry_count, 0);
    }

    #[test]
    fn health_window_closes_circuit_once_all_zero() {
        // 熔断自动恢复：retry 与 failure 同时归零。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        apply(&mut s, Action::SetCircuitOpen);
        s.failure_count = 2;
        assert!(s.circuit_open);
        let o = transition(&mut s, Event::HealthOk);
        assert!(o.actions.contains(&Action::DecayRetry));
        assert!(s.circuit_open, "failure_count 尚未归零时不应关断熔断");
        let o = transition(&mut s, Event::HealthOk);
        assert!(o.actions.contains(&Action::DecayRetry));
        assert!(!s.circuit_open, "计数全归零后熔断应自动关闭");
    }

    // ── 安全模式三级降级（D25/D28）───────────────────────────────

    #[test]
    fn boot_tier_three_levels() {
        assert_eq!(boot_tier(0, false), BootTier::Normal);
        assert_eq!(boot_tier(1, false), BootTier::Normal);
        assert_eq!(boot_tier(2, false), BootTier::Safemode);
        assert_eq!(boot_tier(3, false), BootTier::Safemode);
        // 安全模式下再失败 → 修复模式（D25：堵住永久 boot loop）。
        assert_eq!(boot_tier(2, true), BootTier::Repair);
        assert_eq!(boot_tier(100, true), BootTier::Repair);
    }

    #[test]
    fn trial_enable_falls_back_on_single_failure() {
        // D28：试验性启用，1 次即回落 disabled-by-safemode。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::SafemodeEnter);
        assert_eq!(s.state, State::Disabled);
        assert!(s.disabled_by_safemode);

        let o = transition(&mut s, Event::TrialEnable);
        assert!(!o.illegal, "safemode 禁用者应可试验性启用");
        assert_eq!(s.state, State::Enabled);
        assert!(s.trial_from_safemode);
        assert!(!s.disabled_by_safemode);

        // 出错 → 回落 DISABLED + 重新标 safemode，且只消耗 trial 预算。
        let o = transition(&mut s, Event::ErrorRetryable);
        assert_eq!(s.state, State::Disabled);
        assert!(o.actions.contains(&Action::IncrementTrialFailure));
        assert!(s.disabled_by_safemode);
        assert!(!s.trial_from_safemode);
        assert_eq!(s.trial_failures, 1);
        // 关键：不累入重试/失败预算（应用级启动计数不受污染）。
        assert_eq!(s.retry_count, 0);
        assert_eq!(s.failure_count, 0);
    }

    #[test]
    fn enable_from_safemode_disable_clears_the_flag() {
        // 回归测试：safemode 禁用者被用户显式 Enable 时必须清 `disabled_by_safemode`。
        // 只迁移状态不清标志会让插件停在「state=Enabled 但标志仍 true」——
        // `PluginSummary.disabledBySafemode` 永远说谎，且恢复对账补发的
        // `SafemodeExit` 只认 DISABLED 态、会被当非法迁移忽略，标志再无机会清零。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::SafemodeEnter);
        assert!(s.disabled_by_safemode);

        let o = transition(&mut s, Event::Enable);
        assert!(!o.illegal, "DISABLED → ENABLED 必须合法");
        assert_eq!(s.state, State::Enabled);
        assert!(!s.disabled_by_safemode, "显式启用必须清除 safemode 标志");
        assert!(o.actions.contains(&Action::ClearSafemode));

        // 闭环：标志清了之后，恢复引擎若仍判定应禁用，对账补发的 SafemodeEnter
        // 从 ENABLED 态是合法迁移——安全模式权威不被这条 Enable 旁路。
        let o = transition(&mut s, Event::SafemodeEnter);
        assert!(!o.illegal);
        assert_eq!(s.state, State::Disabled);
        assert!(s.disabled_by_safemode);
    }

    #[test]
    fn trial_budget_is_limited() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::SafemodeEnter);

        for _ in 0..MAX_TRIAL_ATTEMPTS {
            transition(&mut s, Event::TrialEnable);
            assert_eq!(s.state, State::Enabled);
            transition(&mut s, Event::ErrorRetryable);
            assert_eq!(s.state, State::Disabled);
        }
        assert_eq!(s.trial_failures, MAX_TRIAL_ATTEMPTS);

        // 预算耗尽：再试启 = 非法迁移（不静默忽略）。
        let before = s.state;
        let o = transition(&mut s, Event::TrialEnable);
        assert!(o.illegal, "试验预算耗尽后 TrialEnable 应非法");
        assert_eq!(s.state, before);
    }

    #[test]
    fn trial_enable_requires_safemode_flag() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        // 非 safemode 禁用的插件不得试验性启用。
        transition(&mut s, Event::Disable);
        assert_eq!(s.state, State::Disabled);
        assert!(!s.disabled_by_safemode);
        let o = transition(&mut s, Event::TrialEnable);
        assert!(o.illegal);
    }

    #[test]
    fn safemode_exit_restores_enabled() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::SafemodeEnter);
        let o = transition(&mut s, Event::SafemodeExit);
        assert!(!o.illegal, "safemode 禁用语义下退出应合法");
        assert_eq!(s.state, State::Enabled);
        assert!(!s.disabled_by_safemode);
        assert_eq!(s.retry_count, 0);
    }

    // ── R8：reason 不参与分支 ────────────────────────────────────

    #[test]
    fn reason_does_not_affect_branching() {
        // 两种完全不同（甚至带插件前缀）的 reason，迁移结果必须一致。
        let run = |reason: &str| {
            let mut s = PluginState::default();
            transition(&mut s, Event::InstallStart);
            transition(&mut s, Event::InstallOk);
            let o = transition_with(TRANSITIONS, &mut s, Event::Enable, Some(reason));
            (s.state, s.retry_count, s.failure_count, o.illegal)
        };
        assert_eq!(run("plugin:com.example.x:boom"), run("plain reason"));
        assert_eq!(run(""), run("anything@all"));
    }

    #[test]
    fn reason_is_recorded_for_telemetry_only() {
        let mut s = PluginState::default();
        transition_with(TRANSITIONS, &mut s, Event::InstallStart, Some("user click"));
        assert_eq!(s.last_reason.as_deref(), Some("user click"));
        assert_eq!(s.last_event, Some(Event::InstallStart));
    }

    // ── 非法迁移计数 ─────────────────────────────────────────────

    #[test]
    fn illegal_transitions_are_counted() {
        let mut s = PluginState::default();
        // 未安装就卸载。
        let o = transition(&mut s, Event::Uninstall);
        assert!(o.illegal);
        assert_eq!(s.illegal_transitions, 1);
        assert_eq!(s.state, State::Discovered);
    }

    #[test]
    fn uninstalled_is_absorbing() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Uninstall);
        assert_eq!(s.state, State::Uninstalled);
        for ev in Event::ALL {
            let before = s.state;
            let o = transition(&mut s, ev);
            assert!(o.illegal, "终态上任何事件都应非法：{ev}");
            assert_eq!(s.state, before);
        }
    }

    // ── 动作不反向迁移（计划 §4.3）───────────────────────────────

    #[test]
    fn actions_never_change_state() {
        // 逐一应用每个动作，state 必须不变。
        for a in [
            Action::IncrementRetry,
            Action::IncrementFailure,
            Action::IncrementTrialFailure,
            Action::ResetCounters,
            Action::ResetRuntimeCounters,
            Action::DecayRetry,
            Action::SetCircuitOpen,
            Action::SetSafemodeDisabled,
            Action::ClearSafemode,
            Action::SetTrial,
            Action::ClearTrial,
        ] {
            let mut s = PluginState::default();
            s.state = State::Running;
            s.retry_count = 2;
            s.failure_count = 2;
            s.trial_failures = 1;
            apply(&mut s, a);
            assert_eq!(s.state, State::Running, "动作 {a:?} 不得改变 state");
        }
    }

    #[test]
    fn reset_counters_clears_everything() {
        let mut s = PluginState {
            retry_count: 5,
            failure_count: 7,
            trial_failures: 3,
            circuit_open: true,
            disabled_by_safemode: true,
            trial_from_safemode: true,
            ..Default::default()
        };
        apply(&mut s, Action::ResetCounters);
        assert_eq!(s.retry_count, 0);
        assert_eq!(s.failure_count, 0);
        assert_eq!(s.trial_failures, 0);
        assert!(!s.circuit_open);
        // 注意：ResetCounters 不清 safemode/trial 标志位（那是身份语义，不是计数）。
        assert!(s.disabled_by_safemode);
        assert!(s.trial_from_safemode);
    }

    // ── 链深上限 ─────────────────────────────────────────────────

    #[test]
    fn chain_depth_capped() {
        // 构造一条自链到爆炸的表，验证深度上限生效。
        let cyclic = &[
            Rule { from: State::Discovered, event: Event::InstallStart, guard: Guard::None, to: State::Discovered, actions: &[], chain: Some(Event::InstallStart) },
        ];
        let mut s = PluginState::default();
        let o = transition_with(cyclic, &mut s, Event::InstallStart, None);
        assert!(o.illegal, "链深超限应记非法");
        assert!(o.depth > MAX_CHAIN_DEPTH);
        assert!(s.illegal_transitions >= 1);
    }

    #[test]
    fn single_step_depth_is_one() {
        let mut s = PluginState::default();
        let o = transition(&mut s, Event::InstallStart);
        assert_eq!(o.depth, 1);
    }

    // ── 序列化 ───────────────────────────────────────────────────

    #[test]
    fn state_machine_roundtrips() {
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        let json = serde_json::to_string(&s).unwrap();
        let back: PluginState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state, State::Enabled);
        // 默认构造可反序列化（serde(default)）。
        let empty: PluginState = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.state, State::Discovered);
    }

    #[test]
    fn state_and_event_serialize_screeaming() {
        assert_eq!(serde_json::to_string(&State::Enabled).unwrap(), "\"ENABLED\"");
        assert_eq!(serde_json::to_string(&Event::RetryOk).unwrap(), "\"RETRY_OK\"");
        assert_eq!(serde_json::to_string(&BootTier::Safemode).unwrap(), "\"safemode\"");
    }

    // ── D24：语义矛盾消除验证 ────────────────────────────────────

    #[test]
    fn errored_two_tiers_are_distinct_and_not_contradictory() {
        // §4.3 原图示"ERRORED→(自动重试/熔断)" 与 §7"不自动重试，需用户确认"矛盾。
        // 拆档后：retryable 可自动重试，user-confirm 不可。两条规则互斥。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        transition(&mut s, Event::ErrorRetryable);
        assert_eq!(s.state, State::ErroredRetryable);
        // retryable 档：Enable 合法（自动重试）。
        assert!(!transition(&mut s, Event::Enable).illegal);

        let mut s2 = s.clone();
        s2.state = State::ErroredUserConfirm;
        // user-confirm 档：RetryOk 非法（§7-4 不自动重试）。
        assert!(transition(&mut s2, Event::RetryOk).illegal);
        // 但 Enable（用户确认）合法。
        assert!(!transition(&mut s2, Event::Enable).illegal);
    }

    #[test]
    fn enable_from_disabled_never_immediately_recircuits() {
        // D27 死循环回归：被熔断的插件重新启用后，不应"启用即再熔断"。
        let mut s = PluginState::default();
        transition(&mut s, Event::InstallStart);
        transition(&mut s, Event::InstallOk);
        transition(&mut s, Event::Enable);
        for _ in 0..CIRCUIT_THRESHOLD {
            transition(&mut s, Event::ErrorFatal);
        }
        assert!(s.circuit_open);
        transition(&mut s, Event::Disable);
        assert!(s.circuit_open);

        // 用户重启用 → 计数清零、熔断关闭。
        transition(&mut s, Event::Enable);
        assert!(!s.circuit_open, "重启用必须清除熔断标记");
        assert_eq!(s.failure_count, 0);

        // 熔断不会在启用瞬间再次打开：需重新累积 CIRCUIT_THRESHOLD 次失败。
        // 等待确认期间再出错会留在 ERRORED_USER_CONFIRM 并继续累计（自环规则）。
        for _ in 0..(CIRCUIT_THRESHOLD - 1) {
            transition(&mut s, Event::ErrorFatal);
        }
        assert!(!s.circuit_open, "少一次失败不应熔断");
        assert_eq!(s.failure_count, CIRCUIT_THRESHOLD - 1);
        transition(&mut s, Event::ErrorFatal);
        assert!(s.circuit_open, "累积满 {CIRCUIT_THRESHOLD} 次后应再次熔断");
        assert_eq!(s.failure_count, CIRCUIT_THRESHOLD);
    }
}
