//! tauron-shell — tauron Rust 壳层
//!
//! 设计文档引用：
//! - §1.1 Rust 壳层（tauron-shell crate）
//! - §2 核心协议（信封与 IPC）
//! - §3 双层权限模型
//! - §4 插件系统（4+1 形态）
//! - §8 工程化实现细节（tracing）
//!
//! 本 crate 刻意**不依赖 `tauri`**：核心逻辑保持平台无关、可离线测试；
//! Tauri 命令注册层是薄适配器，位于 `commands` 模块并由 `tauri` feature
//! 控制，默认构建不编译它。

pub mod acl;
pub mod config;
pub mod dispatch;
pub mod envelope;
pub mod error;
pub mod eventbus;
pub mod registry;

#[cfg(feature = "tauri")]
pub mod commands;

// ---- 错误 ----
pub use error::{PluginErrorCode, PluginError, TauronResult};

// ---- 宿主命令核心（§2.1）----
pub use dispatch::{EventSink, EVENT_TOPIC_PREFIX, HostState, NullEventSink, PluginDispatcher};

// ---- 信封 ----
pub use envelope::{
    generate_call_id, PluginCancelRequest, PluginInvokeRequest, PluginInvokeResponse,
    PluginErrorBody, ProgressEvent,
};

// ---- ACL ----
pub use acl::{
    check_plugin_permission, grant_permissions, revoke_permissions, PluginPermissionGrant,
    GrantType, PermissionGrants,
};

// ---- 注册表 ----
pub use registry::{
    is_valid_transition, transitions, PluginRegistry, PluginState, PluginType, RegistryEntry,
};

// ---- 事件总线 ----
pub use eventbus::{Event, EventBus, EventBusError, EventSubscriber};

// ---- 配置管理 ----
pub use config::{ConfigLayer, ConfigManager};
