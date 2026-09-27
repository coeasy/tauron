import { describe, expect, it } from 'vitest';

import { LIFECYCLE_EVENTS, LIFECYCLE_STATES, PLUGIN_REPORTABLE_EVENTS } from './lifecycle.js';

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

describe('插件可自报事件白名单（self 档 `host_lifecycle_report`）', () => {
  it('是完整事件枚举的严格子集（等于全量就等于没设闸）', () => {
    expect(PLUGIN_REPORTABLE_EVENTS.length).toBeLessThan(LIFECYCLE_EVENTS.length);
    for (const e of PLUGIN_REPORTABLE_EVENTS) {
      expect(LIFECYCLE_EVENTS as readonly string[]).toContain(e);
    }
  });

  it('排除全部越权事件（放行 / 解禁 / 安装结果 / 终态迁移）', () => {
    // ENABLE / TRIAL_ENABLE / SAFEMODE_EXIT 是 `Disabled → Enabled` 的三条出边：
    // 可自报就等于插件能自行解禁。UNINSTALL / PURGE 进终态后槽位再也收不回。
    const forbidden = [
      'ENABLE',
      'DISABLE',
      'TRIAL_ENABLE',
      'SAFEMODE_ENTER',
      'SAFEMODE_EXIT',
      'INSTALL_START',
      'INSTALL_OK',
      'INSTALL_FAIL',
      'UNINSTALL',
      'PURGE',
    ];
    for (const e of forbidden) {
      expect(PLUGIN_REPORTABLE_EVENTS as readonly string[]).not.toContain(e);
    }
  });

  it('保留宿主无法外部观测的运行时事实', () => {
    for (const required of [
      'ATTACH',
      'DETACH',
      'ERROR_RETRYABLE',
      'ERROR_FATAL',
      'RETRY_OK',
      'RETRY_EXHAUSTED',
      'HEALTH_OK',
      'RUNTIME_CRASH',
    ] as const) {
      expect(PLUGIN_REPORTABLE_EVENTS).toContain(required);
    }
  });
});
