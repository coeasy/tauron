// Toast 状态管理测试

import { describe, expect, it, beforeEach } from 'vitest';
import { ToastStore } from './toast';

/**
 * 取第一条通知的 ID。
 *
 * `noUncheckedIndexedAccess` 下 `store.items[0]` 的类型是 `ToastItem | undefined`，
 * 这里显式收窄，测试断言不依赖非空断言。
 */
function firstItemId(store: ToastStore): string {
  const [first] = store.items;
  if (!first) {
    throw new Error('expected at least one toast item');
  }
  return first.id;
}

describe('ToastStore', () => {
  let store: ToastStore;

  beforeEach(() => {
    store = new ToastStore();
  });

  describe('初始状态', () => {
    it('默认配置正确', () => {
      expect(store.config.maxHistory).toBe(100);
      expect(store.config.defaultTimeout).toBe(5000);
      expect(store.config.systemNotifications).toBe(true);
    });

    it('空列表', () => {
      expect(store.items.length).toBe(0);
      expect(store.unreadCount).toBe(0);
    });
  });

  describe('push', () => {
    it('添加通知', () => {
      const result = store.push({ title: '标题', message: '内容', level: 'info' });
      expect(result.ok).toBe(true);
      expect(store.items.length).toBe(1);
    });

    it('环形裁剪', () => {
      const store = new ToastStore({ maxHistory: 3 });
      for (let i = 0; i < 5; i++) {
        store.push({ title: `通知${i}`, message: '', level: 'info' });
      }
      expect(store.items.length).toBe(3);
    });

    it('未读计数', () => {
      store.push({ title: '1', message: '', level: 'info' });
      store.push({ title: '2', message: '', level: 'info' });
      expect(store.unreadCount).toBe(2);
      store.markRead(firstItemId(store));
      expect(store.unreadCount).toBe(1);
    });
  });

  describe('success/error/warning/info', () => {
    it('success', () => {
      store.success('成功', '操作成功');
      expect(store.items[0]?.level).toBe('success');
    });

    it('error 不自动关闭', () => {
      store.error('错误', '操作失败');
      expect(store.items[0]?.autoDismiss).toBe(false);
    });
  });

  describe('markRead/markAllRead', () => {
    it('标记已读', () => {
      store.push({ title: '1', message: '', level: 'info' });
      const result = store.markRead(firstItemId(store));
      expect(result.ok).toBe(true);
      expect(store.items[0]?.unread).toBe(true);
    });

    it('不存在的通知', () => {
      const result = store.markRead('nonexistent');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_FOUND');
    });

    it('全部标记已读', () => {
      store.push({ title: '1', message: '', level: 'info' });
      store.push({ title: '2', message: '', level: 'info' });
      store.markAllRead();
      expect(store.unreadCount).toBe(0);
    });
  });

  describe('dismiss', () => {
    it('关闭通知', () => {
      store.push({ title: '1', message: '', level: 'info' });
      const result = store.dismiss(firstItemId(store));
      expect(result.ok).toBe(true);
      expect(store.items[0]?.dismissed).toBe(true);
    });
  });

  describe('clear', () => {
    it('清空所有通知', () => {
      store.push({ title: '1', message: '', level: 'info' });
      store.clear();
      expect(store.items.length).toBe(0);
    });
  });

  describe('cleanupPlugin', () => {
    it('清理插件通知', () => {
      store.push({ title: '1', message: '', level: 'info', pluginId: 'plugin-a' });
      store.push({ title: '2', message: '', level: 'info', pluginId: 'plugin-b' });
      store.push({ title: '3', message: '', level: 'info', pluginId: 'plugin-a' });
      store.cleanupPlugin('plugin-a');
      expect(store.items.length).toBe(1);
      expect(store.items[0]?.pluginId).toBe('plugin-b');
    });
  });

  describe('分组计数', () => {
    it('按插件分组', () => {
      store.push({ title: '1', message: '', level: 'info', pluginId: 'plugin-a' });
      store.push({ title: '2', message: '', level: 'info', pluginId: 'plugin-a' });
      store.push({ title: '3', message: '', level: 'info', pluginId: 'plugin-b' });
      const snap = store.snapshot;
      expect(snap.groupCounts['plugin-a']).toBe(2);
      expect(snap.groupCounts['plugin-b']).toBe(1);
    });

    it('系统通知归组', () => {
      store.push({ title: '1', message: '', level: 'info' });
      const snap = store.snapshot;
      expect(snap.groupCounts['system']).toBe(1);
    });
  });

  describe('subscribe', () => {
    it('订阅状态变化', () => {
      const count: number[] = [];
      const unsub = store.subscribe((snap) => count.push(snap.items.length));
      expect(count.length).toBe(1);
      store.push({ title: '1', message: '', level: 'info' });
      expect(count.length).toBe(2);
      unsub();
      store.push({ title: '2', message: '', level: 'info' });
      expect(count.length).toBe(2);
    });
  });
});
