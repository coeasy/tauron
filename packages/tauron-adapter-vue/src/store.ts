/**
 * createReactiveStore（设计文档 §4.2）
 *
 * 创建响应式状态存储，支持 Vue 3 reactive API。
 */

import { reactive, watch } from 'vue';

export interface Store<T extends object> {
  state: T & Record<string, unknown>;
  setState: (value: Partial<T> | ((prev: T) => Partial<T>)) => void;
  subscribe: (callback: (state: T) => void, options?: { immediate?: boolean }) => () => void;
}

export function createStore<T extends object>(initialState: T): Store<T> {
  const state = reactive({ ...initialState }) as T & Record<string, unknown>;

  return {
    state,
    setState: (value) => {
      const nextValue = typeof value === 'function' ? (value as (prev: T) => Partial<T>)(state as T) : value;
      Object.assign(state, nextValue);
    },
    subscribe: (callback, options = {}) => {
      if (options.immediate) {
        callback(state as T);
      }
      const stop = watch(
        () => ({ ...state }),
        (newState) => callback(newState as T),
        { deep: true },
      );
      return () => stop();
    },
  };
}
