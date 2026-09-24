// @vitest-environment node
// host-invoke.ts 测试：宿主侧 __invoke:/__result: 协议客户端

import { describe, it, expect, vi, afterEach } from 'vitest';
import { callPluginMethod } from './host-invoke.js';
import type { PluginBridge } from './bridge.js';
import { registerPlugin } from './register-plugin.js';
import { buildEventMessage, type PluginToBridgeMessage } from '@tauron/types';

/** 最小 Fake Bridge：只实现 host-invoke 用到的两个 API。 */
function createFakeBridge() {
  const handlers = new Map<string, Set<(data: unknown) => void>>();
  const sent: Array<{ eventName: string; payload: unknown }> = [];
  const bridge = {
    onEvent(eventName: string, handler: (data: unknown) => void): () => void {
      let set = handlers.get(eventName);
      if (!set) {
        set = new Set();
        handlers.set(eventName, set);
      }
      set.add(handler);
      return () => {
        set.delete(handler);
      };
    },
    emitToPlugin(eventName: string, payload: unknown): void {
      sent.push({ eventName, payload });
    },
  };
  const deliver = (eventName: string, data: unknown): void => {
    for (const h of handlers.get(eventName) ?? []) h(data);
  };
  const handlerCount = (eventName: string): number => (handlers.get(eventName)?.size ?? 0);
  return { bridge: bridge as unknown as PluginBridge, sent, deliver, handlerCount };
}

describe('callPluginMethod', () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('发送 __invoke:<method> 事件，payload 含 callId 与 args', () => {
    const { bridge, sent } = createFakeBridge();
    // 不等待响应：显式吞掉超时 reject，避免 unhandled rejection 干扰测试
    void callPluginMethod(bridge, 'format', { code: 'a  b' }, { timeoutMs: 1000 }).catch(() => undefined);

    expect(sent).toHaveLength(1);
    expect(sent[0]!.eventName).toBe('__invoke:format');
    const payload = sent[0]!.payload as { callId: string; args: unknown };
    expect(typeof payload.callId).toBe('string');
    expect(payload.callId.length).toBeGreaterThan(0);
    expect(payload.args).toEqual({ code: 'a  b' });
  });

  it('插件返回 ok:true 时 resolve 结果', async () => {
    const { bridge, sent, deliver } = createFakeBridge();
    const p = callPluginMethod<{ formatted: string }>(bridge, 'format', { code: 'a  b' });
    const callId = (sent[0]!.payload as { callId: string }).callId;

    deliver(`__result:${callId}`, { ok: true, result: { formatted: 'a b' } });
    await expect(p).resolves.toEqual({ formatted: 'a b' });
  });

  it('插件返回 ok:false 时 reject 含错误码', async () => {
    const { bridge, sent, deliver } = createFakeBridge();
    const p = callPluginMethod(bridge, 'fail', null);
    const callId = (sent[0]!.payload as { callId: string }).callId;

    deliver(`__result:${callId}`, { ok: false, error: { code: 'E_METHOD_ERROR', message: 'boom' } });
    await expect(p).rejects.toThrow('E_METHOD_ERROR: boom');
  });

  it('无效结果信封被拒绝', async () => {
    const { bridge, sent, deliver } = createFakeBridge();
    const p = callPluginMethod(bridge, 'x', null);
    const callId = (sent[0]!.payload as { callId: string }).callId;

    deliver(`__result:${callId}`, { notAnEnvelope: true });
    await expect(p).rejects.toThrow('Invalid plugin result envelope');
  });

  it('超时后 reject 并清理订阅（无泄漏）', async () => {
    vi.useFakeTimers();
    const { bridge, sent, handlerCount } = createFakeBridge();
    const p = callPluginMethod(bridge, 'never-answers', null, { timeoutMs: 500 });
    const callId = (sent[0]!.payload as { callId: string }).callId;

    expect(handlerCount(`__result:${callId}`)).toBe(1);
    // 先挂断言再推进时钟：避免 fake timer 下 rejection 早于 handler 挂载
    const assertion = expect(p).rejects.toThrow(/timed out after 500ms/);
    await vi.advanceTimersByTimeAsync(500);
    await assertion;
    // 订阅必须已清理
    expect(handlerCount(`__result:${callId}`)).toBe(0);
  });

  it('结果迟到于超时时不产生二次结算', async () => {
    vi.useFakeTimers();
    const { bridge, sent, deliver } = createFakeBridge();
    const p = callPluginMethod(bridge, 'slow', null, { timeoutMs: 100 });
    const callId = (sent[0]!.payload as { callId: string }).callId;

    const assertion = expect(p).rejects.toThrow(/timed out/);
    await vi.advanceTimersByTimeAsync(100);
    await assertion;
    // 迟到的结果不应再触发任何回调（订阅已移除，deliver 空集）
    expect(() => deliver(`__result:${callId}`, { ok: true, result: 1 })).not.toThrow();
  });

  it('timeoutMs: 0 表示不超时', async () => {
    vi.useFakeTimers();
    const { bridge, sent, deliver } = createFakeBridge();
    const p = callPluginMethod(bridge, 'long', null, { timeoutMs: 0 });
    const callId = (sent[0]!.payload as { callId: string }).callId;

    await vi.advanceTimersByTimeAsync(10 * 60 * 1000);
    deliver(`__result:${callId}`, { ok: true, result: 'done' });
    await expect(p).resolves.toBe('done');
  });

  it('协议互通：真实 registerPlugin（插件侧）↔ callPluginMethod（宿主侧）全回路', async () => {
    // ── 插件世界：stub window，用真实 registerPlugin ──
    const pluginMessageHandlers: Array<(event: { data: unknown }) => void> = [];
    const { bridge, sent, deliver } = createFakeBridge();

    vi.stubGlobal('crypto', { randomUUID: () => 'plugin-token-1' });
    vi.stubGlobal('window', {
      addEventListener: (_ev: string, handler: (e: { data: unknown }) => void) => {
        pluginMessageHandlers.push(handler);
      },
      removeEventListener: vi.fn(),
      parent: {
        postMessage: (msg: PluginToBridgeMessage) => {
          // 路由插件 → 宿主：emit 消息进入宿主 onEvent 管道
          if (msg.action === 'emit') {
            const p = msg.payload as { eventName: string; payload: unknown };
            deliver(p.eventName, p.payload);
          }
        },
      },
    });

    const reg = registerPlugin({
      name: 'interop',
      version: '1.0.0',
      methods: {
        async greet(opts) {
          const { who } = opts.args as { who: string };
          return `hello ${who}`;
        },
      },
    });

    // ── 宿主世界：把 emitToPlugin 路由为真实 host-to-plugin event 消息 ──
    // 用真实分发替换 emitToPlugin：构造 host-to-plugin event 消息发给插件
    (bridge as unknown as { emitToPlugin: (n: string, p: unknown) => void }).emitToPlugin = (
      eventName: string,
      payload: unknown,
    ) => {
      sent.push({ eventName, payload });
      const msg = buildEventMessage('plugin-token-1', eventName, payload);
      for (const h of pluginMessageHandlers) h({ data: msg });
    };

    const result = await callPluginMethod<string>(bridge, 'greet', { who: 'world' }, { timeoutMs: 2000 });
    expect(result).toBe('hello world');
    expect(sent[0]!.eventName).toBe('__invoke:greet');

    reg.destroy();
  });
});