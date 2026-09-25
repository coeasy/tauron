// ──────────────────────────────────────────────────────────────────────────
// `registerPlugin` 的生命周期钩子接线。
//
// 这一组测试的意义：`RegisterPluginConfig.onDisable` 此前**声明了却没有任何触发
// 路径**——`registerPlugin` 只 wire 了 `onEnable`，宿主也没有下发禁用通知的手段。
// 插件作者写的清理逻辑（停定时器、断开连接、释放资源）是死代码。这里断言的不是
// "函数被定义了"，而是**宿主发一条通知，插件的 onDisable 真的被调用**。
// ──────────────────────────────────────────────────────────────────────────
import { afterEach, describe, expect, it, vi } from 'vitest';

import { buildEventMessage, buildInitMessage, BRIDGE_MESSAGE_TYPE } from '@tauron/types';
import { registerPlugin, TAURON_DISABLE_EVENT } from './register-plugin.js';

type Listener = (event: MessageEvent) => void;

/** 装上假的 `window`，返回插件侧注册的 message 监听器与"发给宿主"的桩。 */
function setupPluginWindow(): { listeners: Listener[]; toHost: ReturnType<typeof vi.fn> } {
  const listeners: Listener[] = [];
  const toHost = vi.fn();
  vi.stubGlobal('window', {
    addEventListener: (_type: string, cb: Listener) => listeners.push(cb),
    removeEventListener: vi.fn(),
    location: { href: 'http://localhost:5173/plugin.html#tauron-token=tok-1' },
    parent: { postMessage: toHost },
  });
  return { listeners, toHost };
}

/** 把一条宿主消息投递给插件（模拟 postMessage 到达）。 */
function deliver(listeners: Listener[], data: unknown): void {
  expect(listeners.length, '插件必须注册了 message 监听器').toBeGreaterThan(0);
  for (const l of listeners) l({ data } as MessageEvent);
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('registerPlugin：生命周期钩子', () => {
  it('握手 init 之后 onEnable 被调用', () => {
    const { listeners } = setupPluginWindow();
    const onEnable = vi.fn();
    registerPlugin({ name: 'com.example.a', version: '1.0.0', onEnable });

    deliver(listeners, buildInitMessage('tok-1', ['clipboard:read']));
    expect(onEnable).toHaveBeenCalledTimes(1);
  });

  it('宿主发 TAURON_DISABLE_EVENT 时 onDisable 被调用（此前永不触发）', () => {
    const { listeners } = setupPluginWindow();
    const onEnable = vi.fn();
    const onDisable = vi.fn();
    registerPlugin({ name: 'com.example.b', version: '1.0.0', onEnable, onDisable });

    deliver(listeners, buildInitMessage('tok-1', []));
    expect(onDisable).not.toHaveBeenCalled();

    deliver(
      listeners,
      buildEventMessage('tok-1', TAURON_DISABLE_EVENT, { reason: 'uninstalled' }),
    );
    expect(onDisable).toHaveBeenCalledTimes(1);
  });

  it('未声明 onDisable 时收到禁用通知不得抛错（保留事件无人订阅）', () => {
    const { listeners } = setupPluginWindow();
    registerPlugin({ name: 'com.example.c', version: '1.0.0' });

    expect(() =>
      deliver(listeners, buildEventMessage('tok-1', TAURON_DISABLE_EVENT, {})),
    ).not.toThrow();
  });

  it('保留事件名是稳定字面量（宿主与插件两侧靠它对齐）', () => {
    expect(TAURON_DISABLE_EVENT).toBe('tauron:disable');
    // 与 bridge 消息类型同源，避免有人手写错方向。
    expect(BRIDGE_MESSAGE_TYPE).toBeTypeOf('string');
  });

  it('methods 表：__invoke 触发处理器，结果经 __result:<callId> 回发', async () => {
    const { listeners, toHost } = setupPluginWindow();
    registerPlugin({
      name: 'com.example.d',
      version: '1.0.0',
      methods: { ping: ({ args }) => ({ pong: true, echo: args }) },
    });
    deliver(listeners, buildInitMessage('tok-1', []));
    toHost.mockClear();

    deliver(
      listeners,
      buildEventMessage('tok-1', '__invoke:ping', { callId: 'c-1', args: { n: 1 } }),
    );
    await Promise.resolve();
    await Promise.resolve();

    const resultMsg = toHost.mock.calls
      .map((c) => c[0] as { action: string; payload: { eventName: string } })
      .find((m) => m.payload?.eventName === '__result:c-1');
    expect(resultMsg, '必须回发 __result:c-1').toBeDefined();
  });
});
