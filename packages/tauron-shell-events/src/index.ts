// ──────────────────────────────────────────────────────────────────────────
// @tauron/shell-events — 壳层事件契约（零依赖）。
//
// 解决什么问题（R3 / C2）：
// 此前 `oc-*` 事件名是**隐式字符串契约**——派发侧写在 `@tauron/ui-primitives`
// 的组件模板里，监听侧写在 `@tauron/host` 的 `ShellController` 里，两侧各写各的
// 字面量。后果是有先例的：三个按钮（`oc-update-start` / `oc-restart` /
// `oc-plugin-toggle`）曾长期零监听，而 `oc-updater-check` 反过来是**零派发的
// 孤儿监听**（更新对话框根本没有「检查更新」按钮）。
//
// 本包把事件名与 detail 类型收敛为**唯一事实源**：派发方与监听方都从这里
// import，改名/新增在编译期暴露，而不是运行时静默断裂。
//
// 依赖方向（R3 关键）：本包零依赖。`@tauron/ui-primitives`（哑组件）与
// `@tauron/host`（控制器）都依赖它，但两者互不依赖——底座可只取组件不取宿主。
// ──────────────────────────────────────────────────────────────────────────

/**
 * 壳层 CustomEvent 名常量表。
 *
 * 命名约定：全部以 `oc-`（open client）前缀，全小写连字符。
 * 事件都以 `bubbles: true, composed: true` 派发（跨 Shadow DOM 边界可达）。
 */
export const SHELL_EVENTS = {
  // ── 标题栏（<oc-title-bar>）────────────────────────────────────────────
  /** 最小化窗口。detail：无。 */
  minimize: 'oc-minimize',
  /** 最大化 / 还原窗口。detail：无。 */
  maximize: 'oc-maximize',
  /**
   * 关闭窗口。detail：无。
   *
   * **只有标题栏的 ✕ 该派发本事件**。更新对话框的「稍后」曾复用它，后果是
   * `ShellController` 把 `oc-close` 无条件路由到 `windowClose()`——用户点
   * 「稍后」把主窗口关掉了。对话框的收起走 {@link SHELL_EVENTS.updaterDismiss}。
   */
  close: 'oc-close',

  // ── 托盘菜单（<oc-tray-menu>）──────────────────────────────────────────
  /**
   * 托盘菜单项被点击。detail：{@link TrayItemEventDetail}。
   *
   * 注：原生托盘菜单本身由应用装配层的 Tauri tray API 建立；本事件只在
   * 「组件即菜单」的嵌入形态下有意义。
   */
  trayItem: 'oc-tray-item',

  // ── 更新对话框（<oc-updater-dialog>）──────────────────────────────────
  /** 请求检查更新（「检查更新」按钮）。detail：无。 */
  updaterCheck: 'oc-updater-check',
  /**
   * 用户选择「稍后」（收起更新对话框）。detail：无。
   *
   * 语义是**关闭对话框本身**，不是关闭窗口——此前该按钮错误地派发
   * {@link SHELL_EVENTS.close}，被 `ShellController` 当成关闭主窗口。
   * 组件自身收到本动作后会收起；接入方若想持久化「稍后提醒」偏好，
   * 可监听本事件并落到自己的存储（宿主命令面无更新偏好命令）。
   */
  updaterDismiss: 'oc-updater-dismiss',
  /** 开始下载 + 安装更新（「开始更新」按钮）。detail：无。 */
  updateStart: 'oc-update-start',
  /**
   * 立即重启应用（`status === 'ready'` 时的「立即重启」按钮）。detail：无。
   *
   * 词表见 {@link UPDATER_STATUSES}：`'ready'` 是"已下载并安装完成、等重启"的**唯一**
   * 触发态。本事件曾长期只能靠接入方自己想到一个 `'done'` 才点得出来（轮 32 修）。
   */
  restart: 'oc-restart',

  // ── 命令面板（<oc-command-palette>）────────────────────────────────────
  /**
   * 选中的命令。detail：{@link CommandSelectEventDetail}。
   *
   * 消费方是 `ShellController`（1.0-W3 起已接线）：它按
   * `host_contributes_list('command')` 解析该命令的**归属插件**，再经跨主体调用
   * 投递（`host_plugin_call` → 轮询 `host_call_take` 取回结果）。找不到归属
   * （即宿主内建命令）时**如实报错**，不静默丢弃。
   *
   * 此前这里写的是「执行属接入方域、ShellController 不接本事件」——那句话
   * 配上「注释声明即放行」的门禁，让命令面板长期「点了没反应且无处可查」。
   */
  commandSelect: 'oc-command-select',

  // ── 快捷键录入（<oc-shortcut-recorder>）────────────────────────────────
  /**
   * 快捷键变更（录入成功或清除）。detail：{@link ShortcutChangeEventDetail}。
   *
   * 持久化属**接入方域**：宿主命令面当前没有快捷键存储命令。
   */
  shortcutChange: 'oc-shortcut-change',

  // ── 插件管理器（<oc-plugin-manager>）──────────────────────────────────
  /** 插件启用/禁用开关切换。detail：{@link PluginToggleEventDetail}。 */
  pluginToggle: 'oc-plugin-toggle',
  /**
   * 请求卸载插件（「卸载」按钮）。detail：{@link PluginUninstallEventDetail}。
   *
   * 补的是「有接口、无入口」：`PluginManagerStore.uninstall()` 与宿主的
   * `host_registry_admin({op:'uninstall'})` 都在，但此前没有任何按钮能触发。
   */
  pluginUninstall: 'oc-plugin-uninstall',
  pluginInstall: 'oc-plugin-install',

  // ── 主题选择器（<oc-theme-picker>）────────────────────────────────────
  /**
   * 主题切换。detail：{@link ThemeChangeEventDetail}。
   *
   * 主题应用属接入方域（改 CSS 变量 / `data-theme`），宿主命令面无主题命令。
   */
  themeChange: 'oc-theme-change',

  // ── 通知（<oc-toast>）─────────────────────────────────────────────────
  /** 通知内的动作按钮被点击。detail：{@link ToastActionEventDetail}。 */
  toastAction: 'oc-toast-action',
} as const;

