// ──────────────────────────────────────────────────────────────────────────
// ShellClient — 主窗特权客户端（P1 补齐：窗口/设置/通知/i18n/恢复）。
//
// 设计原则：
// - 与 HostClient（插件侧）对称：HostClient 是 self 档，ShellClient 是 privileged 档
// - 所有方法都走 translate_at_boundary → HostException（R2-c 起边界显式）
// - 不持有状态，每次调用都是一次 invoke
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';
import { translate_at_boundary } from './errors.js';
import type { JsonValue } from './events.js';

export interface ShellClientOptions {
  backend: Backend;
}

/** 窗口操作结果。 */
export interface WindowActionResult {
  ok: boolean;
  code?: string;
  message?: string;
}

/**
 * 通知记录（兼容用）。
 *
 * ⚠️ **当前没有任何命令返回它**：`host_notify` 返回 `void`，这份记录只存在于
 * 宿主内部的兼容日志里（有上限）。字段名与 Rust `NotificationRecord` 的
 * camelCase 线形态对齐，但它今天不在线上。
 */
export interface NotificationRecord {
  pluginId: string;
  title: string;
  body: string;
  timestamp: number;
}

/**
 * 单条通知（`host_notifications_list` 条目，主窗通知中心的读取端）。
 *
 * 与 Rust `NotifyEntry` 逐字段 camelCase 映射（`kind` 与
 * `NotifyKind::as_str()` 对齐）。
 */
export interface NotifyItem {
  id: string;
  pluginId: string;
  kind: string;
  title: string;
  message: string;
  /** epoch 毫秒。 */
  ts: number;
  read: boolean;
  /** 附带数据（如跳转链接），缺省 null。 */
  data: JsonValue;
}

/** 一次系统通知投递尝试的结果（Rust `DispatchOutcome::as_str()` 逐字对齐）。 */
export type DispatchOutcome = 'system' | 'degraded' | 'failed';

/**
 * 一次 dispatch **尝试**的日志（`host_notifications_list` 的 `dispatchLog`）。
 *
 * 语义边界（容易误读，故写清）：这是"宿主尝试把通知推给系统"的记录，**不是通知本身**。
 * 两个环互相独立——通知被裁剪后日志仍在，反之亦然。`degraded` 表示系统通道不可用
 * 或拒绝，通知**仍在环形缓冲里**（降级不等于丢失）。`failed` 目前没有生产者。
 */
export interface DispatchRecord {
  /** 对应的通知条目 id（与 `NotifyItem.id` 同源）。 */
  entryId: string;
  outcome: DispatchOutcome;
  /** epoch 毫秒。 */
  ts: number;
}

/** 通知中心快照（`host_notifications_list` 返回值）。 */
export interface NotificationsListResult {
  /** 未读总数——全量计数，不受 `limit` 影响（角标需要真值）。 */
  unread: number;
  /** 存储内通知总数（环形缓冲上限内）。 */
  total: number;
  /** 最近通知，时间倒序。 */
  items: NotifyItem[];
  /**
   * dispatch 尝试日志（环形，上限由宿主给定，**不受 `limit` 影响**）。
   *
   * 未注入系统通道（底座-only 宿主）时为空数组：没有尝试就没有日志，
   * 不伪造 `degraded`。
   */
  dispatchLog: DispatchRecord[];
}

/** 启动恢复标记的加载来源（Rust `LoadSource::as_str()` 逐字对齐）。 */
export type LoadSource = 'disabled' | 'fresh' | 'restored' | 'corrupt';

/** 被禁用的插件条目（`state` 与 Rust `PluginState::as_str()` 逐字对齐）。 */
export interface DisabledPlugin {
  pluginId: string;
  /**
   * 只含禁用态：`disabledPlugins` 由 Rust `disabled_plugins()` 给出，它按
   * `!is_enabled()` 过滤——`TrialEnable` 算启用，所以绝不会出现在这里。
   */
  state: 'disabled-by-safemode' | 'disabled-by-repairmode' | 'permanently-disabled';
}

/** 恢复持久化载体状态（诊断用）。 */
export interface RecoveryPersistence {
  /** 是否启用落盘。宿主没给出数据目录时为 `false`，此时恢复判定只有进程内有效。 */
  enabled: boolean;
  /** 标记文件所在目录。 */
  dir: string | null;
  /** 本轮启动是否仍处于进行中（尚未上报启动结果）。 */
  bootInFlight: boolean;
  /** 最近一次落盘失败原因。持久化是 best-effort：落盘失败不会让命令报错，只在这里暴露。 */
  lastError: string | null;
}

