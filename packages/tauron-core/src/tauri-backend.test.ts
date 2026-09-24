/**
 * createTauriBackend 契约测试（设计文档 §1.3/§2.1/§9）
 *
 * 通过 `loader` 注入假 IPC 层，因此在无 Tauri 运行时也能验证：
 * - 命令名与信封编解码
 * - 结构化错误（IPC 失败 / 超时）而非裸抛
 * - 动态加载失败后可重试（缓存不被污染）
 */

import { describe, expect, it, vi } from 'vitest';

import { buildRequest, PluginErrorCode, type PluginInvokeResponse } from '@tauron/types';
import { createTauriBackend, TAURON_COMMANDS, type TauriApi } from './tauri-backend.js';

/** 记录调用的假 IPC 层。 */
function fakeApi(overrides: Partial<TauriApi> = {}): {
  api: TauriApi;
  calls: Array<{ cmd: string; args?: Record<string, unknown> }>;
} {
  const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
  const api: TauriApi = {
    invoke:
      overrides.invoke ??
      (async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
        calls.push(args === undefined ? { cmd } : { cmd, args });
        return { callId: 'x', ok: true, result: 'ok' } as unknown as T;
      }),
    listen: overrides.listen ?? (async () => () => undefined),
    ...(overrides.Channel ? { Channel: overrides.Channel } : {}),
  };
  return { api, calls };
}

describe('TAURON_COMMANDS', () => {
  it('命令名与 tauron-shell 命令层一致', () => {
    expect(TAURON_COMMANDS).toEqual({
      invoke: 'plugin_invoke',
      cancel: 'plugin_cancel',
      emit: 'plugin_emit',
    });
  });
});

