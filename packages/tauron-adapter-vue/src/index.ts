/**
 * @tauron/adapter-vue — tauron Vue 适配器
 *
 * 设计文档引用：
 * - §4.2 Vue 适配器
 * - §1.2 适配层架构
 */

// ---- Context ----
export {
  provideTauronContext,
  injectTauronContext,
  TauronContextKey,
  type TauronContextValue,
} from './context.js';

// ---- Composables ----
export {
  useInvoke,
  type InvokeState,
  type UseInvokeReturn,
} from './use-invoke.js';

export {
  useEvent,
  type UseEventReturn,
} from './use-event.js';

export {
  useCapabilities,
  useIsTauri,
} from './use-capabilities.js';

export { usePluginId } from './use-plugin-id.js';

// ---- Store ----
export {
  createStore,
  type Store,
} from './store.js';
