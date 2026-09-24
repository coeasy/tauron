/**
 * capabilitiesStore — Svelte store for runtime capabilities
 *
 * 获取当前可用的运行时能力集。
 */

import { readable, type Readable } from 'svelte/store';
import { getCapabilities, isTauri, type RuntimeCapabilities } from '@tauron/core';

export function createCapabilitiesStore(): Readable<RuntimeCapabilities> {
  return readable(getCapabilities());
}

export function createIsTauriStore(): Readable<boolean> {
  return readable(isTauri());
}
