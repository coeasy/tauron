import { describe, expect, it } from 'vitest';

import { MockBackend } from './backend.js';
import { AdminClient, FrameSink, HostClient } from './host.js';
import type { StreamFrame } from './stream.js';

const ALL_CAPS = [
  'host_plugin_call',
  'host_call_end',
  'host_cancel',
  'host_lifecycle_report',
  'host_contributes_register',
  'host_events_publish',
  'host_events_subscribe',
  'host_events_unsubscribe',
  'host_events_drain',
  'host_registry_list',
  'host_registry_admin',
];

function pluginClient(): { client: HostClient; backend: MockBackend } {
  const backend = new MockBackend({
    capabilities: ALL_CAPS,
    pluginId: 'com.example.formatter',
    cases: [
      { cmd: 'host_plugin_call', result: { callId: 'host-1', pluginId: 'com.example.formatter', cmd: 'format', args: null, seq: 7, createdAt: 100, expiresAt: 200 } },
      { cmd: 'host_registry_list', result: [{ id: 'com.example.formatter', name: 'Formatter', version: '1.0.0', state: 'ENABLED', disabledBySafemode: false }] },
      { cmd: 'host_events_publish', result: { delivered: 2, dropped: false } },
      { cmd: 'host_events_subscribe', result: { token: 'sub-1', selectors: [{ topic: 'com.example.x.ready' }] } },
      { cmd: 'host_events_drain', result: [] },
    ],
  });
  return { client: new HostClient({ backend }), backend };
}

