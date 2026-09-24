import { describe, it, expect } from 'vitest';
import { createRegistry } from './registry.js';
import type { PluginManifest } from '@tauron/types';
import type { PluginPackage } from './types.js';

describe('Registry', () => {
  const mockManifest: PluginManifest = {
    id: 'test-plugin',
    name: 'Test Plugin',
    version: '1.0.0',
    author: { name: 'Test Author', email: 'test@example.com' },
    description: 'A test plugin',
    license: 'MIT',
    apiVersion: '1.0.0',
    minFrameworkVersion: '1.0.0',
    platforms: ['linux', 'macos', 'windows'],
    type: 'js',
    entry: {},
    permissions: ['store:read', 'http:fetch'],
  };

  const mockPackage: PluginPackage = {
    id: 'test-plugin',
    version: '1.0.0',
    filename: 'test-plugin-1.0.0.tgz',
    size: 1024,
    sha256: 'abc123def456',
    signature: 'sig123',
    uploadedBy: 'test-user',
    uploadedAt: new Date().toISOString(),
    verified: true,
  };

  it('creates empty registry', () => {
    const registry = createRegistry();
    expect(registry.size()).toBe(0);
  });

  it('registers a plugin', () => {
    const registry = createRegistry();
    const entry = registry.register('test-plugin', '1.0.0', mockManifest, mockPackage);
    expect(entry.id).toBe('test-plugin');
    expect(entry.version).toBe('1.0.0');
    expect(registry.size()).toBe(1);
  });

  it('retrieves registered plugin', () => {
    const registry = createRegistry();
    registry.register('test-plugin', '1.0.0', mockManifest, mockPackage);
    const entry = registry.get('test-plugin');
    expect(entry).toBeDefined();
    expect(entry?.id).toBe('test-plugin');
  });

  it('returns undefined for unregistered plugin', () => {
    const registry = createRegistry();
    expect(registry.get('unknown')).toBeUndefined();
  });

  it('lists all plugins', () => {
    const registry = createRegistry();
    registry.register('plugin-1', '1.0.0', { ...mockManifest, id: 'plugin-1' }, mockPackage);
    registry.register('plugin-2', '1.0.0', { ...mockManifest, id: 'plugin-2' }, mockPackage);
    const all = registry.getAll();
    expect(all.length).toBe(2);
  });

  it('removes a plugin', () => {
    const registry = createRegistry();
    registry.register('test-plugin', '1.0.0', mockManifest, mockPackage);
    expect(registry.remove('test-plugin')).toBe(true);
    expect(registry.size()).toBe(0);
  });

  it('returns false when removing non-existent plugin', () => {
    const registry = createRegistry();
    expect(registry.remove('unknown')).toBe(false);
  });

  it('checks plugin existence', () => {
    const registry = createRegistry();
    expect(registry.has('unknown')).toBe(false);
    registry.register('test-plugin', '1.0.0', mockManifest, mockPackage);
    expect(registry.has('test-plugin')).toBe(true);
  });

  it('clears all plugins', () => {
    const registry = createRegistry();
    registry.register('plugin-1', '1.0.0', { ...mockManifest, id: 'plugin-1' }, mockPackage);
    registry.register('plugin-2', '1.0.0', { ...mockManifest, id: 'plugin-2' }, mockPackage);
    registry.clear();
    expect(registry.size()).toBe(0);
  });

  it('searches plugins by ID', () => {
    const registry = createRegistry();
    registry.register('test-plugin', '1.0.0', { ...mockManifest, id: 'test-plugin', name: 'Alpha Plugin', description: 'Alpha plugin' }, mockPackage);
    registry.register('other-plugin', '1.0.0', { ...mockManifest, id: 'other-plugin', name: 'Beta Plugin', description: 'Beta plugin' }, mockPackage);
    const results = registry.search('test');
    expect(results.length).toBe(1);
    expect(results[0]?.id).toBe('test-plugin');
  });

  it('searches plugins by name', () => {
    const registry = createRegistry();
    registry.register('test-plugin', '1.0.0', mockManifest, mockPackage);
    const results = registry.search('Test Plugin');
    expect(results.length).toBe(1);
  });

  it('exports to JSON', () => {
    const registry = createRegistry();
    registry.register('test-plugin', '1.0.0', mockManifest, mockPackage);
    const json = registry.toJSON();
    expect(json.version).toBe('1.0');
    expect(json.generatedAt).toBeDefined();
    expect(Array.isArray(json.plugins)).toBe(true);
  });
});
