import { describe, it, expect } from 'vitest';
import { createSandbox } from './sandbox.js';
import type { SandboxConfig } from './types.js';

describe('createSandbox', () => {
  const baseConfig: SandboxConfig = {
    pluginId: 'test-plugin',
    name: 'test-sandbox',
    memoryLimit: 64,
    timeout: 30000,
    capabilities: {
      fs: false,
      network: false,
      timers: true,
      dom: false,
      workers: false,
      wasm: false,
    },
    allowedHostFunctions: ['fs.read', 'fs.write'],
    deniedHostFunctions: ['shell.exec'],
    env: {},
  };

  it('creates sandbox with valid config', () => {
    const sandbox = createSandbox(baseConfig);
    expect(sandbox).toBeDefined();
    expect(sandbox.id).toBeDefined();
    expect(sandbox.pluginId).toBe('test-plugin');
    expect(sandbox.config).toBe(baseConfig);
  });

  it('generates unique sandbox IDs', () => {
    const s1 = createSandbox(baseConfig);
    const s2 = createSandbox(baseConfig);
    expect(s1.id).not.toBe(s2.id);
  });

  it('manages shared state', () => {
    const sandbox = createSandbox(baseConfig);
    sandbox.setState('key1', 'value1');
    expect(sandbox.getState('key1')).toBe('value1');
    expect(sandbox.getState('nonexistent')).toBeUndefined();
  });

  it('handles events', () => {
    const sandbox = createSandbox(baseConfig);
    const received: unknown[] = [];

    sandbox.on('test-event', (data) => received.push(data));
    sandbox.emit('test-event', { data: 1 });
    sandbox.emit('test-event', { data: 2 });

    expect(received).toEqual([{ data: 1 }, { data: 2 }]);
  });

  it('unsubscribes from events', () => {
    const sandbox = createSandbox(baseConfig);
    const received: unknown[] = [];

    const unsub = sandbox.on('test-event', (data) => received.push(data));
    sandbox.emit('test-event', { data: 1 });

    unsub();
    sandbox.emit('test-event', { data: 2 });

    expect(received).toEqual([{ data: 1 }]);
  });

  it('calls methods successfully', async () => {
    const config = { ...baseConfig, allowedHostFunctions: ['ping'] };
    const sandbox = createSandbox(config);

    // Register a host function
    const { registerHostFunction } = await import('./sandbox.js');
    registerHostFunction(sandbox, 'ping', async (args, ctx) => {
      return { pong: true, args, ctx };
    });

    const result = await sandbox.call('ping', [{ hello: 'world' }]);
    expect(result.ok).toBe(true);
    expect(result.result).toBeDefined();
  });

  it('handles method call errors', async () => {
    const sandbox = createSandbox(baseConfig);

    const result = await sandbox.call('unknown-method', []);
    expect(result.ok).toBe(false);
    expect(result.error?.code).toBe('CALL_ERROR');
  });

  it('execute() 在未接入运行时前 fail closed（不执行、也不伪报执行成功）', async () => {
    // 轮 11 修正：此前这条断言的是 `ok === true`，而实现**一行代码都没执行**——
    // 测试与实现一起说谎（测试锁住了伪造行为）。现在锁的是"拒绝执行"。
    const sandbox = createSandbox(baseConfig);
    const result = await sandbox.execute('console.log("hello")', 1000);
    expect(result.ok).toBe(false);
    expect(result.error?.code).toBe('SANDBOX_UNAVAILABLE');
    expect(result.error?.message).toMatch(/不执行代码|未接入/);
    // 关键：绝不能出现"已执行"的痕迹。
    expect(result.result).toBeUndefined();
  });

  it('destroys sandbox cleanly', async () => {
    const sandbox = createSandbox(baseConfig);
    await sandbox.destroy();
    // After destroy, calls should still work but state should be cleared
    sandbox.setState('test', 'value');
    expect(sandbox.getState('test')).toBe('value');
  });
});
