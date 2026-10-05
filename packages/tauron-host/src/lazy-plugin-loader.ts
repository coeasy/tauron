// ──────────────────────────────────────────────────────────────────────────
// LazyPluginLoader — 插件懒加载管理器（P2-11）。
//
// 职责：
// 1. 存储插件描述符但延迟初始化
// 2. 首次调用时加载插件
// 3. 提供 isLoaded() 查询状态
// 4. 支持加载失败重试
// 5. 代际取消（V7-P1-02）：`register` / `reset` / `clear` 抬高该 id 的
//    generation，旧一代的 `_doLoad` 落定时只**丢弃结果**——不写状态、
//    不发通知、不留实例；`load(id, { signal })` 可取消当次加载。
//
// 用法：
// ```typescript
// const loader = new LazyPluginLoader();
// loader.register({ id: 'p.lazy', entry: () => import('./plugin') });
//
// // 首次调用触发加载
// await loader.load('p.lazy');
// ```
//
// ⚠️ **取消的真实边界**：只有**配合 `signal` 的 entry** 才会停下来。不配合的
// entry 无法被外部中止，本类能做的是把它的结果丢弃并计入
// `stats().abandonedLoads`——这是「结果作废」，不是「任务已取消」。
// ──────────────────────────────────────────────────────────────────────────

/** 懒加载插件描述符 */
export interface LazyPluginDescriptor {
  id: string;
  /**
   * 插件入口工厂函数（返回 Promise）。
   *
   * 会收到 `load(id, { signal })` 传入的取消信号（无信号时为 `undefined`）。
   * 零参写法依然合法（无参函数可赋给带参签名），但这类 entry **不可取消**：
   * 它的结果只会被丢弃，不会被打断。
   */
  entry: (signal?: AbortSignal) => Promise<unknown>;
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

/** `load()` 的可选参数 */
export interface LazyPluginLoadOptions {
  /**
   * 取消本次加载。中止时 `load()` 以 `name === 'AbortError'`、
   * `code === 'E_PLUGIN_LOAD_ABORTED'` 的错误 reject，状态如实落 `failed`
   * （不会静默回到 `pending`，也不会假装已加载）。
   */
  signal?: AbortSignal;
}

/**
 * 懒加载器的本地错误码。
 *
 * ⚠️ **不是 wire 码**：不进 `contracts/error/error-codes.json`，也不与 Rust
 * `ErrorCode` 比对——那两个是宿主 IPC 的协议面，而这两个只描述「本管理器自己
 * 的一次加载为什么没有产出可用实例」。
 */
export type LazyPluginLoadErrorCode = 'E_PLUGIN_LOAD_ABANDONED' | 'E_PLUGIN_LOAD_ABORTED';

/** 懒加载器的本地错误对象。 */
export class LazyPluginLoadError extends Error {
  readonly code: LazyPluginLoadErrorCode;
  readonly pluginId: string;

