// @tauron/app-plugin-sdk — onSettingsChanged 接消息面（§33 R2-4 / W6）。
//
// 锁定的行为：
// 1. 声明 onSettingsChanged 的插件，激活时真的向宿主订阅设置变更镜像 topic
//    （此前这个钩子只在类型里存在，没有任何投递路径——写了也永远不会被调用）；
// 2. 投递只覆盖本插件命名空间的键，他人键/宿主键/畸形帧一律不触发回调；
// 3. 未获主窗批准（订阅失败）时静默降级，激活不得因此失败；
// 4. 被拒订阅按**有界**退避阶梯重试——批准晚于激活是正常时序，不能要求重启插件；
//    阶梯走完后放弃，不留无限轮询。

import { describe, expect, it, vi } from 'vitest';
import { HostClient, MockBackend } from '@tauron/host';
import { createPluginContext } from './context.js';
import { createPlugin, HOST_SETTINGS_CHANGED_TOPIC } from './createPlugin.js';
import type { PluginDefinition } from './types.js';

const PLUGIN_ID = 'com.example.test';

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

function makeHarness(): {
  backend: MockBackend;
  ctx: ReturnType<typeof createPluginContext>;
} {
  const backend = new MockBackend({
    capabilities: ['host_events_subscribe', 'host_events_unsubscribe'],
    pluginId: PLUGIN_ID,
    cases: [
      { cmd: 'host_events_subscribe', result: { token: 'sub-1', selectors: [] } },
      { cmd: 'host_events_unsubscribe', result: undefined },
    ],
  });
  const host = new HostClient({ backend });
  return { backend, ctx: createPluginContext(PLUGIN_ID, host) };
}

const subscribedTopics = (backend: MockBackend): unknown[] =>
  backend.invocations
    .filter((i) => i.cmd === 'host_events_subscribe')
    .map((i) => (i.args as { sub?: Array<{ topic: string }> })?.sub?.[0]?.topic);

const base: PluginDefinition = {
  id: PLUGIN_ID,
  name: 'Settings Watch Plugin',
  version: '1.0.0',
};

