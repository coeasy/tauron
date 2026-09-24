// ──────────────────────────────────────────────────────────────────────────
// UpdaterDialog 状态管理（开发计划 §4.10）。
//
// 职责：更新对话框状态（检查更新、下载、安装、重启）。
//
// 关键约束：
// - 状态机：idle → checking → available → downloading → downloaded → installing → restarting
// - 下载可取消，暂停/恢复
// - 安装前必须已下载完成
// - 重启前必须已安装完成
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 更新对话框状态。 */
export type UpdaterState =
  | { status: 'idle'; currentVersion: string }
  | { status: 'checking'; currentVersion: string }
  | {
      status: 'available';
      currentVersion: string;
      updateVersion: string;
      changelog: string[];
      releaseDate: string;
    }
  | {
      status: 'downloading';
      currentVersion: string;
      updateVersion: string;
      progress: number;
      speed: number;
    }
  | {
      status: 'downloaded';
      currentVersion: string;
      updateVersion: string;
      size: number;
    }
  | { status: 'installing'; currentVersion: string; updateVersion: string }
  | { status: 'restarting'; currentVersion: string; updateVersion: string }
  | { status: 'error'; currentVersion: string; error: string };

/** 更新检查配置。 */
export interface UpdaterConfig {
  /** 当前版本。 */
  currentVersion: string;
  /** 更新检查间隔（毫秒）。 */
  checkInterval: number;
  /** 是否自动下载更新。 */
  autoDownload: boolean;
  /** 是否自动安装更新。 */
  autoInstall: boolean;
  /** 下载速度限制（字节/秒，0 = 无限制）。 */
  maxDownloadSpeed: number;
  /** 重试次数。 */
  retryCount: number;
  /** 更新端点 URL。 */
  endpoint: string;
  /** 渠道（stable/beta/nightly）。 */
  channel: string;
}

/** 更新信息。 */
export interface UpdateInfo {
  /** 更新版本。 */
  version: string;
  /** 变更日志。 */
  changelog: string[];
  /** 发布日期。 */
  releaseDate: string;
  /** 下载 URL。 */
  downloadUrl: string;
  /** 文件大小（字节）。 */
  size: number;
  /** 签名。 */
  signature: string;
}

/** 更新对话框快照。 */
export interface UpdaterSnapshot {
  state: UpdaterState;
  config: UpdaterConfig;
}

/** 更新操作结果。 */
export type UpdaterActionResult =
  | { ok: true }
  | { ok: false; code: string; message: string };

// ──────────────────────────────────────────────────────────────────────────
// UpdaterStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * UpdaterDialog 状态存储。
 *
 * 管理更新对话框的状态机和操作。
 */
export class UpdaterStore {
  private _state: UpdaterState;
  private _config: UpdaterConfig;
  private _pendingUpdate: UpdateInfo | null = null;
  private readonly _subscribers = new Set<(snapshot: UpdaterSnapshot) => void>();

  constructor(config: Partial<UpdaterConfig> = {}) {
    this._config = {
      currentVersion: config.currentVersion ?? '0.1.0',
      checkInterval: config.checkInterval ?? 3600000, // 1 hour
      autoDownload: config.autoDownload ?? false,
      autoInstall: config.autoInstall ?? false,
      maxDownloadSpeed: config.maxDownloadSpeed ?? 0,
      retryCount: config.retryCount ?? 3,
      endpoint: config.endpoint ?? '',
      channel: config.channel ?? 'stable',
    };
    this._state = { status: 'idle', currentVersion: this._config.currentVersion };
  }

  /** 当前快照。 */
  get snapshot(): UpdaterSnapshot {
    return {
      state: this._state,
      config: { ...this._config },
    };
  }

  /** 当前状态。 */
  get state(): UpdaterState {
    return this._state;
  }

