import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  createPluginContext,
  readHandshakeTokenFromUrl,
  PLUGIN_TOKEN_FRAGMENT_KEY,
} from './plugin-context.js';
import { BRIDGE_MESSAGE_TYPE, type BridgeToPluginMessage } from '@tauron/types';

describe('PluginContext', () => {
  beforeEach(() => {
    vi.stubGlobal('crypto', { randomUUID: () => 'test-call-id-123' });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe('createPluginContext', () => {
    it('sends ready message on creation (handshakeToken 显式覆盖)', () => {
      const parentPostMessage = vi.fn();
      vi.stubGlobal('window', {
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        parent: { postMessage: parentPostMessage },
      });

      createPluginContext({ handshakeToken: 'test-call-id-123' });

      expect(parentPostMessage).toHaveBeenCalledTimes(1);
      const msg = parentPostMessage.mock.calls[0]?.[0];
      expect(msg.type).toBe(BRIDGE_MESSAGE_TYPE);
      expect(msg.direction).toBe('plugin-to-host');
      expect(msg.action).toBe('ready');
      expect((msg.payload as { token: string }).token).toBe('test-call-id-123');
    });

    it('默认从 URL hash 解析宿主注入的 token', () => {
      const parentPostMessage = vi.fn();
      vi.stubGlobal('window', {
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        location: { href: 'http://localhost:5173/plugin.html#tauron-token=injected-uuid' },
        parent: { postMessage: parentPostMessage },
      });

      createPluginContext();

      const msg = parentPostMessage.mock.calls[0]?.[0];
      expect((msg.payload as { token: string }).token).toBe('injected-uuid');
    });

    it('无 token 通道时发送空串（握手显式失败，不假成功）', () => {
      const parentPostMessage = vi.fn();
      vi.stubGlobal('window', {
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        location: { href: 'http://localhost:5173/plugin.html' },
        parent: { postMessage: parentPostMessage },
      });

      createPluginContext();

      const msg = parentPostMessage.mock.calls[0]?.[0];
      expect((msg.payload as { token: string }).token).toBe('');
    });
  });

  describe('readHandshakeTokenFromUrl', () => {
    it('解析标准片段', () => {
      expect(
        readHandshakeTokenFromUrl(`https://app/plugins/p1/index.html#${PLUGIN_TOKEN_FRAGMENT_KEY}=abc-123`),
      ).toBe('abc-123');
    });

    it('多参数片段中定位目标键并 URL 解码', () => {
      expect(
        readHandshakeTokenFromUrl('https://app/p.html#other=1&tauron-token=a%2Fb%3Dc'),
      ).toBe('a/b=c');
    });

    it('缺失时返回空串', () => {
      expect(readHandshakeTokenFromUrl('https://app/p.html#fragment')).toBe('');
      expect(readHandshakeTokenFromUrl('https://app/p.html')).toBe('');
    });

    it('非法 URL 不抛异常', () => {
      expect(readHandshakeTokenFromUrl('::not a url::')).toBe('');
    });
  });

  describe('createPluginContext（init / invoke / destroy 行为）', () => {
    it('handles init message and updates permissions', () => {
      const messageHandlers: Array<(event: { data: unknown }) => void> = [];
      vi.stubGlobal('window', {
        addEventListener: (_event: string, handler: (e: { data: unknown }) => void) => {
          messageHandlers.push(handler);
        },
        removeEventListener: vi.fn(),
        parent: { postMessage: vi.fn() },
      });

      const ctx = createPluginContext();
      expect(ctx.ready).toBe(false);

      // Simulate init message
      const initMsg: BridgeToPluginMessage = {
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'host-to-plugin',
        token: 'test-token',
        action: 'init',
        payload: { permissions: ['store:read', 'http:fetch'] },
      };

      messageHandlers.forEach((h) => h({ data: initMsg }));

      expect(ctx.ready).toBe(true);
      expect(ctx.permissions).toEqual(['store:read', 'http:fetch']);
    });

    it('handles init callback', () => {
      const messageHandlers: Array<(event: { data: unknown }) => void> = [];
      vi.stubGlobal('window', {
        addEventListener: (_event: string, handler: (e: { data: unknown }) => void) => {
          messageHandlers.push(handler);
        },
        removeEventListener: vi.fn(),
        parent: { postMessage: vi.fn() },
      });

      const ctx = createPluginContext();
      let receivedPerms: string[] | null = null;
      ctx.onInit((perms) => {
        receivedPerms = perms;
      });

      const initMsg: BridgeToPluginMessage = {
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'host-to-plugin',
        token: 'test-token',
        action: 'init',
        payload: { permissions: ['store:read'] },
      };

      messageHandlers.forEach((h) => h({ data: initMsg }));

      expect(receivedPerms).toEqual(['store:read']);
    });

    it('emits events', () => {
      const parentPostMessage = vi.fn();
      vi.stubGlobal('window', {
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        parent: { postMessage: parentPostMessage },
      });

      const ctx = createPluginContext();
      ctx.emit('format-done', { result: 'done' });

      expect(parentPostMessage).toHaveBeenCalledTimes(2); // ready + emit
      const emitMsg = parentPostMessage.mock.calls[1]?.[0];
      expect(emitMsg.action).toBe('emit');
      expect(emitMsg.payload).toEqual({
        eventName: 'format-done',
        payload: { result: 'done' },
      });
    });

    it('destroys cleanly', () => {
      const removeEventListener = vi.fn();
      vi.stubGlobal('window', {
        addEventListener: vi.fn(),
        removeEventListener,
        parent: { postMessage: vi.fn() },
      });

      const ctx = createPluginContext();
      ctx.destroy();

      expect(removeEventListener).toHaveBeenCalled();
    });

    it('把宿主推送的 event 消息分发给 onEvent 订阅者', () => {
      const messageHandlers: Array<(event: { data: unknown }) => void> = [];
      vi.stubGlobal('window', {
        addEventListener: (_event: string, handler: (e: { data: unknown }) => void) => {
          messageHandlers.push(handler);
        },
        removeEventListener: vi.fn(),
        parent: { postMessage: vi.fn() },
      });

      const ctx = createPluginContext();
      const received: unknown[] = [];
      const off = ctx.onEvent('config-changed', (p) => received.push(p));

      messageHandlers.forEach((h) =>
        h({
          data: {
            type: BRIDGE_MESSAGE_TYPE,
            direction: 'host-to-plugin',
            token: 't',
            action: 'event',
            payload: { eventName: 'config-changed', payload: { theme: 'dark' } },
          },
        }),
      );

      expect(received).toEqual([{ theme: 'dark' }]);

      // 退订后不再接收
      off();
      messageHandlers.forEach((h) =>
        h({
          data: {
            type: BRIDGE_MESSAGE_TYPE,
            direction: 'host-to-plugin',
            token: 't',
            action: 'event',
            payload: { eventName: 'config-changed', payload: { theme: 'light' } },
          },
        }),
      );
      expect(received).toHaveLength(1);
    });

    it('invoke 在宿主无响应时超时，而不是永久挂起', async () => {
      vi.useFakeTimers();
      try {
        vi.stubGlobal('window', {
          addEventListener: vi.fn(),
          removeEventListener: vi.fn(),
          parent: { postMessage: vi.fn() },
        });

        const ctx = createPluginContext({ timeoutMs: 100 });
        const promise = ctx.invoke('slow', {});
        const assertion = expect(promise).rejects.toThrow(/timed out/);

        await vi.advanceTimersByTimeAsync(150);
        await assertion;
      } finally {
        vi.useRealTimers();
      }
    });

    it('已响应的调用不会被超时定时器误报', async () => {
      vi.useFakeTimers();
      try {
        const messageHandlers: Array<(event: { data: unknown }) => void> = [];
        vi.stubGlobal('window', {
          addEventListener: (_event: string, handler: (e: { data: unknown }) => void) => {
            messageHandlers.push(handler);
          },
          removeEventListener: vi.fn(),
          parent: { postMessage: vi.fn() },
        });

        const ctx = createPluginContext({ timeoutMs: 100 });
        const promise = ctx.invoke('fast', {});

        messageHandlers.forEach((h) =>
          h({
            data: {
              type: BRIDGE_MESSAGE_TYPE,
              direction: 'host-to-plugin',
              token: 't',
              action: 'invoke-result',
              payload: {
                callId: 'test-call-id-123',
                result: { ok: true, result: 'done' },
              },
            },
          }),
        );

        await expect(promise).resolves.toBe('done');
        // 超时窗口过去后不应产生未处理拒绝
        await vi.advanceTimersByTimeAsync(200);
      } finally {
        vi.useRealTimers();
      }
    });
  });
});
