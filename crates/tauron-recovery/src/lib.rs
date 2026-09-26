// §4.14 崩溃恢复 + 安全模式。
//
// 职责（计划 §4.14）：干净退出标记 + 启动校验 + 幂等快照 +
// 恢复向导 + 启动失败计数与安全模式。
//
// 关键约束（D25/D28/D17/D31）：
// - 连续 2 次未正常完成启动 → 第 3 次进安全模式，只加载必需插件；
// - 安全模式下**再失败 → 修复模式**（零插件最小壳 + 错误报告 + 重装/修复引导），
//   堵住"必需插件自身崩溃 → 永久 boot loop"；
// - `--enable-plugin` 属试验性启用，该插件单独计失败、1 次即回落
//   `disabled-by-safemode`，不累入全局计数；
// - 幂等键 = `plugin_id × action_kind × 快照 seq`；
// - 已执行集 = 独立追加式 store、随快照 seq 截断 GC、
//   写入者=恢复动作执行器；
// - 恢复自身幂等（恢复中标记 + 中断续作）；
// - **禁止解析日志字符串**——所有判定基于标记文件与计数。

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

pub mod error;

pub use error::{RecoveryError, RecoveryResult};

/// 启动阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootPhase {
    /// 正常启动。
    Normal,
    /// 安全模式：只加载必需插件。
    Safemode,
    /// 修复模式：零插件最小壳。
    Repairmode,
}

impl BootPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            BootPhase::Normal => "normal",
            BootPhase::Safemode => "safemode",
            BootPhase::Repairmode => "repairmode",
        }
    }
}

impl std::fmt::Display for BootPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 插件状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginState {
    /// 正常启用。
    Enabled,
    /// 安全模式禁用。
    DisabledBySafemode,
    /// 修复模式禁用（仅必需插件在此模式启用）。
    DisabledByRepairmode,
    /// 安全模式下试验性启用。
    TrialEnable,
    /// 已永久禁用。
    PermanentlyDisabled,
}

impl PluginState {
    pub fn is_enabled(self) -> bool {
        matches!(self, PluginState::Enabled | PluginState::TrialEnable)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PluginState::Enabled => "enabled",
            PluginState::DisabledBySafemode => "disabled-by-safemode",
            PluginState::DisabledByRepairmode => "disabled-by-repairmode",
            PluginState::TrialEnable => "trial-enable",
            PluginState::PermanentlyDisabled => "permanently-disabled",
        }
    }
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条**关键事件摘要**（R7「持久化对称」：崩溃后留诊断上下文）。
///
/// 语义边界（刻意不新造平行概念）：
/// - 「故障类型」不是新枚举，而是 `(phase, trial)` 这个**既有类型**的二元组——
///   `phase` 是失败发生时引擎所处的 [`BootPhase`]（正常/安全/修复），
///   `trial` 表示这是不是 `--enable-plugin` 试验性启用插件的失败
///   （走 [`RecoveryEngine::record_trial_failure_at`] 而非全局计数）；
/// - 「故障插件 id」就是 `record_boot_failure` 收的那个 `plugin_id`
///   （`None` = 宿主自身 / 未能归因到具体插件）；
/// - 「摘要」= 该次失败后的既有计数器读数与既有 [`PluginState`]。
///
/// 这只是**记录**，不参与任何判定：判定仍只看标记文件与计数器。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootContextEntry {
    /// 事件时间戳（epoch 毫秒）。`0` = 调用方未提供时钟（引擎自身无时钟依赖）。
    pub ts: u64,
    /// 失败发生时引擎所处的启动阶段。
    pub phase: BootPhase,
    /// 故障插件 id（`None` = 宿主自身 / 未能归因）。
    pub plugin_id: Option<String>,
    /// 是否属试验性启用插件的失败。
    pub trial: bool,
    /// 该次失败后的连续失败计数。
    pub consecutive_failures: u32,
    /// 该次失败后的安全模式内失败计数。
    pub safemode_failures: u32,
    /// 该次失败后该插件的状态（`None` = 引擎未登记该插件）。
    pub plugin_state: Option<PluginState>,
}

impl BootContextEntry {
    /// 故障类型的**派生**线名（由 `phase` + `trial` 推出，只为展示方便）。
    ///
    /// 这是派生标签而非新概念：它没有独立存储、不参与判定，
    /// 改 `phase`/`trial` 的语义它自动跟着变。
    pub fn failure_kind(&self) -> &'static str {
        match (self.phase, self.trial) {
            (_, true) => "trial-failure",
            (BootPhase::Normal, false) => "boot-failure",
            (BootPhase::Safemode, false) => "safemode-failure",
            (BootPhase::Repairmode, false) => "repairmode-failure",
        }
    }
}

/// 恢复快照元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotMeta {
    pub schema_version: String,
    pub seq: u64,
    pub writer: String,
    pub ts: u64,
}

/// 恢复动作。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryAction {
    pub plugin_id: String,
    pub action_kind: String,
    /// 幂等键：`plugin_id:action_kind:seq`。
    pub idempotency_key: String,
}

impl RecoveryAction {
    pub fn idempotency_key(plugin_id: &str, action_kind: &str, seq: u64) -> String {
        format!("{plugin_id}:{action_kind}:{seq}")
    }
}

/// 已执行效果记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectRecord {
    pub effect_id: String,
    pub action_key: String,
    pub ts: u64,
}

/// 启动失败计数器。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BootCounter {
    /// 连续未正常完成的启动次数。
    pub consecutive_failures: u32,
    /// 安全模式下连续失败次数。
    pub safemode_failures: u32,
    /// 试验性启用（--enable-plugin）的失败记录：plugin_id → 次数。
    pub trial_failures: BTreeMap<String, u32>,
}

impl BootCounter {
    /// 连续失败阈值：达到后下次启动进安全模式。
    pub const SAFEMODE_THRESHOLD: u32 = 2;

    /// 安全模式内失败阈值：达到后进修复模式。
    pub const REPAIR_THRESHOLD: u32 = 1;

    /// 试验启用次数上限。
    pub const TRIAL_MAX_ATTEMPTS: u32 = 1;

    /// 当前阶段应由计数器决定。
    pub fn decide_phase(&self) -> BootPhase {
        if self.consecutive_failures >= Self::SAFEMODE_THRESHOLD {
            // 正常模式连续失败 ≥ 2 → 安全模式。
            // 但安全模式内失败应累积 safemode_failures。
            if self.safemode_failures >= Self::REPAIR_THRESHOLD {
                BootPhase::Repairmode
            } else {
                BootPhase::Safemode
            }
        } else {
            BootPhase::Normal
        }
    }

