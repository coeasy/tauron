// ──────────────────────────────────────────────────────────────────────────
// ExitAnimation — 退出动画系统（P2-4/P2-5）。
//
// 职责：
// 1. 管理退出动画生命周期（idle → playing → done）
// 2. 执行退出前钩子（beforeExit）
// 3. 播放退出动画（fade/slide-down/scale-out）
// 4. 动画完成后触发 onExit 回调
//
// 用法：
// ```typescript
// const exit = new ExitAnimation({
//   animation: 'fade',
//   duration: 300,
//   beforeExit: async () => { /* 保存状态 */ },
// });
// await exit.play(); // 执行退出
// ```
// ──────────────────────────────────────────────────────────────────────────

/** 退出动画类型 */
export type ExitAnimationType = 'fade' | 'slide-down' | 'scale-out' | 'none';

/** 退出动画配置 */
export interface ExitAnimationConfig {
  /** 动画类型 */
  animation: ExitAnimationType;
  /** 动画时长（ms） */
  duration: number;
  /** 退出前钩子（返回 Promise，resolve 后继续退出） */
  beforeExit?: () => Promise<void> | void;
  /** 退出完成回调 */
  onExit?: () => void;
  /** 取消退出回调 */
  onCancel?: () => void;
}

/** 退出动画阶段 */
export type ExitPhase = 'idle' | 'beforeExit' | 'playing' | 'done' | 'cancelled';

/**
 * ExitAnimation — 退出动画管理器。
 *
 * 生命周期：
 * 1. idle — 初始状态
 * 2. beforeExit — 执行 beforeExit 钩子
 * 3. playing — 播放退出动画
 * 4. done — 动画完成，触发 onExit
 * 5. cancelled — 退出被取消，触发 onCancel
 */
export class ExitAnimation {
  private _phase: ExitPhase = 'idle';
  private readonly _config: ExitAnimationConfig;
  private _subscribers: Set<(phase: ExitPhase) => void> = new Set();

  constructor(config: ExitAnimationConfig) {
    this._config = config;
  }

  /** 当前阶段 */
  get phase(): ExitPhase {
    return this._phase;
  }

  /** 订阅阶段变化 */
  subscribe(fn: (phase: ExitPhase) => void): () => void {
    this._subscribers.add(fn);
    return () => this._subscribers.delete(fn);
  }

  private _setPhase(phase: ExitPhase): void {
    this._phase = phase;
    this._subscribers.forEach((fn) => {
      try {
        fn(phase);
      } catch {
        // 订阅者异常不影响退出流程
      }
    });
  }

  /**
   * 播放退出动画。
   *
   * 流程：
   * 1. 执行 beforeExit 钩子（如果有的话）
   * 2. 播放退出动画
   * 3. 动画完成后触发 onExit
   *
   * 返回 Promise，在退出完成后 resolve。
   */
  async play(): Promise<void> {
    if (this._phase !== 'idle' && this._phase !== 'cancelled') {
      return; // 防止重复播放
    }

    // 1. 执行 beforeExit 钩子
    if (this._config.beforeExit) {
      this._setPhase('beforeExit');
      try {
        await this._config.beforeExit();
      } catch (err) {
        console.error('beforeExit hook failed:', err);
        this._setPhase('cancelled');
        this._config.onCancel?.();
        return;
      }
    }

    // 检查是否被取消
    if ((this as any)._phase === 'cancelled') return;

    // 2. 播放退出动画
    if (this._config.animation !== 'none') {
      this._setPhase('playing');
      await this._playAnimation();
    }

    // 检查是否被取消
    if ((this as any)._phase === 'cancelled') return;

    // 3. 完成
    this._setPhase('done');
    this._config.onExit?.();
  }

  /**
   * 取消退出。
   */
  cancel(): void {
    if (this._phase === 'playing' || this._phase === 'beforeExit') {
      this._setPhase('cancelled');
      this._config.onCancel?.();
    }
  }

  /**
   * 播放退出动画（DOM 操作）。
   */
  private async _playAnimation(): Promise<void> {
    const target = document.body;
    const { animation, duration } = this._config;

    switch (animation) {
      case 'fade':
        return this._fadeAnimation(target, duration);
      case 'slide-down':
        return this._slideDownAnimation(target, duration);
      case 'scale-out':
        return this._scaleOutAnimation(target, duration);
      case 'none':
        return Promise.resolve();
    }
  }

  /** 淡出动画 */
  private _fadeAnimation(target: HTMLElement, duration: number): Promise<void> {
    return new Promise((resolve) => {
      target.style.transition = `opacity ${duration}ms ease-out`;
      target.style.opacity = '0';
      setTimeout(() => {
        target.style.opacity = '';
        target.style.transition = '';
        resolve();
      }, duration);
    });
  }

  /** 下滑动画 */
  private _slideDownAnimation(target: HTMLElement, duration: number): Promise<void> {
    return new Promise((resolve) => {
      target.style.transition = `transform ${duration}ms ease-in, opacity ${duration}ms ease-in`;
      target.style.transform = 'translateY(100%)';
      target.style.opacity = '0';
      setTimeout(() => {
        target.style.transform = '';
        target.style.opacity = '';
        target.style.transition = '';
        resolve();
      }, duration);
    });
  }

  /** 缩放退出动画 */
  private _scaleOutAnimation(target: HTMLElement, duration: number): Promise<void> {
    return new Promise((resolve) => {
      target.style.transition = `transform ${duration}ms ease-in, opacity ${duration}ms ease-in`;
      target.style.transformOrigin = 'center center';
      target.style.transform = 'scale(0.8)';
      target.style.opacity = '0';
      setTimeout(() => {
        target.style.transform = '';
        target.style.opacity = '';
        target.style.transformOrigin = '';
        target.style.transition = '';
        resolve();
      }, duration);
    });
  }
}

/**
 * 创建退出动画实例。
 *
 * 便捷工厂函数。
 */
export function createExitAnimation(config: ExitAnimationConfig): ExitAnimation {
  return new ExitAnimation(config);
}