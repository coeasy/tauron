// @tauron/app-plugin-sdk — 类型定义。

import type { HostClient, PluginCallRequest } from '@tauron/host';
import type { ContributesEntry, IdentityUnit, ViewConfig } from '@tauron/host';
import type {
  CommandRegisterResult,
  PluginCommandHandler,
  PluginContext as SharedPluginContext,
  PluginEventListener,
  PluginHost,
  SettingsTabConfig as SharedSettingsTabConfig,
} from '@tauron/plugin-context-contract';

// ── 命令定义 ──

/**
 * 命令处理器。
 *
 * **R2 统一**：本别名就是共享契约的 {@link PluginCommandHandler}——签名不再由
 * 本 SDK 单方面定义。契约成员改动会经下面的编译期断言
 * （{@link AppPluginContextIsAssignableToContract} /
 * {@link AppPluginContextMembersMatchContract}）在编译期暴露。
 */
export type CommandHandler<TArgs = unknown, TResult = unknown> = PluginCommandHandler<
  TArgs,
  TResult
>;

/**
 * 声明式命令处理器。
 *
 * 参数类型取 `never` 而非 `unknown`：声明处由插件作者写出**具体**形状
 * （如 `(args: { name: string }) => ...`），运行期由调用方以真实实参调用。
 * 若用 `unknown`，在 `strictFunctionTypes` 的参数逆变规则下，具体形状的
 * 处理器无法赋值给该位置。
 */
export type DeclaredCommandHandler = CommandHandler<never, unknown>;

/** 已注册的命令。 */
export interface RegisteredCommand {
  id: string;
  handler: CommandHandler;
  registeredAt: number;
}

/**
 * 命令注册结果。
 *
 * 与共享契约同名同构（别名而非第二份定义）：`commands.register` 与
 * `settings.registerTab` 共用它，改名必须两侧同时跟上。
 */
export type { CommandRegisterResult };

// ── 设置定义 ──

/** 设置 Tab 配置。 */
export type SettingsTabConfig = SharedSettingsTabConfig;

// ── 事件定义 ──

/**
 * 事件订阅者。
 *
 * 与共享契约的 {@link PluginEventListener} 同构（别名而非第二份定义）。
 */
export type EventListener = PluginEventListener;

// ── 插件定义 ──

/**
 * 插件生命周期上下文。
 *
 * **R2 统一**：本接口 `extends` 共享契约 {@link SharedPluginContext}——成员清单
 * 与语义的唯一事实源在 `@tauron/plugin-context-contract`。这里只做两件事：
 * 1. 把契约的窄类型**收窄到具体类型**（`host` 收窄为 `@tauron/host` 的
 *    `HostClient`，比契约要求的 `PluginHost` 更具体；`commands` / `events` /
 *    `settings` / `log` 同样用本包的类型别名，语义不变）；
 * 2. 作为插件作者看到的类型，保留 `@tauron/host` 的具体能力（如
 *    `host.eventsDrain`）而不削减任何现有能力。
 *
 * 覆盖（override）每个成员是**故意**的：它让「契约成员被删/改名」在编译期
 * 失败，而不是让收窄静默失效。断言见 {@link AppPluginContextIsAssignableToContract}
 * 与 {@link AppPluginContextMembersMatchContract}。
 */
export interface PluginContext extends SharedPluginContext {
  /** 插件 ID。 */
  readonly pluginId: string;

  /**
   * 宿主客户端（具体实现：`@tauron/host` 的 `HostClient`）。
   *
   * 契约要求的是窄面 {@link PluginHost}；这里给出具体类，因为本 SDK 的插件
   * 需要 `contributesRegister` / `eventsDrain` 等更具体的能力。
   */
  readonly host: HostClient;