/** 壳层事件名联合类型（`SHELL_EVENTS` 的值域）。 */
export type ShellEventName = (typeof SHELL_EVENTS)[keyof typeof SHELL_EVENTS];

/** 事件名清单一览（门禁与测试用；顺序与 `SHELL_EVENTS` 声明一致）。 */
export const SHELL_EVENT_NAMES: readonly ShellEventName[] = Object.values(SHELL_EVENTS);

// ──────────────────────────────────────────────────────────────────────────
// detail 类型（与上述事件一一对应）
// ──────────────────────────────────────────────────────────────────────────

/** `oc-tray-item` 事件详情。 */
export interface TrayItemEventDetail {
  /** 被点击菜单项的 ID。 */
  id: string;
}

/** `oc-command-select` 事件详情。 */
export interface CommandSelectEventDetail {
  /** 被选中命令的 ID。 */
  id: string;
}

/** `oc-shortcut-change` 事件详情。 */
export interface ShortcutChangeEventDetail {
  /** 新的快捷键字符串（空串表示已清除）。 */
  shortcut: string;
}

/** `oc-plugin-toggle` 事件详情。 */
export interface PluginToggleEventDetail {
  /** 插件 ID。 */
  id: string;
  /** 请求切换到的启用状态（当前状态取反）。 */
  enabled: boolean;
}

/** `oc-plugin-uninstall` 事件详情。 */
export interface PluginUninstallEventDetail {
  /** 插件 ID。 */
  id: string;
}

/** Main-window local package install request; permission approval follows separately. */
export interface PluginInstallEventDetail {
  packagePath: string;
}

/** `oc-theme-change` 事件详情。 */
export interface ThemeChangeEventDetail {
  /** 主题 ID。 */
  themeId: string;
  /** 主题展示名。 */
  themeName: string;
  /** 是否深色主题。 */
  isDark: boolean;
}

/** `oc-toast-action` 事件详情。 */
export interface ToastActionEventDetail {
  /** 通知 ID。 */
  toastId: string;
  /** 动作 ID。 */
  actionId: string;
  /** 动作按钮文案。 */
  label: string;
}

