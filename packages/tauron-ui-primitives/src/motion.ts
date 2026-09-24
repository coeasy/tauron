// ──────────────────────────────────────────────────────────────────────────
// 动效基础设施（§5.4 动效与启动退出体验架构）。
//
// 职责：
// - 8 个 keyframes 定义（fade/slide/scale/shimmer/pulse）
// - 4 条 easing 曲线（standard/emphasized/decelerated/accelerated）
// - 3 级 duration 阶梯（fast=150ms / normal=300ms / slow=500ms）
// - prefers-reduced-motion 检测与降级
// - defineMotionTheme() 运行时覆盖 API
// - CSS 变量桥（--oc-motion-duration-* / --oc-motion-easing-*）
//
// 设计原则：
// - 纯 CSS + 纯 TS，零外部依赖
// - CSS 变量命名 `--oc-motion-*`，与 tokens.ts 的 `--oc-*` 命名空间一致
// - 任何框架（React/Vue/Svelte）都可直接引用 CSS 变量
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// 类型
// ──────────────────────────────────────────────────────────────────────────

/** 动效预设等级。 */
export type MotionPreset = 'none' | 'subtle' | 'standard' | 'expressive';

/** Keyframe 定义（单个 CSS keyframe）。 */
export interface KeyframeDefinition {
  /** keyframe 进度（0%-100%）。 */
  offset: string;
  /** CSS 属性集合。 */
  properties: Record<string, string>;
}

/** Easing 曲线。 */
export interface EasingCurve {
  /** 名称。 */
  name: string;
  /** CSS cubic-bezier 参数。 */
  cubicBezier: [number, number, number, number];
  /** CSS timing function 值。 */
  value: string;
}

/** Duration 阶梯（毫秒）。 */
export interface DurationScale {
  fast: number;
  normal: number;
  slow: number;
}

/** 动效主题配置。 */
export interface MotionThemeConfig {
  /** 预设等级。 */
  preset: MotionPreset;
  /** Duration 阶梯。 */
  durations: DurationScale;
  /** Easing 曲线集。 */
  easings: Record<string, EasingCurve>;
  /** 自定义 keyframes。 */
  keyframes: Record<string, KeyframeDefinition[]>;
  /** 是否尊重 prefers-reduced-motion。 */
  respectReducedMotion: boolean;
}

/** 动效运行时配置（可覆盖）。 */
export interface MotionConfig {
  preset?: MotionPreset;
  durations?: Partial<DurationScale>;
  easings?: Record<string, EasingCurve>;
  keyframes?: Record<string, KeyframeDefinition[]>;
  respectReducedMotion?: boolean;
}

// ──────────────────────────────────────────────────────────────────────────
// Easing 曲线（4 条）
// ──────────────────────────────────────────────────────────────────────────

/** 标准缓动（大多数 UI 交互）。 */
export const EASING_STANDARD: EasingCurve = {
  name: 'standard',
  cubicBezier: [0.4, 0.0, 0.2, 1.0],
  value: 'cubic-bezier(0.4, 0.0, 0.2, 1.0)',
};

/** 强调缓动（突出操作，如对话框弹出）。 */
export const EASING_EMPHASIZED: EasingCurve = {
  name: 'emphasized',
  cubicBezier: [0.2, 0.0, 0.0, 1.0],
  value: 'cubic-bezier(0.2, 0.0, 0.0, 1.0)',
};

/** 减速缓动（入场，从快变慢）。 */
export const EASING_DECELERATED: EasingCurve = {
  name: 'decelerated',
  cubicBezier: [0.0, 0.0, 0.2, 1.0],
  value: 'cubic-bezier(0.0, 0.0, 0.2, 1.0)',
};

/** 加速缓动（离场，从慢变快）。 */
export const EASING_ACCELERATED: EasingCurve = {
  name: 'accelerated',
  cubicBezier: [0.4, 0.0, 1.0, 1.0],
  value: 'cubic-bezier(0.4, 0.0, 1.0, 1.0)',
};

/** 全部 easing 集合。 */
export const DEFAULT_EASINGS: Record<string, EasingCurve> = {
  standard: EASING_STANDARD,
  emphasized: EASING_EMPHASIZED,
  decelerated: EASING_DECELERATED,
  accelerated: EASING_ACCELERATED,
};

// ──────────────────────────────────────────────────────────────────────────
// Duration 阶梯（3 级）
// ──────────────────────────────────────────────────────────────────────────

/** 默认 duration 阶梯（毫秒）。 */
export const DEFAULT_DURATIONS: DurationScale = {
  fast: 150,
  normal: 300,
  slow: 500,
};

