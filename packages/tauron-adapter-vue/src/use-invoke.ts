/**
 * useInvoke composable（设计文档 §4.2）
 *
 * 提供插件方法调用的 Vue 响应式状态管理。
 */

import { shallowRef, type Ref } from 'vue';
import { invokePlugin, type InvokeOptions } from '@tauron/core';
import type { PluginInvokeResponse } from '@tauron/types';
import { injectTauronContext } from './context.js';

export interface InvokeState<T = unknown> {
  loading: boolean;
  data: T | null;
  error: { code: string; message: string } | null;
  lastArgs: unknown | null;
}

export interface UseInvokeReturn<T = unknown> {
  state: Ref<InvokeState<T>>;
  invoke: (method: string, payload?: unknown, options?: InvokeOptions) => Promise<T | null>;
  reset: () => void;
}

export function useInvoke<T = unknown>(pluginId: string): UseInvokeReturn<T> {
  const { backend } = injectTauronContext();

  const state = shallowRef<InvokeState<T>>({
    loading: false,
    data: null,
    error: null,
    lastArgs: null,
  });

  const invoke = async (method: string, payload?: unknown, options?: InvokeOptions): Promise<T | null> => {
    state.value = { ...state.value, loading: true, error: null, lastArgs: payload ?? null };

    try {
      const response: PluginInvokeResponse = await invokePlugin(backend, pluginId, method, payload, options);

      if (response.ok) {
        const data = response.result as T;
        state.value = { ...state.value, data, loading: false };
        return data;
      } else {
        state.value = { ...state.value, data: null, error: response.error!, loading: false };
        return null;
      }
    } catch (err) {
      state.value = {
        ...state.value,
        data: null,
        error: {
          code: 'SC-2004',
          message: err instanceof Error ? err.message : String(err),
        },
        loading: false,
      };
      return null;
    }
  };

  const reset = () => {
    state.value = {
      loading: false,
      data: null,
      error: null,
      lastArgs: null,
    };
  };

  return { state, invoke, reset };
}
