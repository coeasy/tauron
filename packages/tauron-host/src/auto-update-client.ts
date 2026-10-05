// ──────────────────────────────────────────────────────────────────────────
// AutoUpdateClient — 自动更新客户端（P2-12）。
//
// 职责：
// 1. 检查更新（checkUpdate）——**走宿主的真更新通道** `host_updater_check`
// 2. 下载更新（downloadUpdate）
// 3. 安装更新（installUpdate）
// 4. 重启应用（relaunch）
// 5. 订阅更新状态变化
//
// ⚠️ **检查与下载/安装今天不在同一个可信度上**（轮 29 的口径，勿在文档里合并）：
// 检查是真通道（宿主注入 `EndpointClient` 后跑 `tauron-distribute` 的清单校验 +
// 灰度 + 签名判定）；下载/安装仍打 `host_market_download` / `host_market_install`，
// 那两条今天是有意的桩（`simulated: true`），本客户端见到 `simulated` 一律抛错。
// 也就是说：**能如实报"有更新可用"，还不能真的把更新装上**——真执行器
// `tauron_distribute::UpgradeRunner` 存在但装配腿未接（见 V7 §7 的 Upgrade 行）。
//
// 用法：
// ```typescript
// const updater = new AutoUpdateClient({ backend, config: { currentVersion: '1.4.2' } });
// const info = await updater.checkUpdate();
// if (info.available) {
//   await updater.downloadUpdate();   // 今天的宿主会在这里如实抛错
//   await updater.installUpdate();
//   await updater.relaunch();
// }
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';
import type { UpdaterStatus } from '@tauron/shell-events';
import { isUnsupportedBody, type ProviderResult } from './dialog-client.js';
import type {
  UpdaterCheckOutcome,
  // 轮 33：`host_updater_status` 的**线读数**。它与上一行的契约词表 `UpdaterStatus`
  // 同名不同物（一个是"更新到哪一步"，一个是"通道现在什么状况"），故在本文件里起别名
  // ——那个名字是 `@tauron/host` 1.0 的公开导出类型，改名是破坏性变更，不在本轮偷改。
  UpdaterStatus as UpdaterChannelStatus,
  WindowRelaunchOutcome,
} from './shell-client.js';

/**
 * 更新状态。**取值的事实源在 `@tauron/shell-events`（轮 32）**：本类型是契约
 * `UpdaterStatus` 的别名，而不是第二份字面量清单。
 *
 * 为什么必须同源：这套字符串不只在本客户端的状态机里流动，它还要写给
 * `<oc-updater-dialog>` 的 `status` 属性（用户可见的主按钮就由它决定）。两侧各列一份
 * 字面量的时候，组件只认 `'done'` 才给「立即重启」、而本客户端的已装好终态叫
 * `'ready'`——那条腿在真装配里永远点不出来。别名之后，写侧新增状态而契约没同步是
 * 编译错误，不是运行时静默。
 */
export type UpdateStatus = UpdaterStatus;

/** 更新信息 */
export interface UpdateInfo {
  /** 是否有可用更新 */
  available: boolean;
  /**
   * 宿主明说"这是模拟结果"时为 `true`；`true` 时不得解释为真实检查结论。
   *
   * 轮 29 起检查腿走真通道，而 `UpdaterCheckOutcome` 线形里**没有** `simulated` 字段，
   * 所以本客户端产出的检查结果该字段一律**不设**（不是设成 `false`：那等于替宿主
   * 声明"我这次是真的"）。它保留是为了兼容既有判据（`info.simulated === true`）与
   * 下载/安装两条桩命令的返回。
   */
  simulated?: boolean;
  /** 模拟或不可用时的宿主说明。 */
  reason?: string | null;
  /** 最新版本号 */
  version: string | null;
  /**
   * 当前版本号（本次检查所依据的版本）。
   *
   * 真相来自**接入方**在 `config.currentVersion` 里声明的值：宿主不把版本当结果给
   * （`host_updater_check` 把它当**入参**要，`host_brand_info` 的 `BrandInfo` 里也
   * **没有** version 字段——那是白标身份，不含版本）。缺它就不检查，见
   * [`AutoUpdateConfig.currentVersion`]。
   */
  currentVersion?: string;
  /** 更新大小（字节） */
  size?: number;
  /** 更新日志 */
  releaseNotes?: string;
  /** 发布时间 */
  publishedAt?: string;
  /**
   * 宿主更新通道**没能给出结论**（端点未注入、端点不可达、清单签名非法等）。
   *
   * 与 `available: false` 是两件事：后者可以是"确实没有更新"（已是最新 / 灰度未覆盖），
   * 前者是"这个问题今天答不了"。UI 只在 `degraded` 为假时才该显示"已是最新版本"。
   */
  degraded?: boolean;
}