    /// 记录一次启动失败。
    pub fn record_failure(&mut self, phase: BootPhase, plugin_id: Option<&str>) {
        match phase {
            BootPhase::Normal => {
                self.consecutive_failures += 1;
            }
            BootPhase::Safemode => {
                // 全局计数也加（保持单调），但判定看 safemode_failures。
                self.consecutive_failures += 1;
                self.safemode_failures += 1;
                if let Some(pid) = plugin_id {
                    *self.trial_failures.entry(pid.to_string()).or_insert(0) += 1;
                }
            }
            BootPhase::Repairmode => {
                self.consecutive_failures += 1;
                self.safemode_failures += 1;
            }
        }
    }

    /// 记录一次正常完成。清零所有计数器。
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.safemode_failures = 0;
        self.trial_failures.clear();
    }

    /// 试验启用是否耗尽。
    pub fn trial_exhausted(&self, plugin_id: &str) -> bool {
        self.trial_failures.get(plugin_id).copied().unwrap_or(0) >= Self::TRIAL_MAX_ATTEMPTS
    }

    /// 回落到安全模式禁用。
    pub fn mark_trial_failed(&mut self, plugin_id: &str) {
        *self.trial_failures.entry(plugin_id.to_string()).or_insert(0) += 1;
    }
}

/// 恢复引擎：管理启动阶段决策、插件状态、幂等动作、效果追踪。
pub struct RecoveryEngine {
    /// 计数器。
    counter: BootCounter,
    /// 当前阶段。
    phase: BootPhase,
    /// 插件状态表：plugin_id → 状态。
    plugin_states: BTreeMap<String, PluginState>,
    /// 必需插件集合。
    required_plugins: HashSet<String>,
    /// 全部已知插件（含非必需）。
    all_plugins: HashSet<String>,
    /// 已执行的恢复动作幂等键集合。
    executed_actions: HashSet<String>,
    /// 已执行的外部效果集合。
    executed_effects: HashSet<String>,
    /// 快照元数据。
    snapshot_meta: Option<SnapshotMeta>,
    /// 恢复中标记（恢复自身幂等）。
    recovery_in_progress: bool,
    /// 关键事件摘要（最旧在前，长度 ≤ [`Self::CONTEXT_CAPACITY`]）。
    context: Vec<BootContextEntry>,
}

impl RecoveryEngine {
    /// 关键事件摘要的条数上限。
    ///
    /// 为什么是 8：把一次「走到修复模式」的完整恶化路径（正常模式连续 2 次失败
    /// → 安全模式 1 次失败 → 修复模式）连同安全模式内逐插件试验失败
    /// （`trial_failures` 是 per-plugin 的，多插件场景一次能添好几条）一起装下，
    /// 还留出余量。上下文会随标记文件在**每次启动/上报**时重写，
    /// 8 条 × 每条 ~120 字节 ≈ 1 KB，对启动路径的写放大可忽略。
    pub const CONTEXT_CAPACITY: usize = 8;

    pub fn new(required_plugins: HashSet<String>) -> Self {
        Self {
            counter: BootCounter::default(),
            phase: BootPhase::Normal,
            plugin_states: BTreeMap::new(),
            required_plugins,
            all_plugins: HashSet::new(),
            executed_actions: HashSet::new(),
            executed_effects: HashSet::new(),
            snapshot_meta: None,
            recovery_in_progress: false,
            context: Vec::new(),
        }
    }

    // ── 关键事件摘要（崩溃诊断上下文）──────────────────────────────

    /// 最近的关键事件摘要（最旧在前）。
    pub fn context(&self) -> &[BootContextEntry] {
        &self.context
    }

    /// 最近一条关键事件摘要。
    pub fn last_context(&self) -> Option<&BootContextEntry> {
        self.context.last()
    }

    /// 用**持久化**的上下文播种本轮环形缓冲（跨进程留痕）。
    ///
    /// 超容量的只保留最近 `CONTEXT_CAPACITY` 条：截断而不拒绝，也不 panic——
    /// 诊断字段不该有能力让启动失败。空/损坏的输入直接视为无 context。
    pub fn set_context(&mut self, mut context: Vec<BootContextEntry>) {
        if context.len() > Self::CONTEXT_CAPACITY {
            let excess = context.len() - Self::CONTEXT_CAPACITY;
            context.drain(0..excess);
        }
        self.context = context;
    }

    /// 追加一条摘要，满时丢最旧（环形）。
    fn push_context(&mut self, entry: BootContextEntry) {
        if self.context.len() >= Self::CONTEXT_CAPACITY {
            self.context.remove(0);
        }
        self.context.push(entry);
    }

    // ── 启动决策 ────────────────────────────────────────────────────

    /// 根据计数器决定启动阶段。
    pub fn decide_boot_phase(&self) -> BootPhase {
        self.counter.decide_phase()
    }

    /// 记录启动失败（由调用方在启动异常退出时调用）。
    ///
    /// 时间戳记 `0`（未知）。有真实时钟的调用方用
    /// [`Self::record_boot_failure_at`]。
    pub fn record_boot_failure(&mut self, plugin_id: Option<&str>) {
        self.record_boot_failure_at(plugin_id, 0);
    }

    /// 记录启动失败，并附上时间戳（写进关键事件摘要）。
    ///
    /// `plugin_id` 有**两重**语义，与 [`BootCounter::record_failure`] 一致：
    /// 安全模式下它同时进该插件的试验失败预算，并被记进摘要。
    ///
    /// `phase` 的语义：摘要里存的是**失败发生时**所处的阶段，而不是
    /// 计数之后的阶段——「在安全模式里又失败」与「正常模式里的第一次失败」
    /// 是两类不同的故障，阈值交叉后这两个值会不同。
    pub fn record_boot_failure_at(&mut self, plugin_id: Option<&str>, ts: u64) {
        self.counter.record_failure(self.phase, plugin_id);
        self.finish_boot_failure(plugin_id, ts);
    }

