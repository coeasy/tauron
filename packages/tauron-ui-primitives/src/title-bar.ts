// ──────────────────────────────────────────────────────────────────────────
// TitleBar 状态管理（开发计划 §4.10）。
//
// 职责：窗口标题栏状态（最小化/最大化/关闭按钮可用性、窗口控制）。
//
// 关键约束：
// - 状态机模型：idle → minimizing → minimized → restoring → idle
// - 最大化/还原互斥切换
// - 关闭按钮可用性受窗口锁定状态影响
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 窗口状态。 */
export type WindowState = 'normal' | 'minimized' | 'maximized';

/** TitleBar 状态。 */
export type TitleBarState =
  | { status: 'idle'; windowState: WindowState }
  | { status: 'minimizing' }
  | { status: 'restoring' };

/** TitleBar 配置。 */
export interface TitleBarConfig {
  /** 窗口标题。 */
  title: string;
  /** 是否显示最小化按钮。 */
  showMinimize: boolean;
  /** 是否显示最大化按钮。 */
  showMaximize: boolean;
  /** 是否显示关闭按钮。 */
  showClose: boolean;
  /** 是否允许用户调整窗口大小。 */
  resizable: boolean;
  /** 是否允许用户最小化窗口。 */
  minimizable: boolean;
  /** 是否允许用户最大化窗口。 */
  maximizable: boolean;
  /** 窗口是否被锁定（禁止关闭/最小化）。 */
  locked: boolean;
  /** 最大化的阈值（点击标题栏多少次切换最大化）。 */
  maximizeThreshold: number;
}

/** TitleBar 快照。 */
export interface TitleBarSnapshot {
  state: TitleBarState;
  config: TitleBarConfig;
  clickCount: number;
}

/** 窗口操作结果。 */
export type WindowActionResult =
  | { ok: true }
  | { ok: false; code: string; message: string };

// ──────────────────────────────────────────────────────────────────────────
// TitleBarStore
// ──────────────────────────────────────────────────────────────────────────

/**
 * TitleBar 状态存储。
 *
 * 管理窗口标题栏的状态机和操作。
 */
export class TitleBarStore {
  private _state: TitleBarState = { status: 'idle', windowState: 'normal' };
  private _config: TitleBarConfig;
  private _clickCount = 0;
  private readonly _subscribers = new Set<(snapshot: TitleBarSnapshot) => void>();

  constructor(config: Partial<TitleBarConfig> = {}) {
    this._config = {
      title: config.title ?? 'Open Client',
      showMinimize: config.showMinimize ?? true,
      showMaximize: config.showMaximize ?? true,
      showClose: config.showClose ?? true,
      resizable: config.resizable ?? true,
      minimizable: config.minimizable ?? true,
      maximizable: config.maximizable ?? true,
      locked: config.locked ?? false,
      maximizeThreshold: config.maximizeThreshold ?? 2,
    };
  }

  /** 当前快照。 */
  get snapshot(): TitleBarSnapshot {
    return {
      state: this._state,
      config: { ...this._config },
      clickCount: this._clickCount,
    };
  }

  /** 当前状态。 */
  get state(): TitleBarState {
    return this._state;
  }

  /** 当前配置。 */
  get config(): TitleBarConfig {
    return { ...this._config };
  }

