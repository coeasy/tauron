/**
 * @tauron/plugin-sdk — 共享契约适配器（方案 R2）。
 *
 * 本 SDK 是 **iframe bridge 一代**：插件跑在 `sandbox="allow-scripts"` 的 iframe
 * 里，经 postMessage 与宿主通信，上下文形状是 {@link PluginContext}
 * （`ready` / `permissions` / `invoke` / `emit` / `onEvent` / `onInit` / `destroy`）。
 *
 * 那一代形状与 {@link SharedPluginContext}（`pluginId` / `host` / `commands` /
 * `events` / `settings` / `log`）**没有一条成员同名同义**，于是「为
 * `@tauron/app-plugin-sdk` 写的插件定义」在本 SDK 下根本无法激活。
 *
 * 本模块提供**适配**而不是替换：用既有的 bridge / `invoke` / 本地注册表构造出
 * 契约形状，使**同一份插件定义**能在两个 SDK 下跑（互操作测试见
 * `@tauron/app-plugin-sdk/src/interop.test.ts`）。legacy {@link PluginContext}
 * 原样保留（另一代 API，有既有测试），属**过渡**形状。
 *
 * 诚实标注（未接线的部分，见 {@link ContractContextOptions} 与 README）：
 * 1. **宿主命令名**由选项给出，缺省是一套**约定**（`host_events_publish` 等），
 *    而 legacy 传输本身不定义命令词表——词表是宿主实现的一部分。这是适配层与
 *    `@tauron/app-plugin-sdk` 之间**唯一无法消除**的差异；
 * 2. **取件泵**不在此实现：legacy 的事件投递由宿主经 `emitToPlugin` 推送，
 *    契约的 `events.subscribe` 只挂本地监听（没有 `host_events_drain` 可言）；
 * 3. **设置 Tab 的宿主可见性**：本地账本（去重/注销）完全等价，但「喂给宿主
 *    贡献表」这条副作用受限于同样没有贡献读取端——见 README 的对照表。
 */

import type {
  CommandRegisterResult,
  PluginCommandHandler,
  PluginContext as SharedPluginContext,
  PluginEventListener,
  SettingsTabConfig,
} from '@tauron/plugin-context-contract';

import type { PluginContext as LegacyPluginContext } from './plugin-context.js';

/**
 * 契约各动作对应的宿主命令名。
 *
 * 为什么必须可配置：legacy 传输（postMessage 沙箱）只承诺「把方法名转给宿主」，
 * 不定义命令词表。这里给出的缺省值与 `@tauron/app-plugin-sdk` 消费的宿主命令
 * 同名，使两边能在**同一个宿主**下观察到同一结果；非标准宿主可整套覆盖。
 */
export interface ContractHostCommandNames {
  /** 契约 `host.request(cmd, args)` 的转发方式（legacy 是 `invoke(method, args)`）。 */
  request: string;
  /** 设置 Tab 注册时上报的宿主贡献命令。 */
  contributesRegister: string;
  /** 事件发布命令。 */
  eventsPublish: string;
}

/** 缺省命令名（与 `@tauron/host` 的 `HostClient` 消费的命令一致）。 */
export const DEFAULT_CONTRACT_HOST_COMMANDS: ContractHostCommandNames = {
  request: 'host_plugin_call',
  contributesRegister: 'host_contributes_register',
  eventsPublish: 'host_events_publish',
};

/** {@link createContractContext} 选项。 */
export interface ContractContextOptions {
  /** 插件 id（进入契约的 `pluginId`，也是日志署名）。 */
  pluginId: string;
  /**
   * 宿主命令名覆盖（缺省 {@link DEFAULT_CONTRACT_HOST_COMMANDS}）。
   *
   * 部分覆盖会与缺省值合并，因此只需给出要改的那几条。
   */
  hostCommands?: Partial<ContractHostCommandNames>;
  /**
   * 日志出口（缺省 `console`，前缀 `[pluginId]`）。
   *
   * 注入是为了可观测性：宿主/测试要断言「插件记了什么」，而不是去抓 console。
   */
  logger?: {
    info(message: string, ...args: unknown[]): void;
    warn(message: string, ...args: unknown[]): void;
    error(message: string, ...args: unknown[]): void;
  };
  /**
   * 设置 Tab 上报宿主失败时的告警出口（缺省经 `logger.warn`）。
   *
   * 与 `@tauron/app-plugin-sdk` 同语义：贡献是声明式附加物，上报失败**不得**
   * 推翻本地注册结果，但必须可观测。
   */
  onReportError?: (error: unknown, tab: SettingsTabConfig) => void;
}

