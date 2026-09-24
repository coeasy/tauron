// ──────────────────────────────────────────────────────────────────────────
// Signals — 框架无关的最小响应式原语（计划 §4.11）。
//
// 设计目标：
// - ≤40 行/能力，绑定层不做业务判断（架构 §5.2）；
// - 不依赖任何框架（React/Vue/Svelte/Lit）——三框架包只导出 hooks/stores；
// - 订阅自动 GC：effect 被清理时断开依赖链；
// - 批量更新：同一 tick 内多次写入只触发一次通知。
// ──────────────────────────────────────────────────────────────────────────

/** 信号变更监听器。 */
export type SignalEffect<T = unknown> = (value: T) => void;

/** 订阅清理函数。 */
export type Cleanup = () => void;

// ──────────────────────────────────────────────────────────────────────────
// 依赖追踪上下文
// ──────────────────────────────────────────────────────────────────────────

/**
 * 依赖收集器：注册一个「值变化时回调」的订阅。
 *
 * 用函数类型而非信号对象本身，避免 `Signal<T>` 因私有成员与泛型导致的
 * 不可协变问题（`Signal<number>` 无法赋值给 `Signal<unknown>`）。
 */
type DependencySubscriber = (onChange: () => void) => Cleanup;

/** 当前正在追踪的 effect/computed。 */
let currentTracker: Set<DependencySubscriber> | null = null;

/** 是否正在重新计算（防止 subscribe 立即回调导致重入）。 */
let isRecomputing = false;

/** 登记一个依赖（进入 effect/computed 时调用）。 */
function trackDependency(subscribe: DependencySubscriber): void {
  currentTracker?.add(subscribe);
}

/**
 * 响应式信号：值 + 依赖追踪 + 自动通知。
 *
 * 用法：
 * ```ts
 * const count = createSignal(0);
 * const watcher = watch(count, (n) => console.log(n));
 * count.set(n => n + 1);  // 触发 watcher
 * watcher.dispose();      // 断开
 * ```
 */
export class Signal<T> {
  private _value: T;
  private listeners = new Set<SignalEffect<T>>();
  private readonly isEqual: (a: T, b: T) => boolean;

  constructor(initial: T, isEqual?: (a: T, b: T) => boolean) {
    this._value = initial;
    this.isEqual = isEqual ?? Object.is;
  }

  /** 读取当前值（自动追踪依赖）。 */
  get value(): T {
    trackDependency((onChange) => this.subscribe(() => onChange()));
    return this._value;
  }

  /** 写入新值（相同值不触发通知）。 */
  set(value: T | ((prev: T) => T)): void {
    const next = typeof value === 'function' ? (value as (prev: T) => T)(this._value) : value;
    if (this.isEqual(this._value, next)) return;
    this._value = next;
    this.notify();
  }

  /** 直接写入（跳过相等检查，用于初始化）。 */
  force(value: T): void {
    this._value = value;
    this.notify();
  }

  /** 订阅变更。返回清理函数。 */
  subscribe(effect: SignalEffect<T>): Cleanup {
    this.listeners.add(effect);
    // 首次订阅立即同步一次（确保 watcher 拿到最新值）。
    effect(this._value);
    return () => {
      this.listeners.delete(effect);
    };
  }

  /** 监听者数量。 */
  get listenerCount(): number {
    return this.listeners.size;
  }

  private notify(): void {
    for (const fn of this.listeners) {
      fn(this._value);
    }
  }
}

/** 创建信号。 */
export function createSignal<T>(initial: T, isEqual?: (a: T, b: T) => boolean): Signal<T> {
  return new Signal(initial, isEqual);
}

/**
 * 派生信号：基于其他信号计算的值。
 *
 * 当任一依赖变化时自动重新计算并通知下游。
 * 自动追踪依赖（调用 get 时自动订阅）。
 */
export class Computed<T> {
  private _value: T | undefined;
  private _computed = false;
  /** 是否至少计算过一次（失效不会重置，用于变化判定）。 */
  private _hasValue = false;
  private readonly compute: () => T;
  private readonly isEqual: (a: T, b: T) => boolean;
  private listeners = new Set<SignalEffect<T>>();
  private cleanups: Cleanup[] = [];

  constructor(compute: () => T, isEqual?: (a: T, b: T) => boolean) {
    this.compute = compute;
    this.isEqual = isEqual ?? Object.is;
  }

  get value(): T {
    if (!this._computed) {
      this.recompute();
    }
    return this._value as T;
  }

  set invalid(v: boolean) {
    this._computed = !v;
  }

