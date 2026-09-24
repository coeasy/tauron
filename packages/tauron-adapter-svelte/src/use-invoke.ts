/**
 * invokeStore — Svelte store for plugin invocation
 *
 * 提供插件方法调用的 Svelte 响应式状态管理。
 */

import { writable, type Writable } from 'svelte/store';
import { invokePlugin, type InvokeOptions } from '@tauron/core';
import type { PluginInvokeResponse } from '@tauron/types';
import { getTauronContext } from './context.js';

export interface InvokeState<T = unknown> {
  loading: boolean;
  data: T | null;
  error: { code: string; message: string } | null;
  lastArgs: unknown | null;
}

export interface InvokeStore<T = unknown> extends Writable<InvokeState<T>> {
  invoke: (method: string, payload?: unknown, options?: InvokeOptions) => Promise<T | null>;
  reset: () => void;
}

export function createInvokeStore<T = unknown>(pluginId: string): InvokeStore<T> {
  const { backend } = getTauronContext();

  const { subscribe, set, update } = writable<InvokeState<T>>({
    loading: false,
    data: null,
    error: null,
    lastArgs: null,
  });

  const invoke = async (method: string, payload?: unknown, options?: InvokeOptions): Promise<T | null> => {
    update((state) => ({
      ...state,
      loading: true,
      error: null,
      lastArgs: payload ?? null,
    }));

    try {
      const response: PluginInvokeResponse = await invokePlugin(backend, pluginId, method, payload, options);

      if (response.ok) {
        update((state) => ({
          ...state,
          data: response.result as T,
          loading: false,
        }));
        return response.result as T;
      } else {
        update((state) => ({
          ...state,
          data: null,
          error: response.error!,
          loading: false,
        }));
        return null;
      }
    } catch (err) {
      update((state) => ({
        ...state,
        data: null,
        error: {
          code: 'SC-2004',
          message: err instanceof Error ? err.message : String(err),
        },
        loading: false,
      }));
      return null;
    }
  };

  const reset = () => {
    set({
      loading: false,
      data: null,
      error: null,
      lastArgs: null,
    });
  };

  return { subscribe, set, update, invoke, reset };
}
