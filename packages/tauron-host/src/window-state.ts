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
  /**
   * 最近一次持久化/应用失败的原因（成功或从未失败时为 `null`）。
   *
   * 为什么需要它：`localStorage` 在隐私模式、配额耗尽、跨域 iframe 下会抛异常。
   * 此前 `save()` 用空 `catch {}` 吞掉——窗口状态永远存不上，宿主却毫不知情。
   * 现在失败会被**如实记录**，`save()`/`clear()`/`apply()` 也返回布尔结果。
   */
  private _lastError: string | null = null;

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

  /** 最近一次持久化/应用失败的原因（成功或从未失败时为 `null`）。 */
  get lastError(): string | null {
    return this._lastError;
  }

  /** 记录一次失败原因（内部用）。 */
  private _fail(err: unknown): void {
    this._lastError = err instanceof Error ? err.message : String(err);
  }

  /**
   * 从 localStorage 加载状态。
   */
  private _loadState(): WindowStateData {
    try {
      const raw = window.localStorage.getItem(this._config.storageKey);
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
   *
   * @returns `true` = 已写入；`false` = 写入失败（原因见 {@link lastError}）。
   * 内存态始终更新——即便写盘失败，本次会话内读取仍是一致的。
   */
  save(state: Partial<WindowStateData> = {}): boolean {
    this._currentState = {
      ...this._currentState,
      ...state,
    };
    try {
      window.localStorage.setItem(this._config.storageKey, JSON.stringify(this._currentState));
      this._lastError = null;
      return true;
    } catch (err) {
      // localStorage 不可用（隐私模式 / 配额 / 跨域）——如实记录，不静默吞掉。
      this._fail(err);
      return false;
    }
  }

  /**
   * 清除保存的窗口状态。
   *
   * @returns `true` = 已清除；`false` = 清除失败（原因见 {@link lastError}）。
   * 无论成败，内存态都重置为默认值。
   */
  clear(): boolean {
    let ok = true;
    try {
      window.localStorage.removeItem(this._config.storageKey);
      this._lastError = null;
    } catch (err) {
      this._fail(err);
      ok = false;
    }
    this._currentState = this._defaults();
    return ok;
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
   *
   * @returns `true` = 全部命令成功；`false` = 中途失败（原因见 {@link lastError}）。
   * 失败时窗口停在部分应用的状态，调用方可据此决定是否回退到默认尺寸。
   */
  async apply(backend: Backend): Promise<boolean> {
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
      this._lastError = null;
      return true;
    } catch (err) {
      this._fail(err);
      return false;
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
