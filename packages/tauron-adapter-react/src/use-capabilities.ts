/**
 * useCapabilities 钩子（设计文档 §4.2）
 *
 * 获取当前可用的运行时能力集。
 */

import { useMemo } from 'react';
import {
  getCapabilities,
  isTauri,
  type RuntimeCapabilities,
} from '@tauron/core';

/**
 * useCapabilities — 获取运行时能力集
 *
 * @returns 当前可用的能力集
 */
export function useCapabilities(): RuntimeCapabilities {
  return useMemo(() => getCapabilities(), []);
}

/**
 * useIsTauri — 检测是否在 Tauri 环境中
 *
 * @returns 是否在 Tauri 环境中
 */
export function useIsTauri(): boolean {
  return useMemo(() => isTauri(), []);
}
