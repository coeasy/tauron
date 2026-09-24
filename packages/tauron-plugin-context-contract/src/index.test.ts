// @tauron/plugin-context-contract — 契约自检。
//
// 本包的产物**只有类型**，因此自检由两部分组成：
// 1. 编译期形状断言（下面的 `_Assert*` 常量）：契约被意外放宽（例如
//    `request` 变可选、`commands` 变成可选成员）时，这里直接编译失败。
// 2. 运行期结构满足性检查：用**本包自带**的结构性替身（不引入任何 SDK，
//    否则契约包会反向依赖两代 SDK）证明契约是「可满足且最小」的——
//    只实现 `request` 的宿主就能满足 `PluginHost`，多给一个成员不再是必需。

import { describe, expect, it } from 'vitest';

import type {
  AnyPluginCommandHandler,
  CommandRegisterResult,
  CommandRegistry,
  EventSink,
  PluginCommandHandler,
  PluginContext,
  PluginEventEnvelope,
  PluginEventListener,
  PluginHost,
  PluginLog,
  SettingsTabConfig,
  SettingsTabRegistry,
} from './index.js';

// ──────────────────────────────────────────────────────────────────────────
// 编译期断言（漂移报警器）
// ──────────────────────────────────────────────────────────────────────────

/** 编译期「A 是否可以赋值给 B」断言；不成立时该常量类型变成 `never`。 */
type Assignable<A, B> = [A] extends [B] ? true : never;

/**
 * 宿主最小面断言：只有 `request` 的对象**必须**满足 `PluginHost`。
 *
 * 若哪天有人把 `contributesRegister` 改成必需成员，这行编译失败——
 * 而那种改动会让 iframe 一代（没有贡献面）无法诚实满足契约。
 */
type _MinimalHostIsPluginHost = Assignable<
  { request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> },
  PluginHost
>;
const _minimalHost: _MinimalHostIsPluginHost = true;

/** 上下文成员齐全性断言：契约的六个成员一个都不能少。 */
type _RequiredContextMembers =
  PluginContext extends {
    readonly pluginId: string;
    readonly host: PluginHost;
    readonly commands: CommandRegistry;
    readonly events: EventSink;
    readonly settings: SettingsTabRegistry;
    readonly log: PluginLog;
  }
    ? true
    : never;
const _contextMembers: _RequiredContextMembers = true;

/**
 * 处理器逆变断言：具体形状的处理器**必须**可赋值到泛型注册位。
 *
 * 这是「插件定义写具体参数类型」能成立的前提；若契约把 `TArgs` 收紧成
 * `never` 或退回 `unknown` 逆变失败，这里编译失败。
 */
type _HandlerVariance = Assignable<
  (args: { name: string }, ctx: PluginContext) => { greeting: string },
  PluginCommandHandler<{ name: string }, { greeting: string }>
>;
const _handlerVariance: _HandlerVariance = true;

/** 退订函数形态断言：`subscribe` 必须返回可调用的退订函数（而非 void）。 */
type _UnsubscribeShape = Assignable<() => void, ReturnType<EventSink['subscribe']>>;
const _unsubscribeShape: _UnsubscribeShape = true;

describe('@tauron/plugin-context-contract 编译期断言', () => {
  it('编译期断言常量成立（运行时占位，真正的判定在 tsc）', () => {
    // 这些断言在 `pnpm --filter @tauron/plugin-context-contract typecheck`
    // 里被求值：任一为 `never` 时上面的赋值会编译失败。
    expect([
      _minimalHost,
      _contextMembers,
      _handlerVariance,
      _unsubscribeShape,
    ]).toEqual([true, true, true, true]);
  });
});

// ──────────────────────────────────────────────────────────────────────────
// 结构性替身（不依赖任何 SDK：契约包必须零依赖）
// ──────────────────────────────────────────────────────────────────────────

/**
 * 只实现 `request` 的最小宿主替身。
 *
 * 它**没有** `contributesRegister`——用来证明契约的宿主面确实窄到
 * iframe 一代也能满足，同时把「请求」如实转发到脚本化的应答表。
 */
function makeMinimalHost(answers: Record<string, unknown>): {
  host: PluginHost;
  calls: Array<{ cmd: string; args?: Record<string, unknown> }>;
} {
  const calls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];
  const host: PluginHost = {
    async request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
      calls.push(args === undefined ? { cmd } : { cmd, args });
      if (!(cmd in answers)) {
        // 契约语义：宿主拒绝必须 reject，不得假成功。
        throw new Error(`E_UNKNOWN_COMMAND: ${cmd}`);
      }
      return answers[cmd] as T;
    },
  };
  return { host, calls };
}

