// ──────────────────────────────────────────────────────────────────────────
// ShellController — UI 事件 → ShellClient 桥接。
//
// 职责（只接**宿主命令面真实存在**的动作）：
// - 监听 <oc-title-bar> 的 oc-minimize/oc-maximize/oc-close
// - 监听 <oc-updater-dialog> 的 oc-updater-check / oc-update-start / oc-restart
// - 监听 <oc-plugin-manager> 的 oc-plugin-toggle / oc-plugin-uninstall
//   （→ host_registry_admin 的 enable/disable/uninstall）
// - 监听 <oc-command-palette> 的 oc-command-select（1.0-W3）：按
//   `host_contributes_list` 解析命令归属插件，再经**跨主体调用**投递
//   （`callPlugin` → 轮询 `callTakeResult`）
//
// 事件名与 detail 类型**不再写字面量**：全部取自 `@tauron/shell-events`
// 契约（R3 / C2）。此前两侧各写各的字符串，后果有先例——「开始更新」
// (`oc-update-start`)、「立即重启」(`oc-restart`)、插件启用开关
// (`oc-plugin-toggle`) 曾长期零监听，旧头注还捏造了不存在的
// `oc-updater-install`。改名/漏接现在是编译期或门禁期错误，而非运行时静默断裂。
//
// **仍未接的事件**一律登记在 `./unwired-events.js` 的 `UNWIRED_EVENTS` 里
// （含原因），并由门禁测试断言「每个 `SHELL_EVENTS` 成员要么被本控制器监听、
// 要么在 UNWIRED_EVENTS 中显式登记」——不允许「写句注释就算接线」。
// 当前未接：`oc-tray-item`（原生菜单属应用装配层）、`oc-shortcut-change`
// （无快捷键持久化命令）、`oc-theme-change` / `oc-toast-action`（接入方域）、
// `oc-updater-dismiss`（收起对话框是组件自身行为，无宿主命令）。
//
// 注意 `oc-close` **只**接标题栏的 ✕：更新对话框的「稍后」曾复用它，导致点
// 「稍后」被路由到 `windowClose()` 把主窗口关掉——现已改用 `oc-updater-dismiss`。
//
// 用法：
// ```typescript
// const controller = new ShellController({ backend });
// controller.start();        // 开始监听
// controller.stop();         // 停止监听
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';
import {
  SHELL_EVENTS,
  type CommandSelectEventDetail,
  type PluginToggleEventDetail,
  type PluginUninstallEventDetail,
  type PluginInstallEventDetail,
} from '@tauron/shell-events';
import { ShellClient } from './shell-client.js';
import { AdminClient } from './host.js';
import type { PendingCallInfo } from './events.js';

export interface ShellControllerOptions {
  backend: Backend;
  /**
   * 用户动作失败时的回调（**面向用户**的错误出口）。
   *
   * 为什么需要它：标题栏/更新/插件开关这些动作是**用户直接点的**，此前失败
   * 只 `console.warn`——用户点了"关闭窗口"没反应、点了"启用插件"没反应，
   * 界面上毫无反馈。接入方应把这里接成 toast/内联错误；不传则回落到
   * `console.warn`（保持既有行为，不静默）。
   *
   * @param err 底层错误
   * @param context 出错的用户动作标识（如 `'window.close'`）
   */
  onError?: (err: unknown, context: string) => void;
  /**
   * 贡献命令的取件预算（`_runContributedCommand` 轮询 `host_call_take` 的次数与间隔）。
   *
   * 为什么必须轮询：`callTakeResult` 是**一次性取件**语义——`pending` 表示
   * 「执行方还没回帧」，条目保留、可稍后再取。只取一次就把 `pending` 判成失败，
   * 会把**正常的异步投递**误报为错误：跨 webview 的执行总要几毫秒才回填。
   * 缺省 50 次 × 100ms（5s），与 `examples/minimal-app` 的跨主体调用示例同口径。
   */
  commandResultBudget?: { attempts: number; intervalMs: number };
  /**
   * 注册表变更成功后的回调（安装 / 启用 / 禁用 / 卸载之后触发）。
   *
   * 为什么需要它：`<oc-plugin-manager>` 是**哑组件**，列表由接入方喂
   * （`plugins` 属性，典型来源是 `ShellClient.registryListAll()`）。控制器
   * 改完注册表后若不通知，开关拨了、列表却还是旧的——用户看到「点了没反应」。
   * 接入方在这里重取一次列表即可。不传则什么都不做（保持最小契约）。
   */
  onRegistryChange?: () => void | Promise<void>;
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
  private readonly _onError: (err: unknown, context: string) => void;
  private readonly _commandBudget: { attempts: number; intervalMs: number };
  private readonly _onRegistryChange: (() => void | Promise<void>) | undefined;

