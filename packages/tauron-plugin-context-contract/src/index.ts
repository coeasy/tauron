// ──────────────────────────────────────────────────────────────────────────
// @tauron/plugin-context-contract — 插件上下文共享契约（仅类型，零运行时依赖）。
//
// 解决什么问题（方案 R2 / §3 的 B2 发现）：
// 仓库里并存**两个插件 SDK**，各自定义了一个**同名不同构**的 `PluginContext`：
//
// - `@tauron/app-plugin-sdk`：`createPluginContext(pluginId, host)` 构造，成员为
//   `pluginId` / `host` / `commands` / `settings` / `events` / `log`；
// - `@tauron/plugin-sdk`：iframe bridge + 握手 token 一代，成员为
//   `ready` / `permissions` / `invoke` / `emit` / `onEvent` / `onInit` / `destroy`。
//
// 两份类型同名、都叫「插件上下文」，但**没有一条成员同名同义**。后果是：插件
// 作者为某一个 SDK 写的插件在另一个 SDK 下**根本无法激活**——不是运行时报错，
// 而是类型层面就无法赋值；「同一份插件定义跑在两个 SDK 下」这件事既没被支持、
// 也没被任何测试观测过。
//
// 本包把两代 SDK 都能满足的那部分收敛成**唯一事实源**：
// - `@tauron/app-plugin-sdk` 的 `PluginContext` 直接 `extends` 本契约，
//   并用编译期断言把签名漂移变成编译失败（而不是运行时才暴露）；
// - `@tauron/plugin-sdk` 提供 `createContractContext(...)` 适配器，从它既有的
//   bridge/`invoke`/本地注册表构造出契约形状（legacy `PluginContext` 保留不动，
//   属过渡 API）。
//
// 依赖方向：本包**零运行时依赖、零 import**。特别是它**不**引用 `@tauron/host`
// ——那会把「插件契约」绑死在 Tauri 传输上。`PluginHost` 只声明结构性最小面
// （`request<T>(cmd, args?)`），`HostClient` 与 bridge 适配器都结构满足它：
// 换传输（移动 gateway、测试 mock、postMessage 沙箱）只换实现，契约不动。
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 宿主面（传输无关）
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件面向宿主的**传输无关**最小面。
 *
 * 为什么在契约里：`host` 是「换传输只换实现」的那一层。契约里若直接写
 * `HostClient`，插件契约就被 Tauri（`invoke` / `Channel`）绑死，别的宿主形态
 * （iframe `postMessage` 沙箱、移动端 gateway、测试 mock）只能靠类型谎言绕过。
 *
 * 因此这里只要求**结构性最小面**：
 * - {@link PluginHost.request} 是**必需**的——它是唯一的「发一条请求拿一次应答」
 *   原语，任何传输都能实现（Tauri `invoke`、`postMessage` 往返、HTTP）。
 * - {@link PluginHost.contributesRegister} 是**可选**的——声明式贡献（设置
 *   Tab、命令、菜单）是宿主的高级能力，不是每条传输都提供。可选化让「没有
 *   贡献面的宿主」也能诚实满足契约，而不是伪造一个永远失败的实现。
 */
export interface PluginHost {
  /**
   * 发一条命令请求，拿一次应答。
   *
   * 为什么在契约里：它是**插件访问宿主的唯一必需入口**（R5 的传输无关面）。
   * 需要宿主能力的插件代码只依赖本方法；换传输时实现本方法即可，
   * 插件代码一行不改。
   *
   * 参数约定：`cmd` 是**已加前缀的完整命令名**（如 `host_events_subscribe`）；
   * `args` 是命令参数对象，无参数时省略。
   * 宿主拒绝时必须以 **reject**（抛错）表达，不得用「成功返回错误对象」伪装。
   */
  request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;

  /**
   * 注册一条声明式贡献（菜单/命令/面板/设置 Tab 的展示条目）。
   *
   * 为什么在契约里：设置 Tab 等贡献的**可发现性**由宿主贡献表决定，插件本地
   * 注册只是去重账本——不喂给宿主就是「注册了但永远不可见」。把它放在契约的
   * 宿主面（而不是藏在各 SDK 内部），是为了让「本地注册」与「宿主可见」之间
   * 的落差在类型上就可见。
   *
   * 为什么可选：并非所有宿主形态都有贡献表；没有该能力的宿主**不得**假装
   * 成功——见 {@link SettingsTabRegistry.registerTab} 的返回值语义。
   */
  contributesRegister?(entry: {
    kind: string;
    id: string;
    label: string;
  }): Promise<void>;
}

