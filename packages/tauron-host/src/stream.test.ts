// ──────────────────────────────────────────────────────────────────────────
// 流式帧往返（R5 / P0-1）。
//
// 这一组测试的意义：R5 之前「流式」有类型、有词表、有文档，**却没有通路**——
// `host_plugin_call` 的 `channel` 被当成不透明 JSON 收下、写完就丢，前端表现为
// 「onFrame 永不触发、也不报错」。因此这里断言的不是"函数被调用了"，而是
// **帧真的到了接收方的回调里**，且经过的是真实 `channel` 对象。
// ──────────────────────────────────────────────────────────────────────────
import { describe, expect, it } from 'vitest';

import { MockBackend } from './backend.js';
import { HostClient } from './host.js';
import { toHostRpc } from './rpc.js';
import { isStreamFrame, parseStreamKind } from './stream.js';
import type { StreamFrame } from './stream.js';

const CAPS = [
  'host_plugin_call',
  'host_stream_open',
  'host_stream_write',
  'host_stream_close',
  'host_events_publish',
  'host_events_subscribe',
  'host_events_drain',
  'host_events_unsubscribe',
];

const CALL_ID = 'host-call-1';

function setup(drainFrames: unknown[] = []) {
  const backend = new MockBackend({
    capabilities: CAPS,
    pluginId: 'com.example.streamer',
    cases: [
      {
        cmd: 'host_plugin_call',
        result: {
          callId: CALL_ID,
          pluginId: 'com.example.streamer',
          cmd: 'fmt',
          args: null,
          seq: 1,
          createdAt: 0,
          expiresAt: 0,
        },
      },
      { cmd: 'host_events_drain', result: drainFrames },
      {
        cmd: 'host_events_subscribe',
        result: { token: 'tok-1', selectors: [{ topic: 'plugin:com.example.streamer:ping' }] },
      },
    ],
  });
  return { backend, host: new HostClient({ backend }) };
}

/** 宿主侧开流。 */
async function openStream(backend: MockBackend, callId: string) {
  return backend.invoke<{ streamId: string; callId: string }>('host_stream_open', {
    req: { callId },
  });
}

/** 宿主侧写一帧 data（真机里由 C/D 后端经 `host_stream_write` 发出）。 */
async function writeFrame(
  backend: MockBackend,
  streamId: string,
  body: { argsJson?: unknown; argsRaw?: Uint8Array } = {},
): Promise<StreamFrame> {
  return backend.invoke<StreamFrame>('host_stream_write', {
    req: { streamId, ...body },
  });
}

/** 让挂起的微任务跑完（`openStream` 的开流是异步的）。 */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe('流式帧往返（R5 / P0-1）', () => {
  it('写 3 帧 → 收 3 帧 + end，seq 由宿主铸且连续', async () => {
    const { backend, host } = setup();
    const seen: StreamFrame[] = [];
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      (frame) => seen.push(frame),
    );
    const { streamId } = await openStream(backend, pending.callId);

    for (let i = 1; i <= 3; i += 1) {
      const frame = await writeFrame(backend, streamId, { argsJson: { i } });
      expect(frame.seq).toBe(i);
      expect(frame.kind).toBe('data');
    }
    const end = await backend.invoke<StreamFrame>('host_stream_close', {
      req: { streamId, kind: 'end' },
    });
    expect(end.seq).toBe(4);

    expect(seen.map((f) => f.seq)).toEqual([1, 2, 3, 4]);
    expect(seen.map((f) => f.kind)).toEqual(['data', 'data', 'data', 'end']);
    expect(seen[1]!.argsJson).toEqual({ i: 2 });
    expect(seen.every(isStreamFrame)).toBe(true);
  });

  it('传入的必须是真实 channel 对象（旧实现传的是 FrameSink 自己）', async () => {
    const { backend, host } = setup();
    await host.pluginCall({ callId: 'caller-c', method: 'fmt', kind: 'stream' }, () => {});
    const channel = backend.invocations[0]!.args!.channel as Record<string, unknown>;
    // 真机里 Rust 把这个对象解析成 Tauri Channel（`__CHANNEL__:<id>`）：
    // 必须有 `onmessage`（收帧入口）与 `id`（通道标识）。
    expect(typeof channel.onmessage).toBe('function');
    expect(channel.id).toBeDefined();
    // 旧形状（只暴露 message）会让 Rust 侧收到一个普通对象，帧永远送不出去。
    expect(channel).not.toHaveProperty('message');
  });

  it('二进制载荷经 argsRaw 直达，不经 base64', async () => {
    const { backend, host } = setup();
    const seen: StreamFrame[] = [];
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      (f) => seen.push(f),
    );
    const { streamId } = await openStream(backend, pending.callId);

    const bytes = new Uint8Array([0, 159, 146, 150, 255]);
    await writeFrame(backend, streamId, { argsRaw: bytes });

    expect(seen).toHaveLength(1);
    expect(seen[0]!.argsRaw).toEqual(bytes);
    expect(seen[0]!.argsJson).toBeUndefined();
  });

  it('终帧后句柄失效：再写/再关都是 E_CALL_NOT_FOUND', async () => {
    const { backend, host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );
    const { streamId } = await openStream(backend, pending.callId);
    await backend.invoke('host_stream_close', { req: { streamId, kind: 'error' } });

    await expect(writeFrame(backend, streamId)).rejects.toThrow(/E_CALL_NOT_FOUND/);
    await expect(
      backend.invoke('host_stream_close', { req: { streamId, kind: 'end' } }),
    ).rejects.toThrow(/E_CALL_NOT_FOUND/);
  });

  it('关流只接受终帧种类：data 与拼错的 done 都被拒', async () => {
    const { backend, host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );
    const { streamId } = await openStream(backend, pending.callId);
    await expect(
      backend.invoke('host_stream_close', { req: { streamId, kind: 'data' } }),
    ).rejects.toThrow(/E_INVALID_MANIFEST/);
    await expect(
      backend.invoke('host_stream_close', { req: { streamId, kind: 'done' } }),
    ).rejects.toThrow(/E_INVALID_MANIFEST/);
  });

  it('没有载体的调用开流即失败（不得留下帧进虚空的流）', async () => {
    const { backend } = setup();
    await expect(openStream(backend, 'c-unknown')).rejects.toThrow(/没有帧载体/);
  });

  it('openStream：换载体后旧订阅者不再收帧，退订即关流', async () => {
    const { backend, host } = setup();
    const first: StreamFrame[] = [];
    const second: StreamFrame[] = [];
    const offFirst = host.openStream(CALL_ID, (f) => first.push(f));
    await flush();

    // 第二个订阅者接管载体（换 Channel）：后到者更准确，旧通道不再收到帧
    // （前端重连后新建 Channel 就属这种情形）。
    const offSecond = host.openStream(CALL_ID, (f) => second.push(f));
    await flush();

    const { streamId } = await openStream(backend, CALL_ID);
    await writeFrame(backend, streamId, { argsJson: { n: 1 } });
    expect(second).toHaveLength(1);
    expect(first).toHaveLength(0);

    offFirst();
    offSecond();
    expect(backend.invocations.filter((i) => i.cmd === 'host_stream_close').length).toBeGreaterThan(
      0,
    );
  });
});