  /** 命令注册/注销。 */
  readonly commands: {
    register<TArgs = unknown, TResult = unknown>(
      id: string,
      handler: CommandHandler<TArgs, TResult>,
    ): CommandRegisterResult;
    unregister(id: string): boolean;
    has(id: string): boolean;
    list(): string[];
    execute<TArgs = unknown, TResult = unknown>(
      id: string,
      args?: TArgs,
    ): Promise<TResult>;
  };

  /** 设置 Tab 注册。 */
  readonly settings: {
    registerTab(config: SettingsTabConfig): CommandRegisterResult;
    unregisterTab(id: string): boolean;
  };

  /** 事件发布/订阅。 */
  readonly events: {
    publish(topic: string, payload: unknown): Promise<void>;
    subscribe(topic: string, listener: EventListener): () => void;
  };

  /** 日志。 */
  readonly log: {
    info(message: string, ...args: unknown[]): void;
    warn(message: string, ...args: unknown[]): void;
    error(message: string, ...args: unknown[]): void;
  };
}

/**
 * 编译期断言：本 SDK 的 `PluginContext` **必须**满足共享契约。
 *
 * 这是**接口漂移的报警器**（方案 R2 的硬要求）：签名一旦漂移，这里立刻编译
 * 失败，而不是等到某个插件在两个 SDK 下行为分叉时才被发现。
 */
export type AppPluginContextIsAssignableToContract =
  PluginContext extends SharedPluginContext ? true : never;

/**
 * 「双向类型相等」构造（**R2 的严格档**）。
 *
 * 为什么需要它：单方向的 `A extends B` 挡不住**返回类型收窄/丢失**——返回类型
 * 是协变的，`(() => void)` 可以赋值给 `void`，于是「契约要求 `events.subscribe`
 * 返回退订函数」而「本 SDK 实际返回 `void`」这种**语义破坏**能溜过去（实测：
 * 把契约的 `subscribe` 改成返回 `void`，单方向断言不报错）。
 *
 * 技巧说明：
 * - `[T] extends [() => void]` **不**判等——TS 在条件类型里把 `void` 当作
 *   「函数可赋值」的超类型，`(() => void) extends void` 仍为 `true`；
 * - 因此先把两侧包成**函数参数**再比较：函数参数在 `strictFunctionTypes`
 *   下按**逆变**比较，而 `void` 的逆变比较就是真实的子类型判定——`(() => void)`
 *   与 `void` 在逆向下互不可赋值，漂移随即暴露；
 * - 本 SDK 与契约的类型面都不含 `any`，所以这里不会出现 `any` 双向可赋值
 *   导致的假阳性。
 */
type StrictEqual<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
type AsFunctionParameter<T> = (value: T) => void;
type ContractEqual<A, B> = StrictEqual<AsFunctionParameter<A>, AsFunctionParameter<B>>;

/**
 * 契约**严格相等**断言：跨包边界的那部分（`pluginId` / `commands` / `events` /
 * `settings` / `log`）必须与契约**双向**一致。
 *
 * `host` **有意不在此列**：本 SDK 的 `host` 是 `HostClient`，它是契约
 * {@link PluginHost} 的**真子类型**（多出 `pluginCall` / `eventsDrain` 等），
 * 「相等」对它不成立也不该成立；host 面的守约由
 * {@link AppPluginContextIsAssignableToContract}（可赋值性）保证。
 *
 * 与可赋值性断言的分工：
 * - 可赋值性挡「少一条成员」；
 * - 严格相等挡「返回类型被改坏」（如 `subscribe` 不再返回退订函数、`execute`
 *   不再返回 Promise）——这正是方案要求「签名漂移必须编译失败」的那一条。
 *
 * 已知边界（诚实标注）：TS 的方法参数按**双变**比较，因此「参数类型被改成
 * 不兼容形状」这类漂移未在此处覆盖；它由两件事兜住——实现侧（`context.ts` /
 * `createContractContext`）必须真的写出契约要求的参数类型，以及互操作测试的
 * 运行期对照。
 */