describe('onSettingsChanged 投递链', () => {
  it('声明钩子即订阅宿主设置变更镜像 topic', async () => {
    const { backend, ctx } = makeHarness();
    const plugin = createPlugin({ ...base, onSettingsChanged: () => {} });

    await plugin.activate(ctx);
    await tick();

    expect(subscribedTopics(backend)).toEqual([HOST_SETTINGS_CHANGED_TOPIC]);

    await plugin.deactivate(ctx);
    await tick();
    expect(backend.invocations.filter((i) => i.cmd === 'host_events_unsubscribe').length).toBe(1);
  });

  it('未声明钩子不订阅（不白拿一份跨进程可见性）', async () => {
    const { backend, ctx } = makeHarness();
    const plugin = createPlugin({ ...base });

    await plugin.activate(ctx);
    await tick();

    expect(subscribedTopics(backend)).toEqual([]);
    await plugin.deactivate(ctx);
  });

  it('本插件键投递给钩子；他人键、宿主键与畸形帧不触发', async () => {
    const { ctx } = makeHarness();
    const seen: Array<Record<string, unknown>> = [];
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: (settings) => {
        seen.push(settings);
      },
    });

    await plugin.activate(ctx);
    await tick();

    const frame = (key: unknown, value: unknown) => ({ key, value, source: 'plugin', revision: 1 });
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame(`plugin:${PLUGIN_ID}.theme`, 'cobalt'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame('plugin:com.other.theme', 'x'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame(`plugin:${PLUGIN_ID}x.y`, 'x'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, frame('host.theme', 'x'));
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { value: 'no key' });
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, null);

    expect(seen).toEqual([{ [`plugin:${PLUGIN_ID}.theme`]: 'cobalt' }]);

    await plugin.deactivate(ctx);
  });

  it('命名空间根键（等于 plugin:<id>）也算本插件', async () => {
    const { ctx } = makeHarness();
    const seen: unknown[] = [];
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: (s) => {
        seen.push(s);
      },
    });

    await plugin.activate(ctx);
    await tick();
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { key: `plugin:${PLUGIN_ID}`, value: 1 });

    expect(seen).toEqual([{ [`plugin:${PLUGIN_ID}`]: 1 }]);

    await plugin.deactivate(ctx);
  });

  it('钩子抛错（同步或 async rejection）不杀死订阅，也不冒泡给宿主', async () => {
    const { ctx } = makeHarness();
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: () => {
        throw new Error('sync boom');
      },
    });
    await plugin.activate(ctx);
    await tick();

    expect(() =>
      ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { key: `plugin:${PLUGIN_ID}.a`, value: 1 }),
    ).not.toThrow();

    const asyncPlugin = createPlugin({
      ...base,
      onSettingsChanged: async () => {
        throw new Error('async boom');
      },
    });
    await asyncPlugin.activate(ctx);
    await tick();
    ctx.dispatchEvent(HOST_SETTINGS_CHANGED_TOPIC, { key: `plugin:${PLUGIN_ID}.b`, value: 2 });
    await tick(); // rejection 在下一个微任务批次收口

    expect(asyncPlugin.isActive).toBe(true);
    await asyncPlugin.deactivate(ctx);
    await plugin.deactivate(ctx);
  });

  it('宿主拒绝订阅（未批准私有 topic）时降级为空操作，激活照常成功', async () => {
    // 断言落在**取件泵**上，不是 dispatchEvent：dispatchEvent 是测试/宿主主动
    // 注入的入口，本地有没有监听者都照投，证明不了「未批准就没有帧」。
    // 真机里未批准的后果是订阅拿不到 token → 泵不起跑 → `host_events_drain`
    // 一次都不会发生，钩子因此永不触发。
    const backend = new MockBackend({
      capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
      pluginId: PLUGIN_ID,
      cases: [
        {
          cmd: 'host_events_subscribe',
          error: new Error('E_AUTH_DENIED: 私有 topic 未获主窗批准'),
        },
        { cmd: 'host_events_unsubscribe', result: undefined },
        { cmd: 'host_events_drain', result: [] },
      ],
    });
    const ctx = createPluginContext(PLUGIN_ID, new HostClient({ backend }));
    const seen: unknown[] = [];
    const plugin = createPlugin({
      ...base,
      onSettingsChanged: (s) => {
        seen.push(s);
      },
    });

    await expect(plugin.activate(ctx)).resolves.toBeUndefined();
    await tick();
    await tick();

    expect(subscribedTopics(backend)).toEqual([HOST_SETTINGS_CHANGED_TOPIC]); // 确实申请过
    expect(backend.invocations.filter((i) => i.cmd === 'host_events_drain')).toEqual([]); // 没起泵
    expect(seen).toEqual([]);
    expect(plugin.isActive).toBe(true);

    await plugin.deactivate(ctx);
  });
});

/** 前 `failTimes` 次 `host_events_subscribe` 被拒、之后放行的后端。 */
class LateApprovalBackend extends MockBackend {
  subscribeAttempts = 0;

  constructor(
    options: ConstructorParameters<typeof MockBackend>[0],
    private readonly failTimes: number,
  ) {
    super(options);
  }

  override invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    if (cmd === 'host_events_subscribe') {
      this.subscribeAttempts += 1;
      if (this.subscribeAttempts <= this.failTimes) {
        this.invocations.push(args === undefined ? { cmd } : { cmd, args });
        return Promise.reject(new Error('E_AUTH_DENIED: 私有 topic 未获主窗批准'));
      }
    }
    return super.invoke(cmd, args);
  }
}

const WATCH_CASES = [
  { cmd: 'host_events_subscribe', result: { token: 'sub-late', selectors: [] } },
  { cmd: 'host_events_unsubscribe', result: undefined },
  {
    cmd: 'host_events_drain',
    args: { kind: 'event' },
    result: [
      { topic: HOST_SETTINGS_CHANGED_TOPIC, payload: { key: `plugin:${PLUGIN_ID}.w`, value: 7 } },
    ],
  },
  { cmd: 'host_events_drain', args: { kind: 'request' }, result: [] },
];

