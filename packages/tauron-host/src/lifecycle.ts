// ──────────────────────────────────────────────────────────────────────────
// 生命周期线格式词表（与 Rust `tauron_host::lifecycle` 同构）。
//
// Rust 侧是单一真相源：`State` 与 `Event` 都以 SCREAMING_SNAKE_CASE 上线。
// TS 侧只做**消费与上报**，且两者不可混用：
// - 状态（`LIFECYCLE_STATES`）由宿主**单一写入**（§4.3）——插件无法自行宣告状态；
// - 事件（`LIFECYCLE_EVENTS`）是插件上报的迁移触发（`host_lifecycle_report`）。
//
// 把状态名当事件上报会被宿主以反序列化失败确定性拒绝（这正是应用层曾经的
// 断链：`lifecycleReport({ state: 'RUNNING' })` 在 Tauri 端必失败）。
// `@tauron/contract-tests` 的线格式门禁逐名比对两份词表，防止再次漂移。
// ──────────────────────────────────────────────────────────────────────────

/** 插件生命周期状态线名（Rust `State`，宿主单一写入）。 */
export const LIFECYCLE_STATES = [
  'DISCOVERED',
  'INSTALLING',
  'INSTALLED',
  'ENABLED',
  'RUNNING',
  'DISABLED',
  'ERRORED_RETRYABLE',
  'ERRORED_USER_CONFIRM',
  'INSTALL_FAILED',
  'UNINSTALLED',
] as const;

export type LifecycleState = (typeof LIFECYCLE_STATES)[number];

/** 生命周期事件线名（Rust `Event`，插件上报的迁移触发）。 */
export const LIFECYCLE_EVENTS = [
  'INSTALL_START',
  'INSTALL_OK',
  'INSTALL_FAIL',
  'ENABLE',
  'DISABLE',
  'ATTACH',
  'DETACH',
  'ERROR_RETRYABLE',
  'ERROR_FATAL',
  'RETRY_OK',
  'RETRY_EXHAUSTED',
  'TRIAL_ENABLE',
  'SAFEMODE_ENTER',
  'SAFEMODE_EXIT',
  'HEALTH_OK',
  'UNINSTALL',
  'PURGE',
  /**
   * sidecar（进程插件）意外退出（P0-2：`host_runtime_health` 探测到不存活）。
   *
   * 与 `ERROR_RETRYABLE` 同形：崩溃重启是**有预算的自动重试**
   * （`tauron-proc` 的 `CrashLimit`：缺省 3 次 / 5min），预算耗尽才升级为
   * 需用户确认。试验性启用中崩溃按试验失败回落 `DISABLED`（safemode）。
   */
  'RUNTIME_CRASH',
] as const;

export type LifecycleEvent = (typeof LIFECYCLE_EVENTS)[number];

/**
 * 插件（webview 侧）**可自行上报**的事件子集（Rust `Event::PLUGIN_REPORTABLE`）。
 *
 * 线格式枚举 {@link LIFECYCLE_EVENTS} 是完整的——宿主内部路径与管理面要用全部
 * 18 条。但 `host_lifecycle_report` 是 **self 档**入口，插件能自报什么必须比枚举
 * 小一圈，否则这条通道就是越权写状态的捷径：
 *
 * - 自报 `ENABLE` / `TRIAL_ENABLE` / `SAFEMODE_EXIT` → 绕过用户放行与安全模式
 *   （`SAFEMODE_EXIT` 能让插件**自己给自己解禁**）；
 * - 自报 `UNINSTALL` / `PURGE` → 落到终态 `UNINSTALLED`，此后管理面卸载在终态
 *   非法 → 槽位永远收不回；
 * - 自报 `INSTALL_START` / `INSTALL_OK` / `INSTALL_FAIL` → 伪造安装结果
 *   （`INSTALL_OK` 跳过签名校验与权限审批）；
 * - 自报 `DISABLE` / `SAFEMODE_ENTER` → 只坑自己，但同属「被管对象宣告自己的
 *   状态」，破坏 §4.3 单一写入者原则，一并排除。
 *
 * 留下的是宿主**无法从外部观测**的运行时事实。Rust 侧是单一真相源：本数组由
 * `@tauron/contract-tests` 的线格式门禁逐名比对，越权事件写进来会直接红。
 */
export const PLUGIN_REPORTABLE_EVENTS = [
  'ATTACH',
  'DETACH',
  'ERROR_RETRYABLE',
  'ERROR_FATAL',
  'RETRY_OK',
  'RETRY_EXHAUSTED',
  'HEALTH_OK',
  'RUNTIME_CRASH',
] as const satisfies readonly LifecycleEvent[];

export type PluginReportableEvent = (typeof PLUGIN_REPORTABLE_EVENTS)[number];
