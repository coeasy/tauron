/**
 * 插件注册表（设计文档 §8）
 *
 * 管理插件注册表，提供插件查询和版本管理。
 */

import type { PluginManifest } from '@tauron/types';
import type { IndexEntry, PluginPackage } from './types.js';

/** 注册表内部数据结构 */
interface RegistryInternal {
  plugins: Map<string, IndexEntry>;
  updatedAt: string;
}

/**
 * 创建插件注册表
 */
export function createRegistry(): Registry {
  const internal: RegistryInternal = {
    plugins: new Map(),
    updatedAt: new Date().toISOString(),
  };

  return {
    register(pluginId: string, version: string, manifest: PluginManifest, packageInfo: PluginPackage): IndexEntry {
      const entry: IndexEntry = {
        id: pluginId,
        version,
        package: packageInfo,
        manifest,
        updatedAt: new Date().toISOString(),
      };
      internal.plugins.set(pluginId, entry);
      internal.updatedAt = entry.updatedAt;
      return entry;
    },

    get(pluginId: string): IndexEntry | undefined {
      return internal.plugins.get(pluginId);
    },

    getAll(): IndexEntry[] {
      return Array.from(internal.plugins.values());
    },

    remove(pluginId: string): boolean {
      const deleted = internal.plugins.delete(pluginId);
      if (deleted) {
        internal.updatedAt = new Date().toISOString();
      }
      return deleted;
    },

    has(pluginId: string): boolean {
      return internal.plugins.has(pluginId);
    },

    size(): number {
      return internal.plugins.size;
    },

    clear(): void {
      internal.plugins.clear();
      internal.updatedAt = new Date().toISOString();
    },

    search(query: string): IndexEntry[] {
      const q = query.toLowerCase();
      return Array.from(internal.plugins.values()).filter((entry) =>
        entry.id.toLowerCase().includes(q) ||
        entry.manifest.name.toLowerCase().includes(q) ||
        (entry.manifest.description && entry.manifest.description.toLowerCase().includes(q)),
      );
    },

    toJSON(): Record<string, unknown> {
      return {
        version: '1.0',
        generatedAt: internal.updatedAt,
        plugins: Array.from(internal.plugins.values()),
      };
    },
  };
}

export interface Registry {
  register(pluginId: string, version: string, manifest: PluginManifest, packageInfo: PluginPackage): IndexEntry;
  get(pluginId: string): IndexEntry | undefined;
  getAll(): IndexEntry[];
  remove(pluginId: string): boolean;
  has(pluginId: string): boolean;
  size(): number;
  clear(): void;
  search(query: string): IndexEntry[];
  toJSON(): Record<string, unknown>;
}