describe('被拒订阅的有界退避重试（§33 R2-4 批准晚到时序）', () => {
  it('批准晚于激活：按阶梯重试后接上投递，钩子无需重启插件即可触发', async () => {
    vi.useFakeTimers();
    try {
      const backend = new LateApprovalBackend(
        {
          capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
          pluginId: PLUGIN_ID,
          cases: WATCH_CASES,
        },
        2, // 前两次申请落在批准之前
      );
      const ctx = createPluginContext(PLUGIN_ID, new HostClient({ backend }));
      const seen: unknown[] = [];
      const plugin = createPlugin({
        ...base,
        onSettingsChanged: (s) => {
          seen.push(s);
        },
      });

      await plugin.activate(ctx);
      await vi.advanceTimersByTimeAsync(0);
      expect(backend.subscribeAttempts).toBe(1);
      expect(seen).toEqual([]); // 尚未起泵

      await vi.advanceTimersByTimeAsync(1_000); // 阶梯 1：仍早于批准
      expect(backend.subscribeAttempts).toBe(2);
      await vi.advanceTimersByTimeAsync(2_000); // 阶梯 2：批准已到 → 订阅成立 → 起泵
      expect(backend.subscribeAttempts).toBe(3);

      await vi.advanceTimersByTimeAsync(0);
      expect(seen.length).toBeGreaterThan(0);
      expect(seen[0]).toEqual({ [`plugin:${PLUGIN_ID}.w`]: 7 });

      await plugin.deactivate(ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it('始终未获批准时在阶梯走完后放弃，不无限轮询', async () => {
    vi.useFakeTimers();
    try {
      const backend = new LateApprovalBackend(
        {
          capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
          pluginId: PLUGIN_ID,
          cases: WATCH_CASES,
        },
        Number.POSITIVE_INFINITY,
      );
      const ctx = createPluginContext(PLUGIN_ID, new HostClient({ backend }));
      const plugin = createPlugin({ ...base, onSettingsChanged: () => {} });

      await plugin.activate(ctx);
      await vi.advanceTimersByTimeAsync(10 * 60 * 1000); // 远超整条阶梯

      // 1 次首发 + 6 次退避重试，之后不再申请。
      expect(backend.subscribeAttempts).toBe(7);
      expect(backend.invocations.filter((i) => i.cmd === 'host_events_drain')).toEqual([]);
      expect(plugin.isActive).toBe(true);

      await plugin.deactivate(ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it('阶梯走空后重新订阅会再走一轮（不得把 topic 永久拉黑）', async () => {
    vi.useFakeTimers();
    try {
      // 前 8 次申请全部落在批准之前：1 次首发 + 6 次阶梯重试 = 7 次用尽，
      // 第 8 次来自「批准之后插件显式再订阅」，它必须成功。
      const backend = new LateApprovalBackend(
        {
          capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
          pluginId: PLUGIN_ID,
          cases: WATCH_CASES,
        },
        8,
      );
      const ctx = createPluginContext(PLUGIN_ID, new HostClient({ backend }));
      const seen: unknown[] = [];
      const plugin = createPlugin({
        ...base,
        onSettingsChanged: (s) => {
          seen.push(s);
        },
      });

      await plugin.activate(ctx);
      await vi.advanceTimersByTimeAsync(10 * 60 * 1000);
      expect(backend.subscribeAttempts).toBe(7); // 阶梯用尽，没有无限轮询
      expect(seen).toEqual([]); // 泵没起

      ctx.events.subscribe(HOST_SETTINGS_CHANGED_TOPIC, () => {});
      await vi.advanceTimersByTimeAsync(0); // 第 8 次：仍被拒 → 必须重新排一轮阶梯
      expect(backend.subscribeAttempts).toBe(8);
      await vi.advanceTimersByTimeAsync(1_000); // 第 9 次：批准已到 → 订阅成立 → 起泵
      expect(backend.subscribeAttempts).toBe(9);
      await vi.advanceTimersByTimeAsync(0);
      expect(seen.length).toBeGreaterThan(0);

      await plugin.deactivate(ctx);
    } finally {
      vi.useRealTimers();
    }
  });

  it('退订后不再重试（定时器不得复活已退订的 topic）', async () => {
    vi.useFakeTimers();
    try {
      const backend = new LateApprovalBackend(
        {
          capabilities: ['host_events_subscribe', 'host_events_unsubscribe', 'host_events_drain'],
          pluginId: PLUGIN_ID,
          cases: WATCH_CASES,
        },
        Number.POSITIVE_INFINITY,
      );
      const ctx = createPluginContext(PLUGIN_ID, new HostClient({ backend }));
      const unsubscribe = ctx.events.subscribe(HOST_SETTINGS_CHANGED_TOPIC, () => {});
      await vi.advanceTimersByTimeAsync(0);
      expect(backend.subscribeAttempts).toBe(1);

      unsubscribe();
      await vi.advanceTimersByTimeAsync(60_000);
      expect(backend.subscribeAttempts).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