/** 故障类型（由 Rust 侧 `(phase, trial)` 派生，不参与判定、只供展示）。 */
export type RecoveryFailureKind =
  | 'boot-failure'
  | 'safemode-failure'
  | 'repairmode-failure'
  | 'trial-failure';

/**
 * 一条崩溃诊断上下文（`host_recover_boot` 的 `lastContext` 元素，R7）。
 *
 * 用途：安全模式重启后回答"上次到底发生了什么"——旧实现只有标记（知道崩过），
 * 没有上下文（不知道为什么）。环形最近 N 条（宿主取 N = 8），随每次启动/上报重写。
 *
 * 注意：`pluginState` 是**发生时**的快照；上下文只进诊断、不消耗任何插件的
 * 试验失败预算（否则"安全模式里崩过一次"会永久禁掉它的试启用机会）。
 */
export interface RecoveryContextEntry {
  /** epoch 毫秒。 */
  ts: number;
  /** **失败发生时**所处阶段。 */
  phase: 'normal' | 'safemode' | 'repairmode';
  failureKind: RecoveryFailureKind;
  /** 归因到的插件（无归因时为 `null`）。 */
  pluginId: string | null;
  /** 是否发生在试启用（trial）期间。 */
  trial: boolean;
  consecutiveFailures: number;
  safemodeFailures: number;
  pluginState:
    | 'enabled'
    | 'disabled-by-safemode'
    | 'disabled-by-repairmode'
    | 'trial-enable'
    | 'permanently-disabled'
    | null;
}

/** 启动恢复结果（`host_recover_boot` / `host_recover_report` / `host_recover_trial_enable` 的公共部分）。 */
export interface RecoveryBootResult {
  /** 引擎的启动阶段（与 Rust `BootPhase::as_str()` 逐字对齐）。 */
  phase: 'normal' | 'safemode' | 'repairmode';
  /** 阶段的中文展示名（宿主给出，供 UI 直接显示）。 */
  phaseName: string;
  counter: {
    consecutiveFailures: number;
    safemodeFailures: number;
  };
  /** 引擎判定为禁用的插件。 */
  disabledPlugins: DisabledPlugin[];
  /** 必需插件集合（安全模式/修复模式下仍必须加载）。 */
  requiredPlugins: string[];
  /** 本轮启动恢复状态的来源。 */
  loadSource: LoadSource;
  /** 持久化载体状态（诊断用）。 */
  persistence: RecoveryPersistence;
  /**
   * 最近若干次启动/试启用失败的诊断上下文（R7；时间倒序或顺序由宿主给定）。
   *
   * 正常启动也可能非空——上下文是**历史证据**，修复成功后**不清空**
   * （计数器自愈 ≠ 诊断证据消失），用 `ts` 判断新鲜度。
   */
  lastContext: RecoveryContextEntry[];
}

/** `host_recover_report` 的 outcome（闭集，表外值宿主会拒绝）。 */
export type RecoveryOutcome = 'success' | 'failure';

/** 阶段判定对账结果（Rust `PhaseReconcileOutcome`，camelCase）。 */
export interface PhaseReconcile {
  /** 扫描到的注册表插件数。 */
  scanned: number;
  /** 补发的 `SafemodeEnter`。 */
  entered: number;
  /** 补发的 `SafemodeExit`。 */
  exited: number;
  /** 被忽略的非法迁移（例如对已卸载插件补发）。 */
  ignored: number;
}

/**
 * 上报启动结果（`host_recover_report`）——**驱动恢复引擎的唯一入口**。
 *
 * 每轮启动成功一次就调用一次；否则计数器跨进程累积，两次未上报即进安全模式。
 */
export interface RecoveryReportResult extends RecoveryBootResult {
  outcome: RecoveryOutcome;
  /** 引擎实际执行的记录动作（`bootSuccess` / `bootFailure` / 插件态线值）。 */
  engineAction: string;
  /** 本次失败怀疑的插件 id（未带 pluginId 时为 `null`）。 */
  suspectedPlugin: string | null;
  /** 阶段判定落到注册表的结果。 */
  phaseReconcile: PhaseReconcile;
}

