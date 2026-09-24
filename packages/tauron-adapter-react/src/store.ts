/**
 * createReactiveStore（设计文档 §4.2）
 *
 * 创建一个响应式状态存储，支持：
 * - getState/setState
 * - subscribe/unsubscribe
 * - useStore 钩子（通过 useSyncExternalStore）
 */

import { useSyncExternalStore, useCallback } from 'react';

/** Store 订阅者 */
type StoreListener = () => void;

/** Store 接口 */
export interface Store<T> {
  /** 获取当前状态 */
  getState: () => T;
  /** 设置新状态 */
  setState: (value: T | ((prev: T) => T)) => void;
  /** 订阅状态变更 */
  subscribe: (listener: StoreListener) => () => void;
}

/**
 * 创建响应式存储
 *
 * @param initialState - 初始状态
 */
export function createStore<T>(initialState: T): Store<T> {
  let state = initialState;
  const listeners = new Set<StoreListener>();

  return {
    getState: () => state,

    setState: (value) => {
      const nextValue = typeof value === 'function' ? (value as (prev: T) => T)(state) : value;
      if (nextValue !== state) {
        state = nextValue;
        listeners.forEach((listener) => listener());
      }
    },

    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

/**
 * useStore — 订阅存储状态的 React 钩子
 *
 * @param store - 存储实例
 * @returns 当前状态
 */
export function useStore<T>(store: Store<T>): T {
  return useSyncExternalStore(
    store.subscribe,
    store.getState,
    store.getState,
  );
}

/**
 * useStoreSelector — 使用选择器的存储钩子
 *
 * @param store - 存储实例
 * @param selector - 选择器函数
 * @returns 选择器结果
 */
export function useStoreSelector<T, R>(store: Store<T>, selector: (state: T) => R): R {
  const getSnapshot = useCallback(() => selector(store.getState()), [store, selector]);
  return useSyncExternalStore(store.subscribe, getSnapshot, getSnapshot);
}
