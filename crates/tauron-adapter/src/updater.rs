// tauron-adapter · 更新命令族（T-7 逐字节纯搬移 · 十七片之二）。
//
// 本模块由 crate 根 lib.rs 的命令段整体纯搬移而来（旧 lib 6373–6464 行，一段连续 92 行），
// 未改动任何一行代码——函数体、doc、注释全部逐字节保真，仅把归属文件从 lib.rs 换到 updater.rs。
// 内容：host_updater_check 的检查腿 cmd_updater_check / cmd_updater_check_as，与 host_updater_status
// 的诊断腿 cmd_updater_status / cmd_updater_status_as，连同只服务检查腿的私助手 updater_unavailable
// （缺省无端点时的诚实降级）。updater_unavailable 与调用者同模块可见，故无需 `pub(crate)` 放宽；
// 私有不外泄、不再导出。
//
// 命名与归属区分：本文件只搬**命令包装层**；`UpdaterSink` trait、`DistributeUpdaterSink` /
// `UnavailableUpdaterSink` 实现、以及线形类型 `UpdaterCheckOutcome` / `UpdaterStatus` 都是更广的
// 更新通道能力基础设施，连同 `ShellExtState` 一并留在 lib.rs。cmd_updater_status 读取的
// `state.shell_ext`（update_state / update_state_simulated 账本）与 `state.upgrade_recovery` 亦留 lib；
// 本族经 `use super::*;` 原样可见 `guard` / `require_main_window` / `ProviderResult` / `HostResult` /
// `ErrorCode` / `SubstrateState` 与上述类型，零可见性放宽、纯归属搬移。

use super::*;

fn updater_unavailable() -> ProviderResult<UpdaterCheckOutcome> {
    ProviderResult::Unsupported(unsupported_body(
        "updater provider is not configured",
        Some("注入 EndpointClient（DistributeUpdaterSink::with_endpoint）"),
    ))
}

/// `host_updater_check`：检查更新（主窗专属；真跑 `tauron-distribute`）。
pub fn cmd_updater_check(
    state: &SubstrateState,
    current_version: &str,
) -> HostResult<ProviderResult<UpdaterCheckOutcome>> {
    guard("updater_check", || {
        if current_version.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "current_version 不能为空".to_string(),
            ));
        }
        if !state.updater_sink.native_supported() {
            return Ok(updater_unavailable());
        }
        state.updater_sink.check(current_version)
    })?
}

/// `host_updater_check` 的**带身份判定**版本（仅主窗）。
pub fn cmd_updater_check_as(
    caller: &Caller,
    state: &SubstrateState,
    current_version: &str,
) -> HostResult<ProviderResult<UpdaterCheckOutcome>> {
    require_main_window(caller, "host_updater_check")?;
    cmd_updater_check(state, current_version)
}

/// `host_updater_status`：更新通道状态（主窗专属）。
///
/// 这是 [`ShellExtState::update_state`] 的**真实生产读取方**：把进程内更新状态机
/// （`None → downloaded → installed`）并入返回的 `state` 字段。此前该字段无读取方
/// （见其接入状态注释），本轮补上这条链路。
///
/// **`state` 与 `state_simulated` 必须成对读出**（轮 33）：`update_state` 的写入方
/// 是 `cmd_market_download` / `cmd_market_install` 的分派腿（缺省模拟，轮 40 起装配腿
/// 注入后为真），它们推进账本是如实的，但字符串本身分不出"模拟推进"与"真实推进"。
/// 把 `state` 单独上屏就会把"点了一下模拟安装"说成"已安装"，因此本命令同时带出
/// provenance 位。
///
/// **成对读出落在装配上**（轮 33）：本命令把 `(update_state, update_state_simulated)`
/// 一次性交给 [`UpdaterSink::status`]，由 sink 推导 provenance；sink 的签名里这两个
/// 参数是**必传**的，所以通道实现无法再只报 `state` 而不报它是哪来的，也无法自己
/// 断言"这条状态是真的"。
///
/// **线形**：入参无，返回
/// `{ available: bool, state: string | null, stateSimulated: bool, grayscalePercent: u32, crashGateStopped: bool, reason: string | null }`。
/// `stateSimulated` 在 `state === null` 时恒为 `false`；缺省装配有 `state` 时恒为 `true`，
/// 轮 40 起装配腿（[`DistributeUpgradeInstaller`]）真路径的推进由写入方的 provenance
/// 标记决定（真实效果成功后为 `false`）。
pub fn cmd_updater_status(state: &SubstrateState) -> HostResult<UpdaterStatus> {
    guard("updater_status", || {
        let ext = state.shell_ext.lock();
        let mut status =
            state.updater_sink.status(ext.update_state.clone(), ext.update_state_simulated);
        // V9 N-03：并入 boot 对账得到的升级恢复读数（只读、不改任何状态）。缺省宿主没有
        // `upgrade_recovery_paths` → 报告为 `None` → 本分支不触碰 `reason`，既有读数逐字不变。
        // 挂起项以纯文本并入既有 `reason` 字段，而不是新增线字段：命令面与 `UpdaterStatus`
        // 线形都是冻结面（轮 33 先例「加读数不破形状」，这里连字段都不加）。
        let pending_note = state.upgrade_recovery.lock().as_ref().and_then(|report| {
            if report.has_pending() {
                Some(format!(
                    "上次升级有未收尾操作：未提交交换 {}、staging 残骸 {}、撕裂日志已隔离 {}（可由宿主按 UpgradeReconciler 的 resume/restore 显式处置）",
                    report.needs_decision, report.stale_staging, report.quarantined,
                ))
            } else {
                None
            }
        });
        if let Some(note) = pending_note {
            status.reason = Some(match status.reason.take() {
                Some(existing) => format!("{existing}；{note}"),
                None => note,
            });
        }
        Ok(status)
    })?
}

/// `host_updater_status` 的**带身份判定**版本（仅主窗）。
pub fn cmd_updater_status_as(caller: &Caller, state: &SubstrateState) -> HostResult<UpdaterStatus> {
    require_main_window(caller, "host_updater_status")?;
    cmd_updater_status(state)
}