/** 试验性启用结果（`host_recover_trial_enable`）。 */
export interface RecoveryTrialResult extends RecoveryBootResult {
  pluginId: string;
  /** `notInRegistry` / `alreadyEnabled` / `trialEnable:<to>` / `trialEnable:rejected`。 */
  engineAction: string;
  /** 发试启事件**之前**的前置对账结果（把应禁用者标成 DISABLED+标志，试启才有合法迁移起点）。 */
  phaseReconcile: PhaseReconcile;
}

/** 重启结果（`host_window_relaunch`，Rust `WindowRelaunchOutcome`）。 */
export interface WindowRelaunchOutcome {
  /** 重启**之前**完成的阶段对账结果——它是"先对账、后重启"在运行时可观测的证据。 */
  reconcile: PhaseReconcile;
  /** 是否**真的**向宿主请求了重启；`false` = 宿主没有重启原语（降级路径）。 */
  relaunchRequested: boolean;
  /** `relaunchRequested = false` 时的原因；成功时为 `null`。 */
  reason: string | null;
}

/** 建窗结果（`host_window_create`，Rust `WindowCreateOutcome`）。 */
export interface WindowCreateOutcome {
  /** 宿主铸出的 label，恒为 `plugin-<插件 id>`（回收键，`cleanup_closed_window` 用）。 */
  label: string;
  /** 目标插件 id。 */
  pluginId: string;
  /** 是否**真的**创建了窗口；`false` = 宿主未实现（降级路径）。 */
  created: boolean;
  /** `created = false` 时的原因；成功时为 `null`。 */
  reason: string | null;
}

/** i18n 引擎状态（`host_i18n_stats` 与装载/切语言的返回公共部分）。 */
export interface I18nState {
  /** 当前语言。 */
  locale: string;
  /** 是否从右向左书写（`ar` / `he` / `fa` / `ur`）。 */
  rtl: boolean;
  /** 当前语言的回退链（当前 → 短形式 → 默认）。 */
  fallbackChain: string[];
  /** 已装载的语言。 */
  registeredLocales: string[];
  /** 每个语言的条目数。 */
  bundleKeys: Record<string, number>;
  /** 累计缺失键次数。回退链**全部**落空才算一次；回退命中不计。 */
  missingTotal: number;
}

/** `host_i18n_load` 的返回。 */
export interface I18nLoadResult extends I18nState {
  /** 本次提交的条目数。 */
  loadedKeys: number;
  /** 其中真正新增的条目数（合并语义下重复 key 不计）。 */
  newKeys: number;
  /** 已加命名空间前缀的插件 id（应用级装载为 `null`）。 */
  pluginId: string | null;
}

/** `host_i18n_cleanup_plugin` 的返回。 */
export interface I18nCleanupResult extends I18nState {
  pluginId: string;
  /** 清除的 key 数。 */
  removed: number;
}

/** 品牌信息。 */
export interface BrandInfo {
  name?: string;
  version?: string;
  [key: string]: unknown;
}

/** 市场检查结果（与 `UpdateInfo.available` 同名对齐；Rust 桩返回 `{available:false}`）。 */
export interface MarketCheckResult {
  /** 是否有可用更新。当前恒 `false`（没有更新源）。 */
  available: boolean;
  /**
   * **是否模拟结果**（R8 起是线字段）。当前恒 `true`：本命令不做任何真实可用性探测。
   *
   * 修正前这个信息只写在注释里，前端**没有任何字段**可以据此判断"这是模拟结果"。
   */
  simulated: boolean;
  /** 可用版本号；无则 `null`。 */
  version: string | null;
  /** 为什么不可用 / 为什么是模拟结果；成功且非模拟时为 `null`。 */
  reason: string | null;
}

/**
 * 下载 / 安装更新的返回（`host_market_download` / `host_market_install`，R8 起有类型）。
 *
 * `ok: true` **不代表更新真的落地**——那要看 `simulated`（当前恒 `true`：
 * 没下载任何字节、没验签、没替换文件）。
 */
export interface MarketUpdateResult {
  ok: boolean;
  /** **是否模拟结果**：当前恒 `true`。 */
  simulated: boolean;
  /** 目标版本号；缺省入参时为 `null`。 */
  version: string | null;
  /** 为什么是模拟结果；真实连线后为 `null`。 */
  reason: string | null;
}