  constructor(options: ShellControllerOptions) {
    this.client = new ShellClient({ backend: options.backend });
    this.admin = new AdminClient({ backend: options.backend });
    this._onError =
      options.onError ??
      ((err, context) => console.warn(`ShellController: ${context} 失败`, err));
    this._commandBudget = options.commandResultBudget ?? { attempts: 50, intervalMs: 100 };
    this._onRegistryChange = options.onRegistryChange;
  }

  /** 统一的失败出口：交给 `onError`（缺省回落 console.warn）。 */
  private _fail(context: string): (err: unknown) => void {
    return (err: unknown) => this._onError(err, context);
  }

  /** 通知接入方「注册表变了，请重取列表」。回调自身的失败不吞。 */
  private _notifyRegistryChange(): void {
    if (this._onRegistryChange === undefined) return;
    void Promise.resolve()
      .then(() => this._onRegistryChange!())
      .catch(this._fail('registry.refresh'));
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
    this._listen(elements, SHELL_EVENTS.minimize, () => this.client.windowMinimize().catch(this._fail('window.minimize')));
    this._listen(elements, SHELL_EVENTS.maximize, () => this.client.windowMaximize().catch(this._fail('window.maximize')));
    this._listen(elements, SHELL_EVENTS.close, () => this.client.windowClose().catch(this._fail('window.close')));

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
    this._listen(elements, SHELL_EVENTS.updaterCheck, () => this.client.marketCheck().catch(this._fail('market.check')));
    this._listen(elements, SHELL_EVENTS.updateStart, () => {
      void this.client
        .marketDownload()
        .then(() => this.client.marketInstall())
        .catch(this._fail('market.install'));
    });
    this._listen(elements, SHELL_EVENTS.restart, () => {
      void this.client
        .windowRelaunch()
        .then((outcome) => {
          if (!outcome.relaunchRequested) {
            // 降级不是异常，但用户点了"重启"却没重启，必须让接入方知道。
            this._onError(
              new Error(outcome.reason ?? '宿主没有重启原语'),
              'window.relaunch',
            );
          }
        })
        .catch(this._fail('window.relaunch'));
    });

    // 插件管理器：启用/禁用开关 → host_registry_admin（主窗特权命令）。
    this._listen(elements, SHELL_EVENTS.pluginToggle, (e) => {
      const detail = (e as CustomEvent<PluginToggleEventDetail>).detail;
      if (detail !== undefined) {
        void this.admin
          .registryAdmin({ op: detail.enabled ? 'enable' : 'disable', id: detail.id })
          .then(() => this._notifyRegistryChange())
          .catch(this._fail(`plugin.${detail.enabled ? 'enable' : 'disable'}`));
      }
    });

    // 插件管理器：卸载按钮 → host_registry_admin（主窗特权命令）。
    // 补的是「有接口、无入口」：`host_registry_admin({op:'uninstall'})` 一直存在，
    // 但没有任何按钮能触发它。
    this._listen(elements, SHELL_EVENTS.pluginUninstall, (e) => {
      const detail = (e as CustomEvent<PluginUninstallEventDetail>).detail;
      if (detail !== undefined) {
        void this.admin
          .registryAdmin({ op: 'uninstall', id: detail.id })
          .then(() => this._notifyRegistryChange())
          .catch(this._fail('plugin.uninstall'));
      }
    });
    this._listen(elements, SHELL_EVENTS.pluginInstall, (e) => {
      const detail = (e as CustomEvent<PluginInstallEventDetail>).detail;
      if (detail?.packagePath) void this._installPlugin(detail.packagePath);
    });

    // 命令面板：选中一条命令 → 解析归属插件 → 跨主体调用投递（1.0-W3）。
    //
    // 此前本事件被划为「接入方域」（注释声明即放行），实际后果是**命令面板
    // 点了没反应且没有任何出口可查**——典型的「有派发、无消费者」。现在它接
    // 上真实通路：`host_contributes_list` 找归属 → `host_call_plugin` 投递 →
    // `host_call_take` 取回结果（0.4-A1 的跨主体调用闭环）。
    this._listen(elements, SHELL_EVENTS.commandSelect, (e) => {
      const detail = (e as CustomEvent<CommandSelectEventDetail>).detail;
      if (detail?.id) void this._runContributedCommand(detail.id);
    });
  }

