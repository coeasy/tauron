//! tauron 宿主核心 crate。
//!
//! 对应开发计划 v1.2-draft：
//! - §4.2 manifest 与权限词表（[`manifest`]）
//! - §4.21 宿主命令三档授权（[`authz`]）
//! - §4.3 生命周期状态机（[`lifecycle`]）
//! - §4.1 宿主插件与注册表（[`registry`]）
//! - §4.4 事件总线与发布订阅授权（[`eventbus`]）
//! - ADR-04 结构化错误（[`error`]）
//!
//! 本 crate 刻意**不依赖 `tauri`**：核心逻辑保持平台无关、可离线测试；
//! Tauri 命令注册层是薄适配器（后续单元），便于把"逻辑正确性"与
//! "IPC 接线"分开验证。

pub mod authz;
pub mod config;
pub mod error;
pub mod eventbus;
pub mod lifecycle;
pub mod manifest;
pub mod registry;
pub mod runtime;
pub mod stream;

pub use error::{ErrorCode, HostError, HostResult, guard};
pub use eventbus::{
    BusStats, ChannelKind, EventBus, Frame, PublishResult, QueueStats, SubscribeOutcome,
    MAX_QUEUE, OVERFLOW_STREAK_LIMIT,
};
pub use lifecycle::{Event, Guard, State, TransitionOutcome, PluginState, TRANSITIONS, MAX_RETRY};
pub use manifest::{
    AbiFingerprint, CommandContribute, Contributes, EntrySpec, EventDecl, EventsDecl,
    MenuContribute, PanelContribute, Permission, PermissionEntry, PermissionIndex,
    PluginIdentity, PluginId, PluginManifest, PluginType, Risk, SettingsTabContribute,
    ShortcutContribute, SUPPORTED_PLATFORMS,
};
pub use authz::{ADMIN_COMMANDS, COMMANDS, AuthTier, CommandAuth};
pub use registry::{PendingCall, PluginEntry, PluginFilter, Registry, RegistryConfig};
pub use runtime::{LeaseReaper, ReapOutcome, ReapStats, RuntimeHandle, RuntimeLease, RuntimeTable};
pub use config::{ClientConfig, RegistryConfigOverride};
