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
  /** 关闭窗口。detail：无。更新对话框的「稍后」也复用本事件。 */
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
  /** 开始下载 + 安装更新（「开始更新」按钮）。detail：无。 */
  updateStart: 'oc-update-start',
  /** 立即重启应用（`status === 'done'` 时的「立即重启」按钮）。detail：无。 */
  restart: 'oc-restart',

  // ── 命令面板（<oc-command-palette>）────────────────────────────────────
  /**
   * 选中的命令。detail：{@link CommandSelectEventDetail}。
   *
   * 执行属**接入方域**：跨窗口命令调用走 `host_plugin_call` 的 C/D 后端路径，
   * ShellController 不接本事件。
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

/** 无 detail 的壳层事件名（标题栏三键、更新对话框检查/开始、重启）。 */
export type DetailLessShellEvent =
  | typeof SHELL_EVENTS.minimize
  | typeof SHELL_EVENTS.maximize
  | typeof SHELL_EVENTS.close
  | typeof SHELL_EVENTS.updaterCheck
  | typeof SHELL_EVENTS.updateStart
  | typeof SHELL_EVENTS.restart;
