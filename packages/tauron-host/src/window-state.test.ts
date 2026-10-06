// @vitest-environment happy-dom
// window-state.ts 测试（P2-7：窗口位置/大小持久化）

import { describe, it, expect, beforeEach } from 'vitest';
import { WindowState, createWindowState } from './window-state.js';
import { MockBackend } from './backend.js';

describe('WindowState', () => {
  beforeEach(() => {
    const data = new Map<string, string>();
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: (key: string) => data.get(key) ?? null,
        setItem: (key: string, value: string) => data.set(key, String(value)),
        removeItem: (key: string) => data.delete(key),
        clear: () => data.clear(),
      },
    });
  });

  describe('基本功能', () => {
    it('创建实例，默认状态', () => {
      const ws = new WindowState();
      expect(ws.currentState).toEqual({
        x: 100,
        y: 100,
        width: 1200,
        height: 800,
        isMaximized: false,
      });
    });

    it('createWindowState 工厂函数', () => {
      const ws = createWindowState();
      expect(ws).toBeInstanceOf(WindowState);
    });

    it('自定义配置', () => {
      const ws = new WindowState({
        defaultWidth: 1024,
        defaultHeight: 768,
        defaultX: 50,
        defaultY: 50,
      });
      expect(ws.currentState.width).toBe(1024);
      expect(ws.currentState.height).toBe(768);
      expect(ws.currentState.x).toBe(50);
      expect(ws.currentState.y).toBe(50);
    });
  });

  describe('save() 和恢复', () => {
    it('save() 持久化状态', () => {
      const ws = new WindowState({ storageKey: 'test.window' });
      ws.save({ x: 200, y: 150, width: 1024, height: 768 });
      expect(ws.currentState.x).toBe(200);
      expect(ws.currentState.y).toBe(150);
      expect(ws.currentState.width).toBe(1024);
      expect(ws.currentState.height).toBe(768);
    });

    it('重新创建实例恢复状态', () => {
      const ws1 = new WindowState({ storageKey: 'test.window2' });
      ws1.save({ x: 300, y: 200, width: 800, height: 600 });

      const ws2 = new WindowState({ storageKey: 'test.window2' });
      expect(ws2.currentState.x).toBe(300);
      expect(ws2.currentState.y).toBe(200);
      expect(ws2.currentState.width).toBe(800);
      expect(ws2.currentState.height).toBe(600);
    });

    it('localStorage 损坏时使用默认值', () => {
      window.localStorage.setItem('test.window3', 'invalid json');
      const ws = new WindowState({ storageKey: 'test.window3' });
      expect(ws.currentState.width).toBe(1200);
      expect(ws.currentState.height).toBe(800);
    });
  });

  describe('clear()', () => {
    it('清除保存的状态', () => {
      const ws = new WindowState({ storageKey: 'test.window4' });
      ws.save({ x: 500, y: 400 });
      ws.clear();
      expect(ws.currentState.x).toBe(100);
      expect(ws.currentState.y).toBe(100);
    });
  });

  describe('getRestoreState()', () => {
    it('restoreMaximized: true 时保留最大化状态', () => {
      const ws = new WindowState({ storageKey: 'test.window5', restoreMaximized: true });
      ws.save({ isMaximized: true });
      const restore = ws.getRestoreState();
      expect(restore.isMaximized).toBe(true);
    });

    it('restoreMaximized: false 时强制非最大化', () => {
      const ws = new WindowState({ storageKey: 'test.window6', restoreMaximized: false });
      ws.save({ isMaximized: true });
      const restore = ws.getRestoreState();
      expect(restore.isMaximized).toBe(false);
    });
  });

  describe('isValid()', () => {
    it('有效状态返回 true', () => {
      const ws = new WindowState();
      const valid = ws.isValid(
        { x: 100, y: 100, width: 800, height: 600, isMaximized: false },
        1920,
        1080,
      );
      expect(valid).toBe(true);
    });

    it('超出屏幕范围返回 false', () => {
      const ws = new WindowState();
      const invalid = ws.isValid(
        { x: 2000, y: 100, width: 800, height: 600, isMaximized: false },
        1920,
        1080,
      );
      expect(invalid).toBe(false);
    });

    it('窗口太小返回 false', () => {
      const ws = new WindowState();
      const invalid = ws.isValid(
        { x: 100, y: 100, width: 50, height: 50, isMaximized: false },
        1920,
        1080,
      );
      expect(invalid).toBe(false);
    });
  });

  describe('apply()', () => {
    it('调用后端命令应用状态', async () => {
      const backend = new MockBackend({
        capabilities: ['host_window_set_position', 'host_window_set_size', 'host_window_maximize'],
      });
      const ws = new WindowState({ storageKey: 'test.window7' });
      ws.save({ x: 200, y: 150, width: 1024, height: 768 });
      await ws.apply(backend);
      expect(backend.invocations.some((i) => i.cmd === 'host_window_set_position')).toBe(true);
      expect(backend.invocations.some((i) => i.cmd === 'host_window_set_size')).toBe(true);
    });
  });

  describe('轮 63：读侧失败不再静默 + 存档存在性', () => {
    it('坏 JSON 回落默认值，但把原因记下来（此前是 catch {} 整段吞掉）', () => {
      window.localStorage.setItem('test.window8', '{truncated');
      const ws = new WindowState({ storageKey: 'test.window8' });
      expect(ws.currentState.width).toBe(1200);
      expect(ws.hasSavedState).toBe(false);
      expect(ws.lastError).not.toBeNull();
      window.localStorage.removeItem('test.window8');
    });

    it('存档不是对象（例如一个裸数字）：留痕且不认作存档', () => {
      window.localStorage.setItem('test.window10', '7');
      const ws = new WindowState({ storageKey: 'test.window10' });
      expect(ws.hasSavedState).toBe(false);
      expect(ws.lastError).toContain('存档内容不是对象');
      window.localStorage.removeItem('test.window10');
    });

    it('只有读到合法存档才算 hasSavedState；save 置真、clear 置假', () => {
      const ws = new WindowState({ storageKey: 'test.window9' });
      expect(ws.hasSavedState).toBe(false);
      expect(ws.save({ width: 900 })).toBe(true);
      expect(ws.hasSavedState).toBe(true);
      const reloaded = new WindowState({ storageKey: 'test.window9' });
      expect(reloaded.hasSavedState).toBe(true);
      expect(reloaded.currentState.width).toBe(900);
      expect(reloaded.clear()).toBe(true);
      expect(reloaded.hasSavedState).toBe(false);
      expect(new WindowState({ storageKey: 'test.window9' }).hasSavedState).toBe(false);
    });

    it('apply() 只需要 invoke：带守卫的窄后端即可（恢复腿据此在停止后拒发）', async () => {
      const seen: string[] = [];
      const ws = new WindowState({ storageKey: 'test.window11' });
      ws.save({ x: 5, y: 6, width: 700, height: 500 });
      const ok = await ws.apply({
        invoke: <T>(cmd: string): Promise<T> => {
          seen.push(cmd);
          return Promise.resolve(undefined as T);
        },
      });
      expect(ok).toBe(true);
      expect(seen).toEqual(['host_window_set_position', 'host_window_set_size']);
      window.localStorage.removeItem('test.window11');
    });
  });
});
