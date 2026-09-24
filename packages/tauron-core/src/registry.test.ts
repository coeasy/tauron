import { describe, it, expect, beforeEach } from 'vitest';
import { PluginRegistry, type RegistryConfig } from './registry.js';
import { type PluginState } from '@tauron/types';

describe('PluginRegistry', () => {
  let registry: PluginRegistry;

  beforeEach(() => {
    registry = new PluginRegistry();
  });

  describe('register', () => {
    it('registers a new plugin', () => {
      const result = registry.register('com.example.test', 'js', { id: 'com.example.test' });
      expect(result).toBe(true);
      expect(registry.getState('com.example.test')).toBe('DISCOVERED');
      expect(registry.size).toBe(1);
    });

    it('rejects duplicate registration', () => {
      registry.register('com.example.test', 'js', {});
      const result = registry.register('com.example.test', 'js', {});
      expect(result).toBe(false);
    });

    it('enforces max capacity', () => {
      const limited = new PluginRegistry({ maxPlugins: 2 });
      expect(limited.register('plugin-a', 'js', {})).toBe(true);
      expect(limited.register('plugin-b', 'js', {})).toBe(true);
      expect(limited.register('plugin-c', 'js', {})).toBe(false);
    });
  });

  describe('transition', () => {
    beforeEach(() => {
      registry.register('com.example.test', 'js', {});
    });

    it('allows valid transitions', () => {
      expect(registry.transition('com.example.test', 'INSTALLING')).toBe(true);
      expect(registry.getState('com.example.test')).toBe('INSTALLING');
      expect(registry.transition('com.example.test', 'INSTALLED')).toBe(true);
      expect(registry.getState('com.example.test')).toBe('INSTALLED');
    });

    it('rejects invalid transitions', () => {
      expect(registry.transition('com.example.test', 'ENABLED')).toBe(false);
      expect(registry.getState('com.example.test')).toBe('DISCOVERED');
    });

    it('rejects transitions for unregistered plugins', () => {
      expect(registry.transition('com.unknown.test', 'INSTALLING')).toBe(false);
    });

    it('emits state change events', () => {
      const transitions: Array<[string, PluginState, PluginState]> = [];
      registry.onStateChange((pluginId, from, to) => {
        transitions.push([pluginId, from, to]);
      });

      registry.transition('com.example.test', 'INSTALLING');
      registry.transition('com.example.test', 'INSTALLED');

      expect(transitions).toEqual([
        ['com.example.test', 'DISCOVERED', 'INSTALLING'],
        ['com.example.test', 'INSTALLING', 'INSTALLED'],
      ]);
    });
  });

  describe('listPluginIds', () => {
    it('returns all registered plugin IDs', () => {
      registry.register('plugin-a', 'js', {});
      registry.register('plugin-b', 'wasm', {});

      const ids = registry.listPluginIds();
      expect(ids).toContain('plugin-a');
      expect(ids).toContain('plugin-b');
      expect(ids.length).toBe(2);
    });
  });

  describe('listActivePluginIds', () => {
    it('returns only ENABLED plugins', () => {
      registry.register('plugin-a', 'js', {});
      registry.register('plugin-b', 'wasm', {});

      // Enable plugin-a
      registry.transition('plugin-a', 'INSTALLING');
      registry.transition('plugin-a', 'INSTALLED');
      registry.transition('plugin-a', 'ENABLING');
      registry.transition('plugin-a', 'ENABLED');

      // plugin-b stays in DISCOVERED
      const active = registry.listActivePluginIds();
      expect(active).toEqual(['plugin-a']);
    });
  });

  describe('unregister', () => {
    it('removes plugin in UNINSTALLING state', () => {
      registry.register('plugin-a', 'js', {});
      registry.transition('plugin-a', 'INSTALLING');
      registry.transition('plugin-a', 'INSTALLED');
      registry.transition('plugin-a', 'UNINSTALLING');

      expect(registry.unregister('plugin-a')).toBe(true);
      expect(registry.size).toBe(0);
    });

    it('rejects unregister for active plugins', () => {
      registry.register('plugin-a', 'js', {});
      expect(registry.unregister('plugin-a')).toBe(false);
    });
  });

  describe('clear', () => {
    it('removes all plugins', () => {
      registry.register('plugin-a', 'js', {});
      registry.register('plugin-b', 'wasm', {});
      registry.clear();
      expect(registry.size).toBe(0);
    });
  });
});