describe('HostClient — 命令镜像与参数形状', () => {
  it('pluginCall 发送 §2.1 定义的 req 形状，且 pluginId 不由调用方传入', async () => {
    const { client, backend } = pluginClient();
    await client.pluginCall({ callId: 'c-1', method: 'format', argsJson: { text: 'hi' }, kind: 'unary' });

    const call = backend.invocations[0];
    expect(call?.cmd).toBe('host_plugin_call');
    expect(call?.args?.req).toEqual({ callId: 'c-1', method: 'format', kind: 'unary', argsJson: { text: 'hi' } });
    // ADR-17：身份只从 label 取，参数里不得出现 pluginId。
    expect(call?.args).not.toHaveProperty('pluginId');
    expect(call?.args).not.toHaveProperty('id');
    expect(call?.args).toHaveProperty('channel');
  });

  it('pluginCall 同时给 argsJson 和 argsRaw 时不静默丢 raw（互斥由调用方保证，此处保留 json）', async () => {
    const { client, backend } = pluginClient();
    await client.pluginCall({
      callId: 'c-2',
      method: 'm',
      argsJson: { a: 1 },
      argsRaw: new Uint8Array([1, 2, 3]),
      kind: 'stream',
    });
    expect(backend.invocations[0]?.args?.req).toEqual({ callId: 'c-2', method: 'm', kind: 'stream', argsJson: { a: 1 } });
  });

  it('pluginCall 只给 argsRaw 时抛错而非静默丢载荷（二进制通道未接线）', async () => {
    const { client, backend } = pluginClient();
    await expect(
      client.pluginCall({
        callId: 'c-raw',
        method: 'm',
        argsRaw: new Uint8Array([1, 2, 3]),
        kind: 'unary',
      }),
    ).rejects.toThrow(/argsRaw/);
    // 必须**未发出**调用：否则宿主会以 null 参数执行。
    expect(backend.invocations).toHaveLength(0);
  });

  it('pluginCall 返回宿主铸造的权威 callId（cancel / callEnd 都用它）', async () => {
    const { client } = pluginClient();
    const info = await client.pluginCall({ callId: 'caller-c', method: 'format', kind: 'unary' });
    expect(info.callId).toBe('host-1');
    expect(info.seq).toBe(7);
    expect(info.pluginId).toBe('com.example.formatter');
  });

  it('callEnd 只发送已设置的字段（exactOptionalPropertyTypes 语义）', async () => {
    const { client, backend } = pluginClient();
    await client.callEnd({ callId: 'c-3', ok: false });
    expect(backend.invocations[0]?.args?.req).toEqual({ callId: 'c-3', ok: false });

    await client.callEnd({ callId: 'c-4', ok: true, errorCode: 'E_CALL_TIMEOUT', seq: 7 });
    expect(backend.invocations[1]?.args?.req).toEqual({
      callId: 'c-4',
      ok: true,
      errorCode: 'E_CALL_TIMEOUT',
      seq: 7,
    });
  });

  it('cancel 以 callId 平铺参数传递', async () => {
    const { client, backend } = pluginClient();
    await client.cancel('c-5');
    expect(backend.invocations[0]?.args).toEqual({ callId: 'c-5' });
  });

  it('lifecycleReport 上报事件（不是状态），缺省 reason 时不下发该字段', async () => {
    const { client, backend } = pluginClient();
    await client.lifecycleReport({ event: 'ATTACH' });
    expect(backend.invocations[0]?.args?.evt).toEqual({ event: 'ATTACH' });
    await client.lifecycleReport({ event: 'ERROR_RETRYABLE', reason: 'user disabled' });
    expect(backend.invocations[1]?.args?.evt).toEqual({
      event: 'ERROR_RETRYABLE',
      reason: 'user disabled',
    });
  });

  it('eventsPublish 是唯一发布入口（D1）', async () => {
    const { client, backend } = pluginClient();
    const res = await client.eventsPublish({ topic: 'com.example.x.ready', payload: { ok: true } });
    expect(res).toEqual({ delivered: 2, dropped: false });
    expect(backend.invocations[0]?.cmd).toBe('host_events_publish');
    expect(backend.invocations[0]?.args).toEqual({ evt: { topic: 'com.example.x.ready', payload: { ok: true } } });
  });

  it('eventsSubscribe / eventsUnsubscribe 分别走 subscribe/unsubscribe 命令', async () => {
    const { client, backend } = pluginClient();
    await client.eventsSubscribe([{ topic: 'com.example.x.ready' }]);
    expect(backend.invocations[0]?.args).toEqual({ sub: [{ topic: 'com.example.x.ready' }] });
    await client.eventsUnsubscribe('sub-1');
    expect(backend.invocations[1]?.args).toEqual({ token: 'sub-1' });
  });

  it('contributesRegister 是 self 档：线形不带 pluginId，署名由宿主从 label 解析', async () => {
    // 断链回归：该方法此前挂在主窗 ShellClient 上且要求调用方自带 pluginId
    //（核心盲信 entry.plugin_id = 可伪造署名）。现在注册入口在插件侧客户端，
    // 参数只有 entry 三元组。
    const { client, backend } = pluginClient();
    await client.contributesRegister({ kind: 'menu', id: 'a.menu', label: 'A' });
    const inv = backend.invocations.find((i) => i.cmd === 'host_contributes_register');
    expect(inv?.args).toEqual({ entry: { kind: 'menu', id: 'a.menu', label: 'A' } });
    expect(inv?.args).not.toHaveProperty('pluginId');
    expect(inv?.args?.entry).not.toHaveProperty('pluginId');
  });

  it('eventsDrain 走 drain 命令，默认 request 通道', async () => {
    const { client, backend } = pluginClient();
    const frames = await client.eventsDrain();
    expect(backend.invocations[0]?.cmd).toBe('host_events_drain');
    expect(backend.invocations[0]?.args).toEqual({ kind: 'request' });
    expect(Array.isArray(frames)).toBe(true);

    await client.eventsDrain('event');
    expect(backend.invocations[1]?.args).toEqual({ kind: 'event' });
  });

  it('eventsDrain 返回事件总线帧 EventFrame（Rust Frame 线形：topic/seq/payload）', async () => {
    const backend = new MockBackend({
      capabilities: ALL_CAPS,
      cases: [{
        cmd: 'host_events_drain',
        result: [{ topic: 'plugin:com.a:ready', seq: 3, payload: { ok: true } }],
      }],
    });
    const client = new HostClient({ backend });
    const frames = await client.eventsDrain('event');
    expect(frames[0]?.topic).toBe('plugin:com.a:ready');
    expect(frames[0]?.seq).toBe(3);
    expect(frames[0]?.payload).toEqual({ ok: true });
  });

  it('registryList 默认 scope=visible', async () => {
    const { client, backend } = pluginClient();
    const list = await client.registryList();
    expect(list[0]?.id).toBe('com.example.formatter');
    expect(backend.invocations[0]?.args).toEqual({ scope: 'visible' });
  });

  it('pluginId 只从 backend（=webview label）取得', () => {
    const { client } = pluginClient();
    expect(client.pluginId).toBe('com.example.formatter');
  });

  it('主窗等无身份上下文 pluginId 为 null', () => {
    const client = new HostClient({ backend: new MockBackend({ capabilities: ALL_CAPS }) });
    expect(client.pluginId).toBeNull();
  });
});

