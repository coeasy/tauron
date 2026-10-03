// ──────────────────────────────────────────────────────────────────────────
// HostRpc 取件泵（R5 / A102 接收端消费点）。
//
// 盯的是「宿主写进帧里的顺序元数据，取件泵有没有真的用起来」：泵是插件侧唯一的
// `host_events_drain` 出口，不在这里判定，A102 的 `seq`/`sender`/`receiver` 就
// 只是被传输、被类型化，却没人读的字段。
// ──────────────────────────────────────────────────────────────────────────
import { describe, expect, it } from 'vitest';

import { HostClient } from './host.js';
import { toHostRpc } from './rpc.js';
import type { Backend, Principal, Unlisten } from './backend.js';
import type { EventFrame, EventOrderingViolation, JsonValue } from './events.js';

const CAPS = [
  'host_events_publish',
  'host_events_subscribe',
  'host_events_drain',
  'host_events_unsubscribe',
];

const TOPIC = 'plugin:com.example.pump:tick';

function frame(seq: number, payload: JsonValue): EventFrame {
  return { topic: TOPIC, seq, payload, sender: 'com.example.pump', receiver: 'com.example.app' };
}

/**
 * 一次 `invoke` 消费一批预置帧（取尽后返回空批）——这样测试能精确控制「第几拍收到第几帧」，
 * 顺序判定才可复现。
 */
function batchedBackend(batches: EventFrame[][]): Backend {
  let index = 0;
  return {
    async invoke<T>(cmd: string): Promise<T> {
      if (cmd === 'host_events_drain') return (batches[index++] ?? []) as T;
      if (cmd === 'host_events_subscribe') return { token: 'sub-1', selectors: [] } as T;
      return undefined as T;
    },
    async listen(): Promise<Unlisten> {
      return () => {};
    },
    channel() {
      return { id: 'ch-1', onmessage: null };
    },
    principal(): Principal {
      return { kind: 'plugin', id: 'com.example.app' };
    },
    pluginId() {
      return 'com.example.app';
    },
    capabilities() {
      return new Set(CAPS);
    },
  };
}

/** 一台把节拍交还给测试的适配器：`ticks[i]()` 就是「泵跑一拍」。 */
function scripted(batches: EventFrame[][], violations: EventOrderingViolation[]) {
  const ticks: Array<() => void> = [];
  const rpc = toHostRpc(new HostClient({ backend: batchedBackend(batches) }), {
    scheduler: {
      every: (_ms, tick) => {
        ticks.push(tick);
        return () => {
          const index = ticks.indexOf(tick);
          if (index >= 0) ticks.splice(index, 1);
        };
      },
    },
    onOrderingViolation: (v) => violations.push(v),
  });
  return { rpc, ticks };
}

/** 等异步链路落地（subscribe 的 IPC + drain + 逐订阅者分发）。 */
async function flush(): Promise<void> {
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

describe('HostRpc 取件泵的 A102 判定', () => {
  it('重复取件判 duplicate，但载荷照常分发（丢不丢由业务幂等决定）', async () => {
    // 两拍都返回同一帧：模拟可靠通道的 at-least-once 重投。
    const violations: EventOrderingViolation[] = [];
    const { rpc, ticks } = scripted([[frame(1, { n: 1 })], [frame(1, { n: 1 })]], violations);
    const seen: unknown[] = [];
    const off = await rpc.subscribe(TOPIC, (p) => seen.push(p));

    ticks[0]!();
    await flush();
    ticks[0]!();
    await flush();

    expect(seen).toEqual([{ n: 1 }, { n: 1 }]);
    expect(violations).toEqual([
      { kind: 'duplicate', sender: 'com.example.pump', receiver: 'com.example.app', seq: 1 },
    ]);
    off();
  });

  it('丢帧判 gap 并带上期望/实际序号，随后重同步不再刷屏', async () => {
    const violations: EventOrderingViolation[] = [];
    const { rpc, ticks } = scripted(
      [[frame(1, { n: 1 })], [frame(5, { n: 5 })], [frame(6, { n: 6 })]],
      violations,
    );
    const seen: unknown[] = [];
    const off = await rpc.subscribe(TOPIC, (p) => seen.push(p));

    for (const _ of [0, 1, 2]) {
      ticks[0]!();
      await flush();
    }

    expect(violations).toEqual([
      {
        kind: 'gap',
        sender: 'com.example.pump',
        receiver: 'com.example.app',
        expected: 2,
        actual: 5,
      },
    ]);
    expect(seen).toEqual([{ n: 1 }, { n: 5 }, { n: 6 }]);
    off();
  });

  it('退订收泵会清零顺序状态：重新会话的 seq=1 不被误判重复', async () => {
    const violations: EventOrderingViolation[] = [];
    const { rpc, ticks } = scripted(
      [[frame(1, { n: 1 })], [], [frame(1, { n: 2 })], [frame(2, { n: 3 })]],
      violations,
    );
    const off = await rpc.subscribe(TOPIC, () => {});
    ticks[0]!();
    await flush();
    expect(violations).toEqual([]);

    off(); // handlers 清空 → 收泵 → EventOrderingWatcher.reset()
    const offAgain = await rpc.subscribe(TOPIC, () => {});
    ticks[ticks.length - 1]!();
    await flush();
    ticks[ticks.length - 1]!();
    await flush();
    // 没有 reset 的话，第二次会话的首帧（seq=1）会被判成 duplicate。
    expect(violations, '跨会话不得残留期望序号').toEqual([]);
    offAgain();
  });

  it('缺 A102 元数据的旧宿主帧不参与判定', async () => {
    const legacy = { topic: TOPIC, seq: 9, payload: { n: 1 } } as EventFrame;
    const violations: EventOrderingViolation[] = [];
    const { rpc, ticks } = scripted([[legacy], [legacy]], violations);
    const seen: unknown[] = [];
    const off = await rpc.subscribe(TOPIC, (p) => seen.push(p));

    ticks[0]!();
    await flush();
    ticks[0]!();
    await flush();

    expect(seen).toEqual([{ n: 1 }, { n: 1 }]);
    expect(violations, 'N-1 帧没有 sender/receiver，无从判定').toEqual([]);
    off();
  });
});