/** 按预设返回 duration 阶梯。 */
export function durationsForPreset(preset: MotionPreset): DurationScale {
  switch (preset) {
    case 'none':
      return { fast: 0, normal: 0, slow: 0 };
    case 'subtle':
      return { fast: 100, normal: 200, slow: 300 };
    case 'standard':
      return { ...DEFAULT_DURATIONS };
    case 'expressive':
      return { fast: 200, normal: 400, slow: 700 };
  }
}

// ──────────────────────────────────────────────────────────────────────────
// Keyframes（8 个）
// ──────────────────────────────────────────────────────────────────────────

/** 默认 keyframes 集合。 */
export const DEFAULT_KEYFRAMES: Record<string, KeyframeDefinition[]> = {
  'fade-in': [
    { offset: '0%', properties: { opacity: '0' } },
    { offset: '100%', properties: { opacity: '1' } },
  ],
  'fade-out': [
    { offset: '0%', properties: { opacity: '1' } },
    { offset: '100%', properties: { opacity: '0' } },
  ],
  'slide-up': [
    { offset: '0%', properties: { transform: 'translateY(16px)', opacity: '0' } },
    { offset: '100%', properties: { transform: 'translateY(0)', opacity: '1' } },
  ],
  'slide-down': [
    { offset: '0%', properties: { transform: 'translateY(-16px)', opacity: '0' } },
    { offset: '100%', properties: { transform: 'translateY(0)', opacity: '1' } },
  ],
  'scale-in': [
    { offset: '0%', properties: { transform: 'scale(0.9)', opacity: '0' } },
    { offset: '100%', properties: { transform: 'scale(1)', opacity: '1' } },
  ],
  'scale-out': [
    { offset: '0%', properties: { transform: 'scale(1)', opacity: '1' } },
    { offset: '100%', properties: { transform: 'scale(0.9)', opacity: '0' } },
  ],
  'shimmer': [
    { offset: '0%', properties: { backgroundPosition: '-200% 0' } },
    { offset: '100%', properties: { backgroundPosition: '200% 0' } },
  ],
  'pulse': [
    { offset: '0%', properties: { transform: 'scale(1)', opacity: '1' } },
    { offset: '50%', properties: { transform: 'scale(1.05)', opacity: '0.85' } },
    { offset: '100%', properties: { transform: 'scale(1)', opacity: '1' } },
  ],
};

// ──────────────────────────────────────────────────────────────────────────
// 运行时主题状态
// ──────────────────────────────────────────────────────────────────────────

let _activeTheme: MotionThemeConfig = {
  preset: 'standard',
  durations: { ...DEFAULT_DURATIONS },
  easings: { ...DEFAULT_EASINGS },
  keyframes: { ...DEFAULT_KEYFRAMES },
  respectReducedMotion: true,
};

/** 获取当前活动主题（深拷贝，修改不影响内部状态）。 */
export function getActiveMotionTheme(): MotionThemeConfig {
  return {
    preset: _activeTheme.preset,
    durations: { ..._activeTheme.durations },
    easings: { ..._activeTheme.easings },
    keyframes: { ..._activeTheme.keyframes },
    respectReducedMotion: _activeTheme.respectReducedMotion,
  };
}

// ──────────────────────────────────────────────────────────────────────────
// API
// ──────────────────────────────────────────────────────────────────────────

/**
 * 定义/覆盖动效主题。
 *
 * 三方集成第三档：完全自定义。传入的 keyframes/easings/durations
 * 会合并到默认集合中（同名覆盖，新名追加）。
 *
 * @param config 动效配置（部分字段可选）
 * @returns 合并后的完整主题（可用于进一步引用）
 */
export function defineMotionTheme(config: MotionConfig): MotionThemeConfig {
  _activeTheme = {
    preset: config.preset ?? _activeTheme.preset,
    durations: {
      ...DEFAULT_DURATIONS,
      ..._activeTheme.durations,
      ...config.durations,
    },
    easings: {
      ...DEFAULT_EASINGS,
      ..._activeTheme.easings,
      ...config.easings,
    },
    keyframes: {
      ...DEFAULT_KEYFRAMES,
      ..._activeTheme.keyframes,
      ...config.keyframes,
    },
    respectReducedMotion: config.respectReducedMotion ?? _activeTheme.respectReducedMotion,
  };
  return { ..._activeTheme };
}

/**
 * 重置到默认主题。
 */
export function resetMotionTheme(): MotionThemeConfig {
  _activeTheme = {
    preset: 'standard',
    durations: { ...DEFAULT_DURATIONS },
    easings: { ...DEFAULT_EASINGS },
    keyframes: { ...DEFAULT_KEYFRAMES },
    respectReducedMotion: true,
  };
  return { ..._activeTheme };
}

