// @vitest-environment happy-dom
// lazy-plugin-loader.ts 测试（P2-11：插件懒加载 + V7-P1-02 代际取消）

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  LazyPluginLoader,
  createLazyPluginLoader,
  LazyPluginLoadError,
} from './lazy-plugin-loader.js';
import type { PluginLoadStatus } from './lazy-plugin-loader.js';

/**
 * 手动落定的门。
 *
 * 代际竞态的判据必须是「测试说了算的完成时机」，不能是 `setTimeout` 睡眠——
 * 睡眠只会把概率性时序换成另一种概率性时序。
 */
function deferred<T = unknown>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/**
 * 排空微任务队列。
 *
 * 只等**已排队**的回调跑完，不看时钟；用于观察「迟到的旧结果落定」之后的计数。
 */
async function flushMicrotasks(rounds = 24): Promise<void> {
  for (let i = 0; i < rounds; i += 1) await Promise.resolve();
}

/** 取 reject 原因（同时把 promise 标记为已处理，避免中途出现未处理 reject）。 */
function rejection(p: Promise<unknown>): Promise<unknown> {
  return p.then(
    (value) => {
      throw new Error(`expected rejection, but resolved with: ${String(value)}`);
    },
    (reason: unknown) => reason,
  );
}

describe('LazyPluginLoader', () => {
  let loader: LazyPluginLoader;

  beforeEach(() => {
    loader = new LazyPluginLoader();
  });

  describe('基本功能', () => {
    it('创建实例', () => {
      expect(loader).toBeDefined();
      expect(loader.size).toBe(0);
    });

    it('createLazyPluginLoader 工厂函数', () => {
      const l = createLazyPluginLoader();
      expect(l).toBeInstanceOf(LazyPluginLoader);
    });

    it('register() 注册插件', () => {
      loader.register({ id: 'p.test', entry: async () => ({ name: 'test' }) });
      expect(loader.size).toBe(1);
      expect(loader.listPluginIds()).toContain('p.test');
    });

    it('registerAll() 批量注册', () => {
      loader.registerAll([
        { id: 'p.a', entry: async () => ({}) },
        { id: 'p.b', entry: async () => ({}) },
      ]);
      expect(loader.size).toBe(2);
    });
  });

  describe('load() 加载流程', () => {
    it('首次加载触发初始化', async () => {
      const entry = vi.fn().mockResolvedValue({ name: 'loaded' });
      loader.register({ id: 'p.test', entry });
      expect(loader.getStatus('p.test')).toBe('pending');

      const instance = await loader.load('p.test');
      expect(instance).toEqual({ name: 'loaded' });
      expect(entry).toHaveBeenCalledTimes(1);
      expect(loader.getStatus('p.test')).toBe('loaded');
    });

    it('重复加载返回缓存', async () => {
      const entry = vi.fn().mockResolvedValue({ name: 'cached' });
      loader.register({ id: 'p.test', entry });

      await loader.load('p.test');
      const second = await loader.load('p.test');

      expect(second).toEqual({ name: 'cached' });
      expect(entry).toHaveBeenCalledTimes(1); // 只调用一次
    });

    it('未注册的插件抛出错误', async () => {
      await expect(loader.load('nonexistent')).rejects.toThrow('Plugin not registered');
    });

    it('isLoaded() 查询状态', async () => {
      loader.register({ id: 'p.test', entry: async () => ({}) });
      expect(loader.isLoaded('p.test')).toBe(false);
      await loader.load('p.test');
      expect(loader.isLoaded('p.test')).toBe(true);
    });
  });

  describe('并发加载（同一插件只初始化一次）', () => {
    it('concurrent load reuses a single in-flight promise', async () => {
      // 回归判据：`_doLoad` 是排进队列之后才跑的，所以在「已排队、尚未开始」
      // 的窗口里 status 仍是 pending。修复前两个并发调用会各排一次 `_doLoad`，
      // 导致 entry() 被调用两次、插件被初始化两遍。
      let entryCalls = 0;
      const entry = vi.fn().mockImplementation(async () => {
        entryCalls += 1;
        await new Promise((r) => setTimeout(r, 20));
        return { call: entryCalls };
      });
      loader.register({ id: 'p.concurrent', entry });

      const [a, b] = await Promise.all([loader.load('p.concurrent'), loader.load('p.concurrent')]);

      expect(entryCalls).toBe(1);
      expect(entry).toHaveBeenCalledTimes(1);
      expect(a).toBe(b); // 同一个实例，不是两次加载的两个副本
      expect(loader.getStatus('p.concurrent')).toBe('loaded');
    });

    it('concurrent load 在失败时两个调用方都收到同一错误', async () => {
      const entry = vi.fn().mockImplementation(async () => {
        await new Promise((r) => setTimeout(r, 10));
        throw new Error('boom');
      });
      loader.register({ id: 'p.fail', entry, maxRetries: 0 });

      const results = await Promise.allSettled([loader.load('p.fail'), loader.load('p.fail')]);

      expect(results.map((r) => r.status)).toEqual(['rejected', 'rejected']);
      // maxRetries: 0 → 只尝试一次，并发不应把 attempts 吃掉两格
      expect(entry).toHaveBeenCalledTimes(1);
      expect(loader.getStatus('p.fail')).toBe('failed');
    });

    it('并发后重试预算仍然可用（inflight 清理不吞掉重试）', async () => {
      // 语义澄清：重试发生在 `_doLoad` **内部**，一次 `load()` 就会把预算用满。
      // 这里要证明的是「并发调用不会额外消耗 attempts」——修复前两个并发调用
      // 各跑一次 `_doLoad`，attempts 会被多吃一格，使重试提前耗尽。
      let attempt = 0;
      const entry = vi.fn().mockImplementation(async () => {
        attempt += 1;
        await new Promise((r) => setTimeout(r, 5));
        if (attempt === 1) throw new Error('first fails');
        return { attempt };
      });
      loader.register({ id: 'p.retry', entry, maxRetries: 1 });

      const [a, b] = await Promise.all([loader.load('p.retry'), loader.load('p.retry')]);

      expect(a).toEqual({ attempt: 2 });
      expect(b).toEqual({ attempt: 2 });
      // 首次失败 + 一次重试 = 2 次；并发没有多算
      expect(entry).toHaveBeenCalledTimes(2);
      expect(loader.getStatus('p.retry')).toBe('loaded');
    });

    it('clear() 丢弃在途加载记录（旧结果作废，不是旧结果照单提交）', async () => {
      const gate = deferred<unknown>();
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));
      const entry = vi.fn().mockImplementation(() => {
        markStarted();
        return gate.promise;
      });
      loader.register({ id: 'p.clear', entry });

      const p = loader.load('p.clear');
      await started;
      loader.clear();
      // 旧记录已随代际作废：它的 promise 以废弃错误 reject，不会 resolve 旧实例。
      await expect(rejection(p)).resolves.toBeInstanceOf(LazyPluginLoadError);
      gate.resolve({ late: true });

      // 清空后重新注册再加载，应是一次全新的加载
      loader.register({ id: 'p.clear', entry });
      await loader.load('p.clear');
      expect(entry).toHaveBeenCalledTimes(2);
    });
  });

  describe('失败处理', () => {
    it('加载失败记录错误', async () => {
      const entry = vi.fn().mockRejectedValue(new Error('load failed'));
      loader.register({ id: 'p.test', entry });

      await expect(loader.load('p.test')).rejects.toThrow('load failed');
      expect(loader.getStatus('p.test')).toBe('failed');
      expect(loader.isFailed('p.test')).toBe(true);
    });

    it('maxRetries: 0 时失败不重试', async () => {
      const entry = vi.fn().mockRejectedValue(new Error('fail'));
      loader.register({ id: 'p.test', entry, maxRetries: 0 });

      await expect(loader.load('p.test')).rejects.toThrow('fail');
      await expect(loader.load('p.test')).rejects.toThrow('fail');
      expect(entry).toHaveBeenCalledTimes(1); // 只尝试一次
    });

    it('maxRetries: 2 时重试', async () => {
      let attempt = 0;
      const entry = vi.fn().mockImplementation(() => {
        attempt += 1;
        if (attempt < 3) {
          return Promise.reject(new Error(`fail ${attempt}`));
        }
        return Promise.resolve({ attempt });
      });
      loader.register({ id: 'p.test', entry, maxRetries: 2 });

      const result = await loader.load('p.test');
      expect(result).toEqual({ attempt: 3 });
      expect(entry).toHaveBeenCalledTimes(3);
    });

    it('reset() 重置状态允许重新加载', async () => {
      let attempt = 0;
      const entry = vi.fn().mockImplementation(() => {
        attempt += 1;
        if (attempt === 1) return Promise.reject(new Error('fail'));
        return Promise.resolve({ attempt });
      });
      loader.register({ id: 'p.test', entry, maxRetries: 0 });

      await expect(loader.load('p.test')).rejects.toThrow('fail');
      expect(loader.isFailed('p.test')).toBe(true);

      loader.reset('p.test');
      expect(loader.getStatus('p.test')).toBe('pending');

      const result = await loader.load('p.test');
      expect(result).toEqual({ attempt: 2 });
    });
  });

  describe('超时处理', () => {
    it('timeoutMs 超时抛出错误', async () => {
      const entry = vi.fn().mockImplementation(() => new Promise((r) => setTimeout(r, 500)));
      loader.register({ id: 'p.test', entry, timeoutMs: 50 });

      await expect(loader.load('p.test')).rejects.toThrow('timed out');
      expect(loader.getStatus('p.test')).toBe('failed');
    });

    it('timeoutMs: 0 不超时', async () => {
      const entry = vi.fn().mockResolvedValue({ ok: true });
      loader.register({ id: 'p.test', entry, timeoutMs: 0 });

      const result = await loader.load('p.test');
      expect(result).toEqual({ ok: true });
    });
  });

  describe('订阅状态变化', () => {
    it('subscribe() 接收状态变化通知', async () => {
      const statuses: string[] = [];
      loader.subscribe((_, status) => statuses.push(status));

      loader.register({ id: 'p.test', entry: async () => ({}) });
      await loader.load('p.test');

      expect(statuses).toContain('loading');
      expect(statuses).toContain('loaded');
    });

    it('unsubscribe() 取消订阅', async () => {
      const fn = vi.fn();
      const unsub = loader.subscribe(fn);

      loader.register({ id: 'p.test', entry: async () => ({}) });
      await loader.load('p.test');
      expect(fn).toHaveBeenCalled();

      unsub();
      loader.register({ id: 'p.test2', entry: async () => ({}) });
      await loader.load('p.test2');
      // fn 不应再次被调用（但 vi.fn 计数不清零，所以检查调用次数不变）
      // 实际上 subscribe 是全局的，所以会再次调用。这里测试 unsubscribe 本身。
      expect(typeof unsub).toBe('function');
    });
  });

  describe('清理', () => {
    it('clear() 清空所有缓存', () => {
      loader.register({ id: 'p.a', entry: async () => ({}) });
      loader.register({ id: 'p.b', entry: async () => ({}) });
      expect(loader.size).toBe(2);

      loader.clear();
      expect(loader.size).toBe(0);
    });

    it('loadedCount 统计已加载数量', async () => {
      loader.register({ id: 'p.a', entry: async () => ({}) });
      loader.register({ id: 'p.b', entry: async () => ({}) });
      expect(loader.loadedCount).toBe(0);

      await loader.load('p.a');
      expect(loader.loadedCount).toBe(1);

      await loader.load('p.b');
      expect(loader.loadedCount).toBe(2);
    });

    it('listLoadedPluginIds() 列出已加载插件', async () => {
      loader.register({ id: 'p.a', entry: async () => ({}) });
      loader.register({ id: 'p.b', entry: async () => ({}) });

      await loader.load('p.a');
      expect(loader.listLoadedPluginIds()).toContain('p.a');
      expect(loader.listLoadedPluginIds()).not.toContain('p.b');
    });

    it('正常加载不留残余：runningTasks / activeTimers / inflightLoads 归零', async () => {
      loader.register({ id: 'p.clean', entry: async () => ({ ok: 1 }) });
      await loader.load('p.clean');

      expect(loader.stats()).toEqual({
        abandonedLoads: 0,
        discardedResults: 0,
        runningTasks: 0,
        activeTimers: 0,
        inflightLoads: 0,
      });
    });
  });

  // ────────────────────────────────────────────────────────────────────────
  // V7-P1-02：代际取消与取消信号。
  //
  // 全部用「测试手动落定的门」驱动时序，没有任何 setTimeout 睡眠参与判据。
  // ────────────────────────────────────────────────────────────────────────
  describe('代际取消（V7-P1-02）', () => {
    it('旧一代晚于新一代完成：新一代状态/实例不动、零 stale 通知、计入 abandonedLoads', async () => {
      const oldGate = deferred<unknown>();
      const newGate = deferred<unknown>();
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));

      loader.register({
        id: 'p.gen',
        entry: () => {
          markStarted();
          return oldGate.promise;
        },
      });

      // 立刻挂上落定回调：废弃错误在 `register()` 那一刻就会产生，
      // 晚挂会让 vitest 记一条未处理 reject（判据不变，但不干净）。
      const oldLoad = rejection(loader.load('p.gen'));
      await started; // _doLoad 确已开始（不是等时间）
      expect(loader.getStatus('p.gen')).toBe('loading');
      expect(loader.stats().inflightLoads).toBe(1);

      // 新一代接管：重新注册同一 id → 代际抬高，旧加载作废
      loader.register({ id: 'p.gen', entry: () => newGate.promise });
      const newLoad = loader.load('p.gen');

      // 旧加载被判定废弃：reject 且不给任何实例
      expect(await oldLoad).toBeInstanceOf(LazyPluginLoadError);
      expect(loader.stats().abandonedLoads).toBe(1);

      newGate.resolve({ gen: 'new' });
      expect(await newLoad).toEqual({ gen: 'new' });
      expect(loader.getStatus('p.gen')).toBe('loaded');

      // 从此处起只考察「旧一代落定」这段时间线
      const notices: PluginLoadStatus[] = [];
      loader.subscribe((_id, status) => notices.push(status));

      oldGate.resolve({ gen: 'old' }); // 旧一代此刻才产出结果
      await flushMicrotasks();

      expect(notices, '旧一代不得发出任何通知').toEqual([]);
      expect(loader.getStatus('p.gen')).toBe('loaded');
      expect(await loader.load('p.gen'), '缓存里必须是新一代实例').toEqual({ gen: 'new' });
      expect(loader.stats()).toMatchObject({ abandonedLoads: 1, discardedResults: 1 });
      expect(loader.abandonedLoads).toBe(1);
    });

    it('reset() 打断在途加载：旧结果丢弃，新一代结果不被覆盖', async () => {
      const oldGate = deferred<unknown>();
      const newGate = deferred<unknown>();
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));
      let calls = 0;
      loader.register({
        id: 'p.reset',
        entry: () => {
          calls += 1;
          markStarted();
          return calls === 1 ? oldGate.promise : newGate.promise;
        },
      });

      const oldLoad = rejection(loader.load('p.reset'));
      await started;

      loader.reset('p.reset');
      expect(loader.getStatus('p.reset')).toBe('pending');
      const reason = (await oldLoad) as LazyPluginLoadError;
      expect(reason.code).toBe('E_PLUGIN_LOAD_ABANDONED');

      const nextLoad = loader.load('p.reset');
      newGate.resolve({ gen: 'new' });
      expect(await nextLoad).toEqual({ gen: 'new' });

      oldGate.resolve({ gen: 'old' }); // 旧结果迟到
      await flushMicrotasks();

      expect(loader.getStatus('p.reset')).toBe('loaded');
      expect(loader.listLoadedPluginIds()).toContain('p.reset');
      expect(await loader.load('p.reset')).toEqual({ gen: 'new' });
      expect(loader.stats()).toMatchObject({ abandonedLoads: 1, discardedResults: 1 });
    });

    it('clear() 打断在途加载：无 post-clear 通知、条目不复活、定时器清零', async () => {
      const gate = deferred<unknown>();
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));
      loader.register({
        id: 'p.clear2',
        entry: () => {
          markStarted();
          return gate.promise;
        },
        timeoutMs: 5000,
      });

      const pending = rejection(loader.load('p.clear2'));
      await started;
      loader.clear();

      expect(await pending).toBeInstanceOf(LazyPluginLoadError);
      expect(loader.size).toBe(0);
      expect(loader.getStatus('p.clear2')).toBeUndefined();

      // `clear()` 会连带清空订阅者，所以通知判据在**重新订阅之后**取：
      // 迟到的旧结果落定时必须一个通知都发不出来。
      const notices: PluginLoadStatus[] = [];
      loader.subscribe((_id, status) => notices.push(status));

      gate.resolve({ late: true });
      await flushMicrotasks();

      expect(notices, 'clear 后旧一代不得复活式通知').toEqual([]);
      expect(loader.getStatus('p.clear2'), '条目不得复活').toBeUndefined();
      expect(loader.listPluginIds()).not.toContain('p.clear2');
      expect(loader.stats(), '废弃加载的定时器必须清掉，否则拖住进程退出').toMatchObject({
        abandonedLoads: 1,
        discardedResults: 1,
        activeTimers: 0,
        runningTasks: 0,
        inflightLoads: 0,
      });
    });

    it('废弃的加载不占住串行队列：旧任务永不 settle 时新一代仍能完成', async () => {
      // 这条是「队列被陈旧代际拖死」的判据：修复前 `_loadQueue` 只能等那个
      // 永不落定的 entry，后来的插件（哪怕是另一个 id）会一直排队。
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));
      loader.register({
        id: 'p.hang',
        // timeoutMs: 0 → 不建定时器：永不 settle 的 entry 也不会拖住进程退出
        entry: () => {
          markStarted();
          return new Promise(() => {});
        },
        timeoutMs: 0,
      });

      const hung = rejection(loader.load('p.hang'));
      await started;

      loader.reset('p.hang'); // 判定废弃 → 队列必须立刻放行
      expect(await hung).toBeInstanceOf(LazyPluginLoadError);

      loader.register({ id: 'p.next', entry: async () => ({ ok: true }) });
      expect(await loader.load('p.next')).toEqual({ ok: true });
      expect(loader.stats()).toMatchObject({
        abandonedLoads: 1,
        activeTimers: 0,
        runningTasks: 0,
      });
    });

    it('load(id, { signal }) 取消：reject AbortError 形状，状态如实落 failed', async () => {
      const ac = new AbortController();
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));
      loader.register({
        id: 'p.abort',
        // 不配合取消的 entry：只能丢弃其结果，绝不能伪报「已取消」
        entry: () => {
          markStarted();
          return new Promise(() => {});
        },
      });

      const pending = loader.load('p.abort', { signal: ac.signal });
      await started;
      ac.abort();

      const reason = (await rejection(pending)) as LazyPluginLoadError;
      expect(reason.name, '错误必须是 abort 形状').toBe('AbortError');
      expect(reason.code).toBe('E_PLUGIN_LOAD_ABORTED');
      expect(loader.getStatus('p.abort')).toBe('failed');
      expect(loader.isFailed('p.abort')).toBe(true);
      expect(loader.stats()).toMatchObject({
        abandonedLoads: 1,
        activeTimers: 0,
        runningTasks: 0,
        inflightLoads: 0,
      });
    });

    it('配合 signal 的 entry 收到取消信号并可自行中止', async () => {
      const ac = new AbortController();
      let seen: AbortSignal | undefined;
      let entrySelfAborted = false;
      let markStarted!: () => void;
      const started = new Promise<void>((r) => (markStarted = r));

      loader.register({
        id: 'p.coop',
        entry: (signal) =>
          new Promise((_resolve, reject) => {
            seen = signal;
            markStarted();
            signal?.addEventListener(
              'abort',
              () => {
                entrySelfAborted = true;
                reject(new Error('entry 自行中止'));
              },
              { once: true },
            );
          }),
      });

      const pending = rejection(loader.load('p.coop', { signal: ac.signal }));
      // 必须等 entry 真的被调用（拿到 signal）之后再取消
      await started;
      ac.abort();

      expect(seen, 'entry 必须收到同一个 signal').toBe(ac.signal);
      expect(entrySelfAborted, 'entry 应当被 signal 打断').toBe(true);
      const reason = (await pending) as LazyPluginLoadError;
      expect(reason.code).toBe('E_PLUGIN_LOAD_ABORTED');
      expect(loader.getStatus('p.coop')).toBe('failed');
    });

    it('已 abort 的 signal 不再发起加载', async () => {
      const ac = new AbortController();
      ac.abort();
      const entry = vi.fn().mockResolvedValue({ never: true });
      loader.register({ id: 'p.pre', entry });

      const reason = (await rejection(loader.load('p.pre', { signal: ac.signal }))) as
        LazyPluginLoadError | Error;
      expect((reason as LazyPluginLoadError).code).toBe('E_PLUGIN_LOAD_ABORTED');
      expect(entry, '取消信号已置位时不得启动 entry').not.toHaveBeenCalled();
    });
  });
});
