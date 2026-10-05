import { describe, it, expect, vi } from 'vitest';
import { createSandbox, registerHostFunction, SandboxDestroyedError } from './sandbox.js';
import type { SandboxConfig } from './types.js';

/** 手动落定的门：时序判据不靠 sleep。 */
function deferred<T = void>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

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
    const ids = new Set<string>();
    for (let i = 0; i < 50; i += 1) {
      ids.add(createSandbox(baseConfig).id);
    }
    expect(ids.size).toBe(50);
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

  it('destroy() 之前状态可正常读写', () => {
    const sandbox = createSandbox(baseConfig);
    sandbox.setState('test', 'value');
    expect(sandbox.getState('test')).toBe('value');
    expect(sandbox.destroyed).toBe(false);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// V7-P1-04：destroy 是一次性终态。
//
// 此前 destroy() 只清表不设旗，之后 call/setState/emit/on 照常工作——
// 「已销毁」对外看起来像「还能用」。这里的判据全部围绕**同一错误码**与
// **零副作用**（宿主函数一次都不许执行）。
// ──────────────────────────────────────────────────────────────────────────
describe('createSandbox — destroy 终态（V7-P1-04）', () => {
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
    allowedHostFunctions: ['ping'],
    deniedHostFunctions: ['shell.exec'],
    env: {},
  };

  it('destroy 后所有入口按 SANDBOX_DESTROYED 拒绝，且宿主函数不执行', async () => {
    const hostFn = vi.fn(async () => '不该被执行');
    const received: unknown[] = [];

    const sandbox = createSandbox(baseConfig);
    registerHostFunction(sandbox, 'ping', hostFn);
    sandbox.on('topic', (data) => received.push(data));

    await sandbox.destroy();
    expect(sandbox.destroyed).toBe(true);

    // 返回值型入口：失败结果 + 同一个码
    const callResult = await sandbox.call('ping', []);
    expect(callResult.ok).toBe(false);
    expect(callResult.error?.code).toBe('SANDBOX_DESTROYED');
    const execResult = await sandbox.execute('1 + 1');
    expect(execResult.ok).toBe(false);
    expect(execResult.error?.code).toBe('SANDBOX_DESTROYED');

    // 抛出型入口：同一个码
    expect(() => sandbox.setState('k', 1)).toThrow(SandboxDestroyedError);
    expect(() => sandbox.getState('k')).toThrow(SandboxDestroyedError);
    expect(() => sandbox.emit('topic', 1)).toThrow(SandboxDestroyedError);
    expect(() => sandbox.on('topic2', () => {})).toThrow(SandboxDestroyedError);
    expect(() => registerHostFunction(sandbox, 'late', async () => 1)).toThrow(
      SandboxDestroyedError,
    );
    try {
      sandbox.getState('k');
      expect.unreachable('getState 必须抛错，而不是把「已销毁」降级成 undefined');
    } catch (err) {
      expect((err as SandboxDestroyedError).code).toBe('SANDBOX_DESTROYED');
    }

    // 关键判据：destroy 之后没有任何宿主函数被调用、没有事件被派发。
    expect(hostFn).not.toHaveBeenCalled();
    expect(received).toEqual([]);
  });

  it('destroy() 幂等：重复调用不抛错、不改终态', async () => {
    const sandbox = createSandbox(baseConfig);
    await sandbox.destroy();
    await expect(sandbox.destroy()).resolves.toBeUndefined();
    expect(sandbox.destroyed).toBe(true);
    expect(() => sandbox.setState('k', 1)).toThrow(SandboxDestroyedError);
  });

  it('宿主函数 await 期间 destroy：结果作废，call() 不返回成功', async () => {
    const gate = deferred<string>();
    const sandbox = createSandbox(baseConfig);
    registerHostFunction(sandbox, 'ping', async () => gate.promise);

    const inFlight = sandbox.call('ping', []);
    await sandbox.destroy(); // 启动时还活着，落定前已被销毁
    gate.resolve('late result');

    const result = await inFlight;
    expect(result.ok).toBe(false);
    expect(result.error?.code).toBe('SANDBOX_DESTROYED');
    expect(result.result).toBeUndefined();
  });

  it('destroy 不泄漏处理器：新实例的事件到不了旧实例', async () => {
    const hits: string[] = [];
    const oldSandbox = createSandbox(baseConfig);
    oldSandbox.on('topic', () => hits.push('old'));
    await oldSandbox.destroy();

    const fresh = createSandbox(baseConfig);
    fresh.on('topic', () => hits.push('fresh'));
    fresh.emit('topic', 1);

    expect(hits).toEqual(['fresh']);
    expect(() => oldSandbox.emit('topic', 2)).toThrow(SandboxDestroyedError);
  });

  it('ID 来自单调计数 + UUID，不经 Math.random', () => {
    const randomSpy = vi.spyOn(Math, 'random').mockImplementation(() => 0.42);
    try {
      const ids = new Set<string>();
      for (let i = 0; i < 30; i += 1) ids.add(createSandbox(baseConfig).id);
      expect(ids.size).toBe(30);
      // 有 crypto.randomUUID 时必须是 UUID 形状（不是 base36 随机串）
      expect(Array.from(ids)[0]).toMatch(/^sbx-\d+-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-/);
      expect(randomSpy, '身份生成不得再调用 Math.random').not.toHaveBeenCalled();
    } finally {
      randomSpy.mockRestore();
    }
  });

  it('无 crypto.randomUUID 时退化为单调计数（唯一性仍在，但不声称不可预测）', () => {
    const original = globalThis.crypto;
    vi.stubGlobal('crypto', { randomUUID: undefined });
    try {
      const a = createSandbox(baseConfig);
      const b = createSandbox(baseConfig);
      expect(a.id).toMatch(/noncrypto$/);
      expect(a.id).not.toBe(b.id);
    } finally {
      vi.stubGlobal('crypto', original);
      vi.unstubAllGlobals();
    }
  });
});