  constructor(
    code: LazyPluginLoadErrorCode,
    pluginId: string,
    message: string,
    options?: ErrorOptions,
  ) {
    super(message, options);
    this.name = 'LazyPluginLoadError';
    this.code = code;
    this.pluginId = pluginId;
  }
}

/** 生命周期观测计数（不是业务状态；供测试与泄漏门禁读取）。 */
export interface LazyPluginLoaderStats {
  /**
   * 被判定废弃的在途加载数：代际被 `register`/`reset`/`clear` 抬高，
   * 或调用方的 `signal` 中止。计数发生在**判定时刻**（不等任务 settle），
   * 所以永不 settle 的 entry 也会被计入。
   */
  abandonedLoads: number;
  /**
   * 已废弃的加载**后来又产出了结果**而被丢弃的次数。
   *
   * 这就是审计说的「对外不可见却仍被持有的实例」：entry 不配合取消时无法打断，
   * 只能让它产出的实例作废。`abandonedLoads - discardedResults` 即尚未落定的那批。
   */
  discardedResults: number;
  /** 仍在执行的 `_doLoad` 任务数（含已废弃但尚未落定的）。 */
  runningTasks: number;
  /**
   * 仍在排队的超时定时器数。废弃与落定都必须清 0——残留定时器会拖住进程退出。
   */
  activeTimers: number;
  /** 当前在途（未被废弃）的加载记录数。 */
  inflightLoads: number;
}

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
 * 一次在途加载的运行时记录。
 *
 * 三件事必须同时可表达，否则旧任务会污染新一代：
 * - `generation`：捕获时的代际，落定时与 `_generations` 比对，不符即丢弃；
 * - `release`：队列放行信号。**已废弃但尚未 settle** 的加载不能占住串行队列
 *   （旧 descriptor 卡死时新一代也得能跑），所以队列链取「任务落定」与
 *   「被判废弃」两者之先；
 * - `abandoned` / `discarded`：一次性标记，保证同一加载的各计数器只加一次。
 */
interface InflightLoad {
  readonly generation: number;
  /**
   * 对外暴露的加载 promise，供并发 `load()` 复用。
   *
   * 由 `load()` 在同一个同步块内、任何 `await` 之前覆写为真实排队结果；
   * 初始值只是占位（外部观察不到中间态）。
   */
  promise: Promise<unknown>;
  readonly signal: AbortSignal | undefined;
  readonly release: Promise<void>;
  readonly markAbandoned: () => void;
  abandoned: boolean;
  discarded: boolean;
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
  /**
   * 每个插件**当前这一代**在途的加载记录。
   *
   * 为什么必须单独存一份、而不能只靠 `entry.status === 'loading'` 判断：
   * `_doLoad` 是**排进队列之后**才执行的（`this._loadQueue.then(() => ...)`），
   * 所以在「已排队、尚未开始」这段窗口里 `entry.status` 仍是 `pending`。
   * 两个并发调用都会看到 `pending`，各自往队列里排一次 `_doLoad`，
   * 结果是 `entry()` 被调用两次、插件被初始化两遍（副作用重复、attempts 被多吃一格）。
   * 判据见 `concurrent load reuses a single in-flight promise` 测试。
   */
  private _inflight: Map<string, InflightLoad> = new Map();
  /**
   * 每个插件 id 的代际。`register`/`reset`/`clear` 只增不减。
   *
   * ⚠️ `clear()` 也**不删**这张表（只把每个 id 抬高）：删表会让重新注册撞上
   * 旧任务捕获过的号，废弃任务就能重新「自认为当前」并提交结果。
   */
  private _generations: Map<string, number> = new Map();
  private _subscribers: Set<(pluginId: string, status: PluginLoadStatus) => void> = new Set();
  private _abandonedLoads = 0;
  private _discardedResults = 0;
  private _runningTasks = 0;
  private _activeTimers = 0;

