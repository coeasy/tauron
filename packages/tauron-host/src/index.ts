// ──────────────────────────────────────────────────────────────────────────
// @tauron/host 公共入口（tree-shaking 友好，`sideEffects: false`）。
//
// 有意**不**在此文件 import `@tauri-apps/api`：真实后端从 `@tauron/host/tauri`
// 单独引入，契约测试则从 `@tauron/host/testing` 引入 mock。
// ──────────────────────────────────────────────────────────────────────────

export {
  APP_LAYER_ERROR_CODE_PATTERN,
  HOST_ERROR_CODES,
  HOST_RETRY_CLASS,
  RETRYABLE_HOST_ERROR_CODES,
  HostException,
  isAppLayerErrorCode,
  isHostErrorCode,
  isRetryable,
  isRetryClass,
  retryClassOf,
  normalizeError,
  translate_at_boundary,
  UNWIRED_BOUNDARIES,
} from './errors.js';
export type {
  BoundaryTranslation,
  HostBoundary,
  HostErrorCode,
  HostErrorShape,
  RetryClass,
} from './errors.js';

export type { Backend, ChannelPort, Principal, Unlisten } from './backend.js';
export { MockBackend, adoptRuntimeCapabilities } from './backend.js';
export { MemoryTransport } from './memory-transport.js';
export type { HostTransport } from './backend.js';
export type { MemoryCommandHandler, MemoryTransportOptions } from './memory-transport.js';
export type { MockBackendOptions, MockInvokeCase } from './backend.js';

export {
  AUTH_TIERS,
  CAPABILITIES,
  CONSUMERS,
  capabilityMatrix,
  capabilityOf,
  isAvailable,
} from './capabilities.js';
export type { AuthTier, Capability, CapabilityCommand, Consumer } from './capabilities.js';

export { HostClient, AdminClient, FrameSink } from './host.js';
export type {
  HostClientOptions,
  PluginCallRequest,
  ContributeEntryInput,
  ContributesReconcileReport,
  InstallReviewToken,
  ProductionDoctorCheck,
  ProductionDoctorReport,
} from './host.js';

export { ShellClient, SIDECAR_ABI_CONTRACT } from './shell-client.js';
// 已删除：`WindowActionResult`（曾在此转出）。宿主侧没有任何命令返回该形状，
// 且它与 `@tauron/ui-primitives` 的同名类型**形状不同**（那边是
// `{ok:true}|{ok:false,code,message}` 且有真实生产者），双份同名易被误用。
// 窗口操作的线上形状是 `WindowCreateOutcome` / `WindowRelaunchOutcome`。
export type {
  ShellClientOptions,
  NotificationRecord,
  NotifyItem,
  NotificationsListResult,
  DispatchRecord,
  DispatchOutcome,
  LoadSource,
  DisabledPlugin,
  RecoveryPersistence,
  RecoveryContextEntry,
  RecoveryFailureKind,
  RecoveryBootResult,
  RecoveryOutcome,
  PhaseReconcile,
  RecoveryReportResult,
  RecoveryTrialResult,
  I18nState,
  I18nLoadResult,
  I18nCleanupResult,
  BrandInfo,
  MarketCheckResult,
  MarketUpdateResult,
  WindowRelaunchOutcome,
  WindowCreateOutcome,
  ContributeEntry,
  RuntimeBinarySignature,
  RuntimeAbiFingerprint,
  RuntimeSpawnProfile,
  RuntimeHandle,
  RuntimeHealth,
  HealthReport,
  ResourceStats,
  ReapStats,
  TerminalReapRecord,
  GenerationStats,
  MenuItemSpec,
  MenuSpec,
  MenuOutcome,
  MenuClickFrame,
  MenuClickSource,
  TraySpec,
  TrayOutcome,
  FsEntry,
  FsStat,
  FsReadResult,
  FsWriteResult,
  HttpRequestSpec,
  HttpResponseSpec,
  UpdaterCheckOutcome,
  UpdaterStatus,
  ThemeContribute,
  CapabilityEnforcement,
  CapabilityEnforcementLevel,
  HostCapabilities,
} from './shell-client.js';

export { ShellController } from './shell-controller.js';
export type { ShellControllerOptions } from './shell-controller.js';
// 宿主 → 前端的事件主题线值（轮 36）：Rust `pub const` 的镜像，一致性由门禁比对，
// 不靠注释。接入方要订阅投递信号时需要它，仓库内不应再出现第二份字面量。
export { NOTIFICATION_TOPIC } from './host-topics.js';
// 菜单/托盘点击的约定 topic（轮 37）：同上，唯一事实源是 Rust 的 `pub const`，
// 仓库内不得出现第二份字面量（`wire-gate` 逐字比对两侧）。它**不是**宿主补的
// 默认值——`MenuItemSpec.event` 没填就是不发帧。
export { MENU_CLICK_TOPIC } from './host-topics.js';
// 其余三条宿主会 `emit` 的 topic（轮 38）：深链接投递有仓库内监听方，
// 两条诊断帧（对话框降级、深链接注册）仓库内零监听方——但线值同样只许一份镜像，
// 接入方不必手打字符串。五条一起由 `wire-gate` 的「轮 38」词表门禁逐条比对。
export { DEEP_LINK_TOPIC, DIALOG_DEGRADED_TOPIC, DEEP_LINK_NATIVE_TOPIC } from './host-topics.js';
// 未接线事件的显式登记表（1.0-W3）：接入方自检「哪些事件需要自己接」。
export { UNWIRED_EVENTS, UNWIRED_EVENT_NAMES } from './unwired-events.js';
export type { UnwiredEvent } from './unwired-events.js';

