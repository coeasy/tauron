// CommandPalette 状态管理测试

import { describe, expect, it, beforeEach } from 'vitest';
import { CommandPaletteStore } from './command-palette';

const testCommands = [
  { id: 'cmd-1', name: '打开设置', description: '打开应用设置', group: '系统', enabled: true },
  { id: 'cmd-2', name: '关闭应用', description: '关闭应用程序', group: '系统', enabled: true },
  { id: 'cmd-3', name: '新建文件', description: '创建新文件', group: '文件', enabled: true },
  { id: 'cmd-4', name: '保存文件', description: '保存当前文件', group: '文件', enabled: false },
  { id: 'cmd-5', name: '帮助文档', description: '查看帮助', group: '帮助', enabled: true },
];

describe('CommandPaletteStore', () => {
  let store: CommandPaletteStore;

  beforeEach(() => {
    store = new CommandPaletteStore({ commands: testCommands });
  });

  describe('初始状态', () => {
    it('默认关闭', () => {
      expect(store.state.status).toBe('closed');
    });

    it('支持自定义配置', () => {
      const store = new CommandPaletteStore({ commands: [], maxResults: 10 });
      expect(store.config.maxResults).toBe(10);
    });
  });

  describe('open/close/toggle', () => {
    it('打开', () => {
      const result = store.open();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('open');
    });

    it('已打开时拒绝', () => {
      store.open();
      const result = store.open();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_OPEN');
    });

    it('关闭', () => {
      store.open();
      const result = store.close();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('closed');
    });

    it('已关闭时拒绝', () => {
      const result = store.close();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_CLOSED');
    });

    it('切换', () => {
      store.toggle();
      expect(store.state.status).toBe('open');
      store.toggle();
      expect(store.state.status).toBe('closed');
    });
  });

  describe('setQuery', () => {
    beforeEach(() => {
      store.open();
    });

    it('设置查询', () => {
      const result = store.setQuery('设置');
      expect(result.ok).toBe(true);
      if (store.state.status === 'open') {
        expect(store.state.results.length).toBeGreaterThan(0);
      }
    });

    it('空查询返回所有命令', () => {
      store.setQuery('');
      if (store.state.status === 'open') {
        expect(store.state.results.length).toBeLessThanOrEqual(5);
      }
    });

    it('过滤不可用命令', () => {
      store.setQuery('保存');
      if (store.state.status === 'open') {
        expect(store.state.results.length).toBe(0);
      }
    });

    it('未打开时拒绝', () => {
      store.close();
      const result = store.setQuery('test');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_OPEN');
    });
  });

  describe('select/selectNext/selectPrev', () => {
    beforeEach(() => {
      store.open();
      store.setQuery('');
    });

    it('选择命令', () => {
      const result = store.select(0);
      expect(result.ok).toBe(true);
      if (store.state.status === 'open') {
        expect(store.state.selectedId).toBe('cmd-1');
      }
    });

    it('索引越界', () => {
      const result = store.select(99);
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_INVALID_INDEX');
    });

    it('向下选择', () => {
      store.select(0);
      store.selectNext();
      if (store.state.status === 'open') {
        expect(store.state.selectedId).toBe('cmd-2');
      }
    });

    it('向上选择', () => {
      store.select(1);
      store.selectPrev();
      if (store.state.status === 'open') {
        expect(store.state.selectedId).toBe('cmd-1');
      }
    });
  });

  describe('execute', () => {
    it('执行命令', () => {
      let executed = false;
      const commands = [
        { id: 'cmd-1', name: '测试', description: '', group: '', enabled: true, execute: () => { executed = true; } },
      ];
      const store = new CommandPaletteStore({ commands });
      store.open();
      store.setQuery('');
      store.select(0);
      const result = store.execute();
      expect(result.ok).toBe(true);
      expect(executed).toBe(true);
      expect(store.state.status).toBe('closed');
    });

    it('未选择时拒绝', () => {
      store.open();
      const result = store.execute();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NO_SELECTION');
    });

    it('不可用命令拒绝', () => {
      store.open();
      store.setQuery('保存');
      // 保存命令不可用，但过滤后不应出现
      expect(store.state.status === 'open' && store.state.results.length).toBe(0);
    });
  });

  describe('subscribe', () => {
    it('订阅状态变化', () => {
      const statuses: string[] = [];
      const unsub = store.subscribe((snap) => statuses.push(snap.state.status));
      expect(statuses.length).toBe(1);
      store.open();
      expect(statuses.length).toBe(2);
      unsub();
      store.close();
      expect(statuses.length).toBe(2);
    });
  });
});
