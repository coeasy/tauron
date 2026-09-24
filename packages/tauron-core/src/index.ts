/**
 * @tauron/core — tauron 框架无关核心层
 *
 * 设计文档引用：
 * - §1.1 框架无关核心层
 * - §2 核心协议（信封与 IPC）
 * - §3 双层权限模型
 * - §4 插件系统
 * - §9 Web 降级
 */

// ---- 运行时探测（§9）----
export {
  isTauri,
  getRuntimeMode,
  getCapabilities,
  isCapabilityAvailable,
  TAURI_CAPABILITIES,
  WEB_CAPABILITIES,
  type RuntimeMode,
  type RuntimeCapabilities,
} from './runtime.js';

// ---- Backend 抽象（§1.3）----
export type { TauronBackend, ProgressCallback } from './backend.js';

// ---- Tauri IPC 后端（§1.3/§2.1）----
// `@tauri-apps/api` 在此模块内按需动态加载，因此纯 Web 构建可安全导入本入口。
export {
  createTauriBackend,
  TAURON_COMMANDS,
  type TauriBackendOptions,
  type TauriApi,
} from './tauri-backend.js';

// ---- 信封调用（§2.1）----
export {
  invokePlugin,
  cancelPlugin,
  listenEvent,
  emitEvent,
  type InvokeOptions,
} from './invoke.js';

// ---- 内层 ACL（§3.2）----
export {
  checkPluginPermission,
  grantPermissions,
  revokePermissions,
  getMissingPermissions,
  type PermissionGrants,
} from './acl.js';

// ---- 注册表（§4.4）----
export {
  PluginRegistry,
  type RegistryEntry,
  type RegistryConfig,
  type StateChangeListener,
} from './registry.js';

// ---- 事件总线（§2.3/§4.5）----
export {
  EventBus,
  EventBusError,
  type EventBusConfig,
  type EventSubscriber,
} from './event-bus.js';

// ---- 配置管理（§4.6）----
export {
  ConfigManager,
  type ConfigLayer,
  type ConfigChangeListener,
} from './config.js';