  /**
   * 执行一条被选中的**贡献命令**（1.0-W3）。
   *
   * 解析顺序：`host_contributes_list('command')` 找到该命令的**归属插件**，
   * 再经跨主体调用投递到那个插件（`callPlugin` → 轮询 `callTakeResult`）。
   *
   * **找不到归属 = 不是任何插件的贡献命令**（可能是宿主内建命令，其执行属接入方
   * 域）：如实报错，**不**静默丢弃——否则命令面板点了没反应，且没有任何出口可查。
   */
  private async _runContributedCommand(commandId: string): Promise<void> {
    try {
      const entries = await this.client.contributesList('command');
      const entry = entries.find((c) => c.id === commandId);
      if (entry === undefined) {
        this._onError(
          new Error(
            `命令 \`${commandId}\` 不是任何插件的贡献命令（宿主内建命令的执行属接入方域）`,
          ),
          'command.select',
        );
        return;
      }
      const pending = await this.client.callPlugin(entry.pluginId, commandId);
      const settled = await this._awaitCallResult(pending.callId);
      if (settled === undefined) {
        const { attempts, intervalMs } = this._commandBudget;
        this._onError(
          new Error(
            `命令 \`${commandId}\` 取件超时（${attempts} × ${intervalMs}ms）：` +
              '执行方未回填——确认该插件的 webview 已打开、执行泵在运行',
          ),
          'command.select',
        );
        return;
      }
      if (settled.errorCode !== undefined) {
        this._onError(new Error(`命令 \`${commandId}\` 执行失败：${settled.errorCode}`), 'command.select');
      }
    } catch (err) {
      this._fail('command.select')(err);
    }
  }

  /**
   * 轮询取件直到结算，或预算耗尽。
   *
   * `callTakeResult` 是**一次性**语义：`settled` 取走即删；`pending` 返回副本、
   * 条目保留（「还没好，可稍后再取」）。因此「取一次拿到 `pending` 就报错」是错的
   * ——投递本来就是异步的（跨 webview），首次取件几乎必然 `pending`。
   *
   * @returns 已结算的结果；预算耗尽仍是 `pending` 时返回 `undefined`
   */
  private async _awaitCallResult(callId: string): Promise<PendingCallInfo | undefined> {
    const { attempts, intervalMs } = this._commandBudget;
    for (let i = 0; i < attempts; i++) {
      const info = await this.client.callTakeResult(callId);
      if (info.state === 'settled') return info;
      if (i < attempts - 1 && intervalMs > 0) {
        await new Promise<void>((resolve) => {
          setTimeout(resolve, intervalMs);
        });
      }
    }
    return undefined;
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

  private async _installPlugin(packagePath: string): Promise<void> {
    // 0.4-A2 能力守卫：`host_registry_install*` 是 `plugin-install` feature-gated
    // 的命令，默认构建里**根本不存在**。不先问能力表就 invoke，用户只会拿到
    // 一句 `command not found`（且对话框没有任何可查的出口）。这里按能力表
    // 明确拒绝，并给出可操作的说明。
    if (!this.client.supports('host_registry_install')) {
      this._onError(
        new Error(
          '当前宿主未启用 plugin-install 特性（host_registry_install 未注册），无法安装插件包；' +
            '请重新构建启用该特性的宿主，或改用其他安装方式',
        ),
        'plugin.install',
      );
      return;
    }
    try {
      const preview = await this.admin.registryInstallPreview(packagePath);
      const approved: string[] = [];
      for (const permission of preview.permissions) {
        const allow = window.confirm(`${preview.pluginName} (${preview.version})\n\n${permission.permission}\n${permission.risk}: ${permission.description}\n\n是否授予此权限？`);
        if (!allow) return;
        approved.push(permission.permission);
      }
      const installed = await this.admin.registryInstall(packagePath, approved);
      this._notifyRegistryChange();
      await this.admin.registryAdmin({ op: 'enable', id: installed.pluginId });
      const outcome = await this.client.windowCreate(installed.pluginId);
      if (!outcome.created) {
        this._onError(new Error(outcome.reason ?? '插件页面启动失败'), 'plugin.launch');
        window.alert('插件已安装并启用，但宿主未能启动插件页面。');
        return;
      }
      window.alert('插件已安装、启用并启动。');
    } catch (error) {
      this._onError(error, 'plugin.install');
    }
  }
}
