// @tauron/app-plugin-sdk — 互操作验证（方案 R2）。
//
// 目的：证明「**同一份插件定义**」在两个 SDK 的上下文下激活时，**可观察结果
// 完全一致**——命令注册与派发结果、宿主请求往返、事件收发、设置 Tab 账本、
// 日志调用序列。
//
// 两侧对照（这是本文件唯一有意的差异，其余全部相等）：
// - app 侧：`createPluginContext(pluginId, HostClient)` → 契约形状（直接就是）；
// - legacy 侧：`createContractContext(legacyPluginContext, { pluginId })`
//   → 由 iframe 一代的 `invoke` / `emit` / `onEvent` 适配出契约形状。
//
// 「两边都返回了 ok」不算验证：下面所有断言比较的都是**具体值/序列**
// （`{ sum: 7 }`、`{ ok: true, echo: ... }`、事件载荷数组、Tab id 列表、
// 日志三元组序列），任何一侧行为分叉都会让断言失败。未接线的能力则在
// 最后一组用例里以**显式差异**断言，不假装两边一致。

import { describe, expect, it } from 'vitest';
import { HostClient, MockBackend } from '@tauron/host';
import type {
  CommandRegisterResult,
  PluginContext as SharedPluginContext,
  SettingsTabConfig,
} from '@tauron/plugin-context-contract';

import { createPluginContext } from './context.js';
import type { PluginContext as AppPluginContext } from './types.js';
import { createContractContext, type PluginContext as LegacyPluginContext } from '@tauron/plugin-sdk';

const PLUGIN_ID = 'com.example.interop';
/** 宿主命令：契约的 host.request 落到这一条（与 @tauron/host 的命令名一致）。 */
const HOST_REQUEST_CMD = 'host_plugin_call';
const HOST_REQUEST_ARGS = { req: { callId: 'c-1', method: 'sum', kind: 'tool' } };
const HOST_REQUEST_RESULT = { value: 42, source: 'host' };
/** 事件 topic（插件在 activate 里订阅）。 */
const TOPIC = 'com.example.interop.topic';
/** 设置 Tab（插件在 activate 里注册）。 */
const TAB: SettingsTabConfig = { id: 'interop-main', title: '互操作设置' };

/** 让已排队的微任务全部落地。 */
const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

// ──────────────────────────────────────────────────────────────────────────
// 同一份插件定义（两侧共用；只依赖共享契约，不依赖任何 SDK 私有成员）
// ──────────────────────────────────────────────────────────────────────────

/** 插件内部记录的一条日志。 */
interface LogEntry {
  level: 'info' | 'warn' | 'error';
  message: string;
  args: unknown[];
}

/** 插件内部记录的一次事件投递。 */
interface EventEntry {
  topic: string;
  pluginId: string;
  payload: unknown;
}

/** 插件激活后暴露给测试的可观察快照。 */
interface SharedPluginSnapshot {
  /** 插件 id（两侧必须相同，否则日志署名/事件信封就会分叉）。 */
  pluginId: string;
  /** 日志调用序列（level + message + args，含插件自己记的与库回调记的）。 */
  log: LogEntry[];
  /** 收到的 (topic, pluginId, payload) 序列。 */
  received: EventEntry[];
  /** 首次注册设置 Tab 的返回值（两侧必须逐字段相同）。 */
  initialTab: CommandRegisterResult;
  /**
   * 当前已注册命令 id 列表（读的是上下文注册表本身，不是插件自报的副本）。
   * 只在同一个上下文实例内读取；**不参与两侧深比较**，因为注册顺序由两侧
   * 各自的时间线决定——命令集合由 `commandIds` 单独断言。
   */
  commandIds: () => string[];
}

/**
 * 定义一份「共享插件」。
 *
 * 它只消费契约成员（`commands` / `events` / `settings` / `log` / `host` /
 * `pluginId`），因此同一份定义可以在两个 SDK 下激活。
 */
