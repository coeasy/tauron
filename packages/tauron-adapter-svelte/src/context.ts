/**
 * tauron Svelte Context（设计文档 §4.2）
 *
 * 提供全局上下文，包含 backend。
 */

import type { TauronBackend } from '@tauron/core';

export interface TauronContextValue {
  backend: TauronBackend;
}

let _context: TauronContextValue | null = null;

export function setTauronContext(value: TauronContextValue): void {
  _context = value;
}

export function getTauronContext(): TauronContextValue {
  if (!_context) {
    throw new Error('getTauronContext must be called after setTauronContext');
  }
  return _context;
}

export function clearTauronContext(): void {
  _context = null;
}