    /// 记录一次**归因到某插件、但不计入其试验预算**的启动失败。
    ///
    /// 适配层用这一条：`host_recover_report` 的 `pluginId` 是「疑似故障插件」
    /// 提示（宿主自己也不知道是谁崩的），它只该出现在诊断上下文里；
    /// 试验失败预算（`trial_failures`，1 次即回落、之后不可再试）只有真正的
    /// 试验性启用路径（[`Self::record_trial_failure_at`]）能入账。
    /// 把它顺手记进预算会让「安全模式里插件 A 崩了」永久禁掉 A 的试启用机会。
    pub fn record_boot_failure_suspected_at(&mut self, suspected: Option<&str>, ts: u64) {
        self.counter.record_failure(self.phase, None);
        self.finish_boot_failure(suspected, ts);
    }

    /// 计数之后的公共尾部：推进阶段 + 追加摘要。
    fn finish_boot_failure(&mut self, context_plugin_id: Option<&str>, ts: u64) {
        let phase_at_failure = self.phase;
        let new_phase = self.counter.decide_phase();
        if new_phase != self.phase {
            self.phase = new_phase;
            self.rebuild_plugin_states();
        }
        self.push_context(BootContextEntry {
            ts,
            phase: phase_at_failure,
            plugin_id: context_plugin_id.map(str::to_string),
            trial: false,
            consecutive_failures: self.counter.consecutive_failures,
            safemode_failures: self.counter.safemode_failures,
            // 取**计数与状态重建之后**的状态：诊断上下文关心的是
            // 「这个插件最后落到哪」。
            plugin_state: context_plugin_id.and_then(|pid| self.plugin_state(pid)),
        });
    }

    /// 记录正常完成（启动成功后调用，清零计数器）。
    pub fn record_boot_success(&mut self) {
        self.counter.record_success();
        self.phase = BootPhase::Normal;
        self.rebuild_plugin_states();
    }

    /// 当前启动阶段。
    pub fn phase(&self) -> BootPhase {
        self.phase
    }

    /// 计数器快照。
    pub fn counter(&self) -> &BootCounter {
        &self.counter
    }

    // ── 插件状态 ────────────────────────────────────────────────────

    /// 根据当前阶段重建插件状态表。
    fn rebuild_plugin_states(&mut self) {
        for pid in self.all_plugins.iter() {
            let is_required = self.required_plugins.contains(pid);
            let state = match self.phase {
                BootPhase::Normal => PluginState::Enabled,
                BootPhase::Safemode => {
                    if is_required {
                        PluginState::Enabled
                    } else {
                        PluginState::DisabledBySafemode
                    }
                }
                BootPhase::Repairmode => {
                    if is_required {
                        PluginState::Enabled
                    } else {
                        PluginState::DisabledByRepairmode
                    }
                }
            };
            self.plugin_states.insert(pid.clone(), state);
        }
    }

    /// 设置插件状态（仅用于初始化时注入完整插件列表）。
    pub fn set_plugin(&mut self, plugin_id: &str, is_required: bool) {
        if is_required {
            self.required_plugins.insert(plugin_id.to_string());
        }
        self.all_plugins.insert(plugin_id.to_string());
        self.plugin_states.entry(plugin_id.to_string()).or_insert(match self.phase {
            BootPhase::Normal => PluginState::Enabled,
            BootPhase::Safemode => {
                if is_required {
                    PluginState::Enabled
                } else {
                    PluginState::DisabledBySafemode
                }
            }
            BootPhase::Repairmode => {
                if is_required {
                    PluginState::Enabled
                } else {
                    PluginState::DisabledByRepairmode
                }
            }
        });
    }

    /// 取插件状态。
    pub fn plugin_state(&self, plugin_id: &str) -> Option<PluginState> {
        self.plugin_states.get(plugin_id).copied()
    }

    /// 登记一个注册表已知插件（必需性以当前 `required_plugins` 为准）。
    ///
    /// 适配层用它把注册表内容同步进引擎——引擎只知道自己被显式登记过的
    /// 插件，否则 `disabled_plugins()` 会漏掉注册表里真实存在的插件。
    /// 必需标记本身仍只由 `new` / `set_plugin` / `set_required_plugins` 写入。
    /// 重复调用是幂等的（`set_plugin` 用 `or_insert`，不改写既有状态）。
    ///
    /// # 返回
    ///
    /// 登记后的状态。
    pub fn register_plugin(&mut self, plugin_id: &str) -> PluginState {
        self.set_plugin(plugin_id, self.required_plugins.contains(plugin_id));
        self.plugin_state(plugin_id).unwrap_or(PluginState::Enabled)
    }

    /// 必需插件集合（权威来源是宿主配置，见 [`Self::set_required_plugins`]）。
    pub fn required_plugins(&self) -> &HashSet<String> {
        &self.required_plugins
    }

    /// 引擎已知的全部插件（必需 + 非必需）。
    pub fn all_plugins(&self) -> &HashSet<String> {
        &self.all_plugins
    }

    /// 列出所有被禁用的插件及原因。
    pub fn disabled_plugins(&self) -> Vec<(&str, PluginState)> {
        self.plugin_states
            .iter()
            .filter(|(_, s)| !s.is_enabled())
            .map(|(k, &v)| (k.as_str(), v))
            .collect()
    }

    /// 替换必需插件集合（宿主配置为权威来源，只增不行）。
    ///
    /// `set_plugin` 只会**追加**必需标记，无法撤销；若不移换整个集合，
    /// 宿主配置里被移除的必需插件会一直留在「安全模式下仍加载」名单里，
    /// 安全模式就退化成了普通启动。因此每次从宿主重新取配置时都应调用本方法。
    pub fn set_required_plugins(&mut self, plugins: HashSet<String>) {
        self.required_plugins = plugins.clone();
        // 必需插件必然是「已知插件」（`rebuild_plugin_states` 只遍历 all_plugins）。
        for pid in &self.required_plugins {
            self.all_plugins.insert(pid.clone());
        }
        self.rebuild_plugin_states();
    }

    /// 移除插件（卸载/清除时回收），与 `set_plugin` 对称。
    ///
    /// 引擎状态表会被宿主**持久化**，不回收会让已卸载插件的条目在快照里
    /// 无限累积，`disabled_plugins()` 也会报出早已不存在的插件 id。
    ///
    /// # 返回
    ///
    /// `true` 表示确实移除了至少一项（该插件此前在引擎里有过登记）。
    ///
    /// 注意：不回收 `counter.trial_failures`——试验失败记录是跨卸载保留的
    /// 历史记忆，只在 `record_boot_success`（干净完成）时清零。
    pub fn remove_plugin(&mut self, plugin_id: &str) -> bool {
        // 三张表都必须移除：不能短路。若 `all_plugins` 残留，紧接着的
        // `rebuild_plugin_states` 会把它重新写回状态表，回收就白做了。
        let in_states = self.plugin_states.remove(plugin_id).is_some();
        let in_known = self.all_plugins.remove(plugin_id);
        let in_required = self.required_plugins.remove(plugin_id);
        self.rebuild_plugin_states();
        in_states || in_known || in_required
    }

