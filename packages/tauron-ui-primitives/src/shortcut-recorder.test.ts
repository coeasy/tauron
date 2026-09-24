// ShortcutRecorder 状态管理测试

import { describe, expect, it, beforeEach } from 'vitest';
import { ShortcutRecorderStore } from './shortcut-recorder';

describe('ShortcutRecorderStore', () => {
  let store: ShortcutRecorderStore;

  beforeEach(() => {
    store = new ShortcutRecorderStore();
  });

  describe('初始状态', () => {
    it('默认 idle', () => {
      expect(store.state.status).toBe('idle');
      expect(store.state.current).toBeNull();
    });

    it('支持自定义配置', () => {
      const store = new ShortcutRecorderStore({ current: 'Ctrl+K', assigned: ['Ctrl+K'] });
      expect(store.config.current).toBe('Ctrl+K');
      expect(store.state.current).toBe('Ctrl+K');
    });
  });

  describe('start/stop', () => {
    it('开始录入', () => {
      const result = store.start();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('recording');
    });

    it('已在录入时拒绝', () => {
      store.start();
      const result = store.start();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_ALREADY_RECORDING');
    });

    it('停止录入', () => {
      store.start();
      const result = store.stop();
      expect(result.ok).toBe(true);
      expect(store.state.status).toBe('idle');
    });

    it('未在录入时拒绝停止', () => {
      const result = store.stop();
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_RECORDING');
    });
  });

  describe('recordModifier', () => {
    beforeEach(() => {
      store.start();
    });

    it('记录修饰键', () => {
      const result = store.recordModifier('Ctrl');
      expect(result.ok).toBe(true);
    });

    it('重复修饰键拒绝', () => {
      store.recordModifier('Ctrl');
      const result = store.recordModifier('Ctrl');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_DUPLICATE_MODIFIER');
    });

    it('超出最大按键数', () => {
      const store = new ShortcutRecorderStore({ maxKeys: 2 });
      store.start();
      store.recordModifier('Ctrl');
      store.recordModifier('Shift');
      const result = store.recordModifier('Alt');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_MAX_KEYS');
    });
  });

  describe('recordModifier 未在录入时', () => {
    it('未在录入时拒绝', () => {
      const result = store.recordModifier('Ctrl');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_RECORDING');
    });
  });

  describe('releaseModifier', () => {
    beforeEach(() => {
      store.start();
    });

    it('释放修饰键', () => {
      store.recordModifier('Ctrl');
      const result = store.releaseModifier('Ctrl');
      expect(result.ok).toBe(true);
    });

    it('未按下的修饰键', () => {
      const result = store.releaseModifier('Shift');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_PRESSED');
    });
  });

  describe('recordKey', () => {
    beforeEach(() => {
      store.start();
    });

    it('正常记录按键', () => {
      store.recordModifier('Ctrl');
      const result = store.recordKey('K');
      expect(result.ok).toBe(true);
      if (result.ok && result.shortcut) {
        expect(result.shortcut).toBe('Ctrl+K');
      }
    });

    it('至少 2 键', () => {
      const result = store.recordKey('A');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_TOO_FEW_KEYS');
      expect(store.state.status).toBe('error');
    });

    it('允许单键时通过', () => {
      const store = new ShortcutRecorderStore({ allowSingleKey: true });
      store.start();
      const result = store.recordKey('F5');
      expect(result.ok).toBe(true);
    });

    it('修饰键作为主键拒绝', () => {
      const result = store.recordKey('Control');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_MODIFIER_ONLY');
    });

    it('冲突检测', () => {
      const store = new ShortcutRecorderStore({ assigned: ['Ctrl+K'] });
      store.start();
      store.recordModifier('Ctrl');
      const result = store.recordKey('K');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_CONFLICT');
    });
  });

  describe('recordKey 未在录入时', () => {
    it('未在录入时拒绝', () => {
      const result = store.recordKey('K');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_NOT_RECORDING');
    });
  });

  describe('clear', () => {
    it('清除快捷键', () => {
      const store = new ShortcutRecorderStore({ current: 'Ctrl+K' });
      store.clear();
      expect(store.config.current).toBeNull();
      expect(store.state.current).toBeNull();
    });
  });

  describe('validate', () => {
    it('有效快捷键', () => {
      const result = store.validate('Ctrl+K');
      expect(result.ok).toBe(true);
    });

    it('空快捷键', () => {
      const result = store.validate('');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_EMPTY');
    });

    it('键数不足', () => {
      const result = store.validate('K');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_TOO_FEW_KEYS');
    });

    it('键数过多', () => {
      const result = store.validate('Ctrl+Shift+Alt+Meta+K');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_TOO_MANY_KEYS');
    });

    it('非法字符', () => {
      const result = store.validate('Ctrl+K!');
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.code).toBe('E_INVALID_CHAR');
    });
  });

  describe('subscribe', () => {
    it('订阅状态变化', () => {
      const statuses: string[] = [];
      const unsub = store.subscribe((snap) => statuses.push(snap.state.status));
      expect(statuses.length).toBe(1);
      store.start();
      expect(statuses.length).toBe(2);
      unsub();
      store.stop();
      expect(statuses.length).toBe(2);
    });
  });
});
