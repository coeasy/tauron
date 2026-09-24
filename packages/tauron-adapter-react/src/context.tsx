/**
 * tauron React Context（设计文档 §4.2）
 *
 * TauronProvider 提供全局上下文。
 *
 * ⚠️ 当前上下文**只含 `backend`**（`registry` 在宿主侧、`config`/`eventBus` 需
 * 由应用自行装配；早先的文档曾声称四项都在上下文里，那是错的）。事件总线要
 * 自己接线，`backend.listen()` 不支持通配主题，必须按主题逐个订阅：
 *
 * ```typescript
 * const bus = new EventBus();
 * const off = await backend.listen('plugin:com.example.a:changed', (payload) =>
 *   bus.emit({ sourcePlugin: 'com.example.a', eventName: 'changed', payload, timestamp: new Date().toISOString() }),
 * );
 * // 卸载时：off() 退订，bus.clearPlugin(id) 清队列
 * ```
 */

import { createContext, useContext, type ReactNode, type MutableRefObject } from 'react';
import type { TauronBackend } from '@tauron/core';

/** tauron 上下文值 */
export interface TauronContextValue {
  /** IPC 后端 */
  backend: TauronBackend;
}

/** tauron 上下文 */
export const TauronContext = createContext<TauronContextValue | null>(null);

/** Provider Props */
export interface TauronProviderProps {
  /** IPC 后端 */
  backend: TauronBackend;
  /** 子组件 */
  children: ReactNode;
}

/**
 * TauronProvider — 提供 tauron 上下文
 */
export function TauronProvider({ backend, children }: TauronProviderProps) {
  return (
    <TauronContext.Provider value={{ backend }}>
      {children}
    </TauronContext.Provider>
  );
}

/**
 * useTauronContext — 获取 tauron 上下文
 */
export function useTauronContext(): TauronContextValue {
  const ctx = useContext(TauronContext);
  if (!ctx) {
    throw new Error('useTauronContext must be used within a TauronProvider');
  }
  return ctx;
}
