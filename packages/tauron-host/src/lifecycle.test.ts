import { describe, expect, it } from 'vitest';

import { LIFECYCLE_EVENTS, LIFECYCLE_STATES } from './lifecycle.js';

describe('生命周期线格式词表（与 Rust `tauron_host::lifecycle` 同构）', () => {
  it('事件 18 条 / 状态 10 条（Rust 侧定稿规模）', () => {
    expect(LIFECYCLE_EVENTS).toHaveLength(18);
    expect(LIFECYCLE_STATES).toHaveLength(10);
  });

  it('事件名与状态名互不重叠（状态名不得被当作事件上报）', () => {
    const states = new Set<string>(LIFECYCLE_STATES);
    const collisions = LIFECYCLE_EVENTS.filter((e) => states.has(e));
    expect(collisions).toEqual([]);
  });

  it('全部为 SCREAMING_SNAKE_CASE 线名，且无重复', () => {
    for (const name of [...LIFECYCLE_EVENTS, ...LIFECYCLE_STATES]) {
      expect(name).toMatch(/^[A-Z][A-Z_]*$/);
    }
    expect(new Set(LIFECYCLE_EVENTS).size).toBe(LIFECYCLE_EVENTS.length);
    expect(new Set(LIFECYCLE_STATES).size).toBe(LIFECYCLE_STATES.length);
  });

  it('包含状态机关键事件（attach/enable/health 在列）', () => {
    for (const required of ['ATTACH', 'DETACH', 'ENABLE', 'DISABLE', 'HEALTH_OK'] as const) {
      expect(LIFECYCLE_EVENTS).toContain(required);
    }
  });
});
