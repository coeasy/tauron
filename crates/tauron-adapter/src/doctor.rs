// T-7 拆分：从 `lib.rs` 逐字节纯搬移的生产就绪自检报告命令（二十一片）。
//
// `use super::*;` 让子模块看到 crate 根的全部项——含私有助手（`require_main_window`、
// `guard`）与公共类型（`SubstrateState` / `HostResult`）以及跨 crate 的
// `tauron_host::ProductionDoctorReport` / `tauron_host::AdminAuditFacts` / `tauron_proc::…`
// 路径，因此搬来的命令体与私助手可原样调用与解析，零 `pub(crate)` 放宽。
// 私助手 `production_doctor_report` 仅被本模块命令 `cmd_production_doctor_as` 调用
//（全仓 grep 佐证无其它调用点），随族搬入并**保持私有、不再导出**；`cmd_production_doctor_as`
// 由 crate 根 `pub use doctor::cmd_production_doctor_as` 再导出后，tauri.rs 的
// `crate::cmd_production_doctor_as` 接线与 lib.rs 内联测试的裸名调用照旧解析不变。

use super::*;

/// 主窗读取机器可读的生产就绪自检报告（V4 A109）。
///
/// 与启动门禁同源：底座的 `production_readiness` 在装配时由 [`AdapterConfig`] 推导；
/// 进程运行时事实以**实际注入**的 sandbox descriptor 为准重算（与
/// `validate_process_runtime_for_start` 同一判定），报告不会与启动路径漂移。
pub fn cmd_production_doctor_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<tauron_host::ProductionDoctorReport> {
    require_main_window(caller, "host_production_doctor")?;
    guard("production_doctor", || production_doctor_report(state))
}

/// 就绪报告的**唯一**装配点（`host_production_doctor` 与宿主启动后的自检共用）。
///
/// 轮 11 / F3：`admin-audit` 检查项的真值只来自审计 sink 的
/// [`tauron_host::AdminAuditFacts::healthy`]，报告同时把快照本身带上线（条数 /
/// 裁剪 / 写失败 / 链完整性）——读取侧因此可独立复核，而不必相信一个布尔位。
fn production_doctor_report(state: &SubstrateState) -> tauron_host::ProductionDoctorReport {
    let mut readiness = state.production_readiness.clone();
    if let Some(descriptor) = state.process_sandbox.get() {
        readiness.process_runtime_enabled = true;
        readiness.hard_process_sandbox_available =
            matches!(descriptor.enforcement, tauron_proc::ProcessSandboxEnforcement::Hard);
    }
    let audit = state.admin_audit.as_ref().map(|sink| sink.facts());
    readiness.audit_for_admin_operations_available =
        audit.as_ref().is_some_and(tauron_host::AdminAuditFacts::healthy);
    let mut report = tauron_host::production_doctor(state.deployment_mode, &readiness);
    report.admin_audit = audit;
    report
}