/** 下载进度 */
export interface DownloadProgress {
  /** 已下载字节数 */
  downloaded: number;
  /** 总字节数 */
  total: number;
  /** 进度百分比（0-100） */
  percent: number;
}

/** AutoUpdateClient 配置 */
export interface AutoUpdateConfig {
  /**
   * **当前应用版本**（真更新通道的必填入参）。
   *
   * `host_updater_check` 拿它和更新清单比对，才知道"有没有更新的可用"。缺它时
   * `checkUpdate()` **抛错**而不是返回 `available: false`——后者会被 UI 显示成
   * "已是最新版本"，那是拿缺参冒充结论。
   */
  currentVersion?: string;
  /**
   * 更新端点列表（**今天不生效**）。
   *
   * 权威端点由**宿主装配**注入（`DistributeUpdaterSink::with_endpoint`），不由调用方
   * 下发：让 webview 能指定宿主去哪取更新清单，等于把宿主自身的更新通道交给调用方
   * （供应链投毒的第一步）。保留字段是为了兼容既有配置，接线前不要依赖它。
   */
  endpoints?: string[];
  /** 更新公钥（同 [`AutoUpdateConfig.endpoints`]：今天不生效，验证方是宿主）。 */
  pubkey?: string;
  /** 检查间隔（秒），0 = 不自动检查 */
  checkIntervalSecs?: number;
  /** 是否自动下载 */
  autoDownload?: boolean;
}

/**
 * 宿主 `host_updater_check` 的返回 → 本客户端的 `UpdateInfo`（轮 29）。
 *
 * `Unsupported`（端点未注入）与"没有更新"必须落在不同读数上：前者 `degraded: true`，
 * 后者 `degraded: false`。把它们合并成一种"什么都不显示"的空闲，就是拿缺通道冒充
 * "已是最新版本"。
 *
 * 1.0 起本函数**导出**给 `shell-controller.ts` 复用：更新对话框的「检查更新」按钮与
 * `AutoUpdateClient.checkUpdate()` 是同一个用户动作的两条入口，判定必须同源。
 * 两处各写一份"怎么算答不了"就是双镜像点，而轮 29 修的就是这种分叉。
 */
export function toUpdateInfo(
  result: ProviderResult<UpdaterCheckOutcome> | null | undefined,
  currentVersion: string,
): UpdateInfo {
  if (result == null) {
    // 旧实现把空结果当"没有更新"（UI 于是显示"已是最新版本"）。空就是答不了。
    throw new Error('host_updater_check 返回空结果：无法判定可用性，不当作"没有更新"');
  }
  if (isUnsupportedBody(result)) {
    return {
      available: false,
      version: null,
      currentVersion,
      degraded: true,
      reason: result.reason,
    };
  }
  const outcome = result;
  const info: UpdateInfo = {
    available: outcome.available,
    version: outcome.version,
    currentVersion,
    degraded: outcome.degraded,
    reason: outcome.reason ?? null,
  };
  if (outcome.releasedAt !== null) {
    info.publishedAt = outcome.releasedAt;
  }
  return info;
}

/**
 * 更新通道诊断的一行文案（`host_updater_status` 的读数 → 人话）。
 *
 * `state` 与 `stateSimulated` **必须成对出现**在文案里：宿主侧 `update_state` 的
 * 现有写入方是 `host_market_download` / `host_market_install` 两条桩，桩把账本推到
 * `installed:<v>` 是如实的进程内行为，但只看 `state` 就会把"点了一下模拟安装"
 * 显示成"已安装 2.0.0"（轮 33 立这条线字段就是为了堵住这句话）。
 */