/** 无 detail 的壳层事件名（标题栏三键、更新对话框检查/稍后/开始、重启）。 */
export type DetailLessShellEvent =
  | typeof SHELL_EVENTS.minimize
  | typeof SHELL_EVENTS.maximize
  | typeof SHELL_EVENTS.close
  | typeof SHELL_EVENTS.updaterCheck
  | typeof SHELL_EVENTS.updaterDismiss
  | typeof SHELL_EVENTS.updateStart
  | typeof SHELL_EVENTS.restart;

// ──────────────────────────────────────────────────────────────────────────
// 更新流程状态词表（轮 32）
// ──────────────────────────────────────────────────────────────────────────

/**
 * 更新流程的状态取值——**唯一事实源**。
 *
 * 为什么契约包里要放一份"状态"而不只是"事件名"：本包立的规则是"派发方与监听方
 * 同源"。更新对话框正是这样一个两侧共用的面——
 * - 写侧是 `@tauron/host` 的 `AutoUpdateClient`（它有自己的 `UpdateStatus`），
 * - 读侧是 `@tauron/ui-primitives` 的 `<oc-updater-dialog>`（`status` 属性决定
 *   主按钮是「检查更新」「开始更新」还是「立即重启」）。
 *
 * 轮 32 之前这两侧各用一套字符串：组件只认 `'done'` 才显示「立即重启」，而客户端
 * 安装成功的终态叫 `'ready'`，全仓没有任何一方产出 `'done'`——于是那条腿在真装配里
 * **点不出来**，`'ready'` 反倒落进兜底分支显示「开始更新」（会把下载+安装重跑一遍）。
 * 组件的 `status` 当时是裸 `string`，漂移无处可查。
 *
 * 取值本身**沿用客户端那套**（它已是对外 API，改名属破坏性变更）；新增的是
 * "组件侧必须用同一套"这条约束。词表里刻意**没有** `'done'` / `'updating'`：
 * 它们从未被任何生产者产出过，留着只会再养出一套平行词表。
 */
export const UPDATER_STATUSES = [
  'idle',
  'checking',
  'available',
  'downloading',
  'downloaded',
  'installing',
  'ready',
  'error',
] as const;

/** {@link UPDATER_STATUSES} 的类型形态。 */
export type UpdaterStatus = (typeof UPDATER_STATUSES)[number];

/**
 * 每个状态对应更新对话框的主按钮动作（渲染侧与接入侧共用的判据）。
 *
 * - `'idle'` / `'checking'` / `'error'` → 派发 {@link SHELL_EVENTS.updaterCheck}：
 *   `'error'` 的语义是"这一轮答不了/失败了"，重试是允许的，它不等于"已装好"；
 * - `'available'` / `'downloaded'` → 派发 {@link SHELL_EVENTS.updateStart}：
 *   两条都归"把更新装上"这一个用户动作。控制器那条腿今天把下载与安装**连成一次**
 *   （`market-download` → `market-install`），所以 `'downloaded'` 也走同一个按钮；
 *   把它拆成"只安装"需要新的命令面动作，属命令面冻结外的话题，不在本轮；
 * - `'downloading'` / `'installing'` → 同上，但组件额外显示进度条；
 * - `'ready'` → 派发 {@link SHELL_EVENTS.restart}：**唯一**能触发重启按钮的状态。
 */
export const UPDATER_PRIMARY_ACTION: Record<
  UpdaterStatus,
  typeof SHELL_EVENTS.updaterCheck | typeof SHELL_EVENTS.updateStart | typeof SHELL_EVENTS.restart
> = {
  idle: SHELL_EVENTS.updaterCheck,
  checking: SHELL_EVENTS.updaterCheck,
  error: SHELL_EVENTS.updaterCheck,
  available: SHELL_EVENTS.updateStart,
  downloaded: SHELL_EVENTS.updateStart,
  downloading: SHELL_EVENTS.updateStart,
  installing: SHELL_EVENTS.updateStart,
  ready: SHELL_EVENTS.restart,
};