  /**
   * 注册一个懒加载插件。
   *
   * 重复注册同一 id 会**替换**描述符、把状态重置为 `pending`，并抬高代际使
   * 此前的在途加载作废（其结果只会被丢弃，不会写回新一代）。
   */
  register(descriptor: LazyPluginDescriptor): void {
    this._invalidate(descriptor.id);
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
   * - 已加载 → 直接返回缓存实例；
   * - **同一代际已有在途加载 → 复用同一个 promise**（同一插件只初始化一次）；
   * - 上次失败且重试预算耗尽 → 如实抛出上次的错误；
   * - 否则排队执行加载（队列串行化，但失败/废弃不阻塞后续加载）。
   *
   * `options.signal` 中止时以 `AbortError`（`code: 'E_PLUGIN_LOAD_ABORTED'`）
   * reject，并把状态落为 `failed`。
   */
  async load(pluginId: string, options: LazyPluginLoadOptions = {}): Promise<unknown> {
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

    const generation = this._generationOf(pluginId);

    // 已有**同代**在途加载：复用，**不再**排第二次 `_doLoad`。
    // 这一步同时取代了旧的 `status === 'loading'` 分支——后者漏掉了
    // 「已排队但尚未开始」（status 仍为 pending）的窗口，且靠递归重入，
    // 理论上可无限递归。
    const running = this._inflight.get(pluginId);
    if (running && running.generation === generation) {
      return running.promise;
    }

    // 检查重试次数（attempts 是已尝试次数，maxRetries 是允许重试次数）
    const maxRetries = retryBudget(descriptor);
    if (entry.status === 'failed' && entry.attempts > maxRetries) {
      throw entry.error ?? new Error(`Plugin failed to load: ${pluginId}`);
    }

    let resolveRelease!: () => void;
    const release = new Promise<void>((resolve) => {
      resolveRelease = resolve;
    });
    const load: InflightLoad = {
      generation,
      signal: options.signal,
      release,
      promise: Promise.resolve(undefined),
      abandoned: false,
      discarded: false,
      markAbandoned: () => {
        if (load.abandoned) return;
        load.abandoned = true;
        this._abandonedLoads += 1;
        resolveRelease();
      },
    };

    // 执行加载（队列串行化；落定或废弃都放行，见 `_loadQueue` 那行）
    const loadPromise = this._loadQueue.then(() => this._doLoad(descriptor, entry, load));
    load.promise = loadPromise;
    this._inflight.set(pluginId, load);
    // 失败不阻塞后续；**被废弃的加载同样不阻塞后续**（取两者之先）。
    this._loadQueue = Promise.race([loadPromise.catch(() => {}), load.release]);

    try {
      return await loadPromise;
    } finally {
      // 只清理「仍属于本次」的那条：`reset()`/`register()` 可能已经换掉了它。
      if (this._inflight.get(pluginId) === load) {
        this._inflight.delete(pluginId);
      }
    }
  }

  /**
   * 实际执行加载（含自动重试）。
   *
   * 每一处状态/实例/错误/通知的写入都先过代际闸门：不符就丢弃结果并计数。
   */
  private async _doLoad(
    descriptor: LazyPluginDescriptor,
    entry: PluginCacheEntry,
    load: InflightLoad,
  ): Promise<unknown> {
    const maxRetries = retryBudget(descriptor);
    this._runningTasks += 1;

    try {
      while (true) {
        // 「已排队但代际已换」：一次都不该尝试——不占 attempts，也不发通知。
        if (this._isStale(descriptor.id, load.generation)) {
          throw this._abandonedError(descriptor.id, load.generation);
        }

        entry.status = 'loading';
        entry.loadStartTime = Date.now();
        entry.attempts += 1;
        this._notify(descriptor.id, 'loading');

        try {
          const instance = await this._loadOnce(descriptor, load);

          // 结果到手时代际已换：这是「新一代已经接管」的时刻，旧实例
          // 一律不提交（否则外部看不到它，但它仍被持有 → 逻辑孤儿）。
          if (this._isStale(descriptor.id, load.generation)) {
            this._markDiscardedResult(load);
            throw this._abandonedError(descriptor.id, load.generation);
          }

          if (instance === undefined || instance === null) {
            throw new Error(`Plugin entry returned null/undefined: ${descriptor.id}`);
          }

          entry.status = 'loaded';
          entry.instance = instance;
          delete entry.error;
          this._notify(descriptor.id, 'loaded');

          return instance;
        } catch (err) {
          if (err instanceof LazyPluginLoadError && err.code === 'E_PLUGIN_LOAD_ABANDONED') {
            // 代际作废：不重试、不提交。`entry` 若后来又产出结果，由
            // `_loadOnce` 里的观察回调计入 `discardedResults`。
            throw err;
          }

          if (err instanceof LazyPluginLoadError && err.code === 'E_PLUGIN_LOAD_ABORTED') {
            // 调用方取消：放行队列，并在**仍是当前代际**时如实落 `failed`。
            load.markAbandoned();
            if (!this._isStale(descriptor.id, load.generation)) {
              entry.status = 'failed';
              entry.error = err;
              this._notify(descriptor.id, 'failed');
            }
            throw err;
          }

          const lastError = err instanceof Error ? err : new Error(String(err));

          // 检查是否可以重试
          if (entry.attempts > maxRetries) {
            if (this._isStale(descriptor.id, load.generation)) {
              throw this._abandonedError(descriptor.id, load.generation);
            }
            entry.status = 'failed';
            entry.error = lastError;
            this._notify(descriptor.id, 'failed');
            throw lastError;
          }

          // 重置状态，继续重试。这里同样先过代际闸门：`reset()` 复用同一个
          // entry 对象，陈旧一代写 `pending` 会把新一代的 `loading` 抹掉。
          if (this._isStale(descriptor.id, load.generation)) {
            throw this._abandonedError(descriptor.id, load.generation);
          }
          entry.status = 'pending';
        }
      }
    } finally {
      this._runningTasks -= 1;
    }
  }

  /**
   * 单次加载尝试：同时等 `entry`、超时、代际失效（`release`）、调用方 signal。
   *
   * 定时器与 signal 监听**必须**在落定后立刻摘掉：`Promise.race` 只丢弃**落败者**
   * 的结果，**不会取消**仍在排队的 `setTimeout`，也不会移除监听器。漏掉一处就会让
   * 一个已被废弃的加载把 30s 定时器留在事件循环里，拖住进程退出
   * （验收项：`clear()` 后无残留 timer）。
   * 与 `plugin-sdk` 的 `host-invoke.ts` / `core` 的 `tauri-backend.ts` 同做法。
   */
  private async _loadOnce(descriptor: LazyPluginDescriptor, load: InflightLoad): Promise<unknown> {
    const { signal } = load;
    if (signal?.aborted) {
      throw this._abortedError(descriptor.id, signal);
    }

    const timeoutMs = descriptor.timeoutMs ?? 30000;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let onAbort: (() => void) | undefined;

    // 包一层 then：entry 同步抛错也应变成 reject。
    const entryPromise = Promise.resolve().then(() => descriptor.entry(signal));
    // entry 不配合取消时打断不了它。它若在加载已作废**之后**才产出实例，
    // 那个实例对外不可见却已存在（审计里的「逻辑孤儿」）——这里只做观测计数，
    // 两个回调都取值，避免派生 promise 变成未处理 reject。
    entryPromise.then(
      () => {
        if (load.abandoned) this._markDiscardedResult(load);
      },
      () => {},
    );

    const racers: Promise<unknown>[] = [
      entryPromise,
      load.release.then(() => {
        throw this._abandonedError(descriptor.id, load.generation);
      }),
    ];

    if (timeoutMs > 0) {
      racers.push(
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => {
            reject(new Error(`Plugin load timed out after ${timeoutMs}ms: ${descriptor.id}`));
          }, timeoutMs);
          this._activeTimers += 1;
        }),
      );
    }

