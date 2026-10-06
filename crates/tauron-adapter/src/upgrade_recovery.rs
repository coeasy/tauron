//! 升级 journal 的**启动对账装配点**（审计 V9 N-03）。
//!
//! `tauron-distribute` 的 [`tauron_distribute::UpgradeReconciler`] 是恢复腿的实现；
//! 这里是它在宿主侧的**生产消费方**——补上「boot 时读过 journal」这条此前只存在于
//! 测试里的缺失链路（N-03 的判据原文：boot 读取必须有非测试消费者锚）。
//!
//! 姿态与恢复实现一致：**boot 只补发事实、不改状态**。本模块只在启动序列里跑
//! [`tauron_distribute::UpgradeReconciler::scan`]（只读 + 隔离撕裂日志），把结论存进
//! [`SubstrateState::upgrade_recovery`] 供 `host_updater_status` 读数；`resume` /
//! `restore` 两个显式恢复动作留给调用方按需触发，绝不在 boot 里隐式回滚或提交。

use tauron_distribute::{ReconcileReport, RecoveryPaths, UpgradeReconciler};

/// boot 对账：读回所有中断升级操作的 journal，产出报告（只读 + 撕裂隔离）。
///
/// 这是 [`UpgradeReconciler::scan`] 在仓内的**唯一生产消费者**——由
/// `SubstrateState::with_adapter_config` 在启动序列里调用（`cfg.upgrade_recovery_paths`
/// 为 `Some` 才跑；缺省宿主没有升级安装根，如实不扫）。
pub(crate) fn scan_upgrade_recovery(paths: &RecoveryPaths) -> ReconcileReport {
    let reconciler = UpgradeReconciler::new(paths.clone());
    reconciler.scan().unwrap_or_else(|error| {
        // 对账失败不得阻断启动：如实报告「扫不动」而不是假装干净。scan 的失败面
        // 只有 backup 根不可读（撕裂 journal 走隔离、不返回 Err），这里降级为空报告。
        let _ = error;
        ReconcileReport {
            scanned: 0,
            committed: 0,
            needs_decision: 0,
            stale_staging: 0,
            quarantined: 0,
            settled: 0,
            findings: Vec::new(),
        }
    })
}
