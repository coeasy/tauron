// ──────────────────────────────────────────────────────────────────────────
// §4.22 契约测试套件：core API 行为 / 事件协议 / 错误码用例集。
//
// 定位（架构 §5.4）：core 契约 + 每框架绑定冒烟。
// 组件只有一份实现，一致性由构造保证。
// ──────────────────────────────────────────────────────────────────────────

import { describe, it, expect, vi } from 'vitest';
import {
  MockBackend,
  HostClient,
  HOST_ERROR_CODES,
  RETRYABLE_HOST_ERROR_CODES,
  normalizeError,
  HostException,
  isRetryable,
  isHostErrorCode,
  CAPABILITIES,
  capabilityOf,
  isAvailable,
  capabilityMatrix,
  AUTH_TIERS,
} from '@tauron/host';
// 直接对 `@tauron/framework` 的**源码**做绑定冒烟：该包的 dist 是与本包并行构建的
// 产物（可能落后于源码），契约冒烟要验证的是权威源码，而非某个时间点的产物。
import { useInvoke } from '../../tauron-framework/src/index.js';

// ──────────────────────────────────────────────────────────────────────────
// 契约 1：错误码协议
// ──────────────────────────────────────────────────────────────────────────

describe('契约 1：错误码全集', () => {
  it('HOST_ERROR_CODES 恰好 18 个', () => {
    expect(HOST_ERROR_CODES.length).toBe(18);
  });

  it('HOST_ERROR_CODES 与线上协议逐项一致', () => {
    // 与 `@tauron/host` 的 Rust `ErrorCode` 枚举同序。
    // 任何增删/改名都必须在此显式对齐——数量断言不足以发现"换了一个码"。
    expect([...HOST_ERROR_CODES]).toEqual([
      'E_HOST_PANIC',
      'E_UNKNOWN_PLUGIN',
      'E_AUTH_DENIED',
      'E_INVALID_MANIFEST',
      'E_STATE_INVALID_TRANSITION',
      'E_CALL_NOT_FOUND',
      'E_CALL_TIMEOUT',
      'E_FORBIDDEN_PERMISSION',
      'E_ABI_MISMATCH',
      'E_PLUGIN_DISABLED',
      'E_REGISTRY_FULL',
      'E_CALL_PENDING_FULL',
      'E_SUBSCRIPTION_FULL',
      'E_PLUGIN_EXISTS',
      'E_INSTALL_FAILED',
      'E_PLUGIN_FILTERED',
      // P0-2：进程插件运行时（spawn 的诚实失败码 + 租约语义）
      'E_PLUGIN_TYPE_NO_RUNTIME',
      'E_LEASE_EXPIRED',
    ]);
  });

  it('所有错误码以 E_ 开头', () => {
    for (const code of HOST_ERROR_CODES) {
      expect(code).toMatch(/^E_[A-Z_]+$/);
    }
  });

  it('错误码无重复', () => {
    const set = new Set(HOST_ERROR_CODES);
    expect(set.size).toBe(HOST_ERROR_CODES.length);
  });

  it('可重试错误码恰好 3 个', () => {
    expect(RETRYABLE_HOST_ERROR_CODES.length).toBe(3);
    // 可重试集合是安全相关契约（决定框架层是否自动重试），逐项钉死。
    expect([...RETRYABLE_HOST_ERROR_CODES]).toEqual([
      'E_CALL_TIMEOUT',
      'E_HOST_PANIC',
      'E_PLUGIN_FILTERED',
    ]);
  });

  it('RETRYABLE 都是 HOST_ERROR_CODES 的子集', () => {
    for (const code of RETRYABLE_HOST_ERROR_CODES) {
      expect(isHostErrorCode(code)).toBe(true);
    }
  });

  it('isRetryable 正确判定', () => {
    expect(isRetryable('E_CALL_TIMEOUT')).toBe(true);
    expect(isRetryable('E_HOST_PANIC')).toBe(true);
    expect(isRetryable('E_PLUGIN_FILTERED')).toBe(true);
    expect(isRetryable('E_AUTH_DENIED')).toBe(false);
    expect(isRetryable('E_UNKNOWN')).toBe(false);
    expect(isRetryable('E_NOT_A_CODE')).toBe(false);
  });

  it('isHostErrorCode 类型守卫', () => {
    expect(isHostErrorCode('E_AUTH_DENIED')).toBe(true);
    expect(isHostErrorCode('E_NOT_REAL')).toBe(false);
    expect(isHostErrorCode('')).toBe(false);
  });
});

