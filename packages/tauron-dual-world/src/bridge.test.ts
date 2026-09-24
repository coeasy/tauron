import { describe, it, expect } from 'vitest';
import { createBridge, DEFAULT_BRIDGE_CONFIG } from './bridge.js';

describe('DEFAULT_BRIDGE_CONFIG', () => {
  it('has sensible defaults', () => {
    expect(DEFAULT_BRIDGE_CONFIG.maxMessageSize).toBe(65536);
    expect(DEFAULT_BRIDGE_CONFIG.timeout).toBe(30000);
    expect(DEFAULT_BRIDGE_CONFIG.allowedHostFunctions).toEqual([]);
    expect(DEFAULT_BRIDGE_CONFIG.deniedHostFunctions).toEqual([]);
  });
});

describe('createBridge', () => {
  it('creates bridge with default config', () => {
    const bridge = createBridge();
    expect(bridge).toBeDefined();
    expect(bridge.config).toBeDefined();
  });

  it('creates bridge with custom config', () => {
    const bridge = createBridge({ maxMessageSize: 1024 });
    expect(bridge.config.maxMessageSize).toBe(1024);
  });

  it('registers host functions', () => {
    const bridge = createBridge();
    const fn = async (args: unknown[]) => ({ result: args });
    expect(() => bridge.registerHostFunction('testFn', fn)).not.toThrow();
  });

  it('rejects denied host functions', () => {
    const bridge = createBridge({ deniedHostFunctions: ['dangerousFn'] });
    expect(() => bridge.registerHostFunction('dangerousFn', async () => {})).toThrow();
  });

  it('unregisters host functions', () => {
    const bridge = createBridge();
    bridge.registerHostFunction('testFn', async () => {});
    expect(() => bridge.unregisterHostFunction('testFn')).not.toThrow();
  });

  it('sends host-to-sandbox messages', () => {
    const bridge = createBridge();
    expect(() =>
      bridge.sendMessage({ id: '1', direction: 'host-to-sandbox', action: 'invoke', data: {} }),
    ).not.toThrow();
  });

  it('rejects sandbox-to-host messages in sendMessage', () => {
    const bridge = createBridge();
    expect(() =>
      bridge.sendMessage({ id: '1', direction: 'sandbox-to-host', action: 'invoke', data: {} }),
    ).toThrow();
  });

  it('rejects oversized messages', () => {
    const bridge = createBridge({ maxMessageSize: 10 });
    expect(() =>
      bridge.sendMessage({
        id: '1',
        direction: 'host-to-sandbox',
        action: 'invoke',
        data: { big: 'x'.repeat(100) },
      }),
    ).toThrow();
  });

  it('handles events', () => {
    const bridge = createBridge();
    const received: unknown[] = [];

    bridge.on('test-event', (data) => received.push(data));
    bridge.emit('test-event', { data: 1 });

    expect(received).toEqual([{ data: 1 }]);
  });

  it('unsubscribes from events', () => {
    const bridge = createBridge();
    const received: unknown[] = [];

    const unsub = bridge.on('test-event', (data) => received.push(data));
    bridge.emit('test-event', { data: 1 });
    unsub();
    bridge.emit('test-event', { data: 2 });

    expect(received).toEqual([{ data: 1 }]);
  });

  it('manages message queue', () => {
    const bridge = createBridge();
    expect(bridge.getQueueSize()).toBe(0);

    bridge.sendMessage({ id: '1', direction: 'host-to-sandbox', action: 'invoke', data: {} });
    bridge.sendMessage({ id: '2', direction: 'host-to-sandbox', action: 'invoke', data: {} });
    expect(bridge.getQueueSize()).toBe(2);

    bridge.clearQueue();
    expect(bridge.getQueueSize()).toBe(0);
  });

  it('destroys bridge cleanly', () => {
    const bridge = createBridge();
    bridge.registerHostFunction('testFn', async () => {});
    bridge.on('test-event', () => {});
    bridge.destroy();
    // After destroy, registering should still work but queue should be empty
    expect(bridge.getQueueSize()).toBe(0);
  });

  it('drainQueue 交付并清空消息（队列的唯一消费入口）', () => {
    const bridge = createBridge();
    bridge.sendMessage({ id: '1', direction: 'host-to-sandbox', action: 'invoke', data: {} });
    bridge.sendMessage({ id: '2', direction: 'host-to-sandbox', action: 'invoke', data: {} });

    const drained = bridge.drainQueue();
    expect(drained.map((m) => m.id)).toEqual(['1', '2']);
    expect(bridge.getQueueSize()).toBe(0);
    expect(bridge.drainQueue()).toEqual([]);
  });

  it('队列有界：超出上限丢弃最旧并计数（不再无界增长）', () => {
    const bridge = createBridge({ maxQueueSize: 3 });
    for (let i = 1; i <= 5; i++) {
      bridge.sendMessage({
        id: String(i),
        direction: 'host-to-sandbox',
        action: 'invoke',
        data: {},
      });
    }

    expect(bridge.getQueueSize()).toBe(3);
    expect(bridge.getDroppedCount()).toBe(2);
    // 保留的是最新的 3 条
    expect(bridge.drainQueue().map((m) => m.id)).toEqual(['3', '4', '5']);
  });

  it('allowedHostFunctions 白名单非空时拒绝未列出的函数', () => {
    const bridge = createBridge({ allowedHostFunctions: ['fs.read'] });

    expect(() => bridge.registerHostFunction('shell.exec', async () => {})).toThrow(
      /allowlist/,
    );
    expect(() => bridge.registerHostFunction('fs.read', async () => {})).not.toThrow();
  });

  it('未注册（含被白名单拒绝而未注册）的函数调用返回 NOT_FOUND', async () => {
    const bridge = createBridge({ allowedHostFunctions: ['fs.read'] });
    bridge.registerHostFunction('fs.read', async () => 'ok');

    const res = (await bridge.receiveMessage({
      id: '1',
      direction: 'sandbox-to-host',
      action: 'invoke',
      data: { requestId: 'r1', method: 'shell.exec', args: [] },
    })) as { ok: boolean; error?: { code: string } };

    expect(res.ok).toBe(false);
    expect(res.error?.code).toBe('NOT_FOUND');
  });

  it('getState/setState 经由沙箱上下文真正持久化', async () => {
    const bridge = createBridge();
    bridge.registerHostFunction('remember', async (_args, ctx) => {
      ctx.setState('seen', 42);
      return ctx.getState('seen');
    });

    const res = (await bridge.receiveMessage({
      id: '1',
      direction: 'sandbox-to-host',
      action: 'invoke',
      data: { requestId: 'r1', method: 'remember', args: [] },
    })) as { ok: boolean; result: unknown };

    expect(res.ok).toBe(true);
    expect(res.result).toBe(42);
  });
});
