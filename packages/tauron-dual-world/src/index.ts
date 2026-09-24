/**
 * @tauron/dual-world — tauron B+ 双世界原型
 *
 * 设计文档引用：
 * - §4.3 B+ 双世界架构
 * - §4.4 沙箱运行时
 * - §4.5 JS↔WASM 桥接
 */

// ---- Types ----
export type {
  SandboxCapabilities,
  SandboxConfig,
  SandboxContext,
  SandboxResult,
  HostFunction,
  BridgeMessage,
  PluginLifecycleHooks,
} from './types.js';

export { DEFAULT_SANDBOX_CAPABILITIES } from './types.js';

// ---- Sandbox ----
export {
  createSandbox,
  registerHostFunction,
  type SandboxInstance,
} from './sandbox.js';

// ---- Bridge ----
export {
  createBridge,
  DEFAULT_BRIDGE_CONFIG,
  type Bridge,
  type BridgeConfig,
} from './bridge.js';