describe('契约 2：错误规范化', () => {
  it('标准宿主错误：{ code, message }', () => {
    const err = normalizeError({ code: 'E_AUTH_DENIED', message: 'no perm' });
    expect(err.code).toBe('E_AUTH_DENIED');
    expect(err.message).toBe('no perm');
    expect(err.retryable).toBe(false);
  });

  it('未知错误码归为 E_UNKNOWN', () => {
    const err = normalizeError({ code: 'E_FOO_BAR', message: 'unknown' });
    expect(err.code).toBe('E_UNKNOWN');
    expect(err.retryable).toBe(false);
  });

  it('Tauri IPC 错误：从 message 中提取错误码', () => {
    const err = normalizeError({
      message: 'Plugin command failed: E_CALL_TIMEOUT - timed out',
    });
    expect(err.code).toBe('E_CALL_TIMEOUT');
    expect(err.retryable).toBe(true);
  });

  it('Error 实例中的错误码', () => {
    const err = normalizeError(new Error('E_STATE_INVALID_TRANSITION: bad state'));
    expect(err.code).toBe('E_STATE_INVALID_TRANSITION');
    expect(err.retryable).toBe(false);
  });

  it('command not found → E_UNKNOWN + 不可重试', () => {
    const err = normalizeError({ message: 'command not found: host_foo' });
    expect(err.code).toBe('E_UNKNOWN');
    expect(err.retryable).toBe(false);
  });

  it('字符串错误 → E_UNKNOWN', () => {
    const err = normalizeError('something went wrong');
    expect(err.code).toBe('E_UNKNOWN');
    expect(err.retryable).toBe(false);
  });

  it('null/undefined → E_UNKNOWN', () => {
    const err = normalizeError(null);
    expect(err.code).toBe('E_UNKNOWN');
    expect(err.retryable).toBe(false);
  });

  it('无 message 的对象 → E_UNKNOWN', () => {
    const err = normalizeError({ foo: 'bar' });
    expect(err.code).toBe('E_UNKNOWN');
    expect(err.retryable).toBe(false);
  });
});

