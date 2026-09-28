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
pub mod storage;
pub mod portable_path;
pub mod generation;
pub mod durable;
pub mod activation;
pub mod policy;
pub mod call_graph;
pub mod admission;
pub mod call_delivery;
pub mod call_state;
pub mod config;
pub mod error;
pub mod wire;
pub mod provider;
pub mod execution;
pub mod eventbus;
pub mod lifecycle;
pub mod manifest;
pub mod production;
pub mod registry;
pub mod runtime;
pub mod service_graph;
pub mod stream;
pub mod target;

pub use authz::{AuthTier, CommandAuth, ADMIN_COMMANDS, COMMANDS};
pub use config::{ClientConfig, RegistryConfigOverride};
pub use error::{guard, ErrorCode, HostError, HostResult, RetryClass};
pub use eventbus::{
    BusStats, ChannelKind, EventBus, Frame, PublishResult, QueueStats, SubscribeOutcome, MAX_QUEUE,
    OVERFLOW_STREAK_LIMIT,
};
pub use lifecycle::{Event, Guard, PluginState, State, TransitionOutcome, MAX_RETRY, TRANSITIONS};
pub use manifest::{
    AbiFingerprint, CommandContribute, Contributes, EntrySpec, EventDecl, EventsDecl,
    MenuContribute, PanelContribute, Permission, PermissionEntry, PermissionIndex, PluginId,
    PluginIdentity, PluginManifest, PluginType, Risk, SettingsTabContribute, ShortcutContribute,
    SUPPORTED_PLATFORMS,
};
pub use registry::{PendingCall, PluginEntry, PluginFilter, Registry, RegistryConfig};
pub use runtime::{LeaseReaper, ReapOutcome, ReapStats, RuntimeHandle, RuntimeLease, RuntimeTable};

// V4 universal/industrial foundation exports.
pub use activation::{ActivationError, ActivationRecord, ContentIdentity};
pub use durable::{
    decode_durable, encode_durable, DurableEnvelope, DurableError, MigrationSnapshot,
};
pub use generation::{Generation, GenerationError, GenerationHandle, GenerationRegistry};
pub use portable_path::{join_scoped, validate_portable_relative, PortablePathError};
pub use storage::{SingleWriterLease, StorageNamespace, WriterLeaseError, WriterLeaseTable};
pub use admission::{
    AdmissionController, AdmissionError, CreditWindow, FairQueue, ResourceKind, ResourceLimit,
};
pub use call_graph::{
    CallGraph, CallGraphError, CausationError, EventCausation, ReentrancyPolicy,
    DEFAULT_MAX_CALL_HOPS, DEFAULT_MAX_CAUSATION_DEPTH,
};
pub use policy::{DecisionError, DecisionToken, PolicyAuthority};
pub use execution::{
    current_domain, in_domain, require_domain, ExecutionDomain, ExecutionDomainGuard, ExecutionError,
};
pub use provider::{
    CapabilityEpoch, ProviderLifecycle, ProviderLifecycleError, ProviderState,
};
pub use wire::{
    decode_json as decode_wire_json, encode_json as encode_wire_json, WireError, WireExtensions,
    WireFrame, WireHeader, DEFAULT_MAX_WIRE_BYTES, JSON_V1_CODEC, WIRE_VERSION_V1,
};
pub use call_state::{AtomicCallState, CallTerminalState};
pub use production::{
    is_production_safe, validate as validate_production_readiness, DeploymentMode,
    ProductionReadiness, ReadinessViolation,
};
pub use service_graph::{ServiceGraph, ServiceGraphError, ServiceNode};
pub use target::{
    current_target_spec, resolve_best as resolve_best_target, TargetAbi, TargetArch, TargetOs,
    TargetSpec,
};
