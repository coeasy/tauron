/**
 * @tauron/adapter-svelte — tauron Svelte 适配器
 *
 * 设计文档引用：
 * - §4.2 Svelte 适配器
 * - §1.2 适配层架构
 */

// ---- Context ----
export {
  setTauronContext,
  getTauronContext,
  clearTauronContext,
  type TauronContextValue,
} from './context.js';

// ---- Stores ----
export {
  createInvokeStore,
  type InvokeState,
  type InvokeStore,
} from './use-invoke.js';

export {
  createEventStore,
  type EventState,
  type EventStore,
} from './use-event.js';

export {
  createCapabilitiesStore,
  createIsTauriStore,
} from './use-capabilities.js';

export { createPluginIdStore } from './use-plugin-id.js';