describe('HostRpc（R5）', () => {
  it('request / event / subscribe 走同一条面', async () => {
    const { backend, host } = setup([
      { topic: 'plugin:com.example.streamer:ping', seq: 1, payload: { n: 1 } },
    ]);
    const ticks: Array<() => void> = [];
    const rpc = toHostRpc(host, {
      scheduler: {
        every: (_ms, tick) => {
          ticks.push(tick);
          return () => {};
        },
      },
    });

    await expect(rpc.request('host_unknown_cmd')).rejects.toThrow(/command not found/);

    await rpc.event('plugin:com.example.streamer:ping', { n: 1 });
    expect(backend.invocations.some((i) => i.cmd === 'host_events_publish')).toBe(true);

    const seen: unknown[] = [];
    const off = await rpc.subscribe('plugin:com.example.streamer:ping', (p) => seen.push(p));
    expect(backend.invocations.some((i) => i.cmd === 'host_events_subscribe')).toBe(true);

    // 取件泵一拍：帧按 topic 分发给订阅者
    expect(ticks).toHaveLength(1);
    ticks[0]!();
    await flush();
    expect(seen).toEqual([{ n: 1 }]);

    off();
    expect(backend.invocations.some((i) => i.cmd === 'host_events_unsubscribe')).toBe(true);
    // 幂等：重复退订不得再发一次退订
    const before = backend.invocations.filter((i) => i.cmd === 'host_events_unsubscribe').length;
    off();
    expect(
      backend.invocations.filter((i) => i.cmd === 'host_events_unsubscribe').length,
    ).toBe(before);
  });

  it('stream 经 HostRpc 与经 HostClient 行为一致（同一调用序列）', async () => {
    const direct = setup();
    const viaRpc = setup();

    // 直接路径
    const directSeen: StreamFrame[] = [];
    const directPending = await direct.host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      (f) => directSeen.push(f),
    );
    const directStream = await openStream(direct.backend, directPending.callId);
    await writeFrame(direct.backend, directStream.streamId, { argsJson: { i: 1 } });

    // HostRpc 的 stream 路径
    const rpc = toHostRpc(viaRpc.host, { scheduler: { every: () => () => {} } });
    const rpcSeen: StreamFrame[] = [];
    const off = rpc.stream(CALL_ID, (f) => rpcSeen.push(f));
    await flush();
    const rpcStream = await openStream(viaRpc.backend, CALL_ID);
    await writeFrame(viaRpc.backend, rpcStream.streamId, { argsJson: { i: 1 } });

    expect(rpcSeen.map((f) => f.argsJson)).toEqual(directSeen.map((f) => f.argsJson));
    off();
  });

  it('帧种类词表闭集且大小写敏感', () => {
    expect(parseStreamKind('data')).toBe('data');
    expect(parseStreamKind('end')).toBe('end');
    expect(parseStreamKind('error')).toBe('error');
    expect(parseStreamKind('Data')).toBeNull();
    expect(parseStreamKind('done')).toBeNull();
    expect(parseStreamKind('')).toBeNull();
    expect(parseStreamKind(1)).toBeNull();
  });
});
