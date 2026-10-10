// T-7 拆分：从 `lib.rs` 逐字节纯搬移的生命周期上报命令族。
//
// 唯一命令 `cmd_lifecycle_report` 经 `state.registry.lifecycle_report` 落 self 档白名单
// 闸、再 `reconcile_recovery_phase` 做一次恢复阶段对账；它调用的 `guard`、
// `reconcile_recovery_phase` 都是 crate-root 私有助手，随 `use super::*;` 原样可见，
// 返回类型 `TransitionOutcome` 与 `Event`/`Action` 仍由 crate-root 提供、不随本族搬。
// 按原名再导出，保 `crate::cmd_lifecycle_report`（tauri.rs 接线）解析不变；本族无私有
// 助手需外泄。

use super::*;

/// `host_lifecycle_report`：上报生命周期事件（self 档）。
///
/// 身份从 webview label 解析，忽略入参 pluginId（§2.1）。
///
/// **事件必须落在 [`tauron_host::lifecycle::Event::PLUGIN_REPORTABLE`] 内**——
/// 越权事件（`Enable`/`SafemodeExit`/`Uninstall`/`InstallOk` 等）在
/// `Registry::lifecycle_report` 里以 `E_AUTH_DENIED` 拒绝。这道闸不在本函数里
/// 补一遍，是为了让**所有** self 档调用点共用同一份白名单。
///
/// 上报后做一次恢复阶段对账：插件自报启用后若当前处于安全模式且该插件非必需，
/// `SafemodeEnter` 会立刻把它标记为禁用——恢复判定优先于插件自己的上报。
pub fn cmd_lifecycle_report(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    event: Event,
) -> HostResult<TransitionOutcome> {
    let out = guard("lifecycle_report", || {
        state.registry.lifecycle_report(webview_label, claimed_id, event)
    })??;

    // D28 反向同步边：试启期间插件自报错误 → 注册表按 D28 回落
    // （`IncrementTrialFailure` + 打回 DISABLED + 置安全模式标志）。引擎若
    // 不知情、仍记它 `TrialEnable`，紧接着的对账会把「引擎判启用、注册表却
    // 禁用」当成不一致，补发 `SafemodeExit` 把刚打上的标志又清掉——形成
    // 「插件报错 → 标志被清 → 插件重入」的往返抖动，且两套试验预算各自为
    // 政。这里把试验失败同步进引擎，对账立即静默、预算保持同向。
    // 只补这一条动作：其余动作（重试预算等）引擎不建模，注册表自己闭环。
    if out.actions.contains(&tauron_host::lifecycle::Action::IncrementTrialFailure) {
        if let Ok(id) = tauron_host::authz::resolve_self_identity(webview_label, claimed_id) {
            state.recovery.lock().record_trial_failure(id.as_str());
        }
    }

    reconcile_recovery_phase(state);
    Ok(out)
}