    /// 进入安全模式时的状态转换。
    pub fn enter_safemode(&mut self) {
        self.phase = BootPhase::Safemode;
        self.rebuild_plugin_states();
    }

    /// 进入修复模式。
    pub fn enter_repairmode(&mut self) {
        self.phase = BootPhase::Repairmode;
        self.rebuild_plugin_states();
    }

    // ── 试验性启用 ──────────────────────────────────────────────────

    /// 试验性启用一个插件（`--enable-plugin`）。
    ///
    /// 该插件单独计失败，1 次即回落 `disabled-by-safemode`，
    /// 不累入全局计数。
    pub fn trial_enable(&mut self, plugin_id: &str) -> RecoveryResult<()> {
        if self.phase != BootPhase::Safemode {
            return Err(RecoveryError::RestrictedInSafemode);
        }
        if self.counter.trial_exhausted(plugin_id) {
            return Err(RecoveryError::TrialExhausted(plugin_id.to_string()));
        }
        self.plugin_states.insert(plugin_id.to_string(), PluginState::TrialEnable);
        Ok(())
    }

    /// 记录试验启用的插件启动失败。
    ///
    /// 回落到 `disabled-by-safemode`，不累入全局计数器。
    pub fn record_trial_failure(&mut self, plugin_id: &str) -> PluginState {
        self.record_trial_failure_at(plugin_id, 0)
    }

    /// [`Self::record_trial_failure`] 的带时间戳变体（见
    /// [`Self::record_boot_failure_at`]）。
    pub fn record_trial_failure_at(&mut self, plugin_id: &str, ts: u64) -> PluginState {
        let phase_at_failure = self.phase;
        self.counter.mark_trial_failed(plugin_id);
        let state = PluginState::DisabledBySafemode;
        self.plugin_states.insert(plugin_id.to_string(), state);
        self.push_context(BootContextEntry {
            ts,
            phase: phase_at_failure,
            plugin_id: Some(plugin_id.to_string()),
            trial: true,
            consecutive_failures: self.counter.consecutive_failures,
            safemode_failures: self.counter.safemode_failures,
            plugin_state: Some(state),
        });
        state
    }

    // ── 幂等恢复动作 ────────────────────────────────────────────────

    /// 检查恢复动作是否已执行（幂等去重）。
    pub fn action_executed(&self, key: &str) -> bool {
        self.executed_actions.contains(key)
    }

    /// 执行恢复动作（幂等）。
    ///
    /// 返回 `Ok(true)` 表示本次执行；`Ok(false)` 表示已执行过（去重）。
    /// 返回 `Err` 表示恢复进行中（不可重入）。
    pub fn execute_action(&mut self, action: &RecoveryAction) -> RecoveryResult<bool> {
        if self.recovery_in_progress {
            return Err(RecoveryError::RecoveryInProgress);
        }
        if self.executed_actions.contains(&action.idempotency_key) {
            return Ok(false);
        }
        // 标记恢复进行中（恢复自身幂等）。
        self.recovery_in_progress = true;
        self.executed_actions.insert(action.idempotency_key.clone());
        // 注意：这里不立即清除 recovery_in_progress，
        // 因为恢复动作可能有副作用需要后续调用 complete_recovery。
        Ok(true)
    }

    /// 完成恢复（清除恢复中标记）。
    pub fn complete_recovery(&mut self) {
        self.recovery_in_progress = false;
    }

    /// 恢复中标记是否置位。
    pub fn is_recovery_in_progress(&self) -> bool {
        self.recovery_in_progress
    }

    // ── 外部副作用追踪 ──────────────────────────────────────────────

    /// 检查外部效果是否已执行。
    pub fn effect_executed(&self, effect_id: &str) -> bool {
        self.executed_effects.contains(effect_id)
    }

    /// 记录外部效果已执行（重放前查已执行集）。
    pub fn record_effect(&mut self, record: EffectRecord) {
        self.executed_effects.insert(record.effect_id);
    }

    // ── 快照管理 ────────────────────────────────────────────────────

    /// 设置快照元数据。
    pub fn set_snapshot(&mut self, meta: SnapshotMeta) -> RecoveryResult<()> {
        if meta.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(RecoveryError::SnapshotVersion(
                meta.schema_version,
                CURRENT_SCHEMA_VERSION.to_string(),
            ));
        }
        self.snapshot_meta = Some(meta);
        Ok(())
    }

    /// 快照元数据。
    pub fn snapshot_meta(&self) -> Option<&SnapshotMeta> {
        self.snapshot_meta.as_ref()
    }

    /// 当前快照 seq。
    pub fn current_seq(&self) -> u64 {
        self.snapshot_meta.as_ref().map_or(0, |m| m.seq)
    }

    /// 随快照 seq 截断已执行动作（GC）。
    pub fn gc_executed_actions(&mut self, before_seq: u64) -> usize {
        let before = self.executed_actions.len();
        self.executed_actions.retain(|key| {
            // 幂等键格式：plugin_id:action_kind:seq
            let parts: Vec<&str> = key.split(':').collect();
            if parts.len() >= 3 {
                parts[2].parse::<u64>().map_or(true, |seq| seq >= before_seq)
            } else {
                true // 无法解析的保留
            }
        });
        before - self.executed_actions.len()
    }

    // ── 序列化 / 反序列化 ───────────────────────────────────────────

    /// 序列化为 JSON（用于持久化）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "counter": self.counter,
            "phase": self.phase,
            "plugin_states": self.plugin_states,
            "required_plugins": self.required_plugins,
            "all_plugins": self.all_plugins,
            "executed_actions": self.executed_actions,
            "executed_effects": self.executed_effects,
            "snapshot_meta": self.snapshot_meta,
            // R7：关键事件摘要随引擎快照一起走（持久化载体落盘它，
            // 并在 envelope 顶层再放一份 `lastContext`，两者同源）。
            "context": self.context,
        })
    }

    /// 从 JSON 反序列化。
    pub fn from_json(v: &serde_json::Value) -> RecoveryResult<Self> {
        let counter = serde_json::from_value(v["counter"].clone())
            .map_err(|e| RecoveryError::SnapshotFormat(e.to_string()))?;
        let phase = serde_json::from_value(v["phase"].clone())
            .map_err(|e| RecoveryError::SnapshotFormat(e.to_string()))?;
        let plugin_states = serde_json::from_value(v["plugin_states"].clone())
            .map_err(|e| RecoveryError::SnapshotFormat(e.to_string()))?;
        let required_plugins = serde_json::from_value(v["required_plugins"].clone())
            .map_err(|e| RecoveryError::SnapshotFormat(e.to_string()))?;
        let all_plugins = v
            .get("all_plugins")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let executed_actions = serde_json::from_value(v["executed_actions"].clone())
            .map_err(|e| RecoveryError::SnapshotFormat(e.to_string()))?;
        let executed_effects = serde_json::from_value(v["executed_effects"].clone())
            .map_err(|e| RecoveryError::SnapshotFormat(e.to_string()))?;
        let snapshot_meta = v
            .get("snapshot_meta")
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::from_value(v.clone()).ok());
        // 上下文是**纯诊断**字段：缺失（旧标记文件）或损坏都视为无 context，
        // **绝不**让整份快照判定为损坏——那会平白把一次干净启动算成崩溃，
        // 诊断字段不该有能力改变安全模式判定。
        let context = v
            .get("context")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .unwrap_or_default();
        Ok(Self {
            counter,
            phase,
            plugin_states,
            required_plugins,
            all_plugins,
            executed_actions,
            executed_effects,
            snapshot_meta,
            recovery_in_progress: false,
            context,
        })
    }
}

