// ──────────────────────────────────────────────────────────────────────────
// ShellController — UI 事件 → ShellClient 桥接。
//
// 职责（只接**宿主命令面真实存在**的动作）：
// - 监听 <oc-title-bar> 的 oc-minimize/oc-maximize/oc-close
// - 监听 <oc-updater-dialog> 的 oc-updater-check / oc-update-start / oc-restart
// - 监听 <oc-plugin-manager> 的 oc-plugin-toggle（→ host_registry_admin）
//
// 事件名与 detail 类型**不再写字面量**：全部取自 `@tauron/shell-events`
// 契约（R3 / C2）。此前两侧各写各的字符串，后果有先例——「开始更新」
// (`oc-update-start`)、「立即重启」(`oc-restart`)、插件启用开关
// (`oc-plugin-toggle`) 曾长期零监听，旧头注还捏造了不存在的
// `oc-updater-install`。改名/漏接现在是编译期或门禁期错误，而非运行时静默断裂。
//
// **接入方域**（本控制器不接，宿主命令面无对应动作）：
// - `oc-tray-item`：托盘是原生菜单，动作在应用装配层（Tauri tray API）
// - `oc-shortcut-change`：快捷键持久化无宿主命令
// - `oc-command-select`：命令执行是应用域（跨窗口调用走 C/D 后端路径）
// - `oc-theme-change` / `oc-toast-action`：主题应用与通知动作属接入方域
//
// 用法：
// ```typescript
// const controller = new ShellController({ backend });
// controller.start();        // 开始监听
// controller.stop();         // 停止监听
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';
import { SHELL_EVENTS, type PluginToggleEventDetail } from '@tauron/shell-events';
import { ShellClient } from './shell-client.js';
import { AdminClient } from './host.js';

export interface ShellControllerOptions {
  backend: Backend;
}

/**
 * ShellController — 将 Web Component 的 CustomEvent 路由到 ShellClient。
 *
 * 框架无关：不依赖 React/Vue/Svelte，纯 DOM 事件监听。
 */
export class ShellController {
  private readonly client: ShellClient;
  private readonly admin: AdminClient;
  private readonly listeners: Array<{ el: EventTarget; type: string; fn: EventListener }> = [];

  constructor(options: ShellControllerOptions) {
    this.client = new ShellClient({ backend: options.backend });
    this.admin = new AdminClient({ backend: options.backend });
  }

  /** ShellClient 实例（用于直接调用）。 */
  get shellClient(): ShellClient {
    return this.client;
  }

  /**
   * 开始监听指定元素的事件。
   *
   * @param elements 要监听的事件目标（默认 document.body）
   */
  start(elements: EventTarget[] = [document.body]): void {
    // 标题栏事件
    this._listen(elements, SHELL_EVENTS.minimize, () => this.client.windowMinimize().catch(console.warn));
    this._listen(elements, SHELL_EVENTS.maximize, () => this.client.windowMaximize().catch(console.warn));
    this._listen(elements, SHELL_EVENTS.close, () => this.client.windowClose().catch(console.warn));

    // 更新对话框事件：检查更新 → marketCheck；开始更新 = 下载 + 安装
    // （宿主侧当前为 simulated 桩，`marketCheck`/`marketDownload` 的返回里
    // 带 `simulated`/`reason`，接入方据此**如实**展示"模拟结果"而不是假装更新过；
    // 对话框的状态展示由接入方通过其 `status` 属性驱动）。
    //
    // 「立即重启」(`oc-restart`)：轮 11 R8 起宿主有 `host_window_relaunch` 了
    // ——它**先**把恢复引擎的阶段判定对账回注册表、**再**请求重启（顺序反了会把
    // "上一次未干净结束"的标记带进下一次启动 → 重启循环）。此前这里连的是
    // `host_window_quit`（当时宿主面确实只有 quit），那等于"重启"按钮只退出、
    // 跳过对账。降级路径（宿主没有重启原语 → `relaunchRequested === false`）
    // **不回退到 quit**：那会变成"点了重启却直接退出且不再起来"，比不动更糟；
    // 只如实把 reason 打到控制台。
    this._listen(elements, SHELL_EVENTS.updaterCheck, () => this.client.marketCheck().catch(console.warn));
    this._listen(elements, SHELL_EVENTS.updateStart, () => {
      void this.client
        .marketDownload()
        .then(() => this.client.marketInstall())
        .catch(console.warn);
    });
    this._listen(elements, SHELL_EVENTS.restart, () => {
      void this.client
        .windowRelaunch()
        .then((outcome) => {
          if (!outcome.relaunchRequested) {
            console.warn(`宿主没有重启原语，未重启：${outcome.reason ?? '原因未知'}`);
          }
        })
        .catch(console.warn);
    });

    // 插件管理器：启用/禁用开关 → host_registry_admin（主窗特权命令）。
    this._listen(elements, SHELL_EVENTS.pluginToggle, (e) => {
      const detail = (e as CustomEvent<PluginToggleEventDetail>).detail;
      if (detail !== undefined) {
        void this.admin
          .registryAdmin({ op: detail.enabled ? 'enable' : 'disable', id: detail.id })
          .catch(console.warn);
      }
    });
  }

  /**
   * 停止所有事件监听。
   */
  stop(): void {
    for (const { el, type, fn } of this.listeners) {
      el.removeEventListener(type, fn);
    }
    this.listeners.length = 0;
  }

  private _listen(targets: EventTarget[], type: string, fn: EventListener): void {
    for (const el of targets) {
      el.addEventListener(type, fn);
      this.listeners.push({ el, type, fn });
    }
  }
}