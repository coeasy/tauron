/**
 * tauron Vue Context（设计文档 §4.2）
 *
 * 提供全局上下文，包含 backend。
 */

import { provide, inject, type InjectionKey } from 'vue';
import type { TauronBackend } from '@tauron/core';

export interface TauronContextValue {
  backend: TauronBackend;
}

export const TauronContextKey: InjectionKey<TauronContextValue> = Symbol('tauron');

export function provideTauronContext(value: TauronContextValue): void {
  provide(TauronContextKey, value);
}

export function injectTauronContext(): TauronContextValue {
  const ctx = inject(TauronContextKey);
  if (!ctx) {
    throw new Error('injectTauronContext must be used within a component that called provideTauronContext');
  }
  return ctx;
}