  /** 订阅状态变化。 */
  subscribe(fn: (snapshot: TitleBarSnapshot) => void): () => void {
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
  private _setState(state: TitleBarState): void {
    this._state = state;
    this._notify();
  }

  /** 设置配置。 */
  setConfig(config: Partial<TitleBarConfig>): void {
    this._config = { ...this._config, ...config };
    this._notify();
  }

  // ──────────────────────────────────────────────────────────────────────
  // Actions
  // ──────────────────────────────────────────────────────────────────────

  /**
   * 最小化窗口。
   *
   * 直接切换窗口状态（窗口动画由 OS 处理，框架无需中间态）。
   * P0-1 修复：移除 `minimizing` 中间态，防止生产环境死锁。
   */
  minimize(): WindowActionResult {
    if (this._config.locked) {
      return { ok: false, code: 'E_WINDOW_LOCKED', message: '窗口已锁定，无法最小化' };
    }
    if (!this._config.minimizable) {
      return { ok: false, code: 'E_NOT_MINIMIZABLE', message: '窗口不允许最小化' };
    }
    if (!this._config.showMinimize) {
      return { ok: false, code: 'E_BUTTON_HIDDEN', message: '最小化按钮不可见' };
    }
    if (this._state.status === 'idle' && this._state.windowState === 'minimized') {
      return { ok: false, code: 'E_ALREADY_MINIMIZED', message: '窗口已最小化' };
    }

    this._setState({ status: 'idle', windowState: 'minimized' });
    return { ok: true };
  }

  /**
   * 还原窗口（从最小化状态）。
   *
   * 直接切换窗口状态。P0-1 修复：移除 `restoring` 中间态。
   */
  restore(): WindowActionResult {
    if (this._state.status !== 'idle') {
      return { ok: false, code: 'E_NOT_MINIMIZED', message: '窗口未最小化' };
    }
    if (this._state.windowState !== 'minimized') {
      return { ok: false, code: 'E_NOT_MINIMIZED', message: '窗口未最小化' };
    }
    this._setState({ status: 'idle', windowState: 'normal' });
    return { ok: true };
  }

  /**
   * 最大化窗口。
   */
  maximize(): WindowActionResult {
    if (this._config.locked) {
      return { ok: false, code: 'E_WINDOW_LOCKED', message: '窗口已锁定' };
    }
    if (!this._config.maximizable) {
      return { ok: false, code: 'E_NOT_MAXIMIZABLE', message: '窗口不允许最大化' };
    }
    if (!this._config.showMaximize) {
      return { ok: false, code: 'E_BUTTON_HIDDEN', message: '最大化按钮不可见' };
    }
    if (this._state.status !== 'idle') {
      return { ok: false, code: 'E_NOT_IDLE', message: '窗口正在操作中' };
    }
    const ws = this._state.windowState;
    if (ws === 'maximized') {
      return { ok: false, code: 'E_ALREADY_MAXIMIZED', message: '窗口已最大化' };
    }

    this._setState({ status: 'idle', windowState: 'maximized' });
    this._clickCount = 0;
    return { ok: true };
  }

  /**
   * 还原窗口（从最大化状态）。
   */
  unmaximize(): WindowActionResult {
    if (this._state.status !== 'idle') {
      return { ok: false, code: 'E_NOT_IDLE', message: '窗口正在操作中' };
    }
    if (this._state.windowState !== 'maximized') {
      return { ok: false, code: 'E_NOT_MAXIMIZED', message: '窗口未最大化' };
    }

    this._setState({ status: 'idle', windowState: 'normal' });
    this._clickCount = 0;
    return { ok: true };
  }

  /**
   * 切换最大化状态。
   *
   * 所有操作都在 `idle` 状态下进行（P0-1 修复后无中间态）。
   */
  toggleMaximize(): WindowActionResult {
    if (this._state.status === 'idle' && this._state.windowState === 'maximized') {
      return this.unmaximize();
    }
    return this.maximize();
  }

  /**
   * 锁定窗口（禁止关闭/最小化）。
   */
  lock(): void {
    this._config.locked = true;
    this._notify();
  }

  /**
   * 解锁窗口。
   */
  unlock(): void {
    this._config.locked = false;
    this._notify();
  }

  /**
   * 记录标题栏点击。
   *
   * 达到阈值时切换最大化状态。
   */
  recordClick(): WindowActionResult {
    this._clickCount++;
    this._notify();

    if (this._clickCount >= this._config.maximizeThreshold) {
      this._clickCount = 0;
      return this.toggleMaximize();
    }

    return { ok: true };
  }

  /**
   * 完成最小化（由外部调用，表示最小化已完成）。
   *
   * P0-1 修复：`minimize()` 已直接同步切换状态，此方法保留向后兼容（no-op）。
   */
  completeMinimize(): void {
    // no-op: minimize() 已同步完成状态切换
  }

  /**
   * 完成还原（由外部调用，表示还原已完成）。
   *
   * P0-1 修复：`restore()` 已直接同步切换状态，此方法保留向后兼容（no-op）。
   */
  completeRestore(): void {
    // no-op: restore() 已同步完成状态切换
  }
}
