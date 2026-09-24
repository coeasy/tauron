// ──────────────────────────────────────────────────────────────────────────
// @tauron/host 公共入口（tree-shaking 友好，`sideEffects: false`）。
//
// 有意**不**在此文件 import `@tauri-apps/api`：真实后端从 `@tauron/host/tauri`
// 单独引入，契约测试则从 `@tauron/host/testing` 引入 mock。
// ──────────────────────────────────────────────────────────────────────────

export {
  APP_LAYER_ERROR_CODE_PATTERN,
  HOST_ERROR_CODES,
  RETRYABLE_HOST_ERROR_CODES,
  HostException,
  isAppLayerErrorCode,
  isHostErrorCode,
  isRetryable,
  normalizeError,
  translate_at_boundary,
  UNWIRED_BOUNDARIES,
} from './errors.js';
export type {
  BoundaryTranslation,
  HostBoundary,
  HostErrorCode,
  HostErrorShape,
} from './errors.js';

export type { Backend, ChannelPort, Principal, Unlisten } from './backend.js';
export { MockBackend } from './backend.js';
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
export type { HostClientOptions, PluginCallRequest, ContributeEntryInput } from './host.js';

export { ShellClient } from './shell-client.js';
export type {
  ShellClientOptions,
  WindowActionResult,
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
  ReapStats,
} from './shell-client.js';

export { ShellController } from './shell-controller.js';
export type { ShellControllerOptions } from './shell-controller.js';

export { bootstrap, type BootstrapOptions, type BootstrapResult, type BootstrapPluginDescriptor, type BootPhase, type SplashHandle } from './bootstrap.js';

// 退出动画（ExitAnimation）与组件动画（animateEnter/staggerIn/...）已**迁出**本包，
// 归属 `@tauron/ui-primitives`（R3：它们是纯 DOM 时序工具，不是宿主能力）。
//
// 这里**不做转出**：`@tauron/ui-primitives` 会静态引入 `lit` 与全部 DOM 组件，
// 在宿主入口转出会把它们拖进每个 `@tauron/host` 消费者（含 node 测试环境），
// 违背本包 `sideEffects: false` 的轻量契约。需要者直接从
// `@tauron/ui-primitives` 导入。`bootstrap` 仍以**动态**导入方式可选获取该类能力。
export { WindowState, createWindowState } from './window-state.js';
export type { WindowStateConfig, WindowStateData } from './window-state.js';

export { LazyPluginLoader, createLazyPluginLoader } from './lazy-plugin-loader.js';
export type { LazyPluginDescriptor, PluginLoadStatus, PluginCacheEntry } from './lazy-plugin-loader.js';

export { AutoUpdateClient, createAutoUpdateClient } from './auto-update-client.js';
export type { AutoUpdateConfig, UpdateInfo, UpdateStatus, DownloadProgress } from './auto-update-client.js';

export { DialogClient, createDialogClient } from './dialog-client.js';
export type { FileFilter, OpenFileOptions, SaveFileOptions, MessageOptions, ConfirmOptions } from './dialog-client.js';

export { DeepLinkClient, createDeepLinkClient } from './deep-link-client.js';
export type { DeepLinkConfig, DeepLinkEvent } from './deep-link-client.js';

export {
  CHANNEL_KINDS,
  MAX_QUEUE,
  OVERFLOW_STREAK_LIMIT,
} from './channels.js';
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

export {
  TOPIC_MAX_LENGTH,
} from './events.js';
export type {
  EventFrame,
  EventSelector,
  JsonValue,
  PendingCallInfo,
  PluginDescriptor,
  PublishResult,
  RegistryAdminOp,
  RegistryAdminOpKind,
  Subscription,
  TopicDescriptor,
} from './events.js';

// R5：流式帧（唯一词表，与 Rust `tauron_host::stream::StreamFrame` 同构）。
export { STREAM_KINDS, isStreamFrame, isTerminalKind, parseStreamKind } from './stream.js';
export type { StreamFrame, StreamKind } from './stream.js';

// R5：传输无关的宿主 RPC 面。
export { toHostRpc } from './rpc.js';
export type {
  HostClientSatisfiesPluginRpc,
  HostRpc,
  HostRpcOptions,
  PluginRpc,
  PumpScheduler,
} from './rpc.js';

export { LIFECYCLE_EVENTS, LIFECYCLE_STATES } from './lifecycle.js';
export type { LifecycleEvent, LifecycleState } from './lifecycle.js';

export {
  PluginJsRuntime,
} from './plugin-js.js';
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
} from './client-config.js';
export type { ClientConfig, PluginFilter, RegistryConfigOverride, LogLevel } from './client-config.js';