/** 贡献注册条目。 */
export interface ContributeEntry {
  pluginId: string;
  kind: string;
  id: string;
  label: string;
}

/** sidecar 二进制签名（线形 camelCase，与 Rust `RuntimeBinarySignature` 同构）。 */
export interface RuntimeBinarySignature {
  algorithm: string;
  signature: string;
  signerId: string;
}

/**
 * ABI 指纹（与 Rust `RuntimeAbiFingerprint` 同构）。
 *
 * **不含 `generatedAt`**：校验时刻由宿主时钟决定，前端给的时间戳不可信
 * （给了也不能用于防重放）。
 */
export interface RuntimeAbiFingerprint {
  rustVersion: string;
  interfaceHash: string;
}

/** `host_runtime_spawn` 的 `profile` 载荷（与 Rust `RuntimeSpawnProfile` 同构）。 */
export interface RuntimeSpawnProfile {
  /** 缺省用 manifests 里声明的 sidecar 路径。 */
  binaryPath?: string;
  args: string[];
  env: Record<string, string>;
  signature: RuntimeBinarySignature;
  binaryHash: string;
  abi: RuntimeAbiFingerprint;
}

/** 租约句柄：`lease ↔ pid` 一一绑定（重复 spawn 返回同一个）。 */
export interface RuntimeHandle {
  pid: number;
  lease: string;
}

/**
 * 租约回收统计（**全局**：不属某条租约）。
 *
 * 终止失败的唯一可查出口——"杀不掉"必须能被宿主 UI 看到，否则等于只写日志。
 */
export interface ReapStats {
  /** 尝试终止的次数。 */
  attempts: number;
  /** 确认已终止。 */
  terminated: number;
  /** 进程本来就没了（崩溃后回收租约是常态，故与 failures 分开计数）。 */
  alreadyGone: number;
  /** 终止失败次数（含**未注入终止器**——漏注入表现为这里增长，而非"看起来一切正常"）。 */
  failures: number;
  /** 最近一次失败原因。 */
  lastError: string | null;
}

/** sidecar 健康快照（`host_runtime_health`）。 */
export interface RuntimeHealth {
  /** 进程是否仍存活（宿主以 `Child::try_wait` 真探测，不是猜测）。 */
  alive: boolean;
  pid: number;
  /** **每插件**崩溃窗口内的崩溃次数（`tauron-proc` 的 `CrashTracker`）。 */
  crashes: number;
  /** 恢复引擎的**全局**连续启动失败计数（与 `crashes` 语义不同，勿混用）。 */
  consecutiveFailures: number;
  /** 租约回收统计（全局；与上两个计数都不是一回事）。 */
  reap: ReapStats;
}

/**
 * 主窗特权客户端。
 *
 * 用法：
 * ```typescript
 * const shell = new ShellClient({ backend });
 * await shell.windowMinimize();
 * await shell.windowMaximize();
 * await shell.notify('p.audio', 'Title', 'Body');
 * ```
 */
export class ShellClient {
  private readonly backend: Backend;

  constructor(options: ShellClientOptions) {
    this.backend = options.backend;
  }