/**
 * 把 legacy iframe 上下文适配成共享契约上下文。
 *
 * 用法：
 * ```ts
 * const legacy = createPluginContext({ handshakeToken });
 * const ctx = createContractContext(legacy, { pluginId: 'com.example.p' });
 * await plugin.activate(ctx);   // 与 app-plugin-sdk 下同一份 plugin 定义
 * ```
 *
 * 语义映射（逐条对应契约成员）：
 * - `pluginId`：选项给定（legacy 上下文没有身份字段——身份由握手时宿主授予）。
 * - `host.request` → `legacy.invoke(method, args)`；`host.contributesRegister`
 *   优先用 legacy 上下文**自己的**同名方法（若存在），否则经 `invoke` 转发。
 * - `commands` → 本适配器内的注册表（legacy 没有命令注册面）；处理器收到的是
 *   **契约上下文**，与 app-plugin-sdk 下完全一致。
 * - `events.subscribe` → `legacy.onEvent`（宿主 `emitToPlugin` 推送的帧到达
 *   订阅者）；`events.publish` → `legacy.emit`（发完即返回，与 postMessage 的
 *   单向语义一致）。
 * - `settings` → 本适配器内的去重账本 + best-effort 上报宿主贡献表。
 * - `log` → 注入的 logger（缺省 `console`，带 `[pluginId]` 前缀）。
 */
export function createContractContext(
  legacy: LegacyPluginContext,
  options: ContractContextOptions,
): SharedPluginContext {
  const { pluginId } = options;
  const commands = new Map<string, PluginCommandHandler>();
  const tabs = new Map<string, SettingsTabConfig>();
  const hostCommands: ContractHostCommandNames = {
    ...DEFAULT_CONTRACT_HOST_COMMANDS,
    ...options.hostCommands,
  };

  const logger = options.logger ?? {
    info(message: string, ...args: unknown[]): void {
      console.log(`[${pluginId}] ${message}`, ...args);
    },
    warn(message: string, ...args: unknown[]): void {
      console.warn(`[${pluginId}] ${message}`, ...args);
    },
    error(message: string, ...args: unknown[]): void {
      console.error(`[${pluginId}] ${message}`, ...args);
    },
  };

  /**
   * 上报一条贡献到宿主。
   *
   * 走 legacy 的**本名方法**（若该上下文提供了 `contributesRegister`），否则经
   * `invoke` 转发。返回 rejected Promise 由调用方 catch（与 app 侧一致：只告警、
   * 不推翻本地注册）。
   */
  const reportContribution = (entry: { kind: string; id: string; label: string }): Promise<void> => {
    const selfRegister = (
      legacy as { contributesRegister?: (e: { kind: string; id: string; label: string }) => Promise<void> }
    ).contributesRegister;
    if (typeof selfRegister === 'function') {
      return Promise.resolve(selfRegister.call(legacy, entry));
    }
    // legacy 的 invoke 结果形状未知（宿主决定）：本适配层原样返回，不二次解释。
    return Promise.resolve(
      legacy.invoke(hostCommands.contributesRegister, { entry }),
    ).then(() => undefined);
  };

  const ctx: SharedPluginContext = {
    pluginId,

    host: {
      async request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
        return (await legacy.invoke(cmd, args)) as T;
      },

      async contributesRegister(entry: { kind: string; id: string; label: string }): Promise<void> {
        await reportContribution(entry);
      },
    },

    commands: {
      register<TArgs = unknown, TResult = unknown>(
        id: string,
        handler: PluginCommandHandler<TArgs, TResult>,
      ): CommandRegisterResult {
        if (commands.has(id)) {
          return { ok: false, error: `命令 "${id}" 已存在` };
        }
        commands.set(id, handler as unknown as PluginCommandHandler);
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
        if (!handler) {
          // 契约语义：命令不存在必须 reject（与 app 侧同码同义）。
          throw new Error(`命令 "${id}" 不存在`);
        }
        return (await handler(args, ctx)) as TResult;
      },
    },

    events: {
      async publish(topic: string, payload: unknown): Promise<void> {
        legacy.emit(hostCommands.eventsPublish, { evt: { topic, payload } });
      },

      subscribe(topic: string, listener: PluginEventListener): () => void {
        return legacy.onEvent(topic, (payload: unknown) => {
          listener(payload, { topic, pluginId });
        });
      },
    },

    settings: {
      registerTab(config: SettingsTabConfig): CommandRegisterResult {
        if (tabs.has(config.id)) {
          return { ok: false, error: `设置 Tab "${config.id}" 已存在` };
        }
        tabs.set(config.id, config);
        // best-effort 上报：失败只告警（贡献不得让插件激活失败）。
        void reportContribution({ kind: 'settings', id: config.id, label: config.title }).catch(
          (error: unknown) => {
            if (options.onReportError) {
              options.onReportError(error, config);
              return;
            }
            logger.warn('settings Tab 注册宿主贡献失败（不影响本地注册）', config.id, error);
          },
        );
        return { ok: true };
      },

      unregisterTab(id: string): boolean {
        return tabs.delete(id);
      },
    },

    log: {
      info: (message: string, ...args: unknown[]): void => logger.info(message, ...args),
      warn: (message: string, ...args: unknown[]): void => logger.warn(message, ...args),
      error: (message: string, ...args: unknown[]): void => logger.error(message, ...args),
    },
  };

  return ctx;
}
