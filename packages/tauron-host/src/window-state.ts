// ──────────────────────────────────────────────────────────────────────────
// WindowState — 窗口位置/大小持久化（P2-7）。
//
// 职责：
// 1. 保存窗口位置/大小到 localStorage
// 2. 启动时恢复窗口状态
// 3. 提供 defaultState 作为 fallback
//
// 用法：
// ```typescript
// const state = new WindowState({
//   defaultWidth: 1200,
//   defaultHeight: 800,
// });
// await state.restore(); // 恢复窗口状态
// state.save(); // 保存当前窗口状态
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';

/** 窗口状态 */
export interface WindowStateData {
  x: number;
  y: number;
  width: number;
  height: number;
  isMaximized: boolean;
}

/** WindowState 配置 */
export interface WindowStateConfig {
  /** 存储键名（默认 'tauron.window.state'） */
  storageKey?: string;
  /** 默认宽度 */
  defaultWidth?: number;
  /** 默认高度 */
  defaultHeight?: number;
  /** 默认 X 位置 */
  defaultX?: number;
  /** 默认 Y 位置 */
  defaultY?: number;
  /** 是否启用最大化恢复 */
  restoreMaximized?: boolean;
}

/**
 * WindowState — 窗口位置/大小持久化管理器。
 *
 * 使用 localStorage 持久化窗口状态，避免后端依赖。
 */
export class WindowState {
  private readonly _config: Required<WindowStateConfig>;
  private _currentState: WindowStateData;

  constructor(config: WindowStateConfig = {}) {
    this._config = {
      storageKey: config.storageKey ?? 'tauron.window.state',
      defaultWidth: config.defaultWidth ?? 1200,
      defaultHeight: config.defaultHeight ?? 800,
      defaultX: config.defaultX ?? 100,
      defaultY: config.defaultY ?? 100,
      restoreMaximized: config.restoreMaximized ?? true,
    };

    this._currentState = this._loadState();
  }

  /** 当前保存的窗口状态 */
  get currentState(): WindowStateData {
    return { ...this._currentState };
  }

  /**
   * 从 localStorage 加载状态。
   */
  private _loadState(): WindowStateData {
    try {
      const raw = localStorage.getItem(this._config.storageKey);
      if (raw) {
        const parsed = JSON.parse(raw);
        return {
          x: typeof parsed.x === 'number' ? parsed.x : this._config.defaultX,
          y: typeof parsed.y === 'number' ? parsed.y : this._config.defaultY,
          width: typeof parsed.width === 'number' ? parsed.width : this._config.defaultWidth,
          height: typeof parsed.height === 'number' ? parsed.height : this._config.defaultHeight,
          isMaximized: typeof parsed.isMaximized === 'boolean' ? parsed.isMaximized : false,
        };
      }
    } catch {
      // localStorage 不可用或数据损坏，使用默认值
    }
    return this._defaults();
  }

  /** 默认状态 */
  private _defaults(): WindowStateData {
    return {
      x: this._config.defaultX,
      y: this._config.defaultY,
      width: this._config.defaultWidth,
      height: this._config.defaultHeight,
      isMaximized: false,
    };
  }

  /**
   * 保存当前窗口状态到 localStorage。
   */
  save(state: Partial<WindowStateData> = {}): void {
    this._currentState = {
      ...this._currentState,
      ...state,
    };
    try {
      localStorage.setItem(this._config.storageKey, JSON.stringify(this._currentState));
    } catch {
      // localStorage 不可用，静默失败
    }
  }

  /**
   * 清除保存的窗口状态。
   */
  clear(): void {
    try {
      localStorage.removeItem(this._config.storageKey);
    } catch {
      // 静默失败
    }
    this._currentState = this._defaults();
  }

  /**
   * 获取恢复状态（包含默认值 fallback）。
   */
  getRestoreState(): WindowStateData {
    if (!this._config.restoreMaximized) {
      return {
        ...this._currentState,
        isMaximized: false,
      };
    }
    return this._currentState;
  }

  /**
   * 检查状态是否有效（在屏幕范围内）。
   */
  isValid(state: WindowStateData, screenWidth: number, screenHeight: number): boolean {
    return (
      state.width > 100 &&
      state.height > 100 &&
      state.width <= screenWidth &&
      state.height <= screenHeight &&
      state.x >= -screenWidth &&
      state.y >= -screenHeight &&
      state.x + state.width >= 0 &&
      state.y + state.height >= 0 &&
      state.x <= screenWidth &&
      state.y <= screenHeight
    );
  }

  /**
   * 应用窗口状态（调用后端命令）。
   */
  async apply(backend: Backend): Promise<void> {
    const state = this.getRestoreState();
    try {
      await backend.invoke('host_window_set_position', {
        x: state.x,
        y: state.y,
      });
      await backend.invoke('host_window_set_size', {
        width: state.width,
        height: state.height,
      });
      if (state.isMaximized) {
        await backend.invoke('host_window_maximize');
      }
    } catch (err) {
      console.warn('WindowState: failed to apply state', err);
    }
  }
}

/**
 * 创建 WindowState 实例。
 *
 * 便捷工厂函数。
 */
export function createWindowState(config: WindowStateConfig = {}): WindowState {
  return new WindowState(config);
}