  private call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return this.backend
      .invoke<T>(cmd, args)
      .catch((err: unknown) => {
        // R2-c：壳命令同属插件/壳 webview → 宿主的边界，走同一条显式翻译。
        throw translate_at_boundary(err, 'plugin-webview→host').error;
      });
  }

  // ── 窗口管理 ──

  /** 最小化窗口。 */
  async windowMinimize(): Promise<void> {
    await this.call('host_window_minimize');
  }

  /** 最大化窗口。 */
  async windowMaximize(): Promise<void> {
    await this.call('host_window_maximize');
  }

  /** 还原窗口（从最大化）。 */
  async windowRestore(): Promise<void> {
    await this.call('host_window_restore');
  }

  /** 关闭窗口。 */
  async windowClose(): Promise<void> {
    await this.call('host_window_close');
  }

  /** 退出应用。 */
  async windowQuit(): Promise<void> {
    await this.call('host_window_quit');
  }

  /**
   * 重启应用（R8）。
   *
   * **顺序不变量**：宿主先对账恢复阶段（`phaseReconcile` 里能看到补发了多少
   * `SafemodeEnter` / `SafemodeExit`），**再**请求重启。顺序反了会把"上一次未干净
   * 结束"的标记带进下一次启动，形成重启循环——返回值里带 `phaseReconcile` 正是
   * 为了在运行时能观测到这个顺序。
   *
   * `relaunchRequested === false` = 宿主没有重启原语（降级路径），此时 `reason`
   * 说明原因——**不要**据此认为应用会重启。
   */
  async windowRelaunch(): Promise<WindowRelaunchOutcome> {
    return this.call<WindowRelaunchOutcome>('host_window_relaunch');
  }

  /**
   * 为**已注册**插件创建窗口（R8；仅主窗可调）。
   *
   * 窗口 label 由宿主铸成 `plugin-<id>`（**不接受调用方指定 label 或 URL**）：
   * label 决定身份，URL 只能来自 manifest 的 `entry.ui`。
   *
   * `created === false` = 宿主未实现创建原语（降级路径），`reason` 说明原因。
   * 插件不在注册表内 → 直接报错（`E_UNKNOWN_PLUGIN`），不会留下半态窗口。
   */
  async windowCreate(
    pluginId: string,
    options: { title?: string; width?: number; height?: number } = {},
  ): Promise<WindowCreateOutcome> {
    return this.call<WindowCreateOutcome>('host_window_create', {
      pluginId,
      ...(options.title !== undefined ? { title: options.title } : {}),
      ...(options.width !== undefined ? { width: options.width } : {}),
      ...(options.height !== undefined ? { height: options.height } : {}),
    });
  }

  // ── 设置 ──

  /** 读取设置值。 */
  async settingsGet(key: string): Promise<JsonValue> {
    return this.call<JsonValue>('host_settings_get', { key });
  }

  /** 写入设置值。 */
  async settingsSet(key: string, value: JsonValue): Promise<void> {
    await this.call('host_settings_set', { key, value });
  }

  /**
   * 承接旧版（v1 裸键平铺）设置文档并标成 v1（R7）。
   *
   * 为什么要显式区分"承接"与"写入"：v1 与 v2 的**键编码契约**不同（v2 走转义键），
   * 直接把旧文档 `settingsSet` 进去会按新契约解释，旧键读不出来。宿主侧不做
   * 隐式改写（隐式迁移意味着改一个无关键也可能重写整层，出错无法审计），
   * 所以由调用方先承接、再显式迁移。
   *
   * `doc` 必须是 JSON 对象（否则宿主返回 `E_INVALID_MANIFEST`）。
   */
  async settingsAdoptLegacy(doc: JsonValue): Promise<void> {
    await this.call('host_settings_adopt_legacy', { doc });
  }

  /**
   * 显式执行 settings 数据版本迁移，返回执行的迁移步数（R7）。
   *
   * `0` = 已是最新（幂等）；链路缺失 / 自环 / 迁移结果过不了 schema 校验时
   * **全有或全无**：一个字节都不改，并返回错误。
   */
  async settingsMigrate(): Promise<number> {
    return this.call<number>('host_settings_migrate');
  }

  // ── 通知 ──

  /** 发送通知。 */
  async notify(pluginId: string, title: string, body: string): Promise<void> {
    await this.call('host_notify', { pluginId, title, body });
  }

  /**
   * 通知中心读取端（`host_notify` 的配对）。
   *
   * 返回未读计数与最近通知（时间倒序，`limit` 缺省 50；`unread` 恒为全量）。
   */
  async notificationsList(limit?: number): Promise<NotificationsListResult> {
    return this.call<NotificationsListResult>(
      'host_notifications_list',
      limit === undefined ? undefined : { limit },
    );
  }

  /** 标记通知已读（`id` 缺省 = 全部）。 */
  async notificationsRead(id?: string): Promise<{ marked: number }> {
    return this.call<{ marked: number }>(
      'host_notifications_read',
      id === undefined ? undefined : { id },
    );
  }

  // ── 启动恢复 ──

  /**
   * 查询启动恢复引擎的阶段决策。
   *
   * **只读**：驱动恢复引擎的唯一入口是 {@link ShellClient.recoverReport}。
   * 每轮启动成功一次就上报一次——否则 `consecutiveFailures` 跨进程累积，
   * 连续两次未上报就会把应用推进安全模式（方向安全：一次上报即自愈）。
   *
   * 插件的安全模式状态请看 `PluginSummary.disabledBySafemode`（注册表状态机
   * 给出，由 {@link ShellClient.recoverReport} 触发的阶段对账写入）。
   */
  async recoverBoot(): Promise<RecoveryBootResult> {
    return this.call<RecoveryBootResult>('host_recover_boot');
  }

  /**
   * 上报本轮启动结果（恢复引擎的驱动信号）。
   *
   * - `success`：计数清零、阶段回 `normal`、注册表的 `disabledBySafemode` 清除。
   * - `failure`：累一次失败；连续两次进安全模式，安全模式下再失败两次进修复模式。
   * - `failure` + `pluginId`：仅当该插件正处 `TrialEnable` 时按**试验失败**记
   *   （只累该插件的试验次数，**不**累入全局计数），1 次即回落
   *   `disabled-by-safemode` 且不可再次试启。
   *
   * 上报是幂等的安全方向：重复上报 `success` 只是重复清零。
   * 闭集校验：`outcome` 只接受 `success` / `failure`，表外值抛
   * `HostException`（`E_INVALID_MANIFEST`），不会静默当成 failure。
   */
  async recoverReport(outcome: RecoveryOutcome, pluginId?: string): Promise<RecoveryReportResult> {
    return this.call<RecoveryReportResult>('host_recover_report', { outcome, pluginId });
  }

  /**
   * 安全模式内试验性启用一个插件。
   *
   * 仅 `phase === 'safemode'` 可用（`'normal'` / `'repairmode'` 下抛
   * `E_STATE_INVALID_TRANSITION`）；该插件已试验失败过一次则抛
   * `E_PLUGIN_DISABLED`。
   *
   * 成功后宿主发 D28 `TrialEnable`：注册表清掉 `disabledBySafemode` 并记入
   * 独立试验预算（`trialFromSafemode` 置位，该插件后续自报错误时按 D28 1 次
   * 即回落）。发事件前有一次前置对账（结果在返回值的 `phaseReconcile` 里），
   * 保证刚装载、尚未被标记的插件停在 DISABLED+标志上、试启不撞非法迁移。
   */
  async recoverTrialEnable(pluginId: string): Promise<RecoveryTrialResult> {
    return this.call<RecoveryTrialResult>('host_recover_trial_enable', { pluginId });
  }

  // ── 市场 ──

  /**
   * 检查更新。
   *
   * **当前恒为模拟结果**（`simulated: true` + `reason` 写明"未接入更新源"）：
   * 没有请求任何 endpoint，`available: false` 是**本地桩结论**而不是网络结论。
   */
  async marketCheck(): Promise<MarketCheckResult> {
    return this.call<MarketCheckResult>('host_market_check');
  }

  /**
   * 下载更新。
   *
   * 返回值现在**有类型**（R8）：`ok` 只表示命令跑通，`simulated: true` 才说明
   * 没下载任何字节。旧签名是 `Promise<void>`，把宿主如实上报的 `simulated`/`reason`
   * 直接丢掉了——调用方于是只能"相信"下载发生了。
   */
  async marketDownload(version?: string): Promise<MarketUpdateResult> {
    return this.call<MarketUpdateResult>(
      'host_market_download',
      version === undefined ? undefined : { version },
    );
  }

  /** 安装更新（同上：`simulated: true` = 没有验签、没有替换文件）。 */
  async marketInstall(version?: string): Promise<MarketUpdateResult> {
    return this.call<MarketUpdateResult>(
      'host_market_install',
      version === undefined ? undefined : { version },
    );
  }

  // ── 品牌 ──

  /** 获取品牌信息。 */
  async brandInfo(): Promise<BrandInfo> {
    return this.call<BrandInfo>('host_brand_info');
  }

  // ── i18n ──

  /**
   * 翻译一个 key。
   *
   * 回退链由宿主负责：当前语言 → 短形式（`zh-CN` → `zh`）→ 默认语言（`en-US`）。
   * **回退链全部落空时返回 key 本身**（不是空串）并计入缺失——让用户看到
   * `oc.settings.title` 比看到空按钮更能暴露缺失文案。前端可用
   * `text === key` 检出缺失翻译；聚合可观测性见 {@link ShellClient.i18nStats}。
   */
  async i18nT(key: string): Promise<string> {
    return this.call<string>('host_i18n_t', { key });
  }

  /**
   * 带 `{{param}}` 占位替换的翻译。缺失语义与 {@link ShellClient.i18nT} 相同。
   *
   * 参数值必须是字符串：数字/对象会在宿主侧被拒（`E_INVALID_MANIFEST`）——
   * 需要数字格式化时请先在应用侧格式化成串。
   */
  async i18nTParams(key: string, params: Record<string, string>): Promise<string> {
    return this.call<string>('host_i18n_t_params', { key, params });
  }

  /**
   * 切换语言（§4.20：语言状态的单一来源）。
   *
   * 非法语言代码（空串 / 超过 35 字符）在宿主侧被拒（`E_INVALID_MANIFEST`），
   * 不会污染回退链。返回切换后的完整 i18n 状态。
   */
  async i18nSetLocale(locale: string): Promise<I18nState> {
    return this.call<I18nState>('host_i18n_set_locale', { locale });
  }

  /**
   * 装载一个语言包。
   *
   * **合并语义**：同一语言已有包时按 key 合并，不整体替换——否则先装应用
   * 文案、再装插件文案会把应用自己的文案覆盖掉。
   *
   * 传 `pluginId` 时每个 key 自动加 `plugin:<id>.oc.` 前缀（§4.20 命名空间）；
   * 已带该前缀的 key 原样保留（幂等）。条目值必须是字符串，否则宿主拒绝整个
   * 调用（`E_INVALID_MANIFEST`）——不会部分生效。
   */
  async i18nLoad(
    locale: string,
    entries: Record<string, string>,
    pluginId?: string,
  ): Promise<I18nLoadResult> {
    return this.call<I18nLoadResult>('host_i18n_load', { locale, entries, pluginId });
  }

  /** i18n 状态与缺失键可观测性。 */
  async i18nStats(): Promise<I18nState> {
    return this.call<I18nState>('host_i18n_stats');
  }

  /**
   * 清除一个插件的全部文案。
   *
   * 插件卸载（`registryAdmin('uninstall'|'purge')`）时宿主**已经**自动调用；
   * 这里独立暴露是给「插件被禁用但仍在注册表里」的情形用。
   */
  async i18nCleanupPlugin(pluginId: string): Promise<I18nCleanupResult> {
    return this.call<I18nCleanupResult>('host_i18n_cleanup_plugin', { pluginId });
  }

  // ── 贡献 ──
  // 注册入口（self 档）在插件侧 `HostClient.contributesRegister`——身份取自
  // webview label，只能在插件自己的窗口里调；主窗 label 会被宿主拒绝，
  // 故本客户端只保留读取（scoped-read）。

  /** 列出贡献（scoped-read 档）。 */
  async contributesList(kind?: string): Promise<ContributeEntry[]> {
    return this.call<ContributeEntry[]>('host_contributes_list', kind ? { kind } : undefined);
  }

  // ── 注册表（全量）──

  /** 列出所有插件（privileged 档，仅主窗可用）。 */
  async registryListAll(): Promise<unknown[]> {
    return this.call<unknown[]>('host_registry_list_all');
  }

  // ── 进程插件运行时（P0-2，privileged 档，仅主窗可用）──

  /**
   * 启动进程插件 sidecar。
   *
   * **幂等**：已有租约时返回既有 `{pid, lease}`，不会起第二个进程（两个并发入口
   * 若都按「尚无租约」判定就会起两个进程而租约只指向其一 → 孤儿 PID）。
   *
   * 失败方向：插件类型不是 `process` → `E_PLUGIN_TYPE_NO_RUNTIME`（**不伪造 pid**）；
   * 签名 / ABI / hash 不合格 → `E_ABI_MISMATCH` / `E_INSTALL_FAILED`。
   */
  async runtimeSpawn(pluginId: string, profile: RuntimeSpawnProfile): Promise<RuntimeHandle> {
    return this.call<RuntimeHandle>('host_runtime_spawn', { pluginId, profile });
  }

  /**
   * 按租约查询 sidecar 健康（不存活时才会计入崩溃窗口，重复轮询不重复计数）。
   *
   * 未知或失效租约 → `E_LEASE_EXPIRED`（租约语义：下一步是重新 spawn，
   * 而不是放弃一次 pending 调用）。
   */
  async runtimeHealth(lease: string): Promise<RuntimeHealth> {
    return this.call<RuntimeHealth>('host_runtime_health', { lease });
  }
}