// ──────────────────────────────────────────────────────────────────────────
// 命令面
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件命令处理器。
 *
 * 为什么在契约里：命令是插件对宿主的**主要输出面**，两代 SDK 都必须能注册与
 * 派发同一种处理器，否则「同一份插件定义跑在两个 SDK 下」无从谈起。
 *
 * 泛型取 `TArgs`/`TResult` 而不是 `unknown`：声明处由插件作者写出**具体**形状
 * （如 `(args: { name: string }) => ...`）；`unknown` 会在 `strictFunctionTypes`
 * 的逆变规则下让具体形状的处理器无法赋值到该位置。
 *
 * `ctx` 传回的是**契约形状**的上下文：处理器不得依赖某个 SDK 的私有成员。
 */
export interface PluginCommandHandler<TArgs = unknown, TResult = unknown> {
  (args: TArgs, ctx: PluginContext): Promise<TResult> | TResult;
}

/**
 * 命令处理器的存在性形态（抹掉泛型）。
 *
 * 注册表内部只关心「有个处理器」，具体形状由调用方在 `register` 处给出；
 * 用 `unknown` 而非 `never` 是为了让 `execute(id, args)` 的实参可以是任意值。
 */
export type AnyPluginCommandHandler = PluginCommandHandler<unknown, unknown>;

/** 命令注册结果。 */
export interface CommandRegisterResult {
  /** 是否注册成功（重复 `id` 必须返回 `false` 而不是静默覆盖）。 */
  ok: boolean;
  /** 失败原因（人类可读，用于日志与诊断）。 */
  error?: string;
}

/**
 * 命令注册表（插件侧）。
 *
 * 为什么在契约里：`commands` 是插件定义里唯一**双向**的面——插件注册、宿主/
 * 其他插件经 `execute` 派发。两代 SDK 的注册语义必须一致（重复注册拒绝、
 * 未注册命令 `execute` 抛错），否则同一份插件定义的行为会分叉。
 */
export interface CommandRegistry {
  /**
   * 注册一条命令。
   *
   * 语义要求（契约级，不是实现细节）：
   * - 重复 `id` 返回 `{ ok: false, error }`，**不得**静默覆盖已有处理器；
   * - 成功返回 `{ ok: true }`。
   */
  register<TArgs = unknown, TResult = unknown>(
    id: string,
    handler: PluginCommandHandler<TArgs, TResult>,
  ): CommandRegisterResult;

  /** 注销命令；返回「此前是否存在」（不存在返回 `false`，不抛错）。 */
  unregister(id: string): boolean;

  /** 该命令当前是否已注册。 */
  has(id: string): boolean;

  /** 已注册命令 id 列表（顺序 = 注册顺序，便于测试做确定性断言）。 */
  list(): string[];

  /**
   * 派发一条命令。
   *
   * 语义要求：命令不存在必须 **reject**（抛错），不得返回 `undefined` 假成功
   * ——「静默成功」会让调用方把「插件没加载」当成「命令返回空」。
   */
  execute<TArgs = unknown, TResult = unknown>(
    id: string,
    args?: TArgs,
  ): Promise<TResult>;
}

// ──────────────────────────────────────────────────────────────────────────
// 事件面
// ──────────────────────────────────────────────────────────────────────────

/** 事件信封：订阅者收到的第二条参数，给出 topic 与署名插件。 */
export interface PluginEventEnvelope {
  /** 事件 topic（与订阅时使用的一致）。 */
  topic: string;
  /** **发布该事件的插件 id**（订阅方据此区分来源，而不是靠 payload 里自报）。 */
  pluginId: string;
}

/**
 * 插件事件监听器。
 *
 * 为什么在契约里：事件是两个插件之间**唯一的**解耦通道；监听器签名（载荷 +
 * 信封）若在两代 SDK 下不同，跨 SDK 的事件互操作就是不可能的。
 */
export interface PluginEventListener {
  (payload: unknown, event: PluginEventEnvelope): void;
}

/**
 * 事件收发面。
 *
 * 为什么在契约里：`events` 是插件与外部世界（宿主、其他插件）的双向边界，
 * `publish`/`subscribe` 的**语义**必须在两代 SDK 下一致——尤其是
 * 「订阅返回退订函数」与「发布是异步且可失败」这两点。
 */
export interface EventSink {
  /**
   * 发布一条事件到宿主总线。
   *
   * 为什么是 `Promise<void>`：发布是**跨边界**动作，可能失败（未授权、
   * 队列满、传输断开）。同步 `void` 会让失败无处表达。
   */
  publish(topic: string, payload: unknown): Promise<void>;

  /**
   * 订阅一个 topic。
   *
   * 返回**退订函数**（而不是要求订阅者持有自身引用）：
   * - 同一 topic 多个订阅者互不影响（退掉一个不得影响其他）；
   * - 全部退订后必须释放宿主侧订阅，不得留下悬挂订阅；
   * - 重复调用退订函数必须是幂等的。
   */
  subscribe(topic: string, listener: PluginEventListener): () => void;
}