function describeUpdaterChannel(status: UpdaterChannelStatus): string {
  const parts = [
    `更新通道${status.available ? '已装配' : '未装配'}`,
    `灰度 ${status.grayscalePercent}%`,
    `崩溃门禁${status.crashGateStopped ? '已停发' : '未停发'}`,
  ];
  if (status.state !== null) {
    parts.push(
      `宿主账本 ${status.state}${status.stateSimulated ? '（模拟推进，未真的装上）' : ''}`,
    );
  }
  if (status.reason !== null) parts.push(status.reason);
  return parts.join('｜');
}

/**
 * 给「没有更新」/「通道答不了」的检查结论补一句通道事实（轮 33）。
 *
 * 为什么需要它：`available: false` 在宿主侧至少对应四种事实——已是最新、不在灰度批次、
 * 被崩溃门禁停发、端点未装配。前三种都只写在 `host_updater_status` 里，而那条命令此前
 * 在 TS 侧**零消费者**：四种事实在 UI 上是同一句"没有更新"。
 *
 * 为什么放在本文件、且做成两条入口共用（`AutoUpdateClient.checkUpdate()` 与
 * `ShellController._checkForUpdate()`）：同一个用户动作的两条入口必须同口径——轮 29 修
 * 的是判定分叉，轮 31 修的是检查腿分叉，展示口径若再各写一份就是第三面镜像。
 *
 * 三条失败边界都**不**改结论：命令缺席、报错、答空（`undefined`）都原样返回 `info`。
 * 诊断是附加读数，拿不到事实时不编造一句"通道已装配"。
 *
 * @param readChannel 读 `host_updater_status` 的函数（控制器给 `client.updaterStatus()`，
 *   本客户端给 `_backend.invoke`）——两条入口的读法不同，**判定与合并口径相同**。
 */
export async function enrichUpdaterInfoWithChannel(
  info: UpdateInfo,
  readChannel: () => Promise<UpdaterChannelStatus>,
): Promise<UpdateInfo> {
  if (info.degraded !== true && info.available !== false) return info;
  const channel = await readChannel().catch(() => null);
  if (!channel) return info;
  const line = describeUpdaterChannel(channel);
  info.reason = info.reason == null ? line : `${line}｜${info.reason}`;
  return info;
}

/**
 * AutoUpdateClient — 自动更新管理器。
 *
 * 状态机：idle → checking → available → downloading → downloaded → installing → ready
 */
export class AutoUpdateClient {
  private readonly _backend: Backend;
  private readonly _config: Required<AutoUpdateConfig>;
  private _status: UpdateStatus = 'idle';
  private _info: UpdateInfo | null = null;
  private _subscribers: Set<(status: UpdateStatus, info: UpdateInfo | null) => void> = new Set();
  private _checkTimer: ReturnType<typeof setInterval> | null = null;
  /**
   * 在飞标记（轮 2）。定时器和 `autoDownload` 都是**自发**的调用源：
   * 慢宿主下一次检查可能比 `checkIntervalSecs` 还久，此前每一拍都会再叠一轮，
   * 同一件事的并发数随时间无上限增长（状态被反复覆写、同一个包被并发下载多次）。
   * 现在的边界：同一类操作最多一处在飞，后到的自发调用直接跳过。
   */
  private _checkingInflight = false;
  private _downloadingInflight = false;

  constructor(options: { backend: Backend; config?: AutoUpdateConfig }) {
    this._backend = options.backend;
    this._config = {
      currentVersion: options.config?.currentVersion?.trim() ?? '',
      endpoints: options.config?.endpoints ?? [],
      pubkey: options.config?.pubkey ?? '',
      checkIntervalSecs: options.config?.checkIntervalSecs ?? 0,
      autoDownload: options.config?.autoDownload ?? false,
    };
  }

  /** 当前状态 */
  get status(): UpdateStatus {
    return this._status;
  }

  /** 更新信息 */
  get info(): UpdateInfo | null {
    return this._info;
  }