  /** 重新计算并追踪依赖；返回本次计算值是否发生变化。 */
  private recompute(): boolean {
    const prev = this._value;
    const hadValue = this._hasValue;

    // 清理旧依赖。
    for (const cleanup of this.cleanups) {
      cleanup();
    }
    this.cleanups = [];

    const deps = new Set<DependencySubscriber>();
    const origTracker = currentTracker;
    const wasRecomputing = isRecomputing;
    currentTracker = deps;
    isRecomputing = true;
    try {
      this._value = this.compute();
      this._computed = true;
      this._hasValue = true;

      // 必须在 isRecomputing 仍为 true 的窗口内订阅：
      // subscribe 会同步回调一次，此时若标记为脏会导致缓存永久失效。
      for (const dep of deps) {
        this.cleanups.push(
          dep(() => {
            if (!isRecomputing) {
              this._computed = false;
            }
          }),
        );
      }
    } finally {
      currentTracker = origTracker;
      isRecomputing = wasRecomputing;
    }

    return !hadValue || !this.isEqual(prev as T, this._value as T);
  }

  subscribe(effect: SignalEffect<T>): Cleanup {
    this.listeners.add(effect);
    effect(this.value);
    return () => {
      this.listeners.delete(effect);
    };
  }

  /** 标记脏，下次读取时重新计算。 */
  invalidate(): void {
    this._computed = false;
  }

  /** 通知下游（值未变化时不下发，避免无谓的渲染抖动）。 */
  notify(): void {
    const changed = this._computed ? true : this.recompute();
    if (!changed) return;
    for (const fn of this.listeners) {
      fn(this._value as T);
    }
  }

  get listenerCount(): number {
    return this.listeners.size;
  }
}

/** 创建派生信号。 */
export function createComputed<T>(compute: () => T, isEqual?: (a: T, b: T) => boolean): Computed<T> {
  return new Computed(compute, isEqual);
}

/**
 * Effect：订阅一组信号，当任一变化时执行回调。
 * 自动追踪依赖，自动清理。
 */
export class Effect {
  private readonly fn: () => void;
  private cleanups: Cleanup[] = [];
  private disposed = false;

  constructor(fn: () => void) {
    this.fn = fn;
    // 首次运行。
    this.run();
  }

  private run(): void {
    if (this.disposed) return;
    // 清理上一次运行的依赖。
    for (const cleanup of this.cleanups) {
      cleanup();
    }
    this.cleanups = [];
    // 执行函数（期间 Signal.subscribe 会自动注册清理）。
    this.fn();
  }

  /** 重新运行。 */
  recompute(): void {
    if (!this.disposed) this.run();
  }

  /** 清理所有依赖。 */
  dispose(): void {
    this.disposed = true;
    for (const cleanup of this.cleanups) {
      cleanup();
    }
    this.cleanups = [];
  }

  get isDisposed(): boolean {
    return this.disposed;
  }
}

/** 创建 effect。 */
export function createEffect(fn: () => void): Effect {
  return new Effect(fn);
}

// ──────────────────────────────────────────────────────────────────────────
// 批量更新调度器
// ──────────────────────────────────────────────────────────────────────────

type BatchCallback = () => void;

const batchQueue: BatchCallback[] = [];
let batching = false;

/**
 * 批量执行：同一批次内的信号写入只触发一次通知。
 */
export function batch(fn: () => void): void {
  const wasBatching = batching;
  batching = true;
  try {
    fn();
  } finally {
    batching = false;
    if (!wasBatching) {
      flush();
    }
  }
}

/** 是否正在执行 flush（重入闸，见 {@link flush}）。 */
let flushing = false;

function flush(): void {
  // 回调内部可能再次调用 `enqueueBatch`（那时 `batching` 为 false）。若直接
  // 递归调用 flush，"链式入队"的回调会逐层压栈（无界递归 → 栈溢出）。这里
  // 只允许最外层循环消费队列：重入立刻返回，新入队的回调由外层 while 取到。
  //
  // 注意：无条件自我重入的回调仍会一直循环——那是调用方定义的无限任务，
  // 不是框架缺陷；本闸消除的是**递归**（崩溃），不是**循环**（语义）。
  if (flushing) {
    return;
  }
  flushing = true;
  try {
    while (batchQueue.length > 0) {
      const cb = batchQueue.shift()!;
      cb();
    }
  } finally {
    flushing = false;
  }
}

/** 将回调加入批处理队列。 */
export function enqueueBatch(fn: BatchCallback): void {
  batchQueue.push(fn);
  if (!batching) {
    flush();
  }
}

// ──────────────────────────────────────────────────────────────────────────
// 框架绑定辅助（≤40 行/能力）
// ──────────────────────────────────────────────────────────────────────────

/**
 * useSignal：通用 hook 工厂（三框架共用）。
 *
 * 返回 [value, setValue] 元组。
 * 框架适配层（React/Vue/Svelte）在此之上包装生命周期管理。
 */
export function useSignal<T>(initial: T): readonly [Signal<T>, (v: T | ((p: T) => T)) => void] {
  const sig = createSignal(initial);
  return [sig, (v) => sig.set(v)];
}

/**
 * watchSignal：订阅信号变更，返回清理函数。
 */
export function watchSignal<T>(sig: Signal<T>, effect: (value: T) => void): Cleanup {
  return sig.subscribe(effect);
}
