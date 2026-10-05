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

import type { Backend, Unlisten } from './backend.js';
import {
  SHELL_EVENTS,
  type CommandSelectEventDetail,
  type PluginToggleEventDetail,
  type PluginUninstallEventDetail,
  type PluginInstallEventDetail,
} from '@tauron/shell-events';
import { ShellClient, type NotificationsListResult } from './shell-client.js';
import { NOTIFICATION_TOPIC } from './host-topics.js';
import { isUnsupportedBody } from './dialog-client.js';
import {
  enrichUpdaterInfoWithChannel,
  toUpdateInfo,
  type UpdateInfo,
} from './auto-update-client.js';
import { AdminClient } from './host.js';
import type { PendingCallInfo } from './events.js';
import type { ApprovalRow } from './grants.js';

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
  /**
   * 本次检查所依据的**当前版本号**（更新对话框「检查更新」的入参）。
   *
   * 为什么必须由接入方给：宿主不把版本当结果给——`host_updater_check` 把它当**必填
   * 入参**要，`host_brand_info` 的 `BrandInfo` 里也**没有** version 字段（那是白标
   * 身份，不含版本）。缺它时控制器**不**回落桩命令兜底，见 [`_checkForUpdate`]。
   */
  currentVersion?: string;
  /**
   * 更新检查结果出口（对话框的 `status` / `version` / `message` 由接入方据此驱动）。
   *
   * 读数与 `AutoUpdateClient.checkUpdate()` **同源**（共用 `toUpdateInfo`）：
   * `available: true` 才有新版本号；`degraded: true` 表示宿主通道**答不了**，
   * 此时不得显示"已是最新版本"。不传则与 `onRegistryChange` 同口径——控制器不猜
   * 接入方怎么展示，但结果就没人读（接线点在接入方）。
   */
  onUpdaterCheck?: (info: UpdateInfo) => void | Promise<void>;
  /**
   * 通知快照出口（宿主 `NOTIFICATION_TOPIC` 信号到达后**拉取**到的结果）。
   *
   * 为什么是"信号 + 拉取"而不是"事件里直接给正文"：`Manager::emit` 广播给**所有**
   * webview，通知正文进广播就是跨插件内容泄露（轮 11 把载荷降成
   * `{ id, pluginId, kind, ts }` 的原因）。正文的唯一权威出口是
   * `host_notifications_list`，它按调用方身份过滤。所以监听端拿到的是
   * "有新通知了"，必须再拉一次快照才能上屏——本方法交出的就是这个快照
   * （`unread` 是角标要的真值，`items` 已按时间倒序）。
   *
   * 不传则**根本不订阅**：没有展示出口的订阅只会让每条通知多打一次命令。
   */
  onNotification?: (snapshot: NotificationsListResult) => void | Promise<void>;
  /** 通知快照的拉取条数（`host_notifications_list` 的 `limit`），缺省 20。 */
  notificationLimit?: number;
}

/**
 * ShellController — 将 Web Component 的 CustomEvent 路由到 ShellClient。
 *
 * 框架无关：不依赖 React/Vue/Svelte，纯 DOM 事件监听。
 */
export class ShellController {
  private readonly backend: Backend;
  private readonly client: ShellClient;
  private readonly admin: AdminClient;
  private readonly listeners: Array<{ el: EventTarget; type: string; fn: EventListener }> = [];
  private readonly _onError: (err: unknown, context: string) => void;
  private readonly _commandBudget: { attempts: number; intervalMs: number };
  private readonly _onRegistryChange: (() => void | Promise<void>) | undefined;
  private readonly _currentVersion: string;
  private readonly _onUpdaterCheck: ((info: UpdateInfo) => void | Promise<void>) | undefined;
  private readonly _onNotification:
    ((snapshot: NotificationsListResult) => void | Promise<void>) | undefined;
  private readonly _notificationLimit: number;
  /**
   * 通知腿的代际令牌（与 `tauron-shell-matrix` 的 start/stop 同一纪律）。
   *
   * `listen()` 返回 Promise，`stop()` 可能比它先跑完；拉取快照本身也是异步的，
   * 一条通知的 `host_notifications_list` 回帧完全可能落在 `stop()` 之后。没有
   * 令牌，"已停止的控制器"会继续往接入方的回调里上屏。
   */
  private _notifGeneration = 0;
  private _notifUnlisten: Unlisten | null | 'pending' = null;
  private _notifInFlight: Promise<void> | null = null;
  private _notifDirty = false;

