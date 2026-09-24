//! §4.5 权限授予与审批（`tauron-acl`）。
//!
//! 职责：安装时权限审批 → 物化为 Tauri `Capability`（`add_capability`）绑定插件
//! 身份 Webview；框架侧 `enabled` 标志做即时拒绝。
//!
//! 关键约束（计划 §4.5）：
//! - 授予集落盘带版本并**签名防篡改**（[crate::grant]）；
//! - 权限或 scope 变多必须重新审批（[crate::diff]）；
//! - 撤销语义如实（ADR-05）：收缩不需要重新审批；
//! - 高危档逐条展开成人话并默认不勾（[crate::approval]）；
//! - 禁止授予清单在授予路径上硬拒（复用 `tauron_host::authz::check_grants`）；
//! - 审批 UI 文案与代码共享同一常量来源（权限词表 → `approval_hints` → `ApprovalRow`）。
//!
//! 本 crate 依赖 [`tauron_host`]，但不依赖 `tauri`——`add_capability`
//! 的实际调用在 Tauri 适配层（§4.23）完成，本 crate 只产出 capability 数据。

pub mod approval;
pub mod diff;
pub mod grant;

pub use approval::{build_approval_rows, draft_grant_set, validate_grants, ApprovalRow};
pub use diff::{diff, GrantDiff, ScopeChange};
pub use grant::{
    canonical_bytes, sign, verify, AclStore, GrantEntry, GrantSet, SignedGrantSet,
    GRANT_SET_SCHEMA_VERSION,
};
