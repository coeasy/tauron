/**
 * @tauron/types — tauron 共享类型包
 *
 * 设计文档引用：
 * - §2 核心协议（信封与 IPC）
 * - §3 双层权限模型
 * - §4 插件系统（4+1 形态）
 * - §12 附录：关键接口定义
 */

// ---- 信封协议（§2.1）----
export {
  type PluginInvokeRequest,
  type PluginInvokeResponse,
  type ProgressEvent,
  type PluginCancelRequest,
  DEFAULT_TIMEOUT_MS,
  generateCallId,
  buildRequest,
  buildOkResponse,
  buildErrorResponse,
  validateRequest,
} from './envelope.js';

// ---- 错误码（§2.2）----
export {
  PluginErrorCode,
  RETRYABLE_ERROR_CODES,
  type ErrorCategory,
  ERROR_CATEGORIES,
  ERROR_MESSAGES,
  isRetryable,
  errorCategory,
  errorMessage,
  isValidErrorCode,
} from './errors.js';

// ---- 插件状态与事件（§4.3/§2.3）----
export {
  type PluginState,
  TRANSITIONS,
  TERMINAL_STATES,
  ACTIVE_STATES,
  isValidTransition,
  allowedTransitions,
  type PluginEvent,
  eventNamespace,
  type PluginType,
  type PluginForm,
  pluginForm,
} from './plugin.js';

// ---- 配置（§7.3/§4.6）----
export {
  type ShellSource,
  type CapabilityGroup,
  type TauronPluginsConfig,
  type TauronShellConfig,
  type TauronUpdaterConfig,
  type TauronBrandsConfig,
  type TauronI18nConfig,
  type TauronConfig,
  type TauronEasingCurve,
  type TauronInstallerConfig,
  type TauronSecurityConfig,
  type TauronNetworkConfig,
  type TauronLoggingConfig,
  type TauronAccessibilityConfig,
  type TauronWindowConfig,
  type TauronShortcutConfig,
  type TauronTrayConfig,
  type TauronDeepLinksConfig,
  validateConfig,
} from './config.js';

// ---- ACL（§3）----
export {
  type PluginPermissionGrant,
  PERMISSION_GRANULARITY,
  type PermissionRisk,
  type PermissionMeta,
  hasPermission,
  hasAllPermissions,
  missingPermissions,
  isValidPermission,
} from './acl.js';

// ---- Manifest（§4.1）----
export {
  type ManifestAuthor,
  type ManifestEntry,
  type CommandContribute,
  type MenuContribute,
  type PanelContribute,
  type SettingsTabContribute,
  type ShortcutContribute,
  type ManifestContributes,
  type SettingsSchemaField,
  type ManifestSignature,
  type ManifestPublisher,
  type PluginManifest,
} from './manifest.js';

// ---- postMessage 代理协议（§2.4）----
export {
  BRIDGE_MESSAGE_TYPE,
  type BridgeToPluginMessage,
  type PluginToBridgeMessage,
  type BridgeMessage,
  buildInitMessage,
  buildReadyMessage,
  buildInvokeMessage,
  buildInvokeResultMessage,
  buildEmitMessage,
  buildEventMessage,
  buildCancelMessage,
  isHostToPlugin,
  isPluginToHost,
} from './bridge.js';