  constructor(options: ShellControllerOptions) {
    this.backend = options.backend;
    this.client = new ShellClient({ backend: options.backend });
    this.admin = new AdminClient({ backend: options.backend });
    this._onError =
      options.onError ?? ((err, context) => console.warn(`ShellController: ${context} 失败`, err));
    this._commandBudget = options.commandResultBudget ?? { attempts: 50, intervalMs: 100 };
    this._onRegistryChange = options.onRegistryChange;
    this._currentVersion = options.currentVersion?.trim() ?? '';
    this._onUpdaterCheck = options.onUpdaterCheck;
    this._onNotification = options.onNotification;
    this._notificationLimit = options.notificationLimit ?? 20;
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

  /**
   * 更新对话框的「检查更新」：打宿主**真更新通道**，判定与
   * `AutoUpdateClient.checkUpdate()` 共用同一个 `toUpdateInfo`。
   *
   * 轮 31 之前这里调的是 `host_market_check`（宿主硬编码桩，恒
   * `available: false` + `simulated: true`），**并把返回值整个丢掉**。后果不是"难看"
   * 而是同一次用户动作得到两个相反的答案：SDK 侧（轮 29 已改读真通道）能报
   * `UpdateAvailable`，壳层对话框却永远显示"没有更新"——宿主注入真端点后依然如此。
   *
   * 缺 `currentVersion` 时**什么都不调**：桩命令会给出一个长得像结论的
   * `available: false`，那等于把"答不了"演成"已是最新版本"（正是轮 29 立的那条口径）。
   */
  private async _checkForUpdate(): Promise<void> {
    const currentVersion = this._currentVersion;
    if (currentVersion === '') {
      this._onError(
        new Error(
          '未配置 currentVersion：host_updater_check 把当前版本当必填入参。' +
            '缺参时不检查，也不回落 host_market_check——那会把"答不了"显示成"已是最新版本"。',
        ),
        'updater.check',
      );
      return;
    }
    const info = toUpdateInfo(await this.client.updaterCheck(currentVersion), currentVersion);
    if (this._onUpdaterCheck === undefined) return;
    // 「没有更新」与「通道答不了」都还需要一句**为什么**（轮 33）：灰度百分比、
    // 崩溃门禁、宿主进程内账本这三件事实只有 `host_updater_status` 一个出口，
    // 而它此前在 TS 侧零消费者——于是"你不在灰度批次""更新被崩溃门禁停发"
    // "宿主根本没配端点"在 UI 上都是同一句含糊的"没有更新"。
    // 合并口径与 SDK 的 `checkUpdate()` 共用 `enrichUpdaterInfoWithChannel`：
    // 同一个用户动作的两条入口若各拼一句，就是轮 29 / 轮 31 修过的那类分叉再来一次。
    await enrichUpdaterInfoWithChannel(info, () => this.client.updaterStatus());
    await Promise.resolve(this._onUpdaterCheck(info)).catch(this._fail('updater.check'));
  }

  /** ShellClient 实例（用于直接调用）。 */
  get shellClient(): ShellClient {
    return this.client;
  }

  /**
   * 通知腿：订阅宿主的投递**信号**，收到信号就去拉快照。
   *
   * 这条腿的三个约束缺一就退回轮 36 修掉的那两个断链上：
   * - **没人订阅就等于没有通知中心**：`emit` 每次返回 `Ok`，前端不接**不会有任何
   *   报错**——与轮 31 那条「检查腿打桩且把返回值整个丢弃」是同一类静默断链，
   *   发出端一路绿灯、用户永远看不到。
   * - **事件载荷里没有正文**（`{ id, pluginId, kind, ts }`，见 [`NOTIFICATION_TOPIC`]
   *   的说明：广播带正文就是跨插件泄露），所以拿到信号后**必须**拉
   *   `host_notifications_list`，那里才按调用方身份过滤。
   * - **突发要合并**：一次操作可以连发 N 条通知，逐条各拉一次就是 N 次命令。
   *   这里在途只允许一个拉取，期间的到达合并成"这轮结束后再拉一次"。
   *
   * 没有 `onNotification` 出口时**不订阅**：控制器不该在没人展示的地方挂一条监听，
   * 那会让每条通知白白多打一次命令。
   */
  private _subscribeNotifications(): void {
    if (this._onNotification === undefined) return;
    const generation = this._notifGeneration;
    void this.backend
      .listen(NOTIFICATION_TOPIC, () => {
        void this._pullNotifications(generation);
      })
      .then((unlisten) => {
        // 订阅回帧可能晚于 `stop()`：令牌不符就地退订，不留悬挂监听。
        if (this._notifGeneration !== generation) {
          unlisten();
          return;
        }
        this._notifUnlisten = unlisten;
      }, this._fail('notification.subscribe'));
  }

  /** 收到信号后的那一次拉取：合并并发、按代际令牌决定是否上屏。 */
  private _pullNotifications(generation: number): void {
    if (this._notifGeneration !== generation) return;
    if (this._notifInFlight !== null) {
      this._notifDirty = true;
      return;
    }
    const pull = async (): Promise<void> => {
      const snapshot = await this.client.notificationsList(this._notificationLimit);
      // 快照回帧同样可能晚于 `stop()`——那时它不该再上屏（轮 25 的同一判据）。
      if (this._notifGeneration !== generation) return;
      // 用 try/await 而不是 `Promise.resolve(cb(x)).catch(...)`：后者在 `cb(x)`
      // **同步抛出**时把错误归给外层，于是"接入方渲染炸了"会被报成"拉取失败"，
      // 排查的人会去查命令面而不是渲染层。这条区分是测试逼出来的（轮 36）。
      try {
        await this._onNotification!(snapshot);
      } catch (err) {
        this._onError(err, 'notification.render');
      }
    };
    this._notifInFlight = pull()
      .catch(this._fail('notification.pull'))
      .then(() => {
        this._notifInFlight = null;
        if (this._notifDirty) {
          this._notifDirty = false;
          this._pullNotifications(generation);
        }
      });
  }

  /**
   * 开始监听指定元素的事件。
   *
   * @param elements 要监听的事件目标（默认 document.body）
   */
  start(elements: EventTarget[] = [document.body]): void {
    // 标题栏事件
    this._listen(elements, SHELL_EVENTS.minimize, () =>
      this.client.windowMinimize().catch(this._fail('window.minimize')),
    );
    this._listen(elements, SHELL_EVENTS.maximize, () =>
      this.client.windowMaximize().catch(this._fail('window.maximize')),
    );
    this._listen(elements, SHELL_EVENTS.close, () =>
      this.client.windowClose().catch(this._fail('window.close')),
    );

    // 更新对话框事件：检查更新 → 宿主**真更新通道** `host_updater_check`
    // （判据见 [`_checkForUpdate`]）；开始更新 = 下载 + 安装（这两条宿主侧仍是
    // `simulated` 桩，返回里带 `simulated`/`reason`，接入方据此**如实**展示"模拟结果"
    // 而不是假装更新过；对话框的状态展示由接入方通过 `status` 属性驱动，
    // 数据从 `onUpdaterCheck` 拿）。
    //
    // 「立即重启」(`oc-restart`)：轮 11 R8 起宿主有 `host_window_relaunch` 了
    // ——它**先**把恢复引擎的阶段判定对账回注册表、**再**请求重启（顺序反了会把
    // "上一次未干净结束"的标记带进下一次启动 → 重启循环）。此前这里连的是
    // `host_window_quit`（当时宿主面确实只有 quit），那等于"重启"按钮只退出、
    // 跳过对账。降级路径（宿主没有重启原语 → `relaunchRequested === false`）
    // **不回退到 quit**：那会变成"点了重启却直接退出且不再起来"，比不动更糟；
    // 只如实把 reason 打到控制台。
    this._listen(elements, SHELL_EVENTS.updaterCheck, () => {
      void this._checkForUpdate().catch(this._fail('updater.check'));
    });
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
            this._onError(new Error(outcome.reason ?? '宿主没有重启原语'), 'window.relaunch');
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
    //
    // 轮 43（A83）：卸载走**预览 → 用户确认 → 提交令牌**两步。生产档宿主强制要求
    // 这条链路（无令牌的 uninstall/purge 以 E_AUTH_DENIED 拒绝）；预览返回将被破坏
    // 的事实（id/版本/状态），确认后才以一次性令牌提交——「点一下按钮就删库」的
    // 无声破坏不再可能发生。
    this._listen(elements, SHELL_EVENTS.pluginUninstall, (e) => {
      const detail = (e as CustomEvent<PluginUninstallEventDetail>).detail;
      if (detail !== undefined) void this._uninstallPlugin(detail.id);
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

    // 宿主 → 前端的投递信号（不是 DOM CustomEvent，走 backend 的 Tauri 事件层）。
    this._subscribeNotifications();
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
          new Error(`命令 \`${commandId}\` 不是任何插件的贡献命令（宿主内建命令的执行属接入方域）`),
          'command.select',
        );
        return;
      }
      const pending = await this.client.callPlugin(entry.pluginId, commandId);
      // `Unsupported` = 宿主没有通往该插件的投递通路（不是「执行失败」）。
      // 不分流就直接读 `pending.callId`：那是 `undefined`，`host_call_take` 会以
      // 非法入参失败，把诚实的「无通路」信号换成 IPC 报错（轮 7 修的正是这条）。
      if (isUnsupportedBody(pending)) {
        this._onError(
          new Error(
            `命令 \`${commandId}\` 无可用投递通路：${pending.reason}` +
              (pending.fallback !== null ? `（建议：${pending.fallback}）` : ''),
          ),
          'command.select',
        );
        return;
      }
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
        this._onError(
          new Error(`命令 \`${commandId}\` 执行失败：${settled.errorCode}`),
          'command.select',
        );
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
   * 停止所有事件监听（含宿主的投信号订阅）。
   */
  stop(): void {
    for (const { el, type, fn } of this.listeners) {
      el.removeEventListener(type, fn);
    }
    this.listeners.length = 0;
    // 通知腿：**先升代再退订**。代际一升，在途拉取的回帧、已排队的信号、
    // 尚未 resolve 的 `listen()` Promise 全都失去上屏资格；订阅句柄若已经拿到，
    // 就地释放，若还在 pending，由 `_subscribeNotifications` 的令牌检查收尾。
    this._notifGeneration++;
    if (typeof this._notifUnlisten === 'function') this._notifUnlisten();
    this._notifUnlisten = null;
    this._notifInFlight = null;
    this._notifDirty = false;
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
        // §4.5 的判定已经在宿主做完（Rust `tauron_acl::build_approval_rows` 单源，轮 55）：
        // `defaultChecked` 就是「这一行该不该默认授予」。壳层此前对每一行都问同一句
        // 「是否授予」，等于把宿主的高危判定原样丢掉——现在按判定决定问法，
        // 并把宿主给的事实（scope、确认词）显示出来，不在本地重算 risk。
        const detail =
          `${preview.pluginName} (${preview.version})\n\n` +
          `${permission.permission}\n${permission.risk}: ${permission.description}` +
          (permission.scope ? `\nscope: ${permission.scope}` : '');
        if (permission.defaultChecked) {
          if (!window.confirm(`${detail}\n\n是否授予此权限？`)) return;
          approved.push(permission.permission);
          continue;
        }
        if (!this._confirmHighRiskRow(detail, permission)) return;
        approved.push(permission.permission);
      }
      const installed = await this.admin.registryInstall(
        packagePath,
        approved,
        preview.reviewToken,
      );
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

  /**
   * 轮 56：宿主判定为「默认不勾」的行必须逐字输入确认词才授予。
   *
   * 三条出口，都不授予时**整次安装中止**（不是跳过这一行继续装——批准集缺一项
   * 交给宿主会得到「未覆盖 manifest」的拒绝，那才是假成功的路径）：
   * - `confirmationHint` 缺失：旧宿主或被改坏的宿主。文案唯一来源是宿主，
   *   壳层不代为拟定，经 `onError` 具名上报。
   * - 用户取消（`prompt` 返回 `null`）：与点「否」同义，静默中止。
   * - 输入与确认词不一致：只提示，不上报——用户改主意不是宿主故障。
   */
  private _confirmHighRiskRow(detail: string, permission: ApprovalRow): boolean {
    const hint = permission.confirmationHint;
    if (!hint) {
      this._onError(
        new Error(
          `宿主未对高危权限 ${permission.permission} 给出确认词（confirmationHint 缺失），` +
            '壳层不代为拟定文案；请升级宿主后重新预览',
        ),
        'plugin.install',
      );
      return false;
    }
    const answer = window.prompt(
      `${detail}\n\n此权限为高危档，默认不授予。逐字输入下方确认词以继续：\n${hint}`,
      '',
    );
    if (answer === null) return false;
    if (answer !== hint) {
      window.alert('确认词与宿主给出的文案不一致，未授予该权限，安装已中止。');
      return false;
    }
    return true;
  }

  /**
   * 轮 43（A83）：卸载的预览 → 确认 → 提交两步链。
   *
   * 第一步（`preview: true`）**不产生任何副作用**：宿主只回报将被破坏的事实
   * （id/版本/状态）并铸发一次性令牌。用户拒绝即止——令牌未用，随 TTL 自然作废。
   * 确认后以原样令牌提交，宿主在动作发生前重核 id/操作/版本（版本漂移 → 拒绝，
   * 必须重新预览）。
   *
   * 老宿主（不认识 `preview`）在第一步就会直接执行，返回里没有 `kind: 'review'`
   * 判别——此时**不再**重复提交，避免二次执行。
   */
  private async _uninstallPlugin(pluginId: string): Promise<void> {
    try {
      const outcome = await this.admin.registryAdmin({
        op: 'uninstall',
        id: pluginId,
        preview: true,
      });
      if (outcome.kind === 'review') {
        const allowed = window.confirm(
          `确定卸载插件 ${outcome.pluginId}（${outcome.version}，当前状态 ${outcome.state}）？\n\n` +
            '此操作将从本机移除该插件。',
        );
        if (!allowed) return;
        await this.admin.registryAdmin({
          op: 'uninstall',
          id: pluginId,
          reviewToken: outcome.reviewToken,
        });
      }
      this._notifyRegistryChange();
    } catch (error) {
      this._fail('plugin.uninstall')(error);
    }
  }
}
