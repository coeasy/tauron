/**
 * @tauron/plugin-sdk — tauron 插件 SDK
 *
 * 设计文档引用：
 * - §2.4 postMessage 代理协议
 * - §4.2B JS 前端插件
 * - §4.2B+ 双世界插件
 */

// ---- 宿主侧（PluginBridge）----
export { PluginBridge, PLUGIN_TOKEN_FRAGMENT_KEY } from './bridge.js';

// ---- 宿主侧插件方法调用（__invoke:/__result: 协议客户端）----
export { callPluginMethod, type HostInvokeOptions } from './host-invoke.js';

// ---- 插件侧（PluginContext，legacy iframe 一代）----
export {
  createPluginContext,
  readHandshakeTokenFromUrl,
  type PluginContext,
  type PluginContextOptions,
} from './plugin-context.js';

// ---- 共享契约适配（R2）：把 legacy 上下文适配成 @tauron/plugin-context-contract ----
export {
  createContractContext,
  DEFAULT_CONTRACT_HOST_COMMANDS,
  type ContractContextOptions,
  type ContractHostCommandNames,
} from './contract-context.js';

// ---- 插件注册（registerPlugin）----
export {
  registerPlugin,
  TAURON_DISABLE_EVENT,
  type RegisterPluginConfig,
} from './register-plugin.js';