  /**
   * 订阅状态变化。
   */
  subscribe(fn: (status: UpdateStatus, info: UpdateInfo | null) => void): () => void {
    this._subscribers.add(fn);
    return () => {
      this._subscribers.delete(fn);
    };
  }

  private _setStatus(status: UpdateStatus, info?: UpdateInfo): void {
    this._status = status;
    if (info) {
      this._info = info;
    }
    for (const fn of this._subscribers) {
      try {
        fn(this._status, this._info);
      } catch {
        // 忽略订阅者异常
      }
    }
  }

  /**
   * 检查更新（走宿主的**真**更新通道）。
   *
   * 三条边界都是"缺参/缺通道不冒充结论"：
   * - 未声明 `currentVersion` → 抛错（返回 `available: false` 会被 UI 显示成"已是最新"）；
   * - 宿主未注入端点（`ProviderResult` 为 `Unsupported`）→ `degraded: true` + 原因，
   *   状态落 `'error'`；
   * - 端点可达但清单签名非法 → 同上（`degraded` 由宿主给，客户端不猜）。
   *
   * 只有 `available: false && !degraded`（"已是最新 / 灰度批次未覆盖"）才落 `'idle'`。
   */
  async checkUpdate(): Promise<UpdateInfo> {
    this._setStatus('checking');

    const currentVersion = this._config.currentVersion;
    if (currentVersion === '') {
      const message =
        '未声明 config.currentVersion：host_updater_check 把当前版本当必填入参，' +
        '缺参时不检查（返回"无更新"等于把缺参冒充成"已是最新版本"）。';
      this._setStatus('error', { available: false, version: null, currentVersion: '' });
      throw new Error(message);
    }

    try {
      const result = await this._backend.invoke<ProviderResult<UpdaterCheckOutcome>>(
        'host_updater_check',
        { currentVersion },
      );
      const info = await enrichUpdaterInfoWithChannel(toUpdateInfo(result, currentVersion), () =>
        this._backend.invoke<UpdaterChannelStatus>('host_updater_status'),
      );

      if (info.available === true) {
        this._setStatus('available', info);

        // 自动下载。downloadUpdate 内部已把失败落为 'error' 并 rethrow——
        // 这里必须接住，否则就是 unhandled rejection（后台自动检查把进程
        // 炸掉）。
        if (this._config.autoDownload) {
          // 已有下载在飞就**不再叠一个**：每一拍检查都可能报 `available`，
          // 无守卫时同一个包会被并发下载多次（轮 2）。
          // 失败仍由 `downloadUpdate` 自己落为 `'error'`；这里只保证不炸进程。
          if (!this._downloadingInflight) {
            this._downloadingInflight = true;
            this.downloadUpdate()
              .catch(() => undefined)
              .finally(() => {
                this._downloadingInflight = false;
              });
          }
        }
      } else {
        // 答不了的问题不该长得像"没有更新"：degraded 时状态落 'error'，让订阅方
        // （托盘徽标、设置页）能区分这两种空闲。
        this._setStatus(info.degraded === true ? 'error' : 'idle', info);
      }

      return info;
    } catch (err) {
      this._setStatus('error', {
        available: false,
        version: null,
        currentVersion,
      });
      throw err;
    }
  }

  /**
   * 下载更新。
   *
   * ⚠️ 桩阶段 `onProgress` **不会**被回调：宿主 `host_market_download`
   * 尚未实现进度通道（下载是一次性命令，成功即完成）。参数保留是为了
   * 接线后不加签名；接入进度通道前不要依赖它。
   */
  async downloadUpdate(_onProgress?: (progress: DownloadProgress) => void): Promise<void> {
    if (!this._info?.available || this._info.simulated === true) {
      if (this._info?.simulated === true) {
        throw new Error(this._info.reason ?? 'Update check is simulated by the host.');
      }
      throw new Error('No update available. Call checkUpdate() first.');
    }

    this._setStatus('downloading');

    try {
      const result = await this._backend.invoke<{
        ok: boolean;
        simulated?: boolean;
        reason?: string | null;
      }>('host_market_download', {
        version: this._info.version ?? undefined,
        endpoints: this._config.endpoints,
        pubkey: this._config.pubkey,
      });

      if (result?.simulated) {
        throw new Error(result.reason ?? 'Update download is simulated by the host.');
      }
      if (result && result.ok === false) {
        throw new Error(result.reason ?? 'Update download failed.');
      }

      this._setStatus('downloaded');
    } catch (err) {
      this._setStatus('error');
      throw err;
    }
  }

