/**
 * @tauron/adapter-react — tauron React 适配器
 *
 * 设计文档引用：
 * - §4.2 React 适配器
 * - §1.2 适配层架构
 */

// ---- Context ----
export {
  TauronContext,
  TauronProvider,
  useTauronContext,
  type TauronContextValue,
  type TauronProviderProps,
} from './context.js';

// ---- Hooks ----
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
  useStore,
  useStoreSelector,
  type Store,
} from './store.js';
