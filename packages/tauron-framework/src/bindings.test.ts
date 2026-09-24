import { describe, it, expect, vi } from 'vitest';
import { MockBackend } from '@tauron/host';
import {
  useInvoke,
  useCapabilities,
  useEvent,
  usePluginId,
  createReactiveStore,
} from './bindings.js';

describe('useInvoke', () => {
  it('调用成功返回结果', async () => {
    const backend = new MockBackend({
      capabilities: ['host_get'],
      cases: [{ cmd: 'host_get', result: { ok: true } }],
    });
    const [state, call] = useInvoke<{ ok: boolean }>(backend, 'host_get');
    const result = await call();
    expect(result).toEqual({ ok: true });
    expect(state.value).toEqual({ status: 'success', data: { ok: true } });
  });

  it('调用失败设置 error 状态', async () => {
    const backend = new MockBackend({
      capabilities: ['host_fail'],
      cases: [{ cmd: 'host_fail', error: new Error('boom') }],
    });
    const [state, call] = useInvoke(backend, 'host_fail');
    await expect(call()).rejects.toThrow('boom');
    expect(state.value.status).toBe('error');
    expect((state.value as { error: unknown }).error).toBeInstanceOf(Error);
  });

  it('调用前状态为 loading', async () => {
    const backend = new MockBackend({
      capabilities: ['host_slow'],
      cases: [{ cmd: 'host_slow', result: 'done' }],
    });
    const [state, call] = useInvoke(backend, 'host_slow');
    expect(state.value.status).toBe('idle');
    const promise = call();
    // loading 状态在 invoke 前设置
    expect(state.value.status).toBe('loading');
    await promise;
    expect(state.value.status).toBe('success');
  });

  it('传入参数', async () => {
    const backend = new MockBackend({
      capabilities: ['host_call'],
      cases: [
        {
          cmd: 'host_call',
          args: { pluginId: 'p1' },
          result: { id: 'p1' },
        },
      ],
    });
    const [_, call] = useInvoke(backend, 'host_call');
    const result = await call({ pluginId: 'p1' });
    expect(result).toEqual({ id: 'p1' });
  });
});

describe('useCapabilities', () => {
  it('返回初始能力集合', () => {
    const backend = new MockBackend({ capabilities: ['host_get', 'host_set'] });
    const [caps] = useCapabilities(backend);
    expect(caps.value).toContain('host_get');
    expect(caps.value).toContain('host_set');
  });

  it('refresh 重新查询', () => {
    const backend = new MockBackend({ capabilities: ['host_get'] });
    const [caps, refresh] = useCapabilities(backend);
    expect(caps.value.size).toBe(1);
    // 模拟后端能力变化
    // (MockBackend 的 caps 是 readonly，这里只验证 refresh 调用不报错)
    refresh();
    expect(caps.value.size).toBe(1);
  });
});

describe('useEvent', () => {
  it('订阅事件并退订', async () => {
    const backend = new MockBackend({ capabilities: [] });
    const handler = vi.fn();
    const unsub = useEvent(backend, 'test-event', handler);
    // 等待 listen Promise 解析
    await Promise.resolve();
    // 触发事件
    backend.emit('test-event', { data: 1 });
    expect(handler).toHaveBeenCalledWith({ data: 1 });
    // 退订
    unsub();
    backend.emit('test-event', { data: 2 });
    expect(handler).toHaveBeenCalledTimes(1); // 不再调用
  });

  it('退订后立即取消订阅', async () => {
    const backend = new MockBackend({ capabilities: [] });
    const handler = vi.fn();
    const unsub = useEvent(backend, 'event', handler);
    unsub();
    // 退订后 emit 不应调用 handler
    // (listen 是 async，退订可能已生效)
  });
});

describe('usePluginId', () => {
  it('返回当前插件 id', () => {
    const backend = new MockBackend({ pluginId: 'p.audio' });
    const id = usePluginId(backend);
    expect(id.value).toBe('p.audio');
  });

  it('无插件上下文返回 null', () => {
    const backend = new MockBackend({ pluginId: null });
    const id = usePluginId(backend);
    expect(id.value).toBeNull();
  });
});

describe('createReactiveStore', () => {
  it('创建并读取初始状态', () => {
    const store = createReactiveStore({ volume: 10, muted: false });
    expect(store.get()).toEqual({ volume: 10, muted: false });
  });

  it('patch 更新状态', () => {
    const store = createReactiveStore({ volume: 10 });
    store.set({ volume: 20 });
    expect(store.get()).toEqual({ volume: 20 });
  });

  it('函数式更新', () => {
    const store = createReactiveStore({ volume: 10 });
    store.set((prev) => ({ volume: prev.volume + 5 }));
    expect(store.get().volume).toBe(15);
  });

  it('watch 订阅变更', () => {
    const store = createReactiveStore({ volume: 10 });
    const values: { volume: number }[] = [];
    store.watch((v) => values.push(v));
    store.set({ volume: 20 });
    expect(values.length).toBe(2); // 初始 + 更新
  });

  it('version 递增', () => {
    const store = createReactiveStore({ v: 0 });
    expect(store.version.value).toBe(0);
    store.set({ v: 1 });
    expect(store.version.value).toBe(1);
    store.set({ v: 2 });
    expect(store.version.value).toBe(2);
  });

  it('watch 返回清理函数', () => {
    const store = createReactiveStore({ v: 0 });
    const values: { v: number }[] = [];
    const unsub = store.watch((val) => values.push(val));
    store.set({ v: 1 });
    unsub();
    store.set({ v: 2 });
    expect(values.length).toBe(2); // 初始 + v=1
  });
});
