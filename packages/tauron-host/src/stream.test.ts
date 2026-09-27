// ──────────────────────────────────────────────────────────────────────────
// 流式帧往返（R5 / P0-1）。
//
// 这一组测试的意义：R5 之前「流式」有类型、有词表、有文档，**却没有通路**——
// `host_plugin_call` 的 `channel` 被当成不透明 JSON 收下、写完就丢，前端表现为
// 「onFrame 永不触发、也不报错」。因此这里断言的不是"函数被调用了"，而是
// **帧真的到了接收方的回调里**，且经过的是真实 `channel` 对象。
// ──────────────────────────────────────────────────────────────────────────
import { describe, expect, it, vi } from 'vitest';

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

/** 让已排队的 Promise 链彻底落定（关流 → `sink.dispose()` 的 then 链有好几跳）。 */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
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
    expect(backend.invocations.filter((i) => i.cmd === 'host_events_unsubscribe').length).toBe(
      before,
    );
  });

  it('单个订阅者回调抛错，不得吞掉同批其余帧（逐订阅者隔离）', async () => {
    // 帧在 `eventsDrain` 时就已从宿主队列取走（拉取模型，取走即删）：若一个订阅者
    // 抛错把整拍打断，同批其余帧与其余订阅者就**永久**收不到，且没有任何出口可查。
    const { host } = setup([
      { topic: 'plugin:com.example.streamer:p1', seq: 1, payload: { n: 1 } },
      { topic: 'plugin:com.example.streamer:p2', seq: 2, payload: { n: 2 } },
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
    const boom = vi.spyOn(console, 'error').mockImplementation(() => {});

    const seenP1: unknown[] = [];
    const seenP2: unknown[] = [];
    // 同一 topic 两个订阅者：先入者为抛错者，后者仍须收到。
    const offThrowing = await rpc.subscribe('plugin:com.example.streamer:p1', () => {
      throw new Error('订阅者抛错');
    });
    const offOk = await rpc.subscribe('plugin:com.example.streamer:p1', (p) => seenP1.push(p));
    // 另一个 topic：抛错后**同一拍内**仍须收到自己的帧。
    const offOther = await rpc.subscribe('plugin:com.example.streamer:p2', (p) => seenP2.push(p));

    expect(ticks).toHaveLength(1);
    ticks[0]!();
    await flush();

    expect(seenP1).toEqual([{ n: 1 }]);
    expect(seenP2).toEqual([{ n: 2 }]);
    expect(boom).toHaveBeenCalled();
    boom.mockRestore();

    offThrowing();
    offOk();
    offOther();
  });

  it('宿主订阅失败后重试仍收得到帧（失败不得留下空登记）', async () => {
    // 旧实现先 `handlers.set(topic, new Set())` 再 await 宿主订阅：订阅失败会留下
    // 一个空 `Set`，重试时 `handlers.get(topic)` 命中它、**跳过宿主订阅**，
    // 订阅者从此静默收不到帧（"点了没反应且无处可查"），且该空 Set 再无回收路径。
    let attempts = 0;
    const client = {
      eventsSubscribe: async () => {
        attempts += 1;
        if (attempts === 1) throw new Error('E_ACL_DENIED: 订阅被拒');
        return { token: 'tok-retry' };
      },
      eventsDrain: async () => [{ topic: 'plugin:p:ping', seq: 1, payload: { n: 1 } }],
      eventsUnsubscribe: async () => {},
      eventsPublish: async () => ({}),
    } as unknown as HostClient;
    const ticks: Array<() => void> = [];
    const rpc = toHostRpc(client, {
      scheduler: {
        every: (_ms, tick) => {
          ticks.push(tick);
          return () => {};
        },
      },
    });

    await expect(rpc.subscribe('plugin:p:ping', () => {})).rejects.toThrow(/订阅被拒/);

    const seen: unknown[] = [];
    const off = await rpc.subscribe('plugin:p:ping', (p) => seen.push(p));
    // 关键判据：重试**真的重发了**宿主订阅（旧实现只发一次 → attempts 会是 1）。
    expect(attempts).toBe(2);
    expect(ticks).toHaveLength(1);
    ticks[0]!();
    await flush();
    expect(seen).toEqual([{ n: 1 }]);
    off();
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

  // ── 前端写帧出口（此前 `host_stream_write` 在 TS SDK 里没有入口）────────
  //
  // Rust 侧 `host_stream_write` 一直注册着，`pluginCall` 的错误文案甚至拿它当
  // "已接线"的替代路径来推荐；但 TS 侧没有任何方法能调到它——开流之后既拿不到
  // `streamId` 也没有写帧出口，`open → write → close` 在前端是断的。

  it('openStreamHandle：open → write → close 全程可达，且拿到宿主铸造的 streamId', async () => {
    const { backend, host } = setup();
    const seen: StreamFrame[] = [];
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      (f) => seen.push(f),
    );

    const handle = host.openStreamHandle(pending.callId, () => {});
    const streamId = await handle.ready;
    expect(typeof streamId).toBe('string');
    expect(streamId.length).toBeGreaterThan(0);

    const frame = await handle.write({ argsJson: { hello: 'world' } });
    expect(frame.seq).toBe(1);
    expect(frame.kind).toBe('data');
    expect(frame.argsJson).toEqual({ hello: 'world' });

    handle.close();
    await flush();
    // 关流走的是 `host_stream_close`，请求里必须带刚拿到的 streamId。
    const closeCalls = backend.invocations.filter((c) => c.cmd === 'host_stream_close');
    expect(closeCalls).toHaveLength(1);
    expect(closeCalls[0]!.args).toMatchObject({ req: { streamId, kind: 'end' } });
  });

  it('openStreamHandle：开流落定**之前** close() 也必须补发关流（否则宿主侧流泄漏）', async () => {
    // 回归锁：旧实现用一个 `closed` 标志同时表示「已请求关闭」与「关闭已送出」。
    // 于是 close() 因 `streamId` 还是 null 而不发命令，随后开流落定时又因
    // `closed` 已为 true 而提前返回——`host_stream_close` **永远不会发出**，
    // 宿主侧那条流没有任何人去关。
    const { backend, host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );

    const handle = host.openStreamHandle(pending.callId, () => {});
    handle.close(); // 还没 ready 就关（例如开流后立刻取消）
    const streamId = await handle.ready;
    await flush();

    const closeCalls = backend.invocations.filter((c) => c.cmd === 'host_stream_close');
    expect(closeCalls).toHaveLength(1);
    expect(closeCalls[0]!.args).toMatchObject({ req: { streamId, kind: 'end' } });

    // 幂等：再关不重复发。
    handle.close();
    await flush();
    expect(backend.invocations.filter((c) => c.cmd === 'host_stream_close')).toHaveLength(1);
  });

  it('openStreamHandle：关流成功后释放帧载体（进程内传输的通道不得只增不减）', async () => {
    // `FrameSink` 每次开流都向传输层要一个通道，而进程内传输（MemoryTransport /
    // MockBackend）把通道存在自己的 Map 里、`onmessage` 闭包又强引用整条流的状态。
    // 不释放就是每次开流漏一个不可回收的对象图。流的终态由**关流成功**定义，
    // 因此那是唯一可证明安全的释放点。
    const { backend, host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );
    // `pluginCall` 自己那个通道已知不释放（见 host.ts 的诚实标注），故以它为基线。
    const baseline = backend.openChannelCount;

    const handle = host.openStreamHandle(pending.callId, () => {});
    await handle.ready;
    expect(backend.openChannelCount, '开流必须真的向传输层要了一个通道').toBe(baseline + 1);

    handle.close();
    await settle();
    expect(backend.openChannelCount, '关流成功后通道必须被释放').toBe(baseline);
  });

  it('openStreamHandle：开流失败时也释放帧载体（失败路径不得漏通道）', async () => {
    // 让开流命令本身不可用（真机上等价于宿主没注册 / 构建没开这个 feature）。
    const backend = new MockBackend({
      capabilities: CAPS.filter((c) => c !== 'host_stream_open'),
      pluginId: 'com.example.streamer',
    });
    const host = new HostClient({ backend });
    const baseline = backend.openChannelCount;

    const handle = host.openStreamHandle('c-unknown', () => {});
    await expect(handle.ready).rejects.toThrow(/command not found/);
    await settle();

    expect(backend.openChannelCount, '开流失败 → 流从未存在 → 通道必须释放').toBe(baseline);
  });

  it('writeStream：二进制帧经 argsRaw 直达，不经 base64', async () => {
    const { backend, host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );
    const { streamId } = await openStream(backend, pending.callId);

    const bytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47]);
    await host.writeStream(streamId, { argsRaw: bytes });

    const writeCall = backend.invocations.find((c) => c.cmd === 'host_stream_write');
    expect(writeCall).toBeDefined();
    const req = (writeCall!.args as { req: { argsRaw?: Uint8Array } }).req;
    expect(req.argsRaw).toBeInstanceOf(Uint8Array);
    expect(Array.from(req.argsRaw!)).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it('已关闭的句柄再写帧 → 立即失败（不静默丢弃）', async () => {
    const { host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );
    const handle = host.openStreamHandle(pending.callId, () => {});
    await handle.ready;
    handle.close();
    await expect(handle.write({ argsJson: 1 })).rejects.toThrow(/流已关闭/);
  });

  it('openStream 简写仍只返回退订函数（行为未变）', async () => {
    const { host } = setup();
    const pending = await host.pluginCall(
      { callId: 'caller-c', method: 'fmt', kind: 'stream' },
      () => {},
    );
    const off = host.openStream(pending.callId, () => {});
    expect(typeof off).toBe('function');
    off();
    off(); // 幂等
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
