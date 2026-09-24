// motion.ts 测试（P1-7 动效基础设施）

import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  DEFAULT_DURATIONS,
  DEFAULT_EASINGS,
  DEFAULT_KEYFRAMES,
  EASING_STANDARD,
  EASING_EMPHASIZED,
  EASING_DECELERATED,
  EASING_ACCELERATED,
  defineMotionTheme,
  getActiveMotionTheme,
  getDuration,
  getEasing,
  animation,
  motionToCssVariables,
  motionToCssText,
  durationsForPreset,
  prefersReducedMotion,
  shouldAnimate,
  resetMotionTheme,
  type MotionConfig,
} from './motion.js';

describe('motion', () => {
  beforeEach(() => {
    resetMotionTheme();
  });

  // ── Easing 曲线 ──

  describe('EASING', () => {
    it('standard 曲线正确', () => {
      expect(EASING_STANDARD.cubicBezier).toEqual([0.4, 0.0, 0.2, 1.0]);
      expect(EASING_STANDARD.value).toContain('cubic-bezier');
    });

    it('emphasized 曲线正确', () => {
      expect(EASING_EMPHASIZED.cubicBezier).toEqual([0.2, 0.0, 0.0, 1.0]);
    });

    it('decelerated 曲线正确', () => {
      expect(EASING_DECELERATED.cubicBezier).toEqual([0.0, 0.0, 0.2, 1.0]);
    });

    it('accelerated 曲线正确', () => {
      expect(EASING_ACCELERATED.cubicBezier).toEqual([0.4, 0.0, 1.0, 1.0]);
    });

    it('DEFAULT_EASINGS 包含全部 4 条', () => {
      expect(Object.keys(DEFAULT_EASINGS)).toHaveLength(4);
      expect(DEFAULT_EASINGS.standard).toBe(EASING_STANDARD);
      expect(DEFAULT_EASINGS.emphasized).toBe(EASING_EMPHASIZED);
      expect(DEFAULT_EASINGS.decelerated).toBe(EASING_DECELERATED);
      expect(DEFAULT_EASINGS.accelerated).toBe(EASING_ACCELERATED);
    });
  });

  // ── Duration 阶梯 ──

  describe('DEFAULT_DURATIONS', () => {
    it('默认值为 150/300/500ms', () => {
      expect(DEFAULT_DURATIONS.fast).toBe(150);
      expect(DEFAULT_DURATIONS.normal).toBe(300);
      expect(DEFAULT_DURATIONS.slow).toBe(500);
    });
  });

  describe('durationsForPreset', () => {
    it('none 全部为 0', () => {
      const d = durationsForPreset('none');
      expect(d.fast).toBe(0);
      expect(d.normal).toBe(0);
      expect(d.slow).toBe(0);
    });

    it('subtle 比 standard 短', () => {
      const d = durationsForPreset('subtle');
      expect(d.fast).toBeLessThan(DEFAULT_DURATIONS.fast);
      expect(d.normal).toBeLessThan(DEFAULT_DURATIONS.normal);
      expect(d.slow).toBeLessThan(DEFAULT_DURATIONS.slow);
    });

    it('expressive 比 standard 长', () => {
      const d = durationsForPreset('expressive');
      expect(d.fast).toBeGreaterThan(DEFAULT_DURATIONS.fast);
      expect(d.normal).toBeGreaterThan(DEFAULT_DURATIONS.normal);
      expect(d.slow).toBeGreaterThan(DEFAULT_DURATIONS.slow);
    });
  });

  // ── Keyframes ──

  describe('DEFAULT_KEYFRAMES', () => {
    it('包含 8 个 keyframes', () => {
      expect(Object.keys(DEFAULT_KEYFRAMES)).toHaveLength(8);
      expect(DEFAULT_KEYFRAMES['fade-in']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['fade-out']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['slide-up']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['slide-down']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['scale-in']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['scale-out']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['shimmer']).toBeDefined();
      expect(DEFAULT_KEYFRAMES['pulse']).toBeDefined();
    });

    it('每个 keyframe 有正确的 offset', () => {
      for (const [name, frames] of Object.entries(DEFAULT_KEYFRAMES)) {
        expect(frames.length).toBeGreaterThanOrEqual(2);
        expect(frames[0]!.offset).toBe('0%');
        expect(frames[frames.length - 1]!.offset).toBe('100%');
        for (const f of frames) {
          expect(f.properties).toBeDefined();
          expect(Object.keys(f.properties).length).toBeGreaterThan(0);
        }
      }
    });
  });

  // ── defineMotionTheme ──

  describe('defineMotionTheme', () => {
    it('可覆盖 preset', () => {
      defineMotionTheme({ preset: 'subtle' });
      expect(getActiveMotionTheme().preset).toBe('subtle');
    });

    it('可覆盖 durations', () => {
      defineMotionTheme({ durations: { fast: 50 } });
      expect(getActiveMotionTheme().durations.fast).toBe(50);
      expect(getActiveMotionTheme().durations.normal).toBe(DEFAULT_DURATIONS.normal);
    });

    it('可注册自定义 keyframes', () => {
      defineMotionTheme({
        keyframes: {
          'my-keyframe': [
            { offset: '0%', properties: { opacity: '0' } },
            { offset: '100%', properties: { opacity: '1' } },
          ],
        },
      });
      expect(getActiveMotionTheme().keyframes['my-keyframe']).toBeDefined();
      // 默认 keyframes 仍存在
      expect(getActiveMotionTheme().keyframes['fade-in']).toBeDefined();
    });

    it('可覆盖 easings', () => {
      defineMotionTheme({
        easings: {
          custom: {
            name: 'custom',
            cubicBezier: [0.5, 0.0, 0.5, 1.0],
            value: 'cubic-bezier(0.5, 0.0, 0.5, 1.0)',
          },
        },
      });
      expect(getActiveMotionTheme().easings['custom']).toBeDefined();
    });

    it('多次调用合并累积', () => {
      defineMotionTheme({ preset: 'subtle' });
      defineMotionTheme({ durations: { fast: 50 } });
      expect(getActiveMotionTheme().preset).toBe('subtle');
      expect(getActiveMotionTheme().durations.fast).toBe(50);
    });
  });

  // ── getDuration / getEasing / animation ──

  describe('getDuration', () => {
    it('返回当前主题的 duration', () => {
      expect(getDuration('fast')).toBe(DEFAULT_DURATIONS.fast);
      expect(getDuration('normal')).toBe(DEFAULT_DURATIONS.normal);
      expect(getDuration('slow')).toBe(DEFAULT_DURATIONS.slow);
    });
  });

  describe('getEasing', () => {
    it('返回当前主题的 easing', () => {
      expect(getEasing('standard')).toBe(EASING_STANDARD.value);
      expect(getEasing('unknown')).toBe(EASING_STANDARD.value); // fallback
    });
  });

  describe('animation', () => {
    it('生成正确的 animation shorthand', () => {
      const result = animation('fade-in');
      expect(result).toContain('fade-in');
      expect(result).toContain('300ms');
      expect(result).toContain('cubic-bezier');
    });

    it('preset=none 时返回 none', () => {
      defineMotionTheme({ preset: 'none' });
      expect(animation('fade-in')).toBe('none');
    });

    it('指定 duration level', () => {
      const result = animation('fade-in', 'fast');
      expect(result).toContain('150ms');
    });

    it('指定 easing', () => {
      const result = animation('fade-in', 'normal', 'emphasized');
      expect(result).toContain(EASING_EMPHASIZED.value);
    });
  });

  // ── CSS 变量桥 ──

  describe('motionToCssVariables', () => {
    it('包含 duration 变量', () => {
      const vars = motionToCssVariables();
      expect(vars['--oc-motion-duration-fast']).toBe('150ms');
      expect(vars['--oc-motion-duration-normal']).toBe('300ms');
      expect(vars['--oc-motion-duration-slow']).toBe('500ms');
    });

    it('包含 easing 变量', () => {
      const vars = motionToCssVariables();
      expect(vars['--oc-motion-easing-standard']).toBe(EASING_STANDARD.value);
      expect(vars['--oc-motion-easing-emphasized']).toBe(EASING_EMPHASIZED.value);
    });

    it('包含 preset 标记', () => {
      const vars = motionToCssVariables();
      expect(vars['--oc-motion-preset']).toBe('standard');
    });
  });

  describe('motionToCssText', () => {
    it('包含 :root 变量块', () => {
      const css = motionToCssText();
      expect(css).toContain(':root {');
      expect(css).toContain('--oc-motion-duration-fast');
    });

    it('包含 @keyframes 块', () => {
      const css = motionToCssText();
      expect(css).toContain('@keyframes fade-in');
      expect(css).toContain('@keyframes fade-out');
      expect(css).toContain('@keyframes slide-up');
    });

    it('包含 prefers-reduced-motion 降级', () => {
      const css = motionToCssText();
      expect(css).toContain('prefers-reduced-motion: reduce');
      expect(css).toContain('animation-duration: 0.01ms !important');
    });

    it('respectReducedMotion=false 时无降级块', () => {
      defineMotionTheme({ respectReducedMotion: false });
      const css = motionToCssText();
      expect(css).not.toContain('prefers-reduced-motion');
    });
  });

  // ── prefersReducedMotion / shouldAnimate ──

  describe('prefersReducedMotion', () => {
    it('SSR 环境返回 false', () => {
      expect(prefersReducedMotion()).toBe(false);
    });
  });

  describe('shouldAnimate', () => {
    it('默认返回 true', () => {
      expect(shouldAnimate()).toBe(true);
    });

    it('preset=none 返回 false', () => {
      defineMotionTheme({ preset: 'none' });
      expect(shouldAnimate()).toBe(false);
    });

    it('respectReducedMotion=false 时始终返回 true', () => {
      defineMotionTheme({ respectReducedMotion: false });
      expect(shouldAnimate()).toBe(true);
    });
  });

  // ── resetMotionTheme ──

  describe('resetMotionTheme', () => {
    it('重置为默认值', () => {
      defineMotionTheme({ preset: 'none', durations: { fast: 0 } });
      resetMotionTheme();
      expect(getActiveMotionTheme().preset).toBe('standard');
      expect(getActiveMotionTheme().durations.fast).toBe(DEFAULT_DURATIONS.fast);
    });
  });

  // ── 主题快照 ──

  describe('getActiveMotionTheme', () => {
    it('返回默认主题', () => {
      const theme = getActiveMotionTheme();
      expect(theme.preset).toBe('standard');
      expect(theme.durations).toEqual(DEFAULT_DURATIONS);
      expect(theme.easings).toEqual(DEFAULT_EASINGS);
      expect(theme.keyframes).toEqual(DEFAULT_KEYFRAMES);
      expect(theme.respectReducedMotion).toBe(true);
    });

    it('返回的是副本（修改不影响内部）', () => {
      const theme = getActiveMotionTheme();
      theme.durations.fast = 99999;
      expect(getActiveMotionTheme().durations.fast).toBe(DEFAULT_DURATIONS.fast);
    });
  });
});