/** 按契约语义手工实现一份上下文（用来验证契约是可实现的、且语义有辨别力）。 */
function makeContractContext(pluginId: string, host: PluginHost): PluginContext {
  const commands = new Map<string, AnyPluginCommandHandler>();
  const tabs = new Map<string, SettingsTabConfig>();
  const listeners = new Map<string, Set<PluginEventListener>>();
  const logs: Array<{ level: string; message: string; args: unknown[] }> = [];

  const ctx: PluginContext = {
    pluginId,
    host,

    commands: {
      register<TArgs = unknown, TResult = unknown>(
        id: string,
        handler: PluginCommandHandler<TArgs, TResult>,
      ): CommandRegisterResult {
        if (commands.has(id)) return { ok: false, error: `命令 "${id}" 已存在` };
        commands.set(id, handler as unknown as AnyPluginCommandHandler);
        return { ok: true };
      },
      unregister(id: string): boolean {
        return commands.delete(id);
      },
      has(id: string): boolean {
        return commands.has(id);
      },
      list(): string[] {
        return [...commands.keys()];
      },
      async execute<TArgs = unknown, TResult = unknown>(
        id: string,
        args?: TArgs,
      ): Promise<TResult> {
        const handler = commands.get(id);
        if (!handler) throw new Error(`命令 "${id}" 不存在`);
        return (await handler(args, ctx)) as TResult;
      },
    },

    events: {
      async publish(topic: string, payload: unknown): Promise<void> {
        await host.request('host_events_publish', { evt: { topic, payload } });
      },
      subscribe(topic: string, listener: PluginEventListener): () => void {
        let set = listeners.get(topic);
        if (!set) {
          set = new Set();
          listeners.set(topic, set);
        }
        set.add(listener);
        return () => {
          const current = listeners.get(topic);
          if (!current) return;
          current.delete(listener);
          if (current.size === 0) listeners.delete(topic);
        };
      },
    },

    settings: {
      registerTab(config: SettingsTabConfig): CommandRegisterResult {
        if (tabs.has(config.id)) return { ok: false, error: `设置 Tab "${config.id}" 已存在` };
        tabs.set(config.id, config);
        return { ok: true };
      },
      unregisterTab(id: string): boolean {
        return tabs.delete(id);
      },
    },

    log: {
      info(message: string, ...args: unknown[]): void {
        logs.push({ level: 'info', message, args });
      },
      warn(message: string, ...args: unknown[]): void {
        logs.push({ level: 'warn', message, args });
      },
      error(message: string, ...args: unknown[]): void {
        logs.push({ level: 'error', message, args });
      },
    },
  };

  return ctx;
}

// ──────────────────────────────────────────────────────────────────────────
// 运行期：契约语义的可满足性与辨别力
// ──────────────────────────────────────────────────────────────────────────

describe('@tauron/plugin-context-contract 结构满足性', () => {
  it('只实现 request 的宿主即满足契约（宿主面确实窄）', async () => {
    const { host, calls } = makeMinimalHost({ host_ping: { pong: 1 } });
    const ctx = makeContractContext('com.example.contract', host);

    expect(ctx.pluginId).toBe('com.example.contract');
    await expect(host.request('host_ping')).resolves.toEqual({ pong: 1 });
    expect(calls).toEqual([{ cmd: 'host_ping' }]);
  });

  it('宿主拒绝以 reject 表达，不被吞成假成功', async () => {
    const { host } = makeMinimalHost({});
    const ctx = makeContractContext('com.example.contract', host);
    await expect(ctx.events.publish('t', { v: 1 })).rejects.toThrow(/E_UNKNOWN_COMMAND/);
  });

  it('命令注册语义：重复拒绝、execute 派发、未注册 reject', async () => {
    const { host } = makeMinimalHost({});
    const ctx = makeContractContext('com.example.contract', host);

    expect(ctx.commands.register('add', (args: { a: number; b: number }) => ({ sum: args.a + args.b })))
      .toEqual({ ok: true });
    const dup = ctx.commands.register('add', () => ({ sum: -1 }));
    expect(dup.ok).toBe(false);
    expect(dup.error).toContain('已存在');

    expect(ctx.commands.list()).toEqual(['add']);
    await expect(ctx.commands.execute('add', { a: 2, b: 3 })).resolves.toEqual({ sum: 5 });
    await expect(ctx.commands.execute('missing')).rejects.toThrow(/不存在/);
  });

  it('事件订阅语义：退订幂等、topic 互不影响', () => {
    const { host } = makeMinimalHost({});
    const ctx = makeContractContext('com.example.contract', host);
    const seen: unknown[] = [];
    const off = ctx.events.subscribe('a', (payload: unknown) => seen.push(payload));
    const offOther = ctx.events.subscribe('b', () => seen.push('b'));

    // 契约只规定「退订返回函数且幂等」——投递本体属实现，故此处只锁幂等。
    expect(() => {
      off();
      off();
    }).not.toThrow();
    expect(() => offOther()).not.toThrow();
    expect(seen).toEqual([]);
  });

  it('设置 Tab 语义：重复拒绝、注销返回是否曾存在', () => {
    const { host } = makeMinimalHost({});
    const ctx = makeContractContext('com.example.contract', host);
    expect(ctx.settings.registerTab({ id: 'main', title: '设置' })).toEqual({ ok: true });
    expect(ctx.settings.registerTab({ id: 'main', title: '设置' }).ok).toBe(false);
    expect(ctx.settings.unregisterTab('main')).toBe(true);
    expect(ctx.settings.unregisterTab('main')).toBe(false);
  });

  it('日志面三个级别都带插件署名语义（实现侧署名，契约侧只锁调用可达）', () => {
    const { host } = makeMinimalHost({});
    const ctx = makeContractContext('com.example.contract', host);
    expect(() => {
      ctx.log.info('i');
      ctx.log.warn('w', 1);
      ctx.log.error('e');
    }).not.toThrow();
  });
});

describe('@tauron/plugin-context-contract 事件信封', () => {
  it('信封字段是 topic + pluginId（署名不能靠 payload 自报）', () => {
    const envelope: PluginEventEnvelope = { topic: 'com.example.topic', pluginId: 'com.example.a' };
    const listener: PluginEventListener = (payload, event) => {
      expect(payload).toEqual({ v: 1 });
      expect(event.topic).toBe('com.example.topic');
      expect(event.pluginId).toBe('com.example.a');
    };
    listener({ v: 1 }, envelope);
  });
});
