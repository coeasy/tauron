// ──────────────────────────────────────────────────────────────────────────
// useAnimation — 框架动画 hook（P3-2）。
//
// 职责：
// 1. 提供 Lit 组件可用的动画 hook
// 2. 集成动效系统（motion.ts）
// 3. 尊重 prefers-reduced-motion
// 4. 支持 enter/leave 动画
//
// 用法（Lit 组件）：
// ```typescript
// import { useAnimation } from './animation-hook.js';
//
// class MyComponent extends LitElement {
//   @state() private anim = useAnimation();
//
//   async show() {
//     await this.anim.enter(this, 'fade');
//   }
// }
// ```
// ──────────────────────────────────────────────────────────────────────────

import { prefersReducedMotion } from './motion.js';

/** 动画类型 */
export type AnimationType = 'fade' | 'slide-up' | 'slide-down' | 'slide-left' | 'slide-right' | 'scale' | 'none';

/** 动画 hook 状态 */
export interface AnimationState {
  /** 是否正在播放动画 */
  isPlaying: boolean;
  /** 当前动画类型 */
  currentAnimation: AnimationType | null;
  /** 动画是否被尊重（prefers-reduced-motion） */
  reducedMotion: boolean;
}

/**
 * AnimationHook — 框架动画 hook。
 *
 * 提供 Lit 组件可用的动画能力。
 */
export class AnimationHook {
  private _state: AnimationState = {
    isPlaying: false,
    currentAnimation: null,
    reducedMotion: false,
  };

  private _subscribers: Set<(state: AnimationState) => void> = new Set();

  /**
   * 当前状态。
   */
  get state(): AnimationState {
    return { ...this._state };
  }

  /**
   * 是否正在播放动画。
   */
  get isPlaying(): boolean {
    return this._state.isPlaying;
  }

  /**
   * 当前动画类型。
   */
  get currentAnimation(): AnimationType | null {
    return this._state.currentAnimation;
  }

  /**
   * 订阅状态变化。
   */
  subscribe(fn: (state: AnimationState) => void): () => void {
    this._subscribers.add(fn);
    // 立即通知当前状态
    try {
      fn(this._state);
    } catch {
      // 忽略订阅者异常
    }
    return () => {
      this._subscribers.delete(fn);
    };
  }

  private _setState(state: Partial<AnimationState>): void {
    this._state = { ...this._state, ...state };
    for (const fn of this._subscribers) {
      try {
        fn(this._state);
      } catch {
        // 忽略订阅者异常
      }
    }
  }

  /**
   * 播放进入动画。
   */
  async enter(element: HTMLElement, type: AnimationType = 'fade', duration = 300): Promise<void> {
    if (!this._shouldAnimate() || type === 'none') {
      return;
    }

    this._setState({ isPlaying: true, currentAnimation: type });

    try {
      element.style.opacity = '0';

      switch (type) {
        case 'fade':
          element.style.transition = `opacity ${duration}ms ease-out`;
          break;
        case 'slide-up':
          element.style.transform = 'translateY(20px)';
          element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
          break;
        case 'scale':
          element.style.transform = 'scale(0.9)';
          element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
          break;
      }

      // 强制重排：读一次布局属性，让浏览器在下一帧前完成回流
      void element.offsetHeight;

      element.style.opacity = '1';
      element.style.transform = '';

      await new Promise((r) => setTimeout(r, duration));
      element.style.transition = '';
    } finally {
      this._setState({ isPlaying: false, currentAnimation: null });
    }
  }

  /**
   * 播放离开动画。
   */
  async leave(element: HTMLElement, type: AnimationType = 'fade', duration = 300): Promise<void> {
    if (!this._shouldAnimate() || type === 'none') {
      return;
    }

    this._setState({ isPlaying: true, currentAnimation: type });

    try {
      switch (type) {
        case 'fade':
          element.style.transition = `opacity ${duration}ms ease-in`;
          element.style.opacity = '0';
          break;
        case 'slide-down':
          element.style.transition = `opacity ${duration}ms ease-in, transform ${duration}ms ease-in`;
          element.style.opacity = '0';
          element.style.transform = 'translateY(20px)';
          break;
        case 'scale':
          element.style.transition = `opacity ${duration}ms ease-in, transform ${duration}ms ease-in`;
          element.style.opacity = '0';
          element.style.transform = 'scale(1.1)';
          break;
      }

      await new Promise((r) => setTimeout(r, duration));
      element.style.transition = '';
      element.style.transform = '';
    } finally {
      this._setState({ isPlaying: false, currentAnimation: null });
    }
  }

  /**
   * 检查是否应该播放动画。
   */
  private _shouldAnimate(): boolean {
    this._state.reducedMotion = prefersReducedMotion();
    return !this._state.reducedMotion;
  }
}

/**
 * 创建动画 hook 实例。
 *
 * 便捷工厂函数。
 */
export function useAnimation(): AnimationHook {
  return new AnimationHook();
}