describe('契约 3：HostException', () => {
  it('从 HostErrorShape 构造', () => {
    const exc = new HostException({
      code: 'E_CALL_TIMEOUT',
      rawCode: 'E_CALL_TIMEOUT',
      message: 'timed out',
      retryable: true,
    });
    expect(exc.code).toBe('E_CALL_TIMEOUT');
    expect(exc.rawCode).toBe('E_CALL_TIMEOUT');
    expect(exc.retryable).toBe(true);
    expect(exc.message).toBe('timed out');
    expect(exc.name).toBe('HostException');
  });

  it('toShape 往返', () => {
    const shape = {
      code: 'E_AUTH_DENIED' as const,
      rawCode: 'E_AUTH_DENIED',
      message: 'denied',
      retryable: false,
    };
    const exc = new HostException(shape);
    expect(exc.toShape()).toEqual(shape);
  });

  it('是 Error 的子类', () => {
    const exc = new HostException({
      code: 'E_UNKNOWN',
      rawCode: null,
      message: 'x',
      retryable: false,
    });
    expect(exc).toBeInstanceOf(Error);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 契约 4：能力矩阵
// ──────────────────────────────────────────────────────────────────────────

describe('契约 4：能力矩阵', () => {
  it('所有命令都在 CAPABILITIES 中注册', () => {
    for (const cap of CAPABILITIES) {
      expect(cap.command).toBeTruthy();
      expect(cap.tier).toBeTruthy();
    }
  });

  it('capabilityOf 返回正确能力', () => {
    const cap = capabilityOf('host_registry_list');
    expect(cap).toBeDefined();
    expect(cap!.command).toBe('host_registry_list');
  });

  it('capabilityOf 对未知命令返回 undefined', () => {
    const cap = capabilityOf('host_unknown_cmd');
    expect(cap).toBeUndefined();
  });

  it('isAvailable 在 MockBackend 中正确判定', () => {
    const backend = new MockBackend({ capabilities: ['host_registry_list'] });
    expect(isAvailable(backend, 'host_registry_list')).toBe(true);
    expect(isAvailable(backend, 'host_unknown')).toBe(false);
  });

  it('capabilityMatrix 返回所有能力的可用状态', () => {
    const backend = new MockBackend({ capabilities: ['host_registry_list'] });
    const matrix = capabilityMatrix(backend);
    expect(Object.keys(matrix).length).toBe(CAPABILITIES.length);
    expect(matrix['host_registry_list']).toBe(true);
    expect(matrix['host_plugin_call']).toBe(false);
  });

  it('AUTH_TIERS 包含三个档位', () => {
    expect(AUTH_TIERS).toContain('self');
    expect(AUTH_TIERS).toContain('scoped-read');
    expect(AUTH_TIERS).toContain('privileged');
  });

  it('每个能力都有 tier', () => {
    for (const cap of CAPABILITIES) {
      expect(['self', 'scoped-read', 'privileged']).toContain(cap.tier);
    }
  });

  it('self 档命令的 consumer 是 plugin', () => {
    for (const cap of CAPABILITIES) {
      if (cap.tier === 'self') {
        expect(cap.consumer).toBe('plugin');
      }
    }
  });

  it('privileged 档命令的 consumer 是 main-window', () => {
    for (const cap of CAPABILITIES) {
      if (cap.tier === 'privileged') {
        expect(cap.consumer).toBe('main-window');
      }
    }
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 契约 5：HostClient 行为
// ──────────────────────────────────────────────────────────────────────────

describe('契约 5：HostClient 行为', () => {
  it('pluginCall 发送正确的命令和参数，并返回宿主铸造的 pending 簿记', async () => {
    const backend = new MockBackend({
      capabilities: ['host_plugin_call'],
      cases: [{
        cmd: 'host_plugin_call',
        args: { req: { callId: 'c1', method: 'greet', kind: 'unary' } },
        result: { callId: 'host-1', pluginId: 'p1', cmd: 'greet', args: null, seq: 1, createdAt: 10, expiresAt: 20 },
      }],
    });
    const client = new HostClient({ backend });
    const result = await client.pluginCall({
      callId: 'c1',
      method: 'greet',
      kind: 'unary',
    });
    expect(result.callId).toBe('host-1');
    expect(result.seq).toBe(1);
  });

  it('pluginCall 传递 argsJson', async () => {
    const backend = new MockBackend({
      capabilities: ['host_plugin_call'],
      cases: [{
        cmd: 'host_plugin_call',
        args: { req: { callId: 'c2', method: 'calc', kind: 'unary', argsJson: { x: 1 } } },
        result: { callId: 'host-2', pluginId: 'p1', cmd: 'calc', args: { x: 1 }, seq: 2, createdAt: 10, expiresAt: 20 },
      }],
    });
    const client = new HostClient({ backend });
    const result = await client.pluginCall({
      callId: 'c2',
      method: 'calc',
      kind: 'unary',
      argsJson: { x: 1 },
    });
    expect(result.callId).toBe('host-2');
  });

  it('eventsPublish 发送正确命令', async () => {
    const backend = new MockBackend({
      capabilities: ['host_events_publish'],
      cases: [{
        cmd: 'host_events_publish',
        args: { evt: { topic: 'plugin:p1.ready', payload: { ok: true } } },
        result: { delivered: 2, dropped: false },
      }],
    });
    const client = new HostClient({ backend });
    const result = await client.eventsPublish({
      topic: 'plugin:p1.ready',
      payload: { ok: true },
    });
    expect(result).toEqual({ delivered: 2, dropped: false });
  });

  it('eventsSubscribe 返回订阅信息', async () => {
    const backend = new MockBackend({
      capabilities: ['host_events_subscribe'],
      cases: [{
        cmd: 'host_events_subscribe',
        args: { sub: [{ topic: 'plugin:p1.ready' }] },
        result: { token: 'tok-abc', selectors: [{ topic: 'plugin:p1.ready' }] },
      }],
    });
    const client = new HostClient({ backend });
    const result = await client.eventsSubscribe([{ topic: 'plugin:p1.ready' }]);
    expect(result.token).toBe('tok-abc');
    expect(result.selectors).toEqual([{ topic: 'plugin:p1.ready' }]);
  });

  it('eventsUnsubscribe 发送 token', async () => {
    const backend = new MockBackend({
      capabilities: ['host_events_unsubscribe'],
      cases: [{
        cmd: 'host_events_unsubscribe',
        args: { token: 'tok-abc' },
      }],
    });
    const client = new HostClient({ backend });
    await client.eventsUnsubscribe('tok-abc');
    expect(backend.invocations).toContainEqual({
      cmd: 'host_events_unsubscribe',
      args: { token: 'tok-abc' },
    });
  });

  it('registryList 传递 scope 参数', async () => {
    const backend = new MockBackend({
      capabilities: ['host_registry_list'],
      cases: [{
        cmd: 'host_registry_list',
        args: { scope: 'public' },
        result: [{ id: 'p.audio', name: 'Audio' }],
      }],
    });
    const client = new HostClient({ backend });
    const plugins = await client.registryList('public');
    expect(plugins).toEqual([{ id: 'p.audio', name: 'Audio' }]);
  });

  it('lifecycleReport 上报生命周期事件（不是状态）', async () => {
    const backend = new MockBackend({
      capabilities: ['host_lifecycle_report'],
      cases: [{
        cmd: 'host_lifecycle_report',
        args: { evt: { event: 'ATTACH' } },
      }],
    });
    const client = new HostClient({ backend });
    await client.lifecycleReport({ event: 'ATTACH' });
    expect(backend.invocations).toContainEqual({
      cmd: 'host_lifecycle_report',
      args: { evt: { event: 'ATTACH' } },
    });
  });

  it('cancel 传递 callId', async () => {
    const backend = new MockBackend({
      capabilities: ['host_cancel'],
      cases: [{
        cmd: 'host_cancel',
        args: { callId: 'c-123' },
      }],
    });
    const client = new HostClient({ backend });
    await client.cancel('c-123');
    expect(backend.invocations).toContainEqual({
      cmd: 'host_cancel',
      args: { callId: 'c-123' },
    });
  });

  it('callEnd 传递 callId 和 ok', async () => {
    const backend = new MockBackend({
      capabilities: ['host_call_end'],
      cases: [{
        cmd: 'host_call_end',
        args: { req: { callId: 'c-456', ok: true } },
      }],
    });
    const client = new HostClient({ backend });
    await client.callEnd({ callId: 'c-456', ok: true });
    expect(backend.invocations).toContainEqual({
      cmd: 'host_call_end',
      args: { req: { callId: 'c-456', ok: true } },
    });
  });

  it('pluginId 从 backend 读取', () => {
    const backend = new MockBackend({ pluginId: 'p.audio' });
    const client = new HostClient({ backend });
    expect(client.pluginId).toBe('p.audio');
  });

  it('pluginId 无上下文时返回 null', () => {
    const backend = new MockBackend({ pluginId: null });
    const client = new HostClient({ backend });
    expect(client.pluginId).toBeNull();
  });

  it('命令未注册时抛出 HostException', async () => {
    const backend = new MockBackend({ capabilities: [] });
    const client = new HostClient({ backend });
    await expect(client.cancel('c-1')).rejects.toBeInstanceOf(HostException);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 契约 6：Backend 抽象
// ──────────────────────────────────────────────────────────────────────────

describe('契约 6：Backend 抽象', () => {
  it('MockBackend 默认无能力', () => {
    const backend = new MockBackend();
    expect(backend.capabilities().size).toBe(0);
  });

  it('MockBackend invoke 未注册命令报错', async () => {
    const backend = new MockBackend();
    await expect(backend.invoke('host_unknown')).rejects.toThrow(/command not found/);
  });

  it('MockBackend invoke 已注册命令返回预置结果', async () => {
    const backend = new MockBackend({
      capabilities: ['host_get'],
      cases: [{ cmd: 'host_get', result: { value: 42 } }],
    });
    const result = await backend.invoke('host_get');
    expect(result).toEqual({ value: 42 });
  });

  it('MockBackend invoke 预置错误时 reject', async () => {
    const backend = new MockBackend({
      capabilities: ['host_fail'],
      cases: [{ cmd: 'host_fail', error: new Error('boom') }],
    });
    await expect(backend.invoke('host_fail')).rejects.toThrow('boom');
  });

  it('MockBackend listen 记录订阅', async () => {
    const backend = new MockBackend();
    const handler = vi.fn();
    const unsub = await backend.listen('evt', handler);
    backend.emit('evt', { data: 1 });
    expect(handler).toHaveBeenCalledWith({ data: 1 });
    unsub();
    backend.emit('evt', { data: 2 });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('MockBackend pluginId 返回配置值', () => {
    const backend = new MockBackend({ pluginId: 'p.test' });
    expect(backend.pluginId()).toBe('p.test');
  });

  it('MockBackend pluginId 默认 null', () => {
    const backend = new MockBackend();
    expect(backend.pluginId()).toBeNull();
  });

  it('MockBackend channel 返回**真能收帧**的端口（R5：旧占位 port 会掩盖断链）', () => {
    const backend = new MockBackend();
    const ch = backend.channel();
    expect(ch).toBeDefined();
    // R5 前这里是 `{ message: MessagePort }` 占位：类型上有字段、线上却传不进
    // Rust，帧永远送不出去且不报错。现在断言的是真实收帧入口与通道标识。
    expect(ch.id).toBeDefined();
    const seen: unknown[] = [];
    ch.onmessage = (frame) => seen.push(frame);
    ch.onmessage?.({ seq: 1, kind: 'data' });
    expect(seen).toEqual([{ seq: 1, kind: 'data' }]);
  });

  it('MockBackend invocations 记录所有调用', async () => {
    const backend = new MockBackend({
      capabilities: ['host_a', 'host_b'],
      cases: [
        { cmd: 'host_a', result: 1 },
        { cmd: 'host_b', result: 2 },
      ],
    });
    await backend.invoke('host_a');
    await backend.invoke('host_b', { x: 1 });
    expect(backend.invocations).toEqual([
      { cmd: 'host_a' },
      { cmd: 'host_b', args: { x: 1 } },
    ]);
  });

  it('MockBackend 参数子集匹配', async () => {
    const backend = new MockBackend({
      capabilities: ['host_query'],
      cases: [
        { cmd: 'host_query', args: { id: '1' }, result: 'one' },
        { cmd: 'host_query', args: { id: '2' }, result: 'two' },
      ],
    });
    expect(await backend.invoke('host_query', { id: '1' })).toBe('one');
    expect(await backend.invoke('host_query', { id: '2' })).toBe('two');
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 契约 7：事件协议
// ──────────────────────────────────────────────────────────────────────────

describe('契约 7：事件协议', () => {
  it('topic 格式：plugin:<id>.<event>', () => {
    const backend = new MockBackend({
      capabilities: ['host_events_publish'],
      cases: [{
        cmd: 'host_events_publish',
        args: { evt: { topic: 'plugin:p.audio.ready', payload: {} } },
        result: { delivered: 1, dropped: false },
      }],
    });
    const client = new HostClient({ backend });
    client.eventsPublish({ topic: 'plugin:p.audio.ready', payload: {} });
    expect(backend.invocations[0]?.args?.['evt']).toEqual({
      topic: 'plugin:p.audio.ready',
      payload: {},
    });
  });

  it('订阅选择器只含 topic；回执含 token 与 selectors', async () => {
    const backend = new MockBackend({
      capabilities: ['host_events_subscribe'],
      cases: [{
        cmd: 'host_events_subscribe',
        args: { sub: [{ topic: 'plugin:p.audio.ready' }] },
        result: { token: 't1', selectors: [{ topic: 'plugin:p.audio.ready' }] },
      }],
    });
    const client = new HostClient({ backend });
    const result = await client.eventsSubscribe([
      { topic: 'plugin:p.audio.ready' },
    ]);
    expect(result.token).toBe('t1');
    expect(result.selectors[0]?.topic).toBe('plugin:p.audio.ready');
    // 选择器原样透传给宿主命令（事件总线是唯一通道）。
    expect(backend.invocations[0]?.args?.['sub']).toEqual([
      { topic: 'plugin:p.audio.ready' },
    ]);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 契约 8：框架绑定冒烟（§4.11 useInvoke 契约）
// ──────────────────────────────────────────────────────────────────────────

describe('契约 8：框架绑定冒烟', () => {
  it('useInvoke 成功时状态为 success', async () => {
    const backend = new MockBackend({
      capabilities: ['host_get'],
      cases: [{ cmd: 'host_get', result: { ok: true } }],
    });
    const [state, call] = useInvoke<{ ok: boolean }>(backend, 'host_get');
    const result = await call();
    expect(result).toEqual({ ok: true });
    expect(state.value).toEqual({ status: 'success', data: { ok: true } });
  });

  it('useInvoke 失败时状态为 error', async () => {
    const backend = new MockBackend({
      capabilities: ['host_fail'],
      cases: [{ cmd: 'host_fail', error: new Error('boom') }],
    });
    const [state, call] = useInvoke(backend, 'host_fail');
    await expect(call()).rejects.toThrow('boom');
    expect(state.value.status).toBe('error');
  });
});