// ──────────────────────────────────────────────────────────────────────────
// 设置面
// ──────────────────────────────────────────────────────────────────────────

/** 设置 Tab 配置。 */
export interface SettingsTabConfig {
  /** Tab id（插件内唯一；重复注册必须被拒绝）。 */
  id: string;
  /** Tab 展示标题。 */
  title: string;
  /**
   * 设置项 schema（可选）。
   *
   * 为什么是 `Record<string, unknown>` 而不是具体 schema 类型：schema 方言属
   * 应用层（宿主/设置中心）决定，契约只承诺「原样透传、不解释」。
   */
  schema?: Record<string, unknown>;
  /** 自定义组件标识（可选，宿主据此选择渲染器）。 */
  component?: string;
}

/**
 * 设置 Tab 注册表（插件侧）。
 *
 * 为什么在契约里：设置中心读的是**宿主贡献表**，而插件侧看到的是本地注册
 * 结果——这条落差曾导致「注册了但永远不可见」。把注册与注销放进契约，
 * 是为了让两代 SDK 在**可观察的本地账本**上行为一致（宿主可见性是宿主面的
 * 可选能力，见 {@link PluginHost.contributesRegister}）。
 */
export interface SettingsTabRegistry {
  /**
   * 注册设置 Tab。
   *
   * 语义要求：重复 `id` 返回 `{ ok: false, error }`；成功返回 `{ ok: true }`。
   * 宿主贡献上报失败**不影响**本地注册结果（贡献是声明式附加物，不得让
   * 插件激活失败），但必须可观测（日志）。
   */
  registerTab(config: SettingsTabConfig): CommandRegisterResult;

  /** 注销本地设置 Tab；返回「此前是否存在」。 */
  unregisterTab(id: string): boolean;
}

// ──────────────────────────────────────────────────────────────────────────
// 日志面
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件日志面。
 *
 * 为什么在契约里：日志是插件唯一的**诊断输出面**，必须自动带上插件署名
 * （`pluginId`）——否则多插件同屏时无法归因。三个级别固定，不引入级别枚举：
 * 级别是宿主渲染策略，不是插件契约。
 */
export interface PluginLog {
  /** 常规信息。 */
  info(message: string, ...args: unknown[]): void;
  /** 可恢复的异常/降级（如宿主贡献上报失败）。 */
  warn(message: string, ...args: unknown[]): void;
  /** 失败与不可恢复错误。 */
  error(message: string, ...args: unknown[]): void;
}

// ──────────────────────────────────────────────────────────────────────────
// 上下文本体
// ──────────────────────────────────────────────────────────────────────────

/**
 * 插件生命周期上下文（**共享契约**）。
 *
 * 为什么在契约里：这是方案 R2 的收敛点——两代 SDK 各自定义的 `PluginContext`
 * 从此共享同一份成员清单与语义，于是「同一份插件定义在两个 SDK 下激活」
 * 成为可验证的事实（互操作测试见 `@tauron/app-plugin-sdk` 的
 * `interop.test.ts`）。
 *
 * 成员取舍（为什么是这六个）：
 * - `pluginId`：日志署名、事件信封、诊断归因都要它，且**只读**（身份由宿主
 *   在构造上下文时给定，插件不得改写）；
 * - `host`：{@link PluginHost}——传输无关的宿主面，换传输只换实现；
 * - `commands`：{@link CommandRegistry}——插件对外的命令输出面；
 * - `events`：{@link EventSink}——插件与外部世界的解耦通道；
 * - `settings`：{@link SettingsTabRegistry}——设置中心的注册面；
 * - `log`：{@link PluginLog}——带署名的诊断输出面。
 *
 * 有意**不**放进契约的成员（它们属某代 SDK 的传输细节，不是插件契约）：
 * `ready` / `permissions` / `onInit` / `invoke` / `emit` / `onEvent` / `destroy`
 * ——这些是 `@tauron/plugin-sdk` 一代的 iframe 握手与 postMessage 面，属过渡
 * API，不得成为「所有插件都必须接受」的形状。
 */
export interface PluginContext {
  /** 插件 id（宿主在构造上下文时给定；只读）。 */
  readonly pluginId: string;

  /** 宿主面：传输无关的请求入口（+ 可选贡献注册）。 */
  readonly host: PluginHost;

  /** 命令注册与派发。 */
  readonly commands: CommandRegistry;

  /** 事件发布与订阅。 */
  readonly events: EventSink;

  /** 设置 Tab 注册。 */
  readonly settings: SettingsTabRegistry;

  /** 带插件署名的日志。 */
  readonly log: PluginLog;
}
