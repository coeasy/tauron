// ──────────────────────────────────────────────────────────────────────────
// LazyPluginLoader — 插件懒加载管理器（P2-11）。
//
// 职责：
// 1. 存储插件描述符但延迟初始化
// 2. 首次调用时加载插件
// 3. 提供 isLoaded() 查询状态
// 4. 支持加载失败重试
//
// 用法：
// ```typescript
// const loader = new LazyPluginLoader();
// loader.register({ id: 'p.lazy', entry: () => import('./plugin') });
//
// // 首次调用触发加载
// await loader.load('p.lazy');
// ```
// ──────────────────────────────────────────────────────────────────────────

/** 懒加载插件描述符 */
export interface LazyPluginDescriptor {
  id: string;
  /** 插件入口工厂函数（返回 Promise） */
  entry: () => Promise<unknown>;
  /** 优先级（数字越小越先加载） */
  priority?: number;
  /** 依赖的插件 ID 列表 */
  dependsOn?: string[];
  /** 加载超时（ms），0 = 不超时 */
  timeoutMs?: number;
  /** 最大重试次数 */
  maxRetries?: number;
  [key: string]: unknown;
}

/** 插件加载状态 */
export type PluginLoadStatus = 'pending' | 'loading' | 'loaded' | 'failed';

/** 重试次数上限（防止恶意/错误配置导致无界重试循环）。 */
const RETRY_CEILING = 10;

/** 计算安全重试预算：非负整数且 ≤ RETRY_CEILING。 */
function retryBudget(descriptor: Pick<LazyPluginDescriptor, 'maxRetries'>): number {
  const raw = descriptor.maxRetries ?? 0;
  if (!Number.isFinite(raw)) return 0;
  return Math.max(0, Math.min(Math.floor(raw), RETRY_CEILING));
}

/** 插件实例缓存条目 */
export interface PluginCacheEntry {
  status: PluginLoadStatus;
  instance?: unknown;
  error?: Error;
  loadStartTime?: number;
  attempts: number;
}

/**
 * LazyPluginLoader — 插件懒加载管理器。
 *
 * 线程安全：所有加载操作串行化，避免并发加载同一插件。
 */
export class LazyPluginLoader {
  private _plugins: Map<string, LazyPluginDescriptor> = new Map();
  private _cache: Map<string, PluginCacheEntry> = new Map();
  private _loadQueue: Promise<unknown> = Promise.resolve();
  private _subscribers: Set<(pluginId: string, status: PluginLoadStatus) => void> = new Set();

  /**
   * 注册一个懒加载插件。
   */
  register(descriptor: LazyPluginDescriptor): void {
    this._plugins.set(descriptor.id, descriptor);
    this._cache.set(descriptor.id, {
      status: 'pending',
      attempts: 0,
    });
  }

  /**
   * 批量注册插件。
   */
  registerAll(descriptors: LazyPluginDescriptor[]): void {
    for (const d of descriptors) {
      this.register(d);
    }
  }

  /**
   * 获取插件加载状态。
   */
  getStatus(pluginId: string): PluginLoadStatus | undefined {
    return this._cache.get(pluginId)?.status;
  }

  /**
   * 检查插件是否已加载。
   */
  isLoaded(pluginId: string): boolean {
    return this._cache.get(pluginId)?.status === 'loaded';
  }

  /**
   * 检查插件是否已失败。
   */
  isFailed(pluginId: string): boolean {
    return this._cache.get(pluginId)?.status === 'failed';
  }

  /**
   * 加载插件（首次调用时触发初始化）。
   *
   * 如果插件已加载，直接返回缓存实例。
   * 如果插件正在加载，等待加载完成。
   * 如果插件加载失败，根据 maxRetries 决定是否重试。
   */
  async load(pluginId: string): Promise<unknown> {
    const descriptor = this._plugins.get(pluginId);
    if (!descriptor) {
      throw new Error(`Plugin not registered: ${pluginId}`);
    }

    const entry = this._cache.get(pluginId);
    if (!entry) {
      throw new Error(`Plugin cache entry not found: ${pluginId}`);
    }

    // 已加载，直接返回
    if (entry.status === 'loaded') {
      return entry.instance;
    }

    // 正在加载，等待加载队列完成
    if (entry.status === 'loading') {
      await this._loadQueue;
      return this.load(pluginId);
    }

    // 检查重试次数（attempts 是已尝试次数，maxRetries 是允许重试次数）
    const maxRetries = retryBudget(descriptor);
    if (entry.status === 'failed' && entry.attempts > maxRetries) {
      throw entry.error ?? new Error(`Plugin failed to load: ${pluginId}`);
    }

    // 执行加载（队列串行化，但失败不阻塞后续加载）
    const loadPromise = this._loadQueue.then(() => this._doLoad(descriptor, entry));
    this._loadQueue = loadPromise.catch(() => {}); // 失败不阻塞后续
    return loadPromise;
  }

