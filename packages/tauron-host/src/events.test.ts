// ──────────────────────────────────────────────────────────────────────────
// V4 A102 接收端顺序守卫（§4.4 顺序契约的消费侧）。
//
// 这一组测试盯的是「帧的顺序字段有没有人真的用」：宿主已经为每条
// sender→receiver 流铸造单调 `seq`，接收端不判定就等于契约只写了一半——
// 重复投递与丢帧在业务侧完全不可见。判定语义必须与 Rust
// `tauron_host::OrderingTracker::observe` 对齐（差异处见 events.ts 文档）。
// ──────────────────────────────────────────────────────────────────────────
import { describe, expect, it } from 'vitest';

import { EventOrderingWatcher } from './events.js';
import type { EventFrame } from './events.js';

function frame(overrides: Partial<EventFrame> = {}): EventFrame {
  return {
    topic: 'plugin:com.example.a:tick',
    seq: 1,
    payload: { n: 1 },
    sender: 'com.example.a',
    receiver: 'com.example.b',
    ...overrides,
  };
}

describe('EventOrderingWatcher（A102 接收端）', () => {
  it('连续序号不报异常', () => {
    const watcher = new EventOrderingWatcher();
    expect(watcher.observe(frame({ seq: 1 }))).toBeNull();
    expect(watcher.observe(frame({ seq: 2 }))).toBeNull();
    expect(watcher.observe(frame({ seq: 3 }))).toBeNull();
  });

  it('重复投递判 duplicate，且不推进期望值', () => {
    const watcher = new EventOrderingWatcher();
    watcher.observe(frame({ seq: 1 }));
    watcher.observe(frame({ seq: 2 }));
    expect(watcher.observe(frame({ seq: 2 }))).toEqual({
      kind: 'duplicate',
      sender: 'com.example.a',
      receiver: 'com.example.b',
      seq: 2,
    });
    // 期望值仍是 3：再来一次 seq=2 依旧判重复，而不是被"吸收"成正常。
    expect(watcher.observe(frame({ seq: 2 }))?.kind).toBe('duplicate');
    expect(watcher.observe(frame({ seq: 3 }))).toBeNull();
  });

  it('丢帧判 gap 一次后重同步，不再毒化后续帧', () => {
    const watcher = new EventOrderingWatcher();
    watcher.observe(frame({ seq: 1 }));
    expect(watcher.observe(frame({ seq: 4 }))).toEqual({
      kind: 'gap',
      sender: 'com.example.a',
      receiver: 'com.example.b',
      expected: 2,
      actual: 4,
    });
    // 关键差异点：Rust oracle 保持严格（后续每帧都报 gap），运行视图必须重同步，
    // 否则一次丢帧把整条流变成噪音，真正的重复/倒退反而被埋掉。
    expect(watcher.observe(frame({ seq: 5 }))).toBeNull();
    expect(watcher.observe(frame({ seq: 5 }))?.kind).toBe('duplicate');
  });

  it('state revision 倒退单独判，且不回退已记录的最新值', () => {
    const watcher = new EventOrderingWatcher();
    expect(watcher.observe(frame({ seq: 1, stateRevision: 7 }))).toBeNull();
    expect(watcher.observe(frame({ seq: 2, stateRevision: 5 }))).toEqual({
      kind: 'revision-regression',
      previous: 7,
      actual: 5,
    });
    // 记录值仍是 7：seq 正常、revision 追平后才算恢复正常。
    expect(watcher.observe(frame({ seq: 3, stateRevision: 6 }))).toEqual({
      kind: 'revision-regression',
      previous: 7,
      actual: 6,
    });
    expect(watcher.observe(frame({ seq: 4, stateRevision: 8 }))).toBeNull();
  });

  it('每条 sender→receiver 流各自计数', () => {
    const watcher = new EventOrderingWatcher();
    expect(watcher.observe(frame({ seq: 3 }))).toEqual({
      kind: 'gap',
      sender: 'com.example.a',
      receiver: 'com.example.b',
      expected: 1,
      actual: 3,
    });
    // 另一条流从 1 起算，不被上面那条流的状态污染。
    expect(
      watcher.observe(frame({ seq: 1, sender: 'com.example.c', receiver: 'com.example.b' })),
    ).toBeNull();
  });

  it('缺 A102 元数据的帧（N-1 宿主）不参与判定', () => {
    const watcher = new EventOrderingWatcher();
    const legacy: EventFrame = { topic: 'plugin:com.example.a:tick', seq: 9, payload: null };
    expect(watcher.observe(legacy)).toBeNull();
    expect(watcher.observe({ ...legacy, seq: 1 })).toBeNull();
  });

  it('reset 后期望序号回到 1（宿主重启 / 重新激活）', () => {
    const watcher = new EventOrderingWatcher();
    watcher.observe(frame({ seq: 5 }));
    watcher.reset();
    expect(watcher.observe(frame({ seq: 1 }))).toBeNull();
    expect(watcher.observe(frame({ seq: 1 }))?.kind).toBe('duplicate');
  });
});