function createSharedPlugin(): {
  activate: (ctx: SharedPluginContext) => Promise<SharedPluginSnapshot>;
} {
  return {
    async activate(ctx: SharedPluginContext): Promise<SharedPluginSnapshot> {
      const log: LogEntry[] = [];
      const received: EventEntry[] = [];

      const record = (level: LogEntry['level'], message: string, args: unknown[]): void => {
        log.push({ level, message, args });
        ctx.log[level](message, ...args);
      };

      record('info', 'activate', [ctx.pluginId]);

      // ── 命令：注册 / 派发 / 宿主往返 / 抛错 ──
      const addResult = ctx.commands.register('interop.add', (args: { a: number; b: number }) => ({
        sum: args.a + args.b,
      }));
      record('info', 'register', ['interop.add', addResult.ok]);

      const failResult = ctx.commands.register('interop.fail', () => {
        throw new Error('E_PLUGIN_BOOM');
      });
      record('info', 'register', ['interop.fail', failResult.ok]);

      const hostResult = ctx.commands.register('interop.host', async () => {
        const answer = await ctx.host.request<typeof HOST_REQUEST_RESULT>(
          HOST_REQUEST_CMD,
          HOST_REQUEST_ARGS,
        );
        return { answer, pluginId: ctx.pluginId };
      });
      record('info', 'register', ['interop.host', hostResult.ok]);

      // 重复注册必须被拒绝（两侧同码同语义）
      const dup = ctx.commands.register('interop.add', () => ({ sum: -1 }));
      record('info', 'duplicate', [dup.ok]);
      if (!dup.ok) {
        record('warn', 'duplicate-error', [dup.error]);
      }

      // ── 事件：订阅 ──
      ctx.events.subscribe(TOPIC, (payload, event) => {
        received.push({ topic: event.topic, pluginId: event.pluginId, payload });
        log.push({ level: 'info', message: 'event', args: [event.topic, payload] });
      });
      record('info', 'subscribed', [TOPIC]);

      // ── 设置 Tab：注册 + 重复拒绝 ──
      const tab = ctx.settings.registerTab(TAB);
      record('info', 'tab', [TAB.id, tab.ok]);
      const dupTab = ctx.settings.registerTab(TAB);
      record('info', 'tab-dup', [dupTab.ok]);

      return {
        pluginId: ctx.pluginId,
        log,
        received,
        initialTab: tab,
        commandIds: () => [...ctx.commands.list()],
      };
    },
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 两侧的「宿主环境」——两侧必须给出**同样的**宿主应答与同样的投递能力
// ──────────────────────────────────────────────────────────────────────────

/** 一侧的可观察面：上下文 + 插件快照 + 该侧记录的宿主调用 + 投递入口。 */
interface SideHarness {
  ctx: SharedPluginContext;
  snapshot: SharedPluginSnapshot;
  /** 该侧到达宿主的调用序列（命令名 + 参数）。 */
  hostCalls: Array<{ cmd: string; args?: Record<string, unknown> }>;
  /** 把一条事件投递给插件订阅者（模拟宿主侧投递）。 */
  deliver: (topic: string, payload: unknown) => void;
  /** 收尾（停泵/释放订阅）。 */
  dispose: () => Promise<void>;
}

/**
 * app 侧（现代实现）：真实 `HostClient` + `MockBackend`。
 *
 * 事件投递走 `PluginEventSink.dispatchEvent`——这是本 SDK 内置取件泵在收到
 * 宿主帧后调用的**同一个入口**，因此投递路径与真机一致。
 */
async function runOnAppSdk(): Promise<SideHarness> {
  const backend = new MockBackend({
    pluginId: PLUGIN_ID,
    capabilities: [
      'host_plugin_call',
      'host_contributes_register',
      'host_events_publish',
      'host_events_subscribe',
      'host_events_unsubscribe',
      'host_events_drain',
    ],
    cases: [
      { cmd: HOST_REQUEST_CMD, result: HOST_REQUEST_RESULT },
      { cmd: 'host_contributes_register', result: undefined },
      { cmd: 'host_events_subscribe', result: { token: 'sub-interop', selectors: [] } },
      { cmd: 'host_events_unsubscribe', result: undefined },
      { cmd: 'host_events_publish', result: { delivered: 1, dropped: false } },
      // 取件泵每 100ms 取一拍；给空批，避免把「宿主帧」混进断言。
      { cmd: 'host_events_drain', result: [] },
    ],
  });

  const host = new HostClient({ backend });
  const ctx = createPluginContext(PLUGIN_ID, host);
  const snapshot = await createSharedPlugin().activate(ctx);
  await tick();

  return {
    ctx,
    snapshot,
    hostCalls: backend.invocations,
    deliver: (topic, payload) => ctx.dispatchEvent(topic, payload),
    dispose: () => ctx.disposeEvents(),
  };
}

/** 两侧共同的可观察结果（不含各自的时间线）。 */
interface ObservableOutcome {
  /** 已注册命令 id（排序后比较集合，避免注册顺序干扰）。 */
  commandIds: string[];
  /** 各命令的派发结果（值相等，不是「都 ok」）。 */
  addResult: unknown;
  failError: string;
  unknownError: string;
  hostResult: unknown;
  /** 事件载荷序列（含信封署名）。 */
  received: EventEntry[];
  /** 设置 Tab 账本（首轮注册结果 + 重复拒绝 + 注销存在者）。 */
  registeredTab: CommandRegisterResult;
  duplicateTab: CommandRegisterResult;
  tabAfterUnregister: boolean;
  /** 日志调用序列（level + message + args）。 */
  log: LogEntry[];
  /** 宿主请求往返：命令名 + 实参。 */
  hostRequest: { cmd: string; args?: Record<string, unknown> } | undefined;
}

/**
 * 在一侧执行**同一套**脚本：派发命令、投递事件、注销 Tab，并收集可观察结果。
 *
 * 两侧调用的是同一段代码，因此任何差异都只可能来自 SDK 的上下文实现。
 */
async function observe(side: SideHarness): Promise<ObservableOutcome> {
  const { ctx, snapshot } = side;

  const addResult = await ctx.commands.execute('interop.add', { a: 3, b: 4 });

  let failError = '';
  try {
    await ctx.commands.execute('interop.fail');
  } catch (error) {
    failError = error instanceof Error ? error.message : String(error);
  }

  let unknownError = '';
  try {
    await ctx.commands.execute('interop.missing');
  } catch (error) {
    unknownError = error instanceof Error ? error.message : String(error);
  }

  const hostResult = await ctx.commands.execute('interop.host');

  side.deliver(TOPIC, { n: 1 });
  side.deliver(TOPIC, { n: 2 });

  // 设置 Tab：再注册一次（重复）+ 注销存在的 + 注销不存在的。
  const duplicateTab = ctx.settings.registerTab(TAB);
  const tabAfterUnregister = ctx.settings.unregisterTab(TAB.id);
  const tabGone = ctx.settings.unregisterTab(TAB.id);
  if (tabGone !== false) {
    // 幂等性：第二次注销必须返回 false；两侧若不同会在这里分叉。
    failError += '|tab-second-unregister-not-false';
  }

  await tick();

  return {
    commandIds: [...snapshot.commandIds()].sort(),
    addResult,
    failError,
    unknownError,
    hostResult,
    received: [...snapshot.received],
    registeredTab: snapshot.initialTab,
    duplicateTab,
    tabAfterUnregister,
    log: [...snapshot.log],
    hostRequest: side.hostCalls.find((call) => call.cmd === HOST_REQUEST_CMD),
  };
}

// ──────────────────────────────────────────────────────────────────────────
// 用例
// ──────────────────────────────────────────────────────────────────────────

describe('R2 互操作：同一份插件定义在两个 SDK 下', () => {
  it('可观察结果完全一致（命令/事件/设置/日志/宿主往返）', async () => {
    const appSide = await runOnAppSdk();
    const legacySide = await runOnLegacySdk();
    try {
      const appOutcome = await observe(appSide);
      const legacyOutcome = await observe(legacySide);

      // ── 0. 具体值先各自钉死（防止「两边都错得一样」） ──
      expect(appOutcome.addResult).toEqual({ sum: 7 });
      expect(appOutcome.failError).toBe('E_PLUGIN_BOOM');
      expect(appOutcome.unknownError).toBe('命令 "interop.missing" 不存在');
      expect(appOutcome.hostResult).toEqual({
        answer: HOST_REQUEST_RESULT,
        pluginId: PLUGIN_ID,
      });
      expect(appOutcome.received).toEqual([
        { topic: TOPIC, pluginId: PLUGIN_ID, payload: { n: 1 } },
        { topic: TOPIC, pluginId: PLUGIN_ID, payload: { n: 2 } },
      ]);
      expect(appOutcome.duplicateTab.ok).toBe(false);
      expect(appOutcome.duplicateTab.error).toContain('已存在');
      expect(appOutcome.registeredTab).toEqual({ ok: true });
      expect(appOutcome.tabAfterUnregister).toBe(true);
      expect(appOutcome.commandIds).toEqual(['interop.add', 'interop.fail', 'interop.host']);
      expect(appOutcome.hostRequest).toEqual({ cmd: HOST_REQUEST_CMD, args: HOST_REQUEST_ARGS });

      // 日志序列里必须真的有这四个关键时刻（顺序 + 具体值）
      expect(appOutcome.log.map((entry) => entry.message)).toEqual([
        'activate',
        'register',
        'register',
        'register',
        'duplicate',
        'duplicate-error',
        'subscribed',
        'tab',
        'tab-dup',
        'event',
        'event',
      ]);
      expect(appOutcome.log[0]).toEqual({
        level: 'info',
        message: 'activate',
        args: [PLUGIN_ID],
      });
      expect(appOutcome.log[1]).toEqual({
        level: 'info',
        message: 'register',
        args: ['interop.add', true],
      });

      // ── 1. 两侧逐个可观察量相等（真正的互操作断言） ──
      expect(legacyOutcome.commandIds).toEqual(appOutcome.commandIds);
      expect(legacyOutcome.addResult).toEqual(appOutcome.addResult);
      expect(legacyOutcome.failError).toEqual(appOutcome.failError);
      expect(legacyOutcome.unknownError).toEqual(appOutcome.unknownError);
      expect(legacyOutcome.hostResult).toEqual(appOutcome.hostResult);
      expect(legacyOutcome.received).toEqual(appOutcome.received);
      expect(legacyOutcome.registeredTab).toEqual(appOutcome.registeredTab);
      expect(legacyOutcome.duplicateTab).toEqual(appOutcome.duplicateTab);
      expect(legacyOutcome.tabAfterUnregister).toEqual(appOutcome.tabAfterUnregister);
      expect(legacyOutcome.log).toEqual(appOutcome.log);
      expect(legacyOutcome.hostRequest).toEqual(appOutcome.hostRequest);

      // ── 2. 整份快照相等（兜底：将来新增可观察量时不会漏比） ──
      expect(legacyOutcome).toEqual(appOutcome);

      // ── 3. 编译期：app 侧上下文就是共享契约（签名漂移会编译失败） ──
      const asContract: SharedPluginContext = appSide.ctx;
      expect(asContract.pluginId).toBe(PLUGIN_ID);
    } finally {
      await appSide.dispose();
      await legacySide.dispose();
    }
  });

  it('两侧的宿主边界调用序列一致：请求转发 + 设置 Tab 贡献上报', async () => {
    const appSide = await runOnAppSdk();
    const legacySide = await runOnLegacySdk();
    try {
      // runOnAppSdk / runOnLegacySdk 内部已经用同一份插件定义激活过一次
      // （`side.snapshot`）；这里只补一次宿主请求与一条边界 Tab。
      expect(appSide.snapshot.pluginId).toBe(PLUGIN_ID);
      expect(legacySide.snapshot.pluginId).toBe(PLUGIN_ID);

      await appSide.ctx.commands.execute('interop.host');
      await legacySide.ctx.commands.execute('interop.host');
      await appSide.ctx.settings.registerTab({ id: 'boundary-tab', title: '边界' });
      await legacySide.ctx.settings.registerTab({ id: 'boundary-tab', title: '边界' });
      await tick();

      const appBoundary = appSide.hostCalls.map((call) =>
        call.args === undefined ? { cmd: call.cmd } : { cmd: call.cmd, args: call.args },
      );
      const legacyBoundary = legacySide.hostCalls.map((call) =>
        call.args === undefined ? { cmd: call.cmd } : { cmd: call.cmd, args: call.args },
      );

      // 请求转发：两侧都真的把 (cmd, args) 送到了宿主，参数逐字段相等。
      expect(appBoundary).toContainEqual({ cmd: HOST_REQUEST_CMD, args: HOST_REQUEST_ARGS });
      expect(legacyBoundary).toContainEqual({ cmd: HOST_REQUEST_CMD, args: HOST_REQUEST_ARGS });

      // Tab 贡献上报：两侧都上报了同一条 settings 贡献（label = title）。
      expect(appBoundary).toContainEqual({
        cmd: 'host_contributes_register',
        args: { entry: { kind: 'settings', id: 'boundary-tab', label: '边界' } },
      });
      expect(legacyBoundary).toContainEqual({
        cmd: 'host_contributes_register',
        args: { entry: { kind: 'settings', id: 'boundary-tab', label: '边界' } },
      });
    } finally {
      await appSide.dispose();
      await legacySide.dispose();
    }
  });

  it('显式差异：legacy 侧没有取件泵/握手面，契约不承诺它们', async () => {
    const legacySide = await runOnLegacySdk();
    try {
      // legacy 上下文自己拥有的那一代表面（契约**有意**不包含）：
      const legacyRaw = legacySide.raw as unknown as Record<string, unknown>;
      expect(typeof legacyRaw['invoke']).toBe('function');
      expect(typeof legacyRaw['emit']).toBe('function');
      expect(typeof legacyRaw['onEvent']).toBe('function');
      expect(typeof legacyRaw['destroy']).toBe('function');

      // 契约上下文**不得**把这代表面泄露给插件（否则插件又会依赖 iframe 细节）。
      const contractShaped = legacySide.ctx as unknown as Record<string, unknown>;
      expect(contractShaped['invoke']).toBeUndefined();
      expect(contractShaped['emit']).toBeUndefined();
      expect(contractShaped['ready']).toBeUndefined();

      // app 侧的取件泵入口（`dispatchEvent`）是 SDK 私有扩展，不在契约里：
      // 换传输后它在 legacy 侧不存在，因此插件代码不得依赖它。
      const appSide = await runOnAppSdk();
      try {
        const appShaped = appSide.ctx as unknown as Record<string, unknown>;
        expect(typeof appShaped['dispatchEvent']).toBe('function');
        expect(appSide.ctx.pluginId).toBe(PLUGIN_ID);
      } finally {
        await appSide.dispose();
      }
    } finally {
      await legacySide.dispose();
    }
  });
});

// ──────────────────────────────────────────────────────────────────────────
// legacy 侧宿主环境（放在文件末尾：读者先看两侧共用的插件定义与断言）
// ──────────────────────────────────────────────────────────────────────────

/** legacy 侧的可观察面额外带上原始 legacy 上下文（用于「显式差异」用例）。 */
interface LegacySideHarness extends SideHarness {
  raw: LegacyPluginContext;
}

/**
 * legacy 侧：iframe 一代上下文 + 契约适配器。
 *
 * `invoke` 的应答刻意与 app 侧的 `MockBackend` **逐字段相同**（含
 * `host_events_publish` 的 `{ delivered, dropped }`），否则「两边结果一致」
 * 就成了比对两个不同宿主的差异，而不是比对 SDK 的差异。
 */
async function runOnLegacySdk(): Promise<LegacySideHarness> {
  const hostCalls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
  const listeners = new Map<string, Set<(payload: unknown) => void>>();

  const legacy: LegacyPluginContext = {
    ready: true,
    permissions: [],
    async invoke(method: string, args: unknown): Promise<unknown> {
      hostCalls.push(
        args === undefined
          ? { cmd: method }
          : { cmd: method, args: args as Record<string, unknown> },
      );
      if (method === HOST_REQUEST_CMD) return HOST_REQUEST_RESULT;
      if (method === 'host_contributes_register') return undefined;
      if (method === 'host_events_publish') return { delivered: 1, dropped: false };
      throw new Error(`E_UNKNOWN_COMMAND: ${method}`);
    },
    emit(eventName: string, payload: unknown): void {
      // legacy 的 emit 是「插件 → 宿主」单向；本环境的宿主不做回环投递，
      // 与 app 侧的 publish 语义一致（r5 的 request 通道同样不自动回灌）。
      hostCalls.push({ cmd: `emit:${eventName}`, args: payload as Record<string, unknown> });
    },
    onEvent(eventName: string, handler: (payload: unknown) => void): () => void {
      let set = listeners.get(eventName);
      if (!set) {
        set = new Set();
        listeners.set(eventName, set);
      }
      set.add(handler);
      return () => {
        set.delete(handler);
      };
    },
    onInit(callback: (permissions: string[]) => void): void {
      callback([]);
    },
    destroy(): void {
      listeners.clear();
    },
  };

  const ctx = createContractContext(legacy, {
    pluginId: PLUGIN_ID,
    // 只覆盖「请求」这一条，其余沿用缺省（与 @tauron/host 命令名一致）；
    // 覆盖本身即证明词表是适配层参数，而不是藏在 SDK 里的硬编码。
    hostCommands: { contributesRegister: 'host_contributes_register' },
  });
  const snapshot = await createSharedPlugin().activate(ctx);
  await tick();

  return {
    ctx,
    raw: legacy,
    snapshot,
    hostCalls,
    deliver: (topic, payload) => {
      listeners.get(topic)?.forEach((handler) => handler(payload));
    },
    dispose: async () => {
      legacy.destroy();
    },
  };
}