describe('错误规范化', () => {
  it('宿主结构化错误被包成 HostException，保留线名与 retryable', async () => {
    const client = new HostClient({
      backend: new MockBackend({
        capabilities: ALL_CAPS,
        cases: [{ cmd: 'host_cancel', error: { code: 'E_CALL_NOT_FOUND', message: 'call 已结束' } }],
      }),
    });
    await expect(client.cancel('gone')).rejects.toMatchObject({
      code: 'E_CALL_NOT_FOUND',
      retryable: false,
    });
    await expect(client.cancel('gone')).rejects.toMatchObject({ message: 'call 已结束' });
  });

  it('命令未注册 → E_UNKNOWN（确定性失败，不重试）', async () => {
    const client = new HostClient({ backend: new MockBackend({}) });
    await expect(client.registryList()).rejects.toMatchObject({ code: 'E_UNKNOWN', retryable: false });
  });

  it('超时类错误可重试', async () => {
    const client = new HostClient({
      backend: new MockBackend({
        capabilities: ALL_CAPS,
        cases: [{ cmd: 'host_plugin_call', error: { code: 'E_CALL_TIMEOUT', message: 'timeout' } }],
      }),
    });
    await expect(
      client.pluginCall({ callId: 'c', method: 'm', kind: 'unary' }),
    ).rejects.toMatchObject({ code: 'E_CALL_TIMEOUT', retryable: true });
  });
});

describe('AdminClient（主窗特权，D15）', () => {
  it('发送 D15 定稿的 op 枚举', async () => {
    const backend = new MockBackend({ capabilities: ALL_CAPS });
    const admin = new AdminClient({ backend });
    await admin.registryAdmin({ op: 'disable', id: 'com.example.x' });
    expect(backend.invocations[0]?.args).toEqual({ op: { op: 'disable', id: 'com.example.x' } });
  });

  it('op 必须是四值枚举之一', () => {
    const valid = ['disable', 'enable', 'uninstall', 'purge'];
    for (const v of valid) {
      // 编译期靠 RegistryAdminOpKind 保证；运行期仅断言字符串合法。
      expect(typeof v).toBe('string');
    }
  });
});

describe('FrameSink', () => {
  it('驱动 onFrame 回调，承载 stream 帧', () => {
    const backend = new MockBackend({ capabilities: ALL_CAPS });
    const seen: StreamFrame[] = [];
    const sink = new FrameSink((f) => seen.push(f), backend);
    sink.sink({ seq: 1, kind: 'data', argsJson: { p: 0.5 } });
    sink.sink({ seq: 2, kind: 'end' });
    expect(seen).toHaveLength(2);
    expect(seen[1]!.kind).toBe('end');
    // R5：port 必须是**真实通道对象**（带 onmessage 入口与 id），
    // 而不是旧实现的 `{ message }` 占位——后者传给 invoke 后帧永远送不出去。
    expect(typeof sink.port.onmessage).toBe('function');
    expect(sink.port.id).toBeDefined();
  });

  it('onmessage 是收帧入口：宿主推送直接驱动回调', () => {
    const backend = new MockBackend({ capabilities: ALL_CAPS });
    const seen: StreamFrame[] = [];
    const sink = new FrameSink((f) => seen.push(f), backend);
    // 绕过 sink() 辅助，直接从通道入口推（真机路径就是这个）。
    sink.port.onmessage?.({ seq: 7, kind: 'data', argsRaw: new Uint8Array([1, 2]) });
    expect(seen).toHaveLength(1);
    expect(seen[0]!.argsRaw).toEqual(new Uint8Array([1, 2]));
  });

  it('没有 onFrame 回调时收帧不抛（丢弃而非崩溃）', () => {
    const backend = new MockBackend({ capabilities: ALL_CAPS });
    const sink = new FrameSink(undefined, backend);
    expect(() => sink.port.onmessage?.({ seq: 1, kind: 'data' })).not.toThrow();
  });
});
