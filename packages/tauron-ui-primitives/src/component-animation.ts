// ──────────────────────────────────────────────────────────────────────────
// ComponentAnimation — 组件动画工具（P2-8）。
//
// 职责：
// 1. enter/leave 动画（元素进入/离开时播放）
// 2. stagger 动画（列表项交错播放）
// 3. 与 prefers-reduced-motion 集成
//
// 用法：
// ```typescript
// import { animateEnter, animateLeave, staggerIn } from './component-animation.js';
//
// await animateEnter(element, { type: 'fade', duration: 300 });
// await animateLeave(element, { type: 'slide-up', duration: 300 });
// await staggerIn(elements, { type: 'fade', stagger: 50 });
// ```
// ──────────────────────────────────────────────────────────────────────────

/** 动画类型 */
export type AnimationType = 'fade' | 'slide-up' | 'slide-down' | 'slide-left' | 'slide-right' | 'scale' | 'none';

/** 动画配置 */
export interface AnimationConfig {
  /** 动画类型 */
  type: AnimationType;
  /** 动画时长（ms） */
  duration?: number;
  /** 动画延迟（ms） */
  delay?: number;
  /** 是否尊重 prefers-reduced-motion */
  respectReducedMotion?: boolean;
}

/** 交错动画配置 */
export interface StaggerConfig extends AnimationConfig {
  /** 交错延迟（ms） */
  stagger?: number;
}

/**
 * 检查是否应该播放动画。
 */
function shouldAnimate(respectReducedMotion = true): boolean {
  if (!respectReducedMotion) return true;
  if (typeof window === 'undefined' || !window.matchMedia) return true;
  return !window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/**
 * 播放进入动画。
 */
export async function animateEnter(
  element: HTMLElement,
  config: Partial<AnimationConfig> = {},
): Promise<void> {
  const { type = 'fade', duration = 300, delay = 0, respectReducedMotion = true } = config;

  if (!shouldAnimate(respectReducedMotion) || type === 'none') {
    return;
  }

  element.style.opacity = '0';

  switch (type) {
    case 'fade':
      element.style.transition = `opacity ${duration}ms ease-out`;
      break;
    case 'slide-up':
      element.style.transform = 'translateY(20px)';
      element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
      break;
    case 'slide-down':
      element.style.transform = 'translateY(-20px)';
      element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
      break;
    case 'slide-left':
      element.style.transform = 'translateX(-20px)';
      element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
      break;
    case 'slide-right':
      element.style.transform = 'translateX(20px)';
      element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
      break;
    case 'scale':
      element.style.transform = 'scale(0.9)';
      element.style.transition = `opacity ${duration}ms ease-out, transform ${duration}ms ease-out`;
      break;
  }

  if (delay > 0) {
    await new Promise((r) => setTimeout(r, delay));
  }

  // 强制重排：读一次布局属性，让浏览器在下一帧前完成回流
  void element.offsetHeight;

  element.style.opacity = '1';
  element.style.transform = '';

  await new Promise((r) => setTimeout(r, duration));
  element.style.transition = '';
}

/**
 * 播放离开动画。
 */
export async function animateLeave(
  element: HTMLElement,
  config: Partial<AnimationConfig> = {},
): Promise<void> {
  const { type = 'fade', duration = 300, delay = 0, respectReducedMotion = true } = config;

  if (!shouldAnimate(respectReducedMotion) || type === 'none') {
    return;
  }

  if (delay > 0) {
    await new Promise((r) => setTimeout(r, delay));
  }

  switch (type) {
    case 'fade':
      element.style.transition = `opacity ${duration}ms ease-in`;
      element.style.opacity = '0';
      break;
    case 'slide-up':
      element.style.transition = `opacity ${duration}ms ease-in, transform ${duration}ms ease-in`;
      element.style.opacity = '0';
      element.style.transform = 'translateY(-20px)';
      break;
    case 'slide-down':
      element.style.transition = `opacity ${duration}ms ease-in, transform ${duration}ms ease-in`;
      element.style.opacity = '0';
      element.style.transform = 'translateY(20px)';
      break;
    case 'slide-left':
      element.style.transition = `opacity ${duration}ms ease-in, transform ${duration}ms ease-in`;
      element.style.opacity = '0';
      element.style.transform = 'translateX(-20px)';
      break;
    case 'slide-right':
      element.style.transition = `opacity ${duration}ms ease-in, transform ${duration}ms ease-in`;
      element.style.opacity = '0';
      element.style.transform = 'translateX(20px)';
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
}

/**
 * 交错进入动画。
 */
export async function staggerIn(
  elements: HTMLElement[],
  config: Partial<StaggerConfig> = {},
): Promise<void> {
  const { stagger = 50, ...rest } = config;
  const promises = elements.map((el, i) =>
    animateEnter(el, { ...rest, delay: i * stagger }),
  );
  await Promise.all(promises);
}

/**
 * 交错离开动画。
 */
export async function staggerOut(
  elements: HTMLElement[],
  config: Partial<StaggerConfig> = {},
): Promise<void> {
  const { stagger = 50, ...rest } = config;
  const promises = elements.map((el, i) =>
    animateLeave(el, { ...rest, delay: i * stagger }),
  );
  await Promise.all(promises);
}

/**
 * 列表更新动画（diff + stagger）。
 */
export async function animateListUpdate(
  newItems: Array<{ id: string; element: HTMLElement }>,
  oldItems: Array<{ id: string; element: HTMLElement }>,
  config: Partial<StaggerConfig> = {},
): Promise<void> {
  const oldMap = new Map(oldItems.map((i) => [i.id, i]));
  const newMap = new Map(newItems.map((i) => [i.id, i]));

  // 离开动画：旧项目中不在新项目中的
  const leaving = oldItems.filter((i) => !newMap.has(i.id)).map((i) => i.element);
  if (leaving.length > 0) {
    await staggerOut(leaving, config);
  }

  // 进入动画：新项目中不在旧项目中的
  const entering = newItems.filter((i) => !oldMap.has(i.id)).map((i) => i.element);
  if (entering.length > 0) {
    await staggerIn(entering, config);
  }
}