    if (signal) {
      racers.push(
        new Promise<never>((_, reject) => {
          onAbort = () => reject(this._abortedError(descriptor.id, signal));
          signal.addEventListener('abort', onAbort, { once: true });
        }),
      );
    }

    try {
      return await Promise.race(racers);
    } finally {
      if (timer !== undefined) {
        this._activeTimers -= 1;
        clearTimeout(timer);
      }
      if (signal && onAbort) {
        signal.removeEventListener('abort', onAbort);
      }
    }
  }

  /**
   * 重置插件状态（允许重新加载）。
   *
   * 抬高代际：该插件**此前**的在途加载（已排队或正在跑）落定时只会被丢弃，
   * 既不写回 `loaded`/`failed`，也不发通知。语义是「旧结果作废」，
   * 不是「已打断 entry」——不配合 `signal` 的 entry 仍会跑完，只是结果不算。
   */
  reset(pluginId: string): void {
    this._invalidate(pluginId);
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
   *
   * 抬高**全部**代际并作废全部在途加载：此后旧任务不会复活任何条目，
   * 也不会发出任何通知；残留定时器由 `release` 竞速那条清掉。
   */
  clear(): void {
    for (const pluginId of Array.from(this._generations.keys())) {
      this._invalidate(pluginId);
    }
    this._plugins.clear();
    this._cache.clear();
    this._inflight.clear();
    this._subscribers.clear();
  }

  /**
   * 观测计数快照（废弃加载 / 丢弃结果 / 未结束任务 / 未清定时器）。
   *
   * 泄漏门禁的读法：所有加载落定后 `runningTasks`、`activeTimers`、
   * `inflightLoads` 都应回到 0。
   */
  stats(): LazyPluginLoaderStats {
    return {
      abandonedLoads: this._abandonedLoads,
      discardedResults: this._discardedResults,
      runningTasks: this._runningTasks,
      activeTimers: this._activeTimers,
      inflightLoads: this._inflight.size,
    };
  }

  /**
   * 被判定废弃的在途加载数（`stats().abandonedLoads` 的便捷读法）。
   */
  get abandonedLoads(): number {
    return this._abandonedLoads;
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

  /** 当前代际（未注册过的 id 返回 `-1`，任何捕获值都不会与它相等）。 */
  private _generationOf(pluginId: string): number {
    return this._generations.get(pluginId) ?? -1;
  }

  /** 捕获的代际是否已被 `register`/`reset`/`clear` 取代。 */
  private _isStale(pluginId: string, generation: number): boolean {
    return this._generationOf(pluginId) !== generation;
  }

  /** 抬高代际 + 作废该 id 的在途加载（一次性计数，见 `markAbandoned`）。 */
  private _invalidate(pluginId: string): void {
    this._generations.set(pluginId, this._generationOf(pluginId) + 1);
    const running = this._inflight.get(pluginId);
    if (running) {
      this._inflight.delete(pluginId);
      running.markAbandoned();
    }
  }

  /** 记一次「废弃加载又产出了结果」，按记录幂等。 */
  private _markDiscardedResult(load: InflightLoad): void {
    if (load.discarded) return;
    load.discarded = true;
    this._discardedResults += 1;
  }

  private _abandonedError(pluginId: string, generation: number): LazyPluginLoadError {
    return new LazyPluginLoadError(
      'E_PLUGIN_LOAD_ABANDONED',
      pluginId,
      `Plugin load abandoned: ${pluginId}（generation ${generation} 已不是当前代际，结果已丢弃）`,
    );
  }

  private _abortedError(pluginId: string, signal: AbortSignal): LazyPluginLoadError {
    const err = new LazyPluginLoadError(
      'E_PLUGIN_LOAD_ABORTED',
      pluginId,
      `Plugin load aborted: ${pluginId}`,
      // 保持 abort 形状并把调用方的 reason 原样带上，不改写成语义不同的错误。
      signal.reason === undefined ? undefined : { cause: signal.reason },
    );
    err.name = 'AbortError';
    return err;
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