// ──────────────────────────────────────────────────────────────────────────
// CSS 变量桥
// ──────────────────────────────────────────────────────────────────────────

/** 将当前主题序列化为 CSS 自定义属性。 */
export function motionToCssVariables(theme: MotionThemeConfig = _activeTheme): Record<string, string> {
  const vars: Record<string, string> = {};

  // Durations
  vars['--oc-motion-duration-fast'] = `${theme.durations.fast}ms`;
  vars['--oc-motion-duration-normal'] = `${theme.durations.normal}ms`;
  vars['--oc-motion-duration-slow'] = `${theme.durations.slow}ms`;

  // Easings
  for (const [name, easing] of Object.entries(theme.easings)) {
    vars[`--oc-motion-easing-${name}`] = easing.value;
  }

  // Preset 标记
  vars['--oc-motion-preset'] = theme.preset;

  // Reduce scale (prefers-reduced-motion 时由 CSS 媒体查询覆盖)
  vars['--oc-motion-reduce-scale'] = theme.respectReducedMotion ? '1' : '0';

  return vars;
}

/** 生成动效主题的完整 CSS 文本（含 @keyframes + 变量 + 降级）。 */
export function motionToCssText(theme: MotionThemeConfig = _activeTheme): string {
  const parts: string[] = [];

  // 1. CSS 变量
  const vars = motionToCssVariables(theme);
  const varLines = Object.entries(vars).map(([name, value]) => `  ${name}: ${value};`);
  parts.push(`:root {\n${varLines.join('\n')}\n}`);

  // 2. @keyframes
  for (const [name, frames] of Object.entries(theme.keyframes)) {
    const frameLines = frames.map(
      (f) => `  ${f.offset} {\n    ${Object.entries(f.properties).map(([k, v]) => `${camelToKebab(k)}: ${v};`).join('\n    ')}\n  }`,
    );
    parts.push(`@keyframes ${name} {\n${frameLines.join('\n')}\n}`);
  }

  // 3. prefers-reduced-motion 降级
  if (theme.respectReducedMotion) {
    parts.push(`
@media (prefers-reduced-motion: reduce) {
  *,
  *::before,
  *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
    scroll-behavior: auto !important;
  }
}`);
  }

  return parts.join('\n\n');
}

// ──────────────────────────────────────────────────────────────────────────
// 工具函数
// ──────────────────────────────────────────────────────────────────────────

/** camelCase → kebab-case（CSS 属性名转换）。 */
function camelToKebab(s: string): string {
  return s.replace(/([a-z])([A-Z])/g, '$1-$2').toLowerCase();
}

/** 获取当前主题的 duration 值。 */
export function getDuration(level: keyof DurationScale): number {
  return _activeTheme.durations[level];
}

/** 获取当前主题的 easing 值。 */
export function getEasing(name: string): string {
  return _activeTheme.easings[name]?.value ?? EASING_STANDARD.value;
}

/** 获取当前主题的 animation shorthand（CSS 值）。 */
export function animation(
  keyframeName: string,
  durationLevel: keyof DurationScale = 'normal',
  easingName: string = 'standard',
  fillMode: string = 'both',
  iterationCount: number | string = 1,
): string {
  if (_activeTheme.preset === 'none') return 'none';
  const duration = _activeTheme.durations[durationLevel];
  const easing = getEasing(easingName);
  const iteration = typeof iterationCount === 'number' ? iterationCount : 'infinite';
  return `${keyframeName} ${duration}ms ${easing} ${fillMode} ${iteration}`;
}

// ──────────────────────────────────────────────────────────────────────────
// prefers-reduced-motion 检测（P1-8 无障碍降级）
// ──────────────────────────────────────────────────────────────────────────

/**
 * 检测当前是否处于 prefers-reduced-motion: reduce 模式。
 *
 * 浏览器原生 API，SSR 环境返回 false。
 */
export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return false;
  }
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/**
 * 订阅 prefers-reduced-motion 变化。
 *
 * 返回退订函数。
 */
export function onReducedMotionChange(callback: (reduced: boolean) => void): () => void {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return () => {};
  }
  const query = window.matchMedia('(prefers-reduced-motion: reduce)');
  const listener = (e: MediaQueryListEvent) => callback(e.matches);
  query.addEventListener('change', listener);
  return () => query.removeEventListener('change', listener);
}

/**
 * 判断某个动画是否应该播放（考虑 reduced-motion）。
 *
 * 当用户偏好减少动效且主题启用 respectReducedMotion 时，返回 false。
 */
export function shouldAnimate(): boolean {
  if (!_activeTheme.respectReducedMotion) return true;
  if (_activeTheme.preset === 'none') return false;
  return !prefersReducedMotion();
}