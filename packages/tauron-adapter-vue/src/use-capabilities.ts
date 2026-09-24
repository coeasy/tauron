/**
 * useCapabilities composable（设计文档 §4.2）
 *
 * 获取当前可用的运行时能力集。
 */

import { computed } from 'vue';
import { getCapabilities, isTauri, type RuntimeCapabilities } from '@tauron/core';

export function useCapabilities(): Readonly<RuntimeCapabilities> {
  return computed(() => getCapabilities()).value;
}

export function useIsTauri(): boolean {
  return isTauri();
}
