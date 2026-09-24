// @tauron/app-plugin-sdk — 公共入口。
//
// 插件开发者只需从此入口导入：
// ```typescript
// import { createPlugin } from '@tauron/app-plugin-sdk';
// import { createPluginTestContext, createMockContext } from '@tauron/app-plugin-sdk/testing';
// ```
//
// **R2 统一**：本 SDK 的 `PluginContext` 就是共享契约
// （`@tauron/plugin-context-contract`）的具体化版本——签名漂移由
// `types.ts` 的编译期断言 `AppPluginContextIsContract` 拦下。

// 核心
export { createPlugin } from './createPlugin.js';
export { createPluginContext } from './context.js';

// 类型（其中 PluginContext 是共享契约的具体化；断言类型一并导出便于门禁引用）
export type {
  CommandHandler,
  CommandRegisterResult,
  EventListener,
  PluginContext,
  PluginDefinition,
  PluginInstance,
  PluginHooks,
  SettingsTabConfig,
  AppPluginContextIsAssignableToContract,
  AppPluginContextMembersMatchContract,
  AppSdkHostIsPluginHost,
} from './types.js';

// 共享契约本体转出：插件作者需要一个稳定的 import 点时可直接取用，
// 而不必额外依赖 `@tauron/plugin-context-contract`。
export type {
  CommandRegistry,
  EventSink,
  PluginCommandHandler,
  PluginContext as SharedPluginContext,
  PluginEventEnvelope,
  PluginEventListener,
  PluginHost,
  PluginLog,
  SettingsTabRegistry as SharedSettingsTabRegistry,
} from '@tauron/plugin-context-contract';