// 退出动画（ExitAnimation）与组件动画（animateEnter/staggerIn/...）已**迁出**本包，
// 归属 `@tauron/ui-primitives`（R3：它们是纯 DOM 时序工具，不是宿主能力）。
//
// 这里**不做转出**：`@tauron/ui-primitives` 会静态引入 `lit` 与全部 DOM 组件，
// 在宿主入口转出会把它们拖进每个 `@tauron/host` 消费者（含 node 测试环境），
// 违背本包 `sideEffects: false` 的轻量契约。需要者直接从
// `@tauron/ui-primitives` 导入。
//
// ── 已删除：`bootstrap()` 启动编排器（0.4-W1）────────────────────────────
// 它曾在此转出，但**全仓零生产消费者**（除自己的测试），且是 P1-1 的载体：
// 它 import `@tauron/core` 的 `PluginRegistry`/`ConfigManager`/`EventBus`
// 三个**运行时类**，让「活线（host）依赖死线（core）」成立；而本包声明
// `sideEffects: false`，真实构建里这三个类根本不存在——即它一旦被用就会崩。
// 处置按 W9-3 的「接线 / 移入 testing / 删除，不留第三态」取**删除**：
// 应用启动编排的真实落点是 `examples/minimal-app/src/main.ts`（裸用
// `ShellClient`，含 `recoverReport('success')` 上报），插件注册的真实落点是
// **宿主侧** Rust 注册表（`host_registry_admin` / `host_registry_install`），
// 前端不需要第二份注册表。删除后本包不再依赖 `@tauron/core`（P1-5 的
// 「TS `maxPlugins: 32` vs Rust `max_plugins: 8`」矛盾随之消失）。
export { WindowState, createWindowState } from './window-state.js';
export type { WindowStateConfig, WindowStateData } from './window-state.js';

export {
  LazyPluginLoader,
  createLazyPluginLoader,
  LazyPluginLoadError,
} from './lazy-plugin-loader.js';
export type {
  LazyPluginDescriptor,
  LazyPluginLoadOptions,
  LazyPluginLoadErrorCode,
  LazyPluginLoaderStats,
  PluginLoadStatus,
  PluginCacheEntry,
} from './lazy-plugin-loader.js';

export { AutoUpdateClient, createAutoUpdateClient } from './auto-update-client.js';
export type {
  AutoUpdateConfig,
  UpdateInfo,
  UpdateStatus,
  DownloadProgress,
} from './auto-update-client.js';

export { DialogClient, createDialogClient, isUnsupportedBody } from './dialog-client.js';
export type {
  FileFilter,
  OpenFileOptions,
  SaveFileOptions,
  MessageOptions,
  ConfirmOptions,
  UnsupportedBody,
  DegradedValue,
  ProviderResult,
} from './dialog-client.js';

export { DeepLinkClient, createDeepLinkClient } from './deep-link-client.js';
export type { DeepLinkConfig, DeepLinkEvent } from './deep-link-client.js';

export { CHANNEL_KINDS, MAX_QUEUE, OVERFLOW_STREAK_LIMIT } from './channels.js';
export type {
  BusFrame,
  BusPublishResult,
  BusStats,
  BusSubscriptionMeta,
  BusSubscribeOutcome,
  BusTopicMeta,
  ChannelKind,
  QueueStats,
} from './channels.js';

export {
  GRANT_SET_SCHEMA_VERSION,
  IDENTITY_LABEL_PREFIX,
  RISKS,
  capabilityOfGrantSet,
  requiresReapproval,
  scopeGrew,
} from './grants.js';
export type {
  ApprovalRow,
  TauriCapability,
  GrantDiff,
  GrantEntry,
  GrantSet,
  Risk,
  ScopeChange,
  SignedGrantSet,
} from './grants.js';

export { TOPIC_MAX_LENGTH, EventOrderingWatcher } from './events.js';
export type {
  AdminReviewToken,
  EventFrame,
  EventOrderingViolation,
  EventSelector,
  JsonValue,
  PendingCallInfo,
  PluginDescriptor,
  PublishResult,
  RegistryAdminOp,
  RegistryAdminOpKind,
  RegistryAdminOutcome,
  Subscription,
  TopicDescriptor,
} from './events.js';

// R5：流式帧（唯一词表，与 Rust `tauron_host::stream::StreamFrame` 同构）。
export { STREAM_KINDS, isStreamFrame, isTerminalKind, parseStreamKind } from './stream.js';
export type { StreamFrame, StreamHandle, StreamKind, StreamWriteInput } from './stream.js';

// R5：传输无关的宿主 RPC 面。
export { toHostRpc } from './rpc.js';
export type {
  HostClientSatisfiesPluginRpc,
  HostRpc,
  HostRpcOptions,
  PluginRpc,
  PumpScheduler,
} from './rpc.js';

export { LIFECYCLE_EVENTS, LIFECYCLE_STATES, PLUGIN_REPORTABLE_EVENTS } from './lifecycle.js';
export type { LifecycleEvent, LifecycleState, PluginReportableEvent } from './lifecycle.js';

export type {
  ContributesEntry,
  IdentityActionResult,
  IdentityState,
  IdentityUnit,
  PluginJsConfig,
  ViewConfig,
} from './plugin-js.js';

export {
  validateClientConfig,
  fullLoadConfig,
  minimalConfig,
  templateConfig,
  CLIENT_CONFIG_LANDING,
  clientConfigUnwiredKeys,
  clientConfigUnwiredSummary,
} from './client-config.js';
export type {
  ClientConfig,
  ClientConfigLanding,
  PluginFilter,
  RegistryConfigOverride,
  LogLevel,
} from './client-config.js';
