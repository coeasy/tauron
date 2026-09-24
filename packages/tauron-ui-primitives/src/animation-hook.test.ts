// @vitest-environment happy-dom
// animation-hook.ts 测试（P3-2：框架动画 hook）

import { describe, it, expect, beforeEach } from 'vitest';
import { AnimationHook, useAnimation } from './animation-hook.js';

describe('AnimationHook', () => {
  let hook: AnimationHook;

  beforeEach(() => {
    hook = useAnimation();
  });

  describe('基本功能', () => {
    it('创建实例', () => {
      expect(hook).toBeDefined();
      expect(hook.isPlaying).toBe(false);
      expect(hook.currentAnimation).toBeNull();
    });

    it('useAnimation 工厂函数', () => {
      const h = useAnimation();
      expect(h).toBeInstanceOf(AnimationHook);
    });

    it('state 返回当前状态', () => {
      const state = hook.state;
      expect(state.isPlaying).toBe(false);
      expect(state.currentAnimation).toBeNull();
      expect(state.reducedMotion).toBe(false);
    });
  });

  describe('subscribe()', () => {
    it('订阅状态变化', () => {
      const states: Array<{ isPlaying: boolean }> = [];
      hook.subscribe((s) => states.push({ isPlaying: s.isPlaying }));
      expect(states.length).toBe(1); // 立即通知
    });

    it('unsubscribe 取消订阅', () => {
      let count = 0;
      const unsub = hook.subscribe(() => count++);
      expect(count).toBe(1);
      unsub();
      // 再次订阅不应增加计数
      hook.subscribe(() => count++);
      expect(count).toBe(2);
    });
  });

  describe('enter() 进入动画', () => {
    let el: HTMLElement;

    beforeEach(() => {
      el = document.createElement('div');
      document.body.appendChild(el);
    });

    it('播放进入动画', async () => {
      await hook.enter(el, 'fade', 50);
      expect(el.style.opacity).toBe('1');
      expect(hook.isPlaying).toBe(false);
    });

    it('isPlaying 为 true 期间', async () => {
      const hook2 = useAnimation();
      const promise = hook2.enter(el, 'fade', 100);
      expect(hook2.isPlaying).toBe(true);
      await promise;
      expect(hook2.isPlaying).toBe(false);
    });

    it('type: none 不播放动画', async () => {
      await hook.enter(el, 'none', 50);
      expect(el.style.opacity).toBe('');
      expect(hook.isPlaying).toBe(false);
    });
  });

  describe('leave() 离开动画', () => {
    let el: HTMLElement;

    beforeEach(() => {
      el = document.createElement('div');
      document.body.appendChild(el);
    });

    it('播放离开动画', async () => {
      await hook.leave(el, 'fade', 50);
      expect(el.style.opacity).toBe('0');
      expect(hook.isPlaying).toBe(false);
    });

    it('type: none 不播放动画', async () => {
      await hook.leave(el, 'none', 50);
      expect(el.style.opacity).toBe('');
      expect(hook.isPlaying).toBe(false);
    });
  });

  describe('订阅动画状态变化', () => {
    it('enter 时触发 loading 状态', async () => {
      const events: string[] = [];
      hook.subscribe((s) => events.push(s.isPlaying ? 'playing' : 'idle'));

      const el = document.createElement('div');
      await hook.enter(el, 'fade', 50);

      expect(events).toContain('playing');
      expect(events).toContain('idle');
    });
  });
});