describe('createTauriBackend.invoke', () => {
  it('把信封原样下发给 plugin_invoke', async () => {
    const { api, calls } = fakeApi();
    const backend = createTauriBackend({ loader: async () => api });

    const request = buildRequest({
      pluginId: 'com.example.formatter',
      method: 'format',
      payload: { code: 'x' },
    });
    await backend.invoke(request);

    expect(calls).toHaveLength(1);
    expect(calls[0]!.cmd).toBe('plugin_invoke');
    expect(calls[0]!.args).toEqual({ request });
  });

  it('成功时透传 Rust 返回的响应', async () => {
    const expected: PluginInvokeResponse = { callId: 'c1', ok: true, result: 42 };
    const { api } = fakeApi({
      invoke: async <T,>() => expected as unknown as T,
    });
    const backend = createTauriBackend({ loader: async () => api });

    const res = await backend.invoke(buildRequest({ pluginId: 'p', method: 'm' }));
    expect(res).toEqual(expected);
  });

  it('IPC 抛错时返回 CHANNEL_BROKEN 而不是抛出', async () => {
    const { api } = fakeApi({
      invoke: async () => {
        throw new Error('command not found');
      },
    });
    const backend = createTauriBackend({ loader: async () => api });

    const res = await backend.invoke(buildRequest({ pluginId: 'p', method: 'm' }));
    expect(res.ok).toBe(false);
    expect(res.error?.code).toBe(PluginErrorCode.CHANNEL_BROKEN);
    expect(res.error?.retryable).toBe(true);
  });

  it('加载 @tauri-apps/api 失败时返回 CHANNEL_BROKEN（Web 降级 §9）', async () => {
    const backend = createTauriBackend({
      loader: async () => {
        throw new Error('module not found');
      },
    });

    const res = await backend.invoke(buildRequest({ pluginId: 'p', method: 'm' }));
    expect(res.ok).toBe(false);
    expect(res.error?.code).toBe(PluginErrorCode.CHANNEL_BROKEN);
    expect(res.error?.message).toContain('module not found');
  });

  it('超时返回 TIMEOUT 且带 callId', async () => {
    vi.useFakeTimers();
    try {
      const { api } = fakeApi({
        invoke: () => new Promise<never>(() => undefined),
      });
      const backend = createTauriBackend({ loader: async () => api });

      const request = buildRequest({ pluginId: 'p', method: 'm', timeoutMs: 50 });
      const promise = backend.invoke(request);
      await vi.advanceTimersByTimeAsync(60);
      const res = await promise;

      expect(res.callId).toBe(request.callId);
      expect(res.error?.code).toBe(PluginErrorCode.TIMEOUT);
      expect(res.error?.retryable).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('timeoutMs 为 0 时不施加 JS 侧超时', async () => {
    vi.useFakeTimers();
    try {
      let settled = false;
      const { api } = fakeApi({
        invoke: <T,>() =>
          new Promise<T>((resolve) => {
            setTimeout(() => {
              settled = true;
              resolve({ callId: 'c', ok: true, result: 'late' } as T);
            }, 10_000);
          }),
      });
      const backend = createTauriBackend({ loader: async () => api });

      const promise = backend.invoke(
        buildRequest({ pluginId: 'p', method: 'm', timeoutMs: 0 }),
      );
      await vi.advanceTimersByTimeAsync(60_000);
      const res = await promise;

      expect(settled).toBe(true);
      expect(res.ok).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it('超时后 best-effort 通知宿主中止（plugin_cancel）', async () => {
    vi.useFakeTimers();
    try {
      const seen: string[] = [];
      const { api } = fakeApi({
        invoke: ((cmd: string) => {
          seen.push(cmd);
          return cmd === TAURON_COMMANDS.cancel
            ? Promise.resolve(undefined)
            : new Promise<never>(() => undefined);
        }) as TauriApi['invoke'],
      });
      const backend = createTauriBackend({ loader: async () => api });

      const request = buildRequest({ pluginId: 'p', method: 'm', timeoutMs: 50 });
      const promise = backend.invoke(request);
      await vi.advanceTimersByTimeAsync(60);
      const res = await promise;

      expect(res.error?.code).toBe(PluginErrorCode.TIMEOUT);
      expect(seen).toContain(TAURON_COMMANDS.invoke);
      expect(seen).toContain(TAURON_COMMANDS.cancel);
    } finally {
      vi.useRealTimers();
    }
  });

  it('取消失败不放大超时结果（best-effort：只吞不抛）', async () => {
    vi.useFakeTimers();
    try {
      const { api } = fakeApi({
        invoke: ((cmd: string) =>
          cmd === TAURON_COMMANDS.cancel
            ? Promise.reject(new Error('command plugin_cancel not found'))
            : new Promise<never>(() => undefined)) as TauriApi['invoke'],
      });
      const backend = createTauriBackend({ loader: async () => api });

      const request = buildRequest({ pluginId: 'p', method: 'm', timeoutMs: 50 });
      const promise = backend.invoke(request);
      await vi.advanceTimersByTimeAsync(60);
      const res = await promise;

      // 仍是超时错误（不是取消失败导致的 CHANNEL_BROKEN / 未处理拒绝）。
      expect(res.error?.code).toBe(PluginErrorCode.TIMEOUT);
    } finally {
      vi.useRealTimers();
    }
  });

  it('取消同步抛出时超时仍正常结算（不得让 promise 永久挂起）', async () => {
    vi.useFakeTimers();
    try {
      const { api } = fakeApi({
        invoke: ((cmd: string) => {
          if (cmd === TAURON_COMMANDS.cancel) {
            throw new Error('sync boom');
          }
          return new Promise<never>(() => undefined);
        }) as TauriApi['invoke'],
      });
      const backend = createTauriBackend({ loader: async () => api });

      const request = buildRequest({ pluginId: 'p', method: 'm', timeoutMs: 50 });
      const promise = backend.invoke(request);
      await vi.advanceTimersByTimeAsync(60);
      const res = await promise;

      expect(res.error?.code).toBe(PluginErrorCode.TIMEOUT);
    } finally {
      vi.useRealTimers();
    }
  });

  it('未显式指定时使用构造期默认超时', async () => {
    vi.useFakeTimers();
    try {
      const { api } = fakeApi({ invoke: () => new Promise<never>(() => undefined) });
      const backend = createTauriBackend({ loader: async () => api, timeoutMs: 30 });

      const promise = backend.invoke(buildRequest({ pluginId: 'p', method: 'm' }));
      await vi.advanceTimersByTimeAsync(40);
      expect((await promise).error?.code).toBe(PluginErrorCode.TIMEOUT);
    } finally {
      vi.useRealTimers();
    }
  });

  it('提供 onProgress 且运行时支持 Channel 时建立进度通道', async () => {
    const received: number[] = [];
    const channels: Array<{ onmessage: (m: { percentage: number }) => void }> = [];

    class FakeChannel {
      onmessage: (m: { percentage: number }) => void = () => undefined;
      constructor() {
        channels.push(this);
      }
    }

    const { api } = fakeApi({
      Channel: FakeChannel as unknown as NonNullable<TauriApi['Channel']>,
    });
    const backend = createTauriBackend({ loader: async () => api });

    await backend.invoke(
      buildRequest({ pluginId: 'p', method: 'm' }),
      undefined,
      (event) => received.push(event.percentage),
    );

    expect(channels).toHaveLength(1);
    channels[0]!.onmessage({ percentage: 50 });
    expect(received).toEqual([50]);
  });

  it('运行时无 Channel 时不传 channel（优雅降级）', async () => {
    const { api, calls } = fakeApi();
    const backend = createTauriBackend({ loader: async () => api });

    await backend.invoke(
      buildRequest({ pluginId: 'p', method: 'm' }),
      undefined,
      () => undefined,
    );

    expect(calls[0]!.args).not.toHaveProperty('channel');
  });
});

describe('createTauriBackend.cancel / emit / listen', () => {
  it('cancel 走 plugin_cancel 并携带 callId', async () => {
    const { api, calls } = fakeApi();
    const backend = createTauriBackend({ loader: async () => api });

    await backend.cancel('call-9');
    expect(calls[0]!.cmd).toBe('plugin_cancel');
    expect(calls[0]!.args).toEqual({ request: { callId: 'call-9' } });
  });

  it('emit 走 plugin_emit 并携带 topic/payload', async () => {
    const { api, calls } = fakeApi();
    const backend = createTauriBackend({ loader: async () => api });

    await backend.emit('plugin:p:changed', { n: 1 });
    expect(calls[0]!.cmd).toBe('plugin_emit');
    expect(calls[0]!.args).toEqual({ topic: 'plugin:p:changed', payload: { n: 1 } });
  });

  it('listen 解包 Tauri 事件 payload 并返回退订函数', async () => {
    const unlisten = vi.fn();
    let deliver: ((e: { payload: unknown }) => void) | undefined;

    const { api } = fakeApi({
      listen: async (_name, handler) => {
        deliver = handler;
        return unlisten;
      },
    });
    const backend = createTauriBackend({ loader: async () => api });

    const seen: unknown[] = [];
    const off = await backend.listen('plugin:p:changed', (p) => seen.push(p));

    deliver!({ payload: { v: 7 } });
    expect(seen).toEqual([{ v: 7 }]);

    off();
    expect(unlisten).toHaveBeenCalledOnce();
  });
});

describe('createTauriBackend 加载失败恢复', () => {
  it('一次加载失败不会永久损坏后端，后续调用可恢复', async () => {
    let attempt = 0;
    const backend = createTauriBackend({
      loader: async () => {
        attempt += 1;
        if (attempt === 1) throw new Error('first failure');
        return fakeApi().api;
      },
    });

    const first = await backend.invoke(buildRequest({ pluginId: 'p', method: 'm' }));
    expect(first.ok).toBe(false);

    const second = await backend.invoke(buildRequest({ pluginId: 'p', method: 'm' }));
    expect(second.ok).toBe(true);
  });
});