  /** 当前配置。 */
  get config(): UpdaterConfig {
    return { ...this._config };
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: UpdaterSnapshot) => void): () => void {
    this._subscribers.add(fn);
    try {
      fn(this.snapshot);
    } catch {
      // 订阅者异常不影响其他订阅者
    }
    return () => {
      this._subscribers.delete(fn);
    };
  }

  /** 通知所有订阅者。 */
  private _notify(): void {
    const snapshot = this.snapshot;
    for (const fn of this._subscribers) {
      try {
        fn(snapshot);
      } catch {
        // 订阅者异常不影响其他订阅者
      }
    }
  }

  /** 设置状态。 */
  private _setState(state: UpdaterState): void {
    this._state = state;
    this._notify();
  }

  /** 设置配置。 */
  setConfig(config: Partial<UpdaterConfig>): void {
    this._config = { ...this._config, ...config };
    this._notify();
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 检查更新。
   */
  checkForUpdate(): UpdaterActionResult {
    if (this._state.status === 'checking') {
      return { ok: false, code: 'E_ALREADY_CHECKING', message: '正在检查更新' };
    }
    if (this._state.status === 'downloading' || this._state.status === 'installing') {
      return { ok: false, code: 'E_ALREADY_UPDATING', message: '正在更新中' };
    }

    this._setState({ status: 'checking', currentVersion: this._config.currentVersion });
    return { ok: true };
  }

  /**
   * 设置可用更新信息（由外部调用）。
   */
  setUpdateAvailable(update: UpdateInfo): void {
    if (this._state.status !== 'checking') {
      return;
    }
    this._pendingUpdate = update;
    this._setState({
      status: 'available',
      currentVersion: this._config.currentVersion,
      updateVersion: update.version,
      changelog: update.changelog,
      releaseDate: update.releaseDate,
    });
  }

  /**
   * 开始下载更新。
   */
  startDownload(): UpdaterActionResult {
    if (this._state.status !== 'available') {
      return { ok: false, code: 'E_NOT_AVAILABLE', message: '没有可用更新' };
    }
    if (!this._pendingUpdate) {
      return { ok: false, code: 'E_NO_UPDATE_INFO', message: '缺少更新信息' };
    }

    this._setState({
      status: 'downloading',
      currentVersion: this._config.currentVersion,
      updateVersion: this._pendingUpdate.version,
      progress: 0,
      speed: 0,
    });
    return { ok: true };
  }

  /**
   * 更新下载进度（由外部调用）。
   */
  updateDownloadProgress(progress: number, speed: number): void {
    if (this._state.status !== 'downloading') {
      return;
    }
    if (this._state.status === 'downloading') {
      this._setState({
        ...this._state,
        progress: Math.min(100, Math.max(0, progress)),
        speed,
      });
    }
  }

  /**
   * 完成下载（由外部调用）。
   */
  completeDownload(size: number): void {
    if (this._state.status !== 'downloading') {
      return;
    }
    this._setState({
      status: 'downloaded',
      currentVersion: this._config.currentVersion,
      updateVersion: this._pendingUpdate?.version ?? '',
      size,
    });
  }

  /**
   * 取消下载。
   */
  cancelDownload(): UpdaterActionResult {
    if (this._state.status !== 'downloading') {
      return { ok: false, code: 'E_NOT_DOWNLOADING', message: '未在下载中' };
    }
    this._setState({
      status: 'available',
      currentVersion: this._config.currentVersion,
      updateVersion: this._pendingUpdate?.version ?? '',
      changelog: this._pendingUpdate?.changelog ?? [],
      releaseDate: this._pendingUpdate?.releaseDate ?? '',
    });
    return { ok: true };
  }

  /**
   * 开始安装。
   */
  startInstall(): UpdaterActionResult {
    if (this._state.status !== 'downloaded') {
      return { ok: false, code: 'E_NOT_DOWNLOADED', message: '更新未下载完成' };
    }
    if (this._state.status === 'downloaded') {
      this._setState({
        status: 'installing',
        currentVersion: this._config.currentVersion,
        updateVersion: this._state.updateVersion,
      });
    }
    return { ok: true };
  }

  /**
   * 完成安装（由外部调用）。
   */
  completeInstall(): void {
    if (this._state.status !== 'installing') {
      return;
    }
    // 安装完成后直接重启
    this._setState({
      status: 'restarting',
      currentVersion: this._config.currentVersion,
      updateVersion: this._state.updateVersion,
    });
  }

  /**
   * 取消安装。
   */
  cancelInstall(): UpdaterActionResult {
    if (this._state.status !== 'installing') {
      return { ok: false, code: 'E_NOT_INSTALLING', message: '未在安装中' };
    }
    this._setState({
      status: 'downloaded',
      currentVersion: this._config.currentVersion,
      updateVersion: this._state.updateVersion,
      size: 0,
    });
    return { ok: true };
  }

  /**
   * 忽略更新。
   */
  dismiss(): UpdaterActionResult {
    if (this._state.status !== 'available') {
      return { ok: false, code: 'E_NOT_AVAILABLE', message: '没有可用更新' };
    }
    this._pendingUpdate = null;
    this._setState({ status: 'idle', currentVersion: this._config.currentVersion });
    return { ok: true };
  }

  /**
   * 设置错误状态（由外部调用）。
   */
  setError(error: string): void {
    this._setState({
      status: 'error',
      currentVersion: this._config.currentVersion,
      error,
    });
  }

  /**
   * 重置到空闲状态。
   */
  reset(): void {
    this._pendingUpdate = null;
    this._setState({ status: 'idle', currentVersion: this._config.currentVersion });
  }
}