  /**
   * 安装更新。
   */
  async installUpdate(): Promise<void> {
    if (this._status !== 'downloaded' || this._info?.simulated === true) {
      if (this._info?.simulated === true) {
        throw new Error(this._info.reason ?? 'Update check is simulated by the host.');
      }
      throw new Error('Update not downloaded. Call downloadUpdate() first.');
    }

    this._setStatus('installing');

    try {
      const result = await this._backend.invoke<{
        ok: boolean;
        simulated?: boolean;
        reason?: string | null;
      }>('host_market_install', {
        version: this._info?.version,
      });

      if (result?.simulated) {
        throw new Error(result.reason ?? 'Update installation is simulated by the host.');
      }
      if (result && result.ok === false) {
        throw new Error(result.reason ?? 'Update installation failed.');
      }

      this._setStatus('ready');
    } catch (err) {
      this._setStatus('error');
      throw err;
    }
  }

  /**
   * 重启应用。
   *
   * 必须走 `host_window_relaunch` 而不是 `host_window_quit`：
   * ① `quit` 只退出、应用不会自己回来——调用方要的是重启，实际拿到的是退出，
   *    更新装上后应用就此消失；
   * ② `relaunch` 会**先**把恢复引擎的阶段判定对账到插件侧再请求重启（Rust 侧
   *    是两行、顺序可断言），`quit` 完全跳过这一步，于是被自适应阶段标记的
   *    插件会带着陈旧标记进入下一轮启动。
   *
   * 降级宿主（没有重启原语）返回 `relaunchRequested: false` 并带 `reason`，
   * 本方法如实把结果交回调用方，**不**把它当作"已重启"。
   */
  async relaunch(): Promise<WindowRelaunchOutcome> {
    return this._backend.invoke<WindowRelaunchOutcome>('host_window_relaunch');
  }

  /**
   * 自发调用的唯一入口（轮 2）。
   *
   * 检查在飞时**直接跳过**：`checkIntervalSecs` 小于一次检查的实际耗时时，
   * 不加守卫就会每拍叠一轮在飞的检查——状态被并发覆写，`autoDownload` 更会
   * 把同一个包并发下载多次。立即检查与定时检查共用这一道门，
   * 否则「第一拍立即 + 第二拍定时」这两条自发路径的边界就不一致。
   */
  private _fireCheck(): void {
    if (this._checkingInflight) {
      return;
    }
    this._checkingInflight = true;
    this.checkUpdate()
      .catch(console.warn)
      .finally(() => {
        this._checkingInflight = false;
      });
  }

  /**
   * 开始自动检查。
   */
  startAutoCheck(): void {
    if (this._config.checkIntervalSecs <= 0) {
      return;
    }

    if (this._checkTimer) {
      clearInterval(this._checkTimer);
    }

    // 立即检查一次
    this._fireCheck();

    // 定期检查（每一拍都过 `_fireCheck` 那道门）
    this._checkTimer = setInterval(() => this._fireCheck(), this._config.checkIntervalSecs * 1000);
  }

  /**
   * 停止自动检查。
   */
  stopAutoCheck(): void {
    if (this._checkTimer) {
      clearInterval(this._checkTimer);
      this._checkTimer = null;
    }
  }

  /**
   * 清理资源。
   */
  destroy(): void {
    this.stopAutoCheck();
    this._subscribers.clear();
  }
}

/**
 * 创建 AutoUpdateClient 实例。
 *
 * 便捷工厂函数。
 */
export function createAutoUpdateClient(options: {
  backend: Backend;
  config?: AutoUpdateConfig;
}): AutoUpdateClient {
  return new AutoUpdateClient(options);
}
