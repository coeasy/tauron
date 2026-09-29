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

pub mod activation;
pub mod admission;
pub mod authz;
pub mod call_delivery;
pub mod call_graph;
pub mod call_state;
pub mod config;
pub mod durable;
pub mod error;
pub mod eventbus;
pub mod execution;
pub mod fault;
pub mod generation;
pub mod health;
pub mod lifecycle;
pub mod local_host;
pub mod local_host_reference;
pub mod manifest;
pub mod network_policy;
pub mod ordering;
pub mod policy;
pub mod portable_path;
pub mod production;
pub mod provider;
pub mod registry;
pub mod remote_host;
pub mod runtime;
pub mod scoped_fs;
pub mod service_graph;
pub mod storage;
pub mod stream;
pub mod target;
pub mod time_trust;
pub mod wire;

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
pub use remote_host::{
    RemoteAuthnCredential, RemoteHostConfig, RemoteHostError, RemoteHostSecurity, RemotePrincipal,
    RemoteSessionSnapshot, RemoteSessionState, RemoteTransportEvidence,
};
pub use runtime::{LeaseReaper, ReapOutcome, ReapStats, RuntimeHandle, RuntimeLease, RuntimeTable};
pub use wire::{
    decode_json as decode_wire_json, encode_json as encode_wire_json, WireError, WireExtensions,
    WireFrame, WireHeader, DEFAULT_MAX_WIRE_BYTES, JSON_V1_CODEC, WIRE_VERSION_V1,
};

// V4 universal/industrial foundation exports.
pub use activation::{ActivationError, ActivationRecord, ContentIdentity};
pub use admission::{
    AdmissionController, AdmissionError, CreditWindow, FairQueue, ResourceKind, ResourceLimit,
};
pub use call_graph::{
    CallGraph, CallGraphError, CausationError, EventCausation, ReentrancyPolicy,
    DEFAULT_MAX_CALL_HOPS, DEFAULT_MAX_CAUSATION_DEPTH,
};
pub use call_state::{AtomicCallState, CallTerminalState};
pub use durable::{
    decode_durable, encode_durable, DurableEnvelope, DurableError, MigrationSnapshot,
};
pub use execution::{
    current_domain, in_domain, require_domain, ExecutionDomain, ExecutionDomainGuard,
    ExecutionError,
};
pub use fault::{FaultBoundary, FaultError, FaultRecord, FaultState};
pub use generation::{
    Generation, GenerationError, GenerationHandle, GenerationRegistry, PackCacheKey, PackGcState,
    PackLease, PackLeaseRegistry,
};
pub use health::{Degradation, HealthReport, Liveness, Readiness};
pub use local_host::{
    peer_proof, AuthenticatedPeer, LocalHostBroker, LocalHostBrokerError, LocalHostLease,
    PeerChallenge, PeerCredentialEvidence,
};
pub use local_host_reference::{
    reference_wire_roundtrip, LocalHostReferenceError, REFERENCE_CONTROL_MAX_BYTES,
};
pub use network_policy::{
    is_public_ip, AuthorizedUrl, DomainRule, NetworkEnforcement, NetworkPolicy, NetworkPolicyError,
    PrivateNetworkPolicy, RedirectAuthorization,
};
pub use ordering::{OrderedEventMeta, OrderingError, OrderingTracker};
pub use policy::{DecisionError, DecisionToken, PolicyAuthority};
pub use portable_path::{join_scoped, validate_portable_relative, PortablePathError};
pub use production::{
    doctor as production_doctor, is_production_safe, validate as validate_production_readiness,
    DeploymentMode, ProductionDoctorCheck, ProductionDoctorReport, ProductionReadiness,
    ReadinessViolation,
};
pub use provider::{CapabilityEpoch, ProviderLifecycle, ProviderLifecycleError, ProviderState};
pub use scoped_fs::{
    list_hard as scoped_fs_list_hard, mkdir_hard as scoped_fs_mkdir_hard,
    platform_enforcement as scoped_fs_enforcement, read_hard as scoped_fs_read_hard,
    remove_hard as scoped_fs_remove_hard, stat_hard as scoped_fs_stat_hard,
    write_hard as scoped_fs_write_hard, FsEnforcement, ScopedDirEntry, ScopedFsError, ScopedPath,
};
pub use service_graph::{ServiceGraph, ServiceGraphError, ServiceNode};
pub use storage::{SingleWriterLease, StorageNamespace, WriterLeaseError, WriterLeaseTable};
pub use target::{
    current_target_spec, resolve_best as resolve_best_target, TargetAbi, TargetArch, TargetOs,
    TargetSpec,
};
pub use time_trust::{
    require_unexpired, suspicious_if_skew_exceeds, SystemTimeProvider, TimeTrustError,
    TimeTrustState, TrustedTime, TrustedTimeProvider,
};
