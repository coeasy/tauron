/**
 * tauron 插件注册表（设计文档 §4.4）
 *
 * TypeScript 侧注册表：管理插件状态和元数据。
 * Rust 侧注册表在 tauron-shell crate 中实现。
 */

import {
  type PluginState,
  type PluginType,
  TRANSITIONS,
  isValidTransition,
  TERMINAL_STATES,
  ACTIVE_STATES,
  PluginErrorCode,
  type PluginInvokeResponse,
  buildOkResponse,
  buildErrorResponse,
} from '@tauron/types';

/** 插件注册表条目 */
export interface RegistryEntry {
  pluginId: string;
  state: PluginState;
  type: PluginType;
  manifest: unknown;
  registeredAt: string;
  updatedAt: string;
}

/** 插件注册表配置 */
export interface RegistryConfig {
  /** 最大插件数量 */
  maxPlugins?: number;
  /** 是否允许自动更新 */
  autoUpdate?: boolean;
}

/** 插件状态变更回调 */
export type StateChangeListener = (
  pluginId: string,
  from: PluginState,
  to: PluginState,
) => void;

/**
 * 插件注册表
 */
export class PluginRegistry {
  private entries: Map<string, RegistryEntry> = new Map();
  private listeners: Set<StateChangeListener> = new Set();
  private config: RegistryConfig;

  constructor(config: RegistryConfig = {}) {
    this.config = config;
  }

  /**
   * 注册插件
   */
  register(
    pluginId: string,
    type: PluginType,
    manifest: unknown,
  ): boolean {
    // Check capacity
    const max = this.config.maxPlugins ?? Infinity;
    if (this.entries.size >= max) {
      return false;
    }

    // Check for duplicate
    if (this.entries.has(pluginId)) {
      return false;
    }

    const now = new Date().toISOString();
    this.entries.set(pluginId, {
      pluginId,
      state: 'DISCOVERED',
      type,
      manifest,
      registeredAt: now,
      updatedAt: now,
    });

    return true;
  }

  /**
   * 迁移插件状态（表驱动 + 单点收口）
   */
  transition(pluginId: string, to: PluginState): boolean {
    const entry = this.entries.get(pluginId);
    if (!entry) return false;

    const from = entry.state;
    if (!isValidTransition(from, to)) {
      console.error(`Invalid transition: ${from} → ${to} for ${pluginId}`);
      return false;
    }

    entry.state = to;
    entry.updatedAt = new Date().toISOString();

    // Notify listeners
    this.listeners.forEach((listener) => {
      try {
        listener(pluginId, from, to);
      } catch (err) {
        console.error('State change listener error:', err);
      }
    });

    return true;
  }

  /**
   * 获取插件状态
   */
  getState(pluginId: string): PluginState | undefined {
    return this.entries.get(pluginId)?.state;
  }

  /**
   * 获取插件条目
   */
  getEntry(pluginId: string): RegistryEntry | undefined {
    return this.entries.get(pluginId);
  }

  /**
   * 列出所有已注册的插件 ID
   */
  listPluginIds(): string[] {
    return [...this.entries.keys()];
  }

  /**
   * 列出所有活跃（ENABLED）的插件 ID
   */
  listActivePluginIds(): string[] {
    return [...this.entries.values()]
      .filter((entry) => ACTIVE_STATES.has(entry.state))
      .map((entry) => entry.pluginId);
  }

  /**
   * 注销插件（仅当处于终态或可卸载状态）
   */
  unregister(pluginId: string): boolean {
    const entry = this.entries.get(pluginId);
    if (!entry) return false;

    // Must be in UNINSTALLING terminal state or DISABLED
    if (entry.state !== 'UNINSTALLING' && entry.state !== 'DISABLED') {
      return false;
    }

    this.entries.delete(pluginId);
    return true;
  }

  /**
   * 注销所有插件
   */
  clear(): void {
    this.entries.clear();
  }

  /**
   * 获取当前大小
   */
  get size(): number {
    return this.entries.size;
  }

  /**
   * 订阅状态变更
   */
  onStateChange(listener: StateChangeListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
}