  /**
   * 实际执行加载（含自动重试）。
   */
  private async _doLoad(
    descriptor: LazyPluginDescriptor,
    entry: PluginCacheEntry,
  ): Promise<unknown> {
    const maxRetries = retryBudget(descriptor);
    let lastError: Error | undefined;

    while (true) {
      entry.status = 'loading';
      entry.loadStartTime = Date.now();
      entry.attempts += 1;
      this._notify(descriptor.id, 'loading');

      try {
        const instance = await this._loadWithTimeout(descriptor);

        if (instance === undefined || instance === null) {
          throw new Error(`Plugin entry returned null/undefined: ${descriptor.id}`);
        }

        entry.status = 'loaded';
        entry.instance = instance;
        delete entry.error;
        this._notify(descriptor.id, 'loaded');

        return instance;
      } catch (err) {
        lastError = err instanceof Error ? err : new Error(String(err));

        // 检查是否可以重试
        if (entry.attempts > maxRetries) {
          entry.status = 'failed';
          entry.error = lastError;
          this._notify(descriptor.id, 'failed');
          throw lastError;
        }

        // 重置状态，继续重试
        entry.status = 'pending';
      }
    }
  }

  /**
   * 带超时的加载。
   */
  private async _loadWithTimeout(descriptor: LazyPluginDescriptor): Promise<unknown> {
    const timeoutMs = descriptor.timeoutMs ?? 30000;

    if (!timeoutMs || timeoutMs <= 0) {
      return descriptor.entry();
    }

    return Promise.race([
      descriptor.entry(),
      new Promise<never>((_, reject) => {
        setTimeout(() => {
          reject(new Error(`Plugin load timed out after ${timeoutMs}ms: ${descriptor.id}`));
        }, timeoutMs);
      }),
    ]);
  }

  /**
   * 重置插件状态（允许重新加载）。
   */
  reset(pluginId: string): void {
    const entry = this._cache.get(pluginId);
    if (entry) {
      entry.status = 'pending';
      delete entry.instance;
      delete entry.error;
      delete entry.loadStartTime;
      entry.attempts = 0;
      this._notify(pluginId, 'pending');
    }
  }

  /**
   * 获取所有已注册的插件 ID。
   */
  listPluginIds(): string[] {
    return Array.from(this._plugins.keys());
  }

  /**
   * 获取所有已加载的插件 ID。
   */
  listLoadedPluginIds(): string[] {
    return Array.from(this._cache.entries())
      .filter(([, entry]) => entry.status === 'loaded')
      .map(([id]) => id);
  }

  /**
   * 订阅加载状态变化。
   */
  subscribe(fn: (pluginId: string, status: PluginLoadStatus) => void): () => void {
    this._subscribers.add(fn);
    return () => {
      this._subscribers.delete(fn);
    };
  }

  /**
   * 通知订阅者。
   */
  private _notify(pluginId: string, status: PluginLoadStatus): void {
    for (const fn of this._subscribers) {
      try {
        fn(pluginId, status);
      } catch {
        // 订阅者异常不影响其他订阅者
      }
    }
  }

  /**
   * 清理所有缓存。
   */
  clear(): void {
    this._plugins.clear();
    this._cache.clear();
    this._subscribers.clear();
  }

  /**
   * 获取插件缓存数量。
   */
  get size(): number {
    return this._plugins.size;
  }

  /**
   * 获取已加载插件数量。
   */
  get loadedCount(): number {
    return Array.from(this._cache.values()).filter((e) => e.status === 'loaded').length;
  }
}

/**
 * 创建 LazyPluginLoader 实例。
 *
 * 便捷工厂函数。
 */
export function createLazyPluginLoader(): LazyPluginLoader {
  return new LazyPluginLoader();
}