/**
 * pluginIdStore — Svelte store for plugin ID
 *
 * 获取当前插件的 ID。
 */

import { readable, type Readable } from 'svelte/store';

export function createPluginIdStore(): Readable<string | null> {
  return readable(getPluginId());
}

function getPluginId(): string | null {
  if (typeof window === 'undefined') return null;

  const hash = window.location.hash;
  const match = hash.match(/plugin-id=([^&]+)/);
  if (match && match[1]) return decodeURIComponent(match[1]);

  try {
    const parentUrl = window.parent?.location?.href ?? '';
    const parentMatch = parentUrl.match(/plugins\/([^\/]+)/);
    if (parentMatch && parentMatch[1]) return parentMatch[1];
  } catch {
    // Cross-origin
  }

  return null;
}
