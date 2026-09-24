// ──────────────────────────────────────────────────────────────────────────
// 框架绑定层（计划 §4.11）：core 能力 → signals 的薄封装。
//
// 关键约束：
// - ≤40 行/能力；
// - 不做业务判断（架构 §5.2）；
// - Vue/Svelte 包只导出 hooks/stores（本文件是框架无关的核心绑定）；
// - 不得出现第二份组件实现。
// ──────────────────────────────────────────────────────────────────────────

import type { Backend, Principal, Unlisten } from '@tauron/host';
import {
  createSignal,
  type Signal,
  type Cleanup,
} from './signals.js';

/** 调用状态。 */
export type CallState<T> =
  | { status: 'idle' }
  | { status: 'loading'; data?: T }
  | { status: 'success'; data: T }
  | { status: 'error'; error: unknown };

/**
 * useInvoke：将 core 的 invoke 封装为响应式调用。
 *
 * 返回 [state, call]：state 是响应式状态，call 是执行函数。
 * 调用时自动管理 loading/success/error 状态。
 */
export function useInvoke<T = unknown>(
  backend: Backend,
  cmd: string,
): readonly [
  Signal<CallState<T>>,
  (args?: Record<string, unknown>) => Promise<T | undefined>,
] {
  const state = createSignal<CallState<T>>({ status: 'idle' });

  const call = async (args?: Record<string, unknown>): Promise<T | undefined> => {
    state.set({ status: 'loading' });
    try {
      const result = await backend.invoke<T>(cmd, args);
      state.set({ status: 'success', data: result });
      return result;
    } catch (error) {
      state.set({ status: 'error', error });
      throw error;
    }
  };

  return [state, call];
}

/**
 * useCapabilities：将 core 的 capabilities 封装为响应式信号。
 *
 * 返回 [available, refresh]：available 是响应式的可用命令集合，
 * refresh 重新查询宿主。
 */
export function useCapabilities(
  backend: Backend,
): readonly [Signal<ReadonlySet<string>>, () => void] {
  const caps = createSignal<ReadonlySet<string>>(backend.capabilities());

  const refresh = () => {
    caps.set(backend.capabilities());
  };

  return [caps, refresh];
}

/**
 * useEvent：订阅宿主事件，返回退订函数。
 *
 * 每次订阅都会创建一个 cleanup，支持多个订阅。
 */
export function useEvent(
  backend: Backend,
  event: string,
  handler: (payload: unknown) => void,
): Cleanup {
  let unlisten: Unlisten | null = null;
  let disposed = false;

  backend.listen(event, handler).then((u) => {
    if (disposed) {
      u();
    } else {
      unlisten = u;
    }
  });

  return () => {
    disposed = true;
    unlisten?.();
  };
}

/**
 * usePluginId：将 core 的 pluginId 封装为响应式信号。
 *
 * 派生自 {@link usePrincipal}（R4 后身份的唯一事实源是主体）。
 */
export function usePluginId(backend: Backend): Signal<string | null> {
  const id = createSignal<string | null>(backend.pluginId());
  return id;
}

/**
 * usePrincipal：将调用方身份主体封装为响应式信号（R4）。
 *
 * 主窗由 `{ kind: 'main-window' }` 显式表达，而不是 `pluginId === null`——
 * 这样主窗可以被分级（受限主窗 / kiosk），也能携带 origin 供诊断。
 */
export function usePrincipal(backend: Backend): Signal<Principal> {
  return createSignal<Principal>(backend.principal());
}

/**
 * createReactiveStore：创建一个响应式状态容器。
 *
 * 适用于 settings 等需要响应式更新的场景。
 * 返回 { get, set, watch }。
 */
export function createReactiveStore<T extends Record<string, unknown>>(initial: T) {
  const state = createSignal<T>(initial);
  const versions = createSignal(0);

  const get = (): T => state.value;

  const set = (patch: Partial<T> | ((prev: T) => T)): void => {
    state.set((prev) => {
      const next = typeof patch === 'function' ? patch(prev) : { ...prev, ...patch };
      return next;
    });
    versions.set((v) => v + 1);
  };

  const watch = (effect: (value: T) => void): Cleanup => {
    return state.subscribe(effect);
  };

  return { get, set, watch, version: versions };
}
