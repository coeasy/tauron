// ──────────────────────────────────────────────────────────────────────────
// AutoUpdateClient — 自动更新客户端（P2-12）。
//
// 职责：
// 1. 检查更新（checkUpdate）
// 2. 下载更新（downloadUpdate）
// 3. 安装更新（installUpdate）
// 4. 重启应用（relaunch）
// 5. 订阅更新状态变化
//
// 用法：
// ```typescript
// const updater = new AutoUpdateClient({ backend });
// const status = await updater.checkUpdate();
// if (status.available) {
//   await updater.downloadUpdate();
//   await updater.installUpdate();
//   await updater.relaunch();
// }
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';
import type { WindowRelaunchOutcome } from './shell-client.js';

/** 更新状态 */
export type UpdateStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'downloaded' | 'installing' | 'ready' | 'error';

/** 更新信息 */
export interface UpdateInfo {
  /** 是否有可用更新 */
  available: boolean;
  /** 最新版本号 */
  version: string;
  /** 当前版本号 */
  currentVersion: string;
  /** 更新大小（字节） */
  size?: number;
  /** 更新日志 */
  releaseNotes?: string;
  /** 发布时间 */
  publishedAt?: string;
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
  /** 更新端点列表 */
  endpoints?: string[];
  /** 更新公钥 */
  pubkey?: string;
  /** 检查间隔（秒），0 = 不自动检查 */
  checkIntervalSecs?: number;
  /** 是否自动下载 */
  autoDownload?: boolean;
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

  constructor(options: { backend: Backend; config?: AutoUpdateConfig }) {
    this._backend = options.backend;
    this._config = {
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
   * 检查更新。
   */
  async checkUpdate(): Promise<UpdateInfo> {
    this._setStatus('checking');

    try {
      const result = await this._backend.invoke<UpdateInfo>('host_market_check', {
        endpoints: this._config.endpoints,
        pubkey: this._config.pubkey,
      });

      // 桩实现返回 { available: false }；可选链兜底，防宿主返回 Null 时崩溃。
      if (result?.available === true) {
        this._setStatus('available', result);

        // 自动下载。downloadUpdate 内部已把失败落为 'error' 并 rethrow——
        // 这里必须接住，否则就是 unhandled rejection（后台自动检查把进程
        // 炸掉）。
        if (this._config.autoDownload) {
          void this.downloadUpdate().catch(() => undefined);
        }
      } else {
        this._setStatus('idle', result);
      }

      return result;
    } catch (err) {
      this._setStatus('error', {
        available: false,
        version: '',
        currentVersion: '',
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
  async downloadUpdate(onProgress?: (progress: DownloadProgress) => void): Promise<void> {
    if (!this._info?.available) {
      throw new Error('No update available. Call checkUpdate() first.');
    }

    this._setStatus('downloading');

    try {
      await this._backend.invoke('host_market_download', {
        version: this._info.version,
        endpoints: this._config.endpoints,
        pubkey: this._config.pubkey,
      });

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
    if (this._status !== 'downloaded') {
      throw new Error('Update not downloaded. Call downloadUpdate() first.');
    }

    this._setStatus('installing');

    try {
      await this._backend.invoke('host_market_install', {
        version: this._info?.version,
      });

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
    this.checkUpdate().catch(console.warn);

    // 定期检查
    this._checkTimer = setInterval(() => {
      this.checkUpdate().catch(console.warn);
    }, this._config.checkIntervalSecs * 1000);
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
export function createAutoUpdateClient(options: { backend: Backend; config?: AutoUpdateConfig }): AutoUpdateClient {
  return new AutoUpdateClient(options);
}