import { describe, it, expect, beforeEach } from 'vitest';
import { ConfigManager } from './config.js';

describe('ConfigManager', () => {
  let config: ConfigManager;

  beforeEach(() => {
    config = new ConfigManager();
  });

  describe('setDefaults / get', () => {
    it('returns default value', () => {
      config.setDefaults({ theme: 'dark', volume: 50 });
      expect(config.get('theme')).toBe('dark');
      expect(config.get('volume')).toBe(50);
    });

    it('returns undefined for missing key', () => {
      config.setDefaults({ theme: 'dark' });
      expect(config.get('nonexistent')).toBeUndefined();
    });
  });

  describe('user config (Layer 2)', () => {
    it('overrides default with user value', () => {
      config.setDefaults({ theme: 'dark', volume: 50 });
      config.setUserConfig({ theme: 'light' });

      expect(config.get('theme')).toBe('light');
      expect(config.get('volume')).toBe(50); // Still default
    });
  });

  describe('plugin override (Layer 3)', () => {
    it('overrides user with plugin value', () => {
      config.setDefaults({ theme: 'dark' });
      config.setUserConfig({ theme: 'light' });
      config.setPluginOverride('com.example.plugin', { theme: 'blue' });

      expect(config.get('theme')).toBe('blue');
    });
  });

  describe('session override (Layer 4)', () => {
    it('has highest priority', () => {
      config.setDefaults({ theme: 'dark' });
      config.setUserConfig({ theme: 'light' });
      config.setPluginOverride('com.example.plugin', { theme: 'blue' });
      config.setSessionOverride('theme', 'green');

      expect(config.get('theme')).toBe('green');
    });

    it('removes session override on remove', () => {
      config.setDefaults({ theme: 'dark' });
      config.setUserConfig({ theme: 'light' });
      config.setSessionOverride('theme', 'green');
      config.removeSessionOverride('theme');

      expect(config.get('theme')).toBe('light'); // Falls back to user
    });
  });

  describe('priority: session > plugin > user > default', () => {
    it('correctly resolves priority chain', () => {
      config.setDefaults({ volume: 50 });
      config.setUserConfig({ volume: 75 });
      config.setPluginOverride('p1', { volume: 60 });
      config.setSessionOverride('volume', 90);

      expect(config.get('volume')).toBe(90);

      config.removeSessionOverride('volume');
      expect(config.get('volume')).toBe(60);

      config.removePluginOverride('p1');
      expect(config.get('volume')).toBe(75);

      config.setUserConfig({});
      expect(config.get('volume')).toBe(50);
    });
  });

  describe('getAll', () => {
    it('returns merged config with all layers', () => {
      config.setDefaults({ a: 1, b: 2, c: 3 });
      config.setUserConfig({ b: 20, d: 4 });
      config.setPluginOverride('p1', { c: 30 });
      config.setSessionOverride('d', 40);

      const all = config.getAll();
      expect(all).toEqual({ a: 1, b: 20, c: 30, d: 40 });
    });
  });

  describe('onChange', () => {
    it('notifies listeners on set', () => {
      const changes: Array<[string, unknown]> = [];
      config.onChange((key, value) => {
        changes.push([key, value]);
      });

      config.set('theme', 'dark', 'user');
      config.set('theme', 'light', 'session');

      expect(changes).toEqual([
        ['theme', 'dark'],
        ['theme', 'light'],
      ]);
    });

    it('allows unsubscribing', () => {
      let count = 0;
      const unsub = config.onChange(() => count++);

      config.set('a', 1, 'user');
      unsub();
      config.set('b', 2, 'user');

      expect(count).toBe(1);
    });
  });

  describe('clear', () => {
    it('removes all config', () => {
      config.setDefaults({ a: 1 });
      config.setUserConfig({ b: 2 });
      config.clear();

      expect(config.get('a')).toBeUndefined();
      expect(config.get('b')).toBeUndefined();
    });
  });

  describe('原型安全（收敛审计：防原型链泄漏与污染）', () => {
    it('get 不返回 Object.prototype 继承成员', () => {
      config.setDefaults({ theme: 'dark' });
      // 修复前 `key in obj` 会命中原型链，返回继承的函数
      expect(config.get('toString')).toBeUndefined();
      expect(config.get('constructor')).toBeUndefined();
      expect(config.get('hasOwnProperty')).toBeUndefined();
    });

    it('set __proto__ 键不改变存储对象原型', () => {
      const evil = { polluted: true } as Record<string, unknown>;
      config.set('__proto__', evil, 'session');
      // 后续正常读写不受影响
      config.set('theme', 'x', 'user');
      expect(config.get('theme')).toBe('x');
      expect(({} as Record<string, unknown>)['polluted']).toBeUndefined();
    });

    it('getAll 结果原型不被 __proto__ 键破坏', () => {
      config.setSessionOverride('__proto__', { polluted: true });
      const all = config.getAll();
      expect(Object.getPrototypeOf(all)).toBe(Object.prototype);
      expect(({} as Record<string, unknown>)['polluted']).toBeUndefined();
    });

    it('setUserConfig 拷贝含 __proto__ 自有键的源对象时不触发原型 setter', () => {
      const src = Object.create(null) as Record<string, unknown>;
      src['__proto__'] = { polluted: true };
      src['theme'] = 'safe';
      config.setUserConfig(src);
      expect(config.get('theme')).toBe('safe');
      const all = config.getAll();
      expect(Object.getPrototypeOf(all)).toBe(Object.prototype);
      expect(({} as Record<string, unknown>)['polluted']).toBeUndefined();
    });
  });
});
