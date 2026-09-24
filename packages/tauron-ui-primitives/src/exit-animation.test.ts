// @vitest-environment happy-dom
// exit-animation.ts 测试（P2-4/P2-5：退出动画 + beforeExit 钩子）

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { ExitAnimation, createExitAnimation } from './exit-animation.js';

describe('ExitAnimation', () => {
  beforeEach(() => {
    document.body.style.cssText = '';
  });

  describe('基本功能', () => {
    it('创建实例，初始阶段为 idle', () => {
      const exit = new ExitAnimation({ animation: 'fade', duration: 300 });
      expect(exit.phase).toBe('idle');
    });

    it('createExitAnimation 工厂函数', () => {
      const exit = createExitAnimation({ animation: 'fade', duration: 300 });
      expect(exit).toBeInstanceOf(ExitAnimation);
    });

    it('subscribe 订阅阶段变化', () => {
      const exit = new ExitAnimation({ animation: 'fade', duration: 300 });
      const phases: string[] = [];
      const unsub = exit.subscribe((p) => phases.push(p));
      expect(phases).toHaveLength(0);
      unsub();
    });
  });

  describe('play() 流程', () => {
    it('animation: none 立即完成', async () => {
      const exit = new ExitAnimation({ animation: 'none', duration: 0 });
      const phases: string[] = [];
      exit.subscribe((p) => phases.push(p));
      await exit.play();
      expect(exit.phase).toBe('done');
      expect(phases).toContain('done');
    });

    it('fade 动画播放后完成', async () => {
      const exit = new ExitAnimation({ animation: 'fade', duration: 50 });
      await exit.play();
      expect(exit.phase).toBe('done');
    });

    it('slide-down 动画播放后完成', async () => {
      const exit = new ExitAnimation({ animation: 'slide-down', duration: 50 });
      await exit.play();
      expect(exit.phase).toBe('done');
    });

    it('scale-out 动画播放后完成', async () => {
      const exit = new ExitAnimation({ animation: 'scale-out', duration: 50 });
      await exit.play();
      expect(exit.phase).toBe('done');
    });
  });

  describe('beforeExit 钩子', () => {
    it('beforeExit 被调用', async () => {
      const beforeExit = vi.fn().mockResolvedValue(undefined);
      const exit = new ExitAnimation({ animation: 'none', duration: 0, beforeExit });
      await exit.play();
      expect(beforeExit).toHaveBeenCalledTimes(1);
    });

    it('beforeExit 异步等待', async () => {
      const phases: string[] = [];
      const exit = new ExitAnimation({
        animation: 'none',
        duration: 0,
        beforeExit: () => new Promise((r) => setTimeout(r, 10)),
      });
      exit.subscribe((p) => phases.push(p));
      await exit.play();
      expect(phases).toContain('beforeExit');
      expect(phases).toContain('done');
    });

    it('beforeExit 失败时取消退出', async () => {
      const onCancel = vi.fn();
      const exit = new ExitAnimation({
        animation: 'none',
        duration: 0,
        beforeExit: () => Promise.reject(new Error('save failed')),
        onCancel,
      });
      await exit.play();
      expect(exit.phase).toBe('cancelled');
      expect(onCancel).toHaveBeenCalledTimes(1);
    });
  });

  describe('onExit 回调', () => {
    it('onExit 在动画完成后调用', async () => {
      const onExit = vi.fn();
      const exit = new ExitAnimation({ animation: 'none', duration: 0, onExit });
      await exit.play();
      expect(onExit).toHaveBeenCalledTimes(1);
    });
  });

  describe('cancel()', () => {
    it('取消后阶段变为 cancelled', async () => {
      const onCancel = vi.fn();
      const exit = new ExitAnimation({ animation: 'fade', duration: 100, onCancel });
      const playPromise = exit.play();
      // 等待进入 playing 阶段
      await new Promise((r) => setTimeout(r, 5));
      exit.cancel();
      await playPromise;
      expect(exit.phase).toBe('cancelled');
      expect(onCancel).toHaveBeenCalledTimes(1);
    });
  });

  describe('防止重复播放', () => {
    it('done 后再次 play 不执行', async () => {
      const beforeExit = vi.fn();
      const exit = new ExitAnimation({ animation: 'none', duration: 0, beforeExit });
      await exit.play();
      await exit.play();
      expect(beforeExit).toHaveBeenCalledTimes(1); // 只调用一次
    });
  });
});