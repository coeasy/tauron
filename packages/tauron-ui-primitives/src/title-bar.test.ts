// TitleBar 状态管理测试

import { describe, expect, it, beforeEach } from 'vitest';
import { TitleBarStore } from './title-bar';

describe('TitleBarStore', () => {
  let store: TitleBarStore;

  beforeEach(() => {
    store = new TitleBarStore();
  });

  describe('初始状态', () => {
    it('默认状态为 idle/normal', () => {
      expect(store.state.status).toBe('idle');
      if (store.state.status === 'idle') {
        expect(store.state.windowState).toBe('normal');
      }
    });

    it('默认配置正确', () => {
      const config = store.config;
      expect(config.title).toBe('Open Client');
      expect(config.showMinimize).toBe(true);
      expect(config.showMaximize).toBe(true);
      expect(config.showClose).toBe(true);
      expect(config.locked).toBe(false);
      expect(config.maximizeThreshold).toBe(2);
    });

    it('支持自定义配置', () => {
      const store = new TitleBarStore({ title: 'Custom', showClose: false });
      expect(store.config.title).toBe('Custom');
      expect(store.config.showClose).toBe(false);
    });
  });

  describe('minimize', () => {
    it('正常最小化（P0-1 修复后同步切换）', () => {
      const result = store.minimize();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('idle');
      if (store.state.status === 'idle') {
        expect(store.state.windowState).toBe('minimized');
      }
    });

    it('窗口锁定时拒绝', () => {
      store.setConfig({ locked: true });
      const result = store.minimize();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_WINDOW_LOCKED');
    });

    it('不可最小化时拒绝', () => {
      store.setConfig({ minimizable: false });
      const result = store.minimize();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_MINIMIZABLE');
    });

    it('按钮隐藏时拒绝', () => {
      store.setConfig({ showMinimize: false });
      const result = store.minimize();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_BUTTON_HIDDEN');
    });

    it('已最小化时拒绝', () => {
      store.minimize();
      const result = store.minimize();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_MINIMIZED');
    });
  });

  describe('restore', () => {
    it('最小化后可还原（P0-1 修复后同步切换）', () => {
      store.minimize();
      const result = store.restore();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('idle');
      if (store.state.status === 'idle') {
        expect(store.state.windowState).toBe('normal');
      }
    });

    it('未最小化时拒绝', () => {
      const result = store.restore();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_MINIMIZED');
    });
  });

  describe('maximize/unmaximize', () => {
    it('正常最大化', () => {
      const result = store.maximize();
      expect(result.ok).toBe(true);
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('maximized');
    });

    it('已最大化时拒绝', () => {
      store.maximize();
      const result = store.maximize();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_MAXIMIZED');
    });

    it('窗口锁定时拒绝', () => {
      store.setConfig({ locked: true });
      const result = store.maximize();
      expect(result.ok).toBe(false);
    });

    it('不可最大化时拒绝', () => {
      store.setConfig({ maximizable: false });
      const result = store.maximize();
      expect(result.ok).toBe(false);
    });

    it('正常还原', () => {
      store.maximize();
      const result = store.unmaximize();
      expect(result.ok).toBe(true);
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('normal');
    });

    it('未最大化时拒绝还原', () => {
      const result = store.unmaximize();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_MAXIMIZED');
    });
  });

  describe('toggleMaximize', () => {
    it('normal → maximized', () => {
      store.toggleMaximize();
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('maximized');
    });

    it('maximized → normal', () => {
      store.maximize();
      store.toggleMaximize();
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('normal');
    });
  });

  describe('lock/unlock', () => {
    it('锁定窗口', () => {
      store.lock();
      expect(store.config.locked).toBe(true);
    });

    it('解锁窗口', () => {
      store.lock();
      store.unlock();
      expect(store.config.locked).toBe(false);
    });
  });

  describe('recordClick', () => {
    it('达到阈值时切换最大化', () => {
      const store = new TitleBarStore({ maximizeThreshold: 2 });
      store.recordClick();
      store.recordClick();
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('maximized');
    });

    it('未达阈值时不切换', () => {
      const store = new TitleBarStore({ maximizeThreshold: 3 });
      store.recordClick();
      store.recordClick();
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('normal');
    });

    it('重置点击计数', () => {
      const store = new TitleBarStore({ maximizeThreshold: 2 });
      store.recordClick();
      expect(store.snapshot.clickCount).toBe(1);
      store.recordClick();
      expect(store.snapshot.clickCount).toBe(0);
    });
  });

  describe('completeMinimize/completeRestore', () => {
    it('完成最小化（P0-1 修复后 minimize 已同步完成，completeMinimize 为 no-op）', () => {
      store.minimize();
      store.completeMinimize(); // no-op: minimize 已同步完成
      expect(store.state.status).toBe('idle');
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('minimized');
    });

    it('完成还原（P0-1 修复后 restore 已同步完成，completeRestore 为 no-op）', () => {
      store.minimize();
      store.restore();
      store.completeRestore(); // no-op: restore 已同步完成
      expect(store.state.status).toBe('idle');
      if (store.state.status === 'idle') expect(store.state.windowState).toBe('normal');
    });
  });

  describe('subscribe', () => {
    it('订阅状态变化', () => {
      const states: string[] = [];
      const unsub = store.subscribe((snap) => states.push(snap.state.status));
      expect(states.length).toBe(1);
      store.minimize();
      expect(states.length).toBe(2);
      unsub();
      store.completeMinimize();
      expect(states.length).toBe(2);
    });
  });
});
