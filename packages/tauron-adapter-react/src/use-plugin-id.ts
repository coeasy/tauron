/**
 * usePluginId 钩子（设计文档 §4.2）
 *
 * 获取当前插件的 ID（如果在 iframe 内运行）。
 */

import { useMemo } from 'react';

/**
 * usePluginId — 获取当前插件 ID
 *
 * 在 iframe 内运行时，插件 ID 通过 URL 参数或
 * postMessage 握手传递。
 *
 * @returns 插件 ID 或 null（非插件环境）
 */
export function usePluginId(): string | null {
  return useMemo(() => {
    // Check if running in an iframe with a plugin ID
    if (typeof window === 'undefined') return null;

    // Check URL hash for plugin ID
    const hash = window.location.hash;
    const match = hash.match(/plugin-id=([^&]+)/);
    if (match && match[1]) return decodeURIComponent(match[1]);

    // Check parent URL for plugin ID
    try {
      const parentUrl = window.parent?.location?.href ?? '';
      const parentMatch = parentUrl.match(/plugins\/([^\/]+)/);
      if (parentMatch && parentMatch[1]) return parentMatch[1];
    } catch {
      // Cross-origin: cannot access parent URL
    }

    return null;
  }, []);
}
