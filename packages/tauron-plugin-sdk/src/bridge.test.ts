import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { PluginBridge } from './bridge.js';
import { BRIDGE_MESSAGE_TYPE, type BridgeMessage, type PluginToBridgeMessage } from '@tauron/types';

describe('PluginBridge', () => {
  let bridge: PluginBridge;
  let mockIframe: {
    contentWindow: { postMessage: ReturnType<typeof vi.fn> } | null;
    sandbox: string;
    src: string;
    style: Record<string, string>;
    parentNode: { removeChild: ReturnType<typeof vi.fn> };
  };

  beforeEach(() => {
    vi.stubGlobal('crypto', { randomUUID: () => 'test-token-123' });
    vi.stubGlobal('document', {
      createElement: () => mockIframe,
      body: { appendChild: vi.fn(), removeChild: vi.fn() },
    });
    vi.stubGlobal('window', {
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    });
    mockIframe = {
      contentWindow: { postMessage: vi.fn() },
      sandbox: '',
      src: '',
      style: {},
      parentNode: { removeChild: vi.fn() },
    };
    bridge = new PluginBridge(
      ['store:read', 'http:fetch'],
      async (method, args) => ({ result: `${method}:${JSON.stringify(args)}` }),
      async () => {},
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** 模拟来自本桥 iframe 的消息（带正确的 event.source）。 */
  const deliver = (msg: PluginToBridgeMessage): void => {
    (bridge as unknown as { handleMessage: (e: { data: unknown; source: unknown }) => void }).handleMessage({
      data: msg,
      source: mockIframe.contentWindow,
    });
  };

  /** 模拟来自陌生窗口的消息（伪造信封）。 */
  const deliverFromForeignWindow = (msg: PluginToBridgeMessage): void => {
    (bridge as unknown as { handleMessage: (e: { data: unknown; source: unknown }) => void }).handleMessage({
      data: msg,
      source: { postMessage: vi.fn() },
    });
  };

  const handshake = (): void => {
    deliver({
      type: BRIDGE_MESSAGE_TYPE,
      direction: 'plugin-to-host',
      action: 'ready',
      payload: { token: 'test-token-123' },
    });
  };

  const sentMessages = (): BridgeMessage[] =>
    (mockIframe.contentWindow!.postMessage as ReturnType<typeof vi.fn>).mock.calls.map(
      (c) => c[0] as BridgeMessage,
    );

  describe('createIframe', () => {
    it('creates an iframe with sandbox attribute and injected handshake token', () => {
      const iframe = bridge.createIframe('tauri://localhost/plugins/test/index.html');
      expect(mockIframe.sandbox).toBe('allow-scripts');
      // token 经 URL hash 注入插件页（握手协议的关键传输通道）
      expect(mockIframe.src).toBe(
        'tauri://localhost/plugins/test/index.html#tauron-token=test-token-123',
      );
      expect(bridge.handshakeToken).toBe('test-token-123');
      expect(mockIframe.style.display).toBe('none');
      expect(iframe).toBe(mockIframe);
    });

    it('src 自带 hash 时被 token 片段替换（避免歧义）', () => {
      bridge.createIframe('https://app/p.html#stale');
      expect(mockIframe.src).toBe('https://app/p.html#tauron-token=test-token-123');
    });
  });

  describe('handshake', () => {
    it('sends init message on valid ready', () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');
      handshake();

      const initMsg = sentMessages().find((m) => m.action === 'init');
      expect(initMsg).toBeDefined();
      expect((initMsg!.payload as { permissions: string[] }).permissions).toEqual([
        'store:read',
        'http:fetch',
      ]);
    });

    it('does not send init on wrong token', () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');

      deliver({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'ready',
        payload: { token: 'wrong-token' },
      });

      expect(mockIframe.contentWindow!.postMessage).not.toHaveBeenCalled();
    });
  });

  describe('来源校验（安全边界）', () => {
    it('陌生窗口伪造的信封被丢弃，不触发任何处理', () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');

      // 伪造 ready（带正确 token）——但来源不是本 iframe
      deliverFromForeignWindow({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'ready',
        payload: { token: 'test-token-123' },
      });
      expect(mockIframe.contentWindow!.postMessage).not.toHaveBeenCalled();

      // 伪造 invoke——桥仍未握手，不得执行
      deliverFromForeignWindow({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'invoke',
        callId: 'evil-1',
        payload: { method: 'fs:write', args: {} },
      });
      const msgs = sentMessages();
      expect(msgs.find((m) => m.action === 'invoke-result')).toBeUndefined();
      expect(msgs.find((m) => m.action === 'init')).toBeUndefined();
    });

    it('iframe 未创建时忽略一切消息', () => {
      deliver({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'ready',
        payload: { token: 'test-token-123' },
      });
      expect(mockIframe.contentWindow!.postMessage).not.toHaveBeenCalled();
    });
  });

  describe('invoke', () => {
    it('handles invoke and sends result', async () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');
      handshake();

      deliver({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'invoke',
        callId: 'call-123',
        payload: { method: 'format', args: { code: 'let x = 1;' } },
      });

      await new Promise((r) => setTimeout(r, 20));

      const resultMsg = sentMessages().find((m) => m.action === 'invoke-result');
      expect(resultMsg).toBeDefined();
      const payload = resultMsg!.payload as { callId: string; result: { ok: boolean } };
      expect(payload.callId).toBe('call-123');
      expect(payload.result.ok).toBe(true);
    });

    it('invokeHandler 抛错时回发结构化错误', async () => {
      const failing = new PluginBridge([], async () => { throw new Error('boom'); }, async () => {});
      (failing as unknown as { iframe: unknown }).iframe = mockIframe;
      (failing as unknown as { ready: boolean }).ready = true;

      (
        failing as unknown as { handleMessage: (e: { data: unknown; source: unknown }) => void }
      ).handleMessage({
        data: {
          type: BRIDGE_MESSAGE_TYPE,
          direction: 'plugin-to-host',
          action: 'invoke',
          callId: 'call-err',
          payload: { method: 'x', args: {} },
        },
        source: mockIframe.contentWindow,
      });

      await new Promise((r) => setTimeout(r, 10));

      const resultMsg = sentMessages().find((m) => m.action === 'invoke-result');
      expect(resultMsg).toBeDefined();
      const result = (resultMsg!.payload as { result: { ok: boolean; error: { code: string; message: string } } }).result;
      expect(result.ok).toBe(false);
      expect(result.error.code).toBe('SC-9001');
      expect(result.error.message).toBe('boom');
    });

    it('配置了方法权限且未授予时拒绝（SC-1002）', async () => {
      const guarded = new PluginBridge(
        ['store:read'], // 未授予 fs:write
        async () => ({}),
        async () => {},
        { 'fs:write': 'fs:write' },
      );
      (guarded as unknown as { iframe: unknown }).iframe = mockIframe;
      (guarded as unknown as { ready: boolean }).ready = true;

      (
        guarded as unknown as { handleMessage: (e: { data: unknown; source: unknown }) => void }
      ).handleMessage({
        data: {
          type: BRIDGE_MESSAGE_TYPE,
          direction: 'plugin-to-host',
          action: 'invoke',
          callId: 'call-perm',
          payload: { method: 'fs:write', args: {} },
        },
        source: mockIframe.contentWindow,
      });

      await new Promise((r) => setTimeout(r, 10));

      const resultMsg = sentMessages().find((m) => m.action === 'invoke-result');
      expect(resultMsg).toBeDefined();
      const result = (resultMsg!.payload as { result: { ok: boolean; error: { code: string } } }).result;
      expect(result.ok).toBe(false);
      expect(result.error.code).toBe('SC-1002');
    });
  });

  describe('cancel 语义', () => {
    it('cancel 通知后端并丢弃已取消调用的结果', async () => {
      const cancelHandler = vi.fn(async () => {});
      let resolveInvoke!: (v: unknown) => void;
      const deferred = new PluginBridge(
        [],
        () => new Promise((resolve) => { resolveInvoke = resolve; }),
        cancelHandler,
      );
      (deferred as unknown as { iframe: unknown }).iframe = mockIframe;
      (deferred as unknown as { ready: boolean }).ready = true;
      const deliverTo = (
        msg: PluginToBridgeMessage,
      ): void => {
        (deferred as unknown as { handleMessage: (e: { data: unknown; source: unknown }) => void }).handleMessage({
          data: msg,
          source: mockIframe.contentWindow,
        });
      };

      deliverTo({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'invoke',
        callId: 'call-cancel',
        payload: { method: 'slow', args: {} },
      });

      deferred.cancel('call-cancel');
      expect(cancelHandler).toHaveBeenCalledWith('call-cancel');
      expect(sentMessages().some((m) => m.action === 'cancel')).toBe(true);

      // 迟到的结果：必须被丢弃，不得回发
      resolveInvoke({ late: true });
      await new Promise((r) => setTimeout(r, 10));

      expect(sentMessages().some((m) => m.action === 'invoke-result')).toBe(false);
    });
  });

  describe('event subscription', () => {
    it('forwards emitted events to handlers', () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');
      handshake();

      const received: unknown[] = [];
      bridge.onEvent('format-done', (payload) => received.push(payload));

      deliver({
        type: BRIDGE_MESSAGE_TYPE,
        direction: 'plugin-to-host',
        action: 'emit',
        payload: { eventName: 'format-done', payload: { result: 'done' } },
      });

      expect(received).toEqual([{ result: 'done' }]);
    });

    it('emitToPlugin 把事件推送给插件（Host → 插件）', () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');
      handshake();

      bridge.emitToPlugin('config-changed', { theme: 'dark' });

      const eventMsg = sentMessages().find((m) => m.action === 'event');
      expect(eventMsg).toBeDefined();
      expect(eventMsg!.payload).toEqual({ eventName: 'config-changed', payload: { theme: 'dark' } });
    });
  });

  describe('destroy', () => {
    it('removes iframe from DOM', () => {
      bridge.createIframe('tauri://localhost/plugins/test/index.html');
      bridge.destroy();
      expect(mockIframe.parentNode.removeChild).toHaveBeenCalledWith(mockIframe);
    });

    it('destroy 后在途结果被丢弃', async () => {
      let resolveInvoke!: (v: unknown) => void;
      const deferred = new PluginBridge(
        [],
        () => new Promise((resolve) => { resolveInvoke = resolve; }),
        async () => {},
      );
      (deferred as unknown as { iframe: unknown }).iframe = mockIframe;
      (deferred as unknown as { ready: boolean }).ready = true;

      (
        deferred as unknown as { handleMessage: (e: { data: unknown; source: unknown }) => void }
      ).handleMessage({
        data: {
          type: BRIDGE_MESSAGE_TYPE,
          direction: 'plugin-to-host',
          action: 'invoke',
          callId: 'call-d',
          payload: { method: 'slow', args: {} },
        },
        source: mockIframe.contentWindow,
      });

      deferred.destroy();
      resolveInvoke({ late: true });
      await new Promise((r) => setTimeout(r, 10));

      expect(sentMessages().some((m) => m.action === 'invoke-result')).toBe(false);
    });
  });
});