export type AppPluginContextMembersMatchContract = ContractEqual<
  {
    pluginId: PluginContext['pluginId'];
    commands: PluginContext['commands'];
    events: PluginContext['events'];
    settings: PluginContext['settings'];
    log: PluginContext['log'];
  },
  {
    pluginId: SharedPluginContext['pluginId'];
    commands: SharedPluginContext['commands'];
    events: SharedPluginContext['events'];
    settings: SharedPluginContext['settings'];
    log: SharedPluginContext['log'];
  }
> extends true
  ? true
  : never;

/**
 * `HostClient` 必须满足契约的宿主窄面。
 *
 * 单方向（子类型）断言：本 SDK 的宿主实现比契约**更具体**是允许的，
 * 少一条契约成员则不允许。
 */
export type AppSdkHostIsPluginHost = HostClient extends PluginHost ? true : never;

/**
 * 断言求值点：任一断言不成立时对应类型变为 `never`，该常量赋值随即编译失败
 * （`Type 'true' is not assignable to type 'never'`）。
 */
const _appPluginContextIsAssignable: AppPluginContextIsAssignableToContract = true;
const _appPluginContextMembersMatch: AppPluginContextMembersMatchContract = true;
const _appSdkHostIsPluginHost: AppSdkHostIsPluginHost = true;
void _appPluginContextIsAssignable;
void _appPluginContextMembersMatch;
void _appSdkHostIsPluginHost;

/** 插件生命周期钩子。 */
export interface PluginHooks {
  /** 插件激活时调用。 */
  activate?: (ctx: PluginContext) => Promise<void> | void;
  /** 插件失活时调用。 */
  deactivate?: (ctx: PluginContext) => Promise<void> | void;
  /** 插件被卸载时调用（清理资源）。 */
  dispose?: (ctx: PluginContext) => Promise<void> | void;
  /**
   * 收到已声明订阅的事件时调用。
   *
   * 仅对 `definition.events.subscribe` 中声明的 topic 生效。
   */
  onEvent?: (topic: string, payload: unknown, ctx: PluginContext) => void;
  /** 插件设置变化时调用。 */
  onSettingsChanged?: (settings: Record<string, unknown>, ctx: PluginContext) => Promise<void> | void;
}

/** 插件定义（声明式）。 */
export interface PluginDefinition {
  /** 插件 ID（反域名格式）。 */
  id: string;
  /** 插件名称。 */
  name: string;
  /** 版本号。 */
  version: string;
  /** 描述。 */
  description?: string;
  /** 作者。 */
  author?: string;
  /** 许可证。 */
  license?: string;

  /** 生命周期钩子。 */
  activate?: PluginHooks['activate'];
  deactivate?: PluginHooks['deactivate'];
  dispose?: PluginHooks['dispose'];
  onSettingsChanged?: PluginHooks['onSettingsChanged'];
  onEvent?: PluginHooks['onEvent'];

  /** 命令声明（可选，可不在 activate 中注册）。 */
  commands?: Record<string, DeclaredCommandHandler>;

  /** 设置 Tab 声明。 */
  settings?: SettingsTabConfig[];

  /** 事件订阅声明。 */
  events?: {
    publish?: string[];
    subscribe?: string[];
  };

  /** 贡献声明。 */
  contributes?: {
    commands?: { id: string; title: string }[];
    menus?: { id: string; command: string }[];
    panels?: { id: string; title: string; icon: string }[];
    settingsTabs?: { id: string; title: string }[];
    shortcuts?: { accelerator: string; command: string }[];
  };
}

// ── 插件实例 ──

/** 插件实例。 */
export interface PluginInstance {
  readonly id: string;
  readonly name: string;
  readonly version: string;

  /** 当前是否激活。 */
  readonly isActive: boolean;

  /** 激活插件。 */
  activate(ctx: PluginContext): Promise<void>;
  /** 失活插件。 */
  deactivate(ctx: PluginContext): Promise<void>;
  /** 清理资源。 */
  dispose(ctx: PluginContext): Promise<void>;
}