/// 当前快照 schema 版本。
pub const CURRENT_SCHEMA_VERSION: &str = "1.0.0";

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_with_plugins() -> RecoveryEngine {
        let required: HashSet<String> = ["p.core".into()].into_iter().collect();
        let mut e = RecoveryEngine::new(required);
        e.set_plugin("p.core", true);
        e.set_plugin("p.audio", false);
        e.set_plugin("p.video", false);
        e
    }

    // ── 启动阶段决策 ────────────────────────────────────────────────

    #[test]
    fn initial_phase_is_normal() {
        let e = engine_with_plugins();
        assert_eq!(e.decide_boot_phase(), BootPhase::Normal);
    }

    #[test]
    fn two_failures_trigger_safemode() {
        let mut e = engine_with_plugins();
        assert_eq!(e.phase(), BootPhase::Normal);
        e.record_boot_failure(None);
        assert_eq!(e.phase(), BootPhase::Normal, "第 1 次失败仍在正常模式");
        e.record_boot_failure(None);
        assert_eq!(e.phase(), BootPhase::Safemode, "第 2 次失败后进安全模式");
    }

    #[test]
    fn safemode_failure_triggers_repairmode() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None); // 进 safemode
        assert_eq!(e.phase(), BootPhase::Safemode);
        e.record_boot_failure(Some("p.core")); // safemode 内失败
        assert_eq!(e.phase(), BootPhase::Repairmode, "安全模式内再失败 → 修复模式");
    }

    #[test]
    fn success_resets_counters() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        assert_eq!(e.phase(), BootPhase::Safemode);
        e.record_boot_success();
        assert_eq!(e.phase(), BootPhase::Normal);
        assert_eq!(e.counter().consecutive_failures, 0);
        assert_eq!(e.counter().safemode_failures, 0);
    }

    #[test]
    fn repairmode_requires_two_normal_failures_then_safemode_failure() {
        // 完整路径：Normal → Normal → Safemode → Repairmode。
        let mut e = engine_with_plugins();
        e.record_boot_failure(None); // 1
        assert_eq!(e.phase(), BootPhase::Normal);
        e.record_boot_failure(None); // 2
        assert_eq!(e.phase(), BootPhase::Safemode);
        e.record_boot_failure(None); // safemode 内失败
        assert_eq!(e.phase(), BootPhase::Repairmode);
    }

    // ── 插件状态 ────────────────────────────────────────────────────

    #[test]
    fn normal_mode_all_plugins_enabled() {
        let e = engine_with_plugins();
        assert_eq!(e.plugin_state("p.core"), Some(PluginState::Enabled));
        assert_eq!(e.plugin_state("p.audio"), Some(PluginState::Enabled));
    }

    #[test]
    fn safemode_disables_non_required() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        assert_eq!(e.phase(), BootPhase::Safemode);
        // 重建状态。
        e.enter_safemode();
        assert_eq!(
            e.plugin_state("p.core"),
            Some(PluginState::Enabled),
            "必需插件在安全模式仍启用"
        );
        assert_eq!(e.plugin_state("p.audio"), Some(PluginState::DisabledBySafemode));
        assert_eq!(e.plugin_state("p.video"), Some(PluginState::DisabledBySafemode));
    }

    #[test]
    fn repairmode_disables_non_required() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        assert_eq!(e.phase(), BootPhase::Repairmode);
        e.enter_repairmode();
        assert_eq!(e.plugin_state("p.core"), Some(PluginState::Enabled));
        assert_eq!(e.plugin_state("p.audio"), Some(PluginState::DisabledByRepairmode));
    }

    #[test]
    fn disabled_plugins_lists_only_disabled() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        let disabled = e.disabled_plugins();
        assert_eq!(disabled.len(), 2);
        assert!(disabled.iter().all(|(_, s)| *s == PluginState::DisabledBySafemode));
    }

    #[test]
    fn set_required_plugins_replaces_rather_than_only_adding() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        assert_eq!(e.plugin_state("p.audio").unwrap(), PluginState::DisabledBySafemode);
        // 从必需集合中移除 p.core → 它也必须被禁用。这是 `set_plugin`
        // 的只增语义做不到的，因此必须由宿主用本方法整体替换。
        e.set_required_plugins(HashSet::new());
        assert_eq!(e.plugin_state("p.core").unwrap(), PluginState::DisabledBySafemode);
        // 再加回来 → 恢复加载。
        let mut required = HashSet::new();
        required.insert("p.core".to_string());
        e.set_required_plugins(required);
        assert_eq!(e.plugin_state("p.core").unwrap(), PluginState::Enabled);
    }

    #[test]
    fn set_required_plugins_registers_new_plugin_as_known() {
        let mut e = RecoveryEngine::new(HashSet::new());
        let mut required = HashSet::new();
        required.insert("p.fresh".to_string());
        e.set_required_plugins(required);
        // 必需插件必须同时进入已知集合，否则 rebuild_plugin_states
        // 不会给它建状态表条目。
        assert!(e.plugin_state("p.fresh").is_some());
        assert_eq!(e.plugin_state("p.fresh").unwrap(), PluginState::Enabled);
    }

    #[test]
    fn remove_plugin_frees_persisted_entries() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        assert!(e.disabled_plugins().iter().any(|(id, _)| *id == "p.audio"));
        assert!(e.remove_plugin("p.audio"));
        // 状态表与已知集合都要清空（快照不会再带着已卸载的插件）。
        assert!(e.plugin_state("p.audio").is_none());
        assert!(!e.disabled_plugins().iter().any(|(id, _)| *id == "p.audio"));
        // 重复移除是幂等 no-op。
        assert!(!e.remove_plugin("p.audio"));
        // 从未登记的插件同样是 no-op（不误报）。
        assert!(!e.remove_plugin("p.never.seen"));
        // 其他插件不受影响。
        assert_eq!(e.plugin_state("p.core").unwrap(), PluginState::Enabled);
    }

    // ── 试验性启用 ──────────────────────────────────────────────────

    #[test]
    fn trial_enable_only_in_safemode() {
        let mut e = engine_with_plugins();
        // 正常模式下不可试验启用。
        assert!(matches!(e.trial_enable("p.audio"), Err(RecoveryError::RestrictedInSafemode)));
    }

    #[test]
    fn trial_enable_works_in_safemode() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        assert!(e.trial_enable("p.audio").is_ok());
        assert_eq!(e.plugin_state("p.audio"), Some(PluginState::TrialEnable));
    }

    #[test]
    fn trial_failure_disables_plugin_not_global() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        e.trial_enable("p.audio").unwrap();
        let state = e.record_trial_failure("p.audio");
        assert_eq!(state, PluginState::DisabledBySafemode);
        // 全局计数器不应增加（不累入全局）。
        assert_eq!(e.counter().safemode_failures, 0);
    }

    #[test]
    fn trial_exhausted_after_one_failure() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        e.trial_enable("p.audio").unwrap();
        e.record_trial_failure("p.audio");
        // 第二次试验启用应被拒。
        assert!(matches!(
            e.trial_enable("p.audio"),
            Err(RecoveryError::TrialExhausted(ref pid)) if pid == "p.audio"
        ));
    }

    #[test]
    fn trial_failures_per_plugin() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        // p.audio 和 p.video 各自独立计数。
        e.trial_enable("p.audio").unwrap();
        e.record_trial_failure("p.audio");
        e.trial_enable("p.video").unwrap();
        e.record_trial_failure("p.video");
        assert!(e.counter().trial_exhausted("p.audio"));
        assert!(e.counter().trial_exhausted("p.video"));
        // 全局 safemode_failures 不应增加。
        assert_eq!(e.counter().safemode_failures, 0);
    }

    // ── 幂等恢复动作 ────────────────────────────────────────────────

    #[test]
    fn idempotency_key_format() {
        let key = RecoveryAction::idempotency_key("p.audio", "restart", 42);
        assert_eq!(key, "p.audio:restart:42");
    }

    #[test]
    fn action_executed_returns_true_on_duplicate() {
        let mut e = engine_with_plugins();
        let action = RecoveryAction {
            plugin_id: "p.audio".into(),
            action_kind: "restart".into(),
            idempotency_key: RecoveryAction::idempotency_key("p.audio", "restart", 1),
        };
        assert!(e.execute_action(&action).unwrap(), "首次执行返回 true");
        e.complete_recovery();
        assert!(!e.execute_action(&action).unwrap(), "重复执行返回 false（去重）");
        e.complete_recovery();
    }

    #[test]
    fn recovery_in_progress_blocks_concurrent() {
        let mut e = engine_with_plugins();
        let action = RecoveryAction {
            plugin_id: "p.audio".into(),
            action_kind: "restart".into(),
            idempotency_key: RecoveryAction::idempotency_key("p.audio", "restart", 1),
        };
        assert!(e.execute_action(&action).unwrap());
        // 恢复进行中，第二次执行应报错。
        assert!(matches!(e.execute_action(&action), Err(RecoveryError::RecoveryInProgress)));
        e.complete_recovery();
    }

    #[test]
    fn action_executed_query() {
        let mut e = engine_with_plugins();
        let key = RecoveryAction::idempotency_key("p.audio", "restart", 1);
        assert!(!e.action_executed(&key));
        let action = RecoveryAction {
            plugin_id: "p.audio".into(),
            action_kind: "restart".into(),
            idempotency_key: key.clone(),
        };
        assert!(e.execute_action(&action).unwrap());
        assert!(e.action_executed(&key));
        e.complete_recovery();
    }

    // ── 外部副作用追踪 ──────────────────────────────────────────────

    #[test]
    fn effect_tracking_prevents_replay() {
        let mut e = engine_with_plugins();
        assert!(!e.effect_executed("effect-1"));
        e.record_effect(EffectRecord {
            effect_id: "effect-1".into(),
            action_key: "p.audio:restart:1".into(),
            ts: 1000,
        });
        assert!(e.effect_executed("effect-1"), "已执行的效果应标记");
        assert!(!e.effect_executed("effect-2"));
    }

    // ── 快照管理 ────────────────────────────────────────────────────

    #[test]
    fn snapshot_version_mismatch_is_rejected() {
        let mut e = engine_with_plugins();
        let meta = SnapshotMeta {
            schema_version: "2.0.0".into(),
            seq: 1,
            writer: "test".into(),
            ts: 1000,
        };
        assert!(matches!(
            e.set_snapshot(meta),
            Err(RecoveryError::SnapshotVersion(ref got, ref want))
                if got == "2.0.0" && want == CURRENT_SCHEMA_VERSION
        ));
    }

    #[test]
    fn snapshot_set_and_get() {
        let mut e = engine_with_plugins();
        let meta = SnapshotMeta {
            schema_version: CURRENT_SCHEMA_VERSION.into(),
            seq: 42,
            writer: "recovery-engine".into(),
            ts: 1000,
        };
        e.set_snapshot(meta).unwrap();
        assert_eq!(e.current_seq(), 42);
        assert_eq!(e.snapshot_meta().unwrap().writer, "recovery-engine");
    }

    #[test]
    fn gc_executed_actions_by_seq() {
        let mut e = engine_with_plugins();
        // 插入不同 seq 的动作。
        for seq in [1u64, 5, 10] {
            let action = RecoveryAction {
                plugin_id: "p.x".into(),
                action_kind: "restart".into(),
                idempotency_key: RecoveryAction::idempotency_key("p.x", "restart", seq),
            };
            e.execute_action(&action).unwrap();
            e.complete_recovery();
        }
        assert_eq!(e.executed_actions.len(), 3);
        // GC seq < 5 的动作。
        let removed = e.gc_executed_actions(5);
        assert_eq!(removed, 1, "seq=1 应被 GC");
        assert_eq!(e.executed_actions.len(), 2);
    }

    // ── 序列化 / 反序列化 ───────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();
        let action = RecoveryAction {
            plugin_id: "p.audio".into(),
            action_kind: "restart".into(),
            idempotency_key: RecoveryAction::idempotency_key("p.audio", "restart", 1),
        };
        e.execute_action(&action).unwrap();
        e.complete_recovery();

        let json = e.to_json();
        let e2 = RecoveryEngine::from_json(&json).unwrap();
        assert_eq!(e2.phase(), BootPhase::Safemode);
        assert_eq!(e2.counter().consecutive_failures, 2);
        assert_eq!(e2.plugin_state("p.audio"), Some(PluginState::DisabledBySafemode));
    }

    #[test]
    fn from_json_rejects_corrupt_data() {
        let json = serde_json::json!({"counter": "not_an_object"});
        assert!(RecoveryEngine::from_json(&json).is_err());
    }

    // ── 关键事件摘要（R7：崩溃诊断上下文）──────────────────────────

    #[test]
    fn context_records_phase_plugin_and_failure_kind() {
        let mut e = engine_with_plugins();
        e.record_boot_failure_at(Some("p.audio"), 111);
        let c = e.last_context().expect("失败必须留一条摘要");
        assert_eq!(c.ts, 111);
        assert_eq!(c.plugin_id.as_deref(), Some("p.audio"));
        assert_eq!(c.phase, BootPhase::Normal, "摘要记的是失败发生时的阶段");
        assert!(!c.trial);
        assert_eq!(c.consecutive_failures, 1);
        assert_eq!(c.failure_kind(), "boot-failure");
        // 引擎里的插件状态是取状态、不是伪造。
        assert_eq!(c.plugin_state, Some(PluginState::Enabled));
    }

    #[test]
    fn context_distinguishes_safemode_and_trial_failures() {
        let mut e = engine_with_plugins();
        e.record_boot_failure_at(None, 1);
        e.record_boot_failure_at(None, 2); // → safemode
        assert_eq!(e.phase(), BootPhase::Safemode);
        e.record_boot_failure_at(Some("p.core"), 3);
        assert_eq!(
            e.last_context().unwrap().failure_kind(),
            "safemode-failure",
            "安全模式内的失败与正常模式的失败必须可区分"
        );
        // 回到安全模式做试验失败的对照（`enter_safemode` 只改阶段，不动计数）。
        e.enter_safemode();
        e.trial_enable("p.audio").unwrap();
        e.record_trial_failure_at("p.audio", 4);
        let c = e.last_context().unwrap();
        assert_eq!(c.failure_kind(), "trial-failure");
        assert!(c.trial, "试验失败的 trial 维必须置位");
        assert_eq!(c.plugin_state, Some(PluginState::DisabledBySafemode));
    }

    #[test]
    fn context_ring_drops_oldest_at_capacity() {
        let mut e = engine_with_plugins();
        for i in 0..(RecoveryEngine::CONTEXT_CAPACITY as u64 + 3) {
            e.record_boot_failure_at(None, i);
        }
        assert_eq!(e.context().len(), RecoveryEngine::CONTEXT_CAPACITY);
        // 最旧的三条（ts 0/1/2）被丢掉，保留最新的 8 条。
        assert_eq!(e.context()[0].ts, 3);
        assert_eq!(e.last_context().unwrap().ts, RecoveryEngine::CONTEXT_CAPACITY as u64 + 2);
    }

    #[test]
    fn context_survives_success_reset() {
        // 计数器清零是为了自愈；诊断上下文不是判定依据，不该被自愈顺手抹掉
        // ——崩溃证据要留给下一次启动读。
        let mut e = engine_with_plugins();
        e.record_boot_failure_at(Some("p.audio"), 9);
        e.record_boot_success();
        assert_eq!(e.counter().consecutive_failures, 0);
        assert_eq!(e.context().len(), 1, "成功启动不清上下文");
        assert_eq!(e.last_context().unwrap().ts, 9);
    }

    #[test]
    fn set_context_seeds_and_truncates() {
        let mut e = engine_with_plugins();
        let many: Vec<BootContextEntry> = (0..(RecoveryEngine::CONTEXT_CAPACITY as u64 + 5))
            .map(|i| BootContextEntry {
                ts: i,
                phase: BootPhase::Normal,
                plugin_id: None,
                trial: false,
                consecutive_failures: 1,
                safemode_failures: 0,
                plugin_state: None,
            })
            .collect();
        e.set_context(many);
        assert_eq!(e.context().len(), RecoveryEngine::CONTEXT_CAPACITY);
        assert_eq!(e.context()[0].ts, 5, "播种时只保留最近的 N 条");
        // 播种之后继续追加 → 仍是环形。
        e.record_boot_failure_at(None, 999);
        assert_eq!(e.context().len(), RecoveryEngine::CONTEXT_CAPACITY);
        assert_eq!(e.last_context().unwrap().ts, 999);
    }

    #[test]
    fn context_round_trips_through_json() {
        let mut e = engine_with_plugins();
        e.record_boot_failure_at(Some("p.audio"), 42);
        let e2 = RecoveryEngine::from_json(&e.to_json()).unwrap();
        assert_eq!(e2.context(), e.context());
        assert_eq!(e2.last_context().unwrap().plugin_id.as_deref(), Some("p.audio"));
    }

    #[test]
    fn corrupt_context_does_not_corrupt_the_snapshot() {
        // 诊断字段坏了不能让整份快照判废——那会平白多算一次启动失败。
        let mut v = engine_with_plugins().to_json();
        v["context"] = serde_json::json!("not-an-array");
        let e = RecoveryEngine::from_json(&v).expect("context 损坏不应让快照不可解析");
        assert!(e.context().is_empty(), "损坏 = 无 context，不伪造");
        assert_eq!(e.counter().consecutive_failures, 0, "其它字段照常恢复");
    }

    #[test]
    fn missing_context_is_empty_for_old_snapshots() {
        let mut v = engine_with_plugins().to_json();
        v.as_object_mut().unwrap().remove("context");
        let e = RecoveryEngine::from_json(&v).unwrap();
        assert!(e.context().is_empty());
    }

    // ── BootCounter 单元测试 ────────────────────────────────────────

    #[test]
    fn boot_counter_thresholds() {
        let mut c = BootCounter::default();
        assert_eq!(c.decide_phase(), BootPhase::Normal);
        c.record_failure(BootPhase::Normal, None);
        assert_eq!(c.decide_phase(), BootPhase::Normal, "1 次失败不进安全模式");
        c.record_failure(BootPhase::Normal, None);
        assert_eq!(c.decide_phase(), BootPhase::Safemode, "2 次失败进安全模式");
        c.record_failure(BootPhase::Safemode, None);
        assert_eq!(c.decide_phase(), BootPhase::Repairmode, "安全模式内 1 次失败进修复模式");
    }

    #[test]
    fn boot_counter_success_resets() {
        let mut c = BootCounter::default();
        c.record_failure(BootPhase::Normal, None);
        c.record_failure(BootPhase::Normal, None);
        c.record_success();
        assert_eq!(c.consecutive_failures, 0);
        assert_eq!(c.safemode_failures, 0);
        assert!(c.trial_failures.is_empty());
    }

    // ── PluginState ────────────────────────────────────────────────

    #[test]
    fn plugin_state_is_enabled_matrix() {
        assert!(PluginState::Enabled.is_enabled());
        assert!(PluginState::TrialEnable.is_enabled());
        assert!(!PluginState::DisabledBySafemode.is_enabled());
        assert!(!PluginState::DisabledByRepairmode.is_enabled());
        assert!(!PluginState::PermanentlyDisabled.is_enabled());
    }

    #[test]
    fn plugin_state_serde_roundtrip() {
        for s in [
            PluginState::Enabled,
            PluginState::DisabledBySafemode,
            PluginState::DisabledByRepairmode,
            PluginState::TrialEnable,
            PluginState::PermanentlyDisabled,
        ] {
            let v = serde_json::to_value(s).unwrap();
            let s2 = serde_json::from_value::<PluginState>(v).unwrap();
            assert_eq!(s, s2);
        }
    }

    // ── BootPhase ──────────────────────────────────────────────────

    #[test]
    fn boot_phase_serde_roundtrip() {
        for p in [BootPhase::Normal, BootPhase::Safemode, BootPhase::Repairmode] {
            let v = serde_json::to_value(p).unwrap();
            let p2 = serde_json::from_value::<BootPhase>(v).unwrap();
            assert_eq!(p, p2);
        }
    }

    // ── 端到端 ────────────────────────────────────────────────────

    /// 计划 §4.14 测试项：必崩插件 → 第 3 次启动进入安全模式且客户端可用。
    #[test]
    fn end_to_end_safemode_on_crashing_plugin() {
        let mut e = engine_with_plugins();

        // 模拟 2 次启动失败（必崩插件）。
        e.record_boot_failure(Some("p.audio"));
        e.record_boot_failure(Some("p.audio"));

        // 第 3 次启动：进入安全模式。
        assert_eq!(e.phase(), BootPhase::Safemode);
        // 必需插件仍可用。
        assert_eq!(e.plugin_state("p.core"), Some(PluginState::Enabled));
        // 崩溃插件被禁用。
        assert_eq!(e.plugin_state("p.audio"), Some(PluginState::DisabledBySafemode));
    }

    /// 计划 §4.14 测试项：必需插件必崩 → 第 3 次进修复模式（不再 boot loop）。
    #[test]
    fn end_to_end_repairmode_on_required_plugin_crash() {
        let mut e = RecoveryEngine::new(["p.core".into()].into_iter().collect());
        e.set_plugin("p.core", true);
        e.set_plugin("p.audio", false);

        e.record_boot_failure(Some("p.core"));
        e.record_boot_failure(Some("p.core"));
        assert_eq!(e.phase(), BootPhase::Safemode);
        // 安全模式下必需插件崩溃。
        e.record_boot_failure(Some("p.core"));
        assert_eq!(e.phase(), BootPhase::Repairmode, "必需插件崩溃 → 修复模式，不再 boot loop");
    }

    /// 计划 §4.14 测试项：安全模式内逐个试启用坏插件 → 不整机重启。
    #[test]
    fn end_to_end_trial_enable_no_reboot() {
        let mut e = engine_with_plugins();
        e.record_boot_failure(None);
        e.record_boot_failure(None);
        e.enter_safemode();

        // 试启用 p.audio。
        e.trial_enable("p.audio").unwrap();
        e.record_trial_failure("p.audio");
        assert_eq!(e.plugin_state("p.audio"), Some(PluginState::DisabledBySafemode));
        // 全局计数器不应增加。
        assert_eq!(e.counter().consecutive_failures, 2);

        // 试启用 p.video。
        e.trial_enable("p.video").unwrap();
        e.record_trial_failure("p.video");
        assert_eq!(e.plugin_state("p.video"), Some(PluginState::DisabledBySafemode));
        // 仍然在安全模式，不重启。
        assert_eq!(e.phase(), BootPhase::Safemode);
    }

    /// 计划 §4.14 测试项：幂等去重。
    #[test]
    fn end_to_end_idempotent_recovery() {
        let mut e = engine_with_plugins();
        e.set_snapshot(SnapshotMeta {
            schema_version: CURRENT_SCHEMA_VERSION.into(),
            seq: 10,
            writer: "engine".into(),
            ts: 1000,
        })
        .unwrap();

        let action = RecoveryAction {
            plugin_id: "p.audio".into(),
            action_kind: "restart".into(),
            idempotency_key: RecoveryAction::idempotency_key("p.audio", "restart", 10),
        };

        // 第一次执行。
        assert!(e.execute_action(&action).unwrap());
        e.complete_recovery();

        // 第二次执行（去重）。
        assert!(!e.execute_action(&action).unwrap());
        e.complete_recovery();

        // 第三次执行（仍去重）。
        assert!(!e.execute_action(&action).unwrap());
        e.complete_recovery();

        // GC：移除 seq < 10 的动作。
        let removed = e.gc_executed_actions(10);
        assert_eq!(removed, 0, "seq=10 不应被 GC");
    }
}
