import { describe, it, expect } from 'vitest';
import { createShellManager } from './manager.js';
import type { LocalShellConfig, LocalServerShellConfig, RemoteUrlShellConfig, SubWebviewShellConfig } from './types.js';
import type { PluginManifest } from '@tauron/types';

describe('Shell Manager', () => {
  const mockManifest: PluginManifest = {
    id: 'test-plugin',
    name: 'Test Plugin',
    version: '1.0.0',
    author: { name: 'Test Author', email: 'test@example.com' },
    license: 'MIT',
    apiVersion: '1.0.0',
    minFrameworkVersion: '1.0.0',
    platforms: ['linux', 'macos', 'windows'],
    type: 'js',
    entry: {},
    permissions: ['store:read'],
  };

  it('creates local shell', () => {
    const manager = createShellManager();
    const config: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    const shell = manager.create(config);
    expect(shell).toBeDefined();
    expect(shell.config.form).toBe('local');
    expect(shell.status).toBe('loading');
  });

  it('creates local-server shell', () => {
    const manager = createShellManager();
    const config: LocalServerShellConfig = {
      form: 'local-server',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      port: 8080,
      host: 'localhost',
      rootPath: '/plugins/test-plugin',
    };
    const shell = manager.create(config);
    expect(shell).toBeDefined();
    expect(shell.config.form).toBe('local-server');
  });

  it('creates remote-url shell', () => {
    const manager = createShellManager();
    const config: RemoteUrlShellConfig = {
      form: 'remote-url',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      url: 'https://plugins.example.com/test-plugin',
      cspEnabled: true,
      allowedDomains: ['plugins.example.com'],
    };
    const shell = manager.create(config);
    expect(shell).toBeDefined();
    expect(shell.config.form).toBe('remote-url');
  });

  it('creates sub-webview shell', () => {
    const manager = createShellManager();
    const config: SubWebviewShellConfig = {
      form: 'sub-webview',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      label: 'test-plugin-webview',
      webviewUrl: 'https://plugins.example.com/test-plugin',
      devtoolsEnabled: true,
    };
    const shell = manager.create(config);
    expect(shell).toBeDefined();
    expect(shell.config.form).toBe('sub-webview');
  });

  it('starts shell successfully（"成功"目前是**模拟**成功，必须能从实例上看出来）', async () => {
    const manager = createShellManager();
    const config: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    const shell = manager.create(config);
    await shell.start();
    expect(shell.status).toBe('ready');
    // 诚实性断言（轮 11）：`start()` 不装载资源，只是模拟延迟 → 必须带 simulated。
    // 若将来真接了装载实现，这条会红，提醒同时把它改成 false 并更新 README/类型注释。
    expect(shell.simulated).toBe(true);
  });

  it('stops shell', async () => {
    const manager = createShellManager();
    const config: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    const shell = manager.create(config);
    await shell.start();
    await shell.stop();
    expect(shell.status).toBe('unloaded');
  });

  it('gets shell URL for local', () => {
    const manager = createShellManager();
    const config: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    const shell = manager.create(config);
    expect(shell.getUrl()).toBe('file:///plugins/test-plugin/index.html');
  });

  it('gets shell URL for local-server', () => {
    const manager = createShellManager();
    const config: LocalServerShellConfig = {
      form: 'local-server',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      port: 8080,
      host: 'localhost',
      rootPath: '/plugins/test-plugin',
    };
    const shell = manager.create(config);
    expect(shell.getUrl()).toBe('http://localhost:8080/');
  });

  it('gets shell URL for remote-url', () => {
    const manager = createShellManager();
    const config: RemoteUrlShellConfig = {
      form: 'remote-url',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      url: 'https://plugins.example.com/test-plugin',
      cspEnabled: true,
      allowedDomains: ['plugins.example.com'],
    };
    const shell = manager.create(config);
    expect(shell.getUrl()).toBe('https://plugins.example.com/test-plugin');
  });

  it('gets shell URL for sub-webview', () => {
    const manager = createShellManager();
    const config: SubWebviewShellConfig = {
      form: 'sub-webview',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      label: 'test-plugin-webview',
      webviewUrl: 'https://plugins.example.com/test-plugin',
      devtoolsEnabled: true,
    };
    const shell = manager.create(config);
    expect(shell.getUrl()).toBe('https://plugins.example.com/test-plugin');
  });

  it('retrieves shell by plugin ID', () => {
    const manager = createShellManager();
    const config: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    manager.create(config);
    const shell = manager.get('test-plugin');
    expect(shell).toBeDefined();
  });

  it('returns undefined for non-existent shell', () => {
    const manager = createShellManager();
    expect(manager.get('unknown')).toBeUndefined();
  });

  it('lists all shells', () => {
    const manager = createShellManager();
    const config1: LocalShellConfig = {
      form: 'local',
      pluginId: 'plugin-1',
      manifest: { ...mockManifest, id: 'plugin-1' },
      installDir: '/plugins/plugin-1',
      entryPath: '/plugins/plugin-1/index.html',
    };
    const config2: LocalShellConfig = {
      form: 'local',
      pluginId: 'plugin-2',
      manifest: { ...mockManifest, id: 'plugin-2' },
      installDir: '/plugins/plugin-2',
      entryPath: '/plugins/plugin-2/index.html',
    };
    manager.create(config1);
    manager.create(config2);
    expect(manager.list().length).toBe(2);
  });

  it('destroys shell', async () => {
    const manager = createShellManager();
    const config: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    manager.create(config);
    await manager.destroy('test-plugin');
    expect(manager.get('test-plugin')).toBeUndefined();
  });

  it('clears all shells', async () => {
    const manager = createShellManager();
    const config1: LocalShellConfig = {
      form: 'local',
      pluginId: 'plugin-1',
      manifest: { ...mockManifest, id: 'plugin-1' },
      installDir: '/plugins/plugin-1',
      entryPath: '/plugins/plugin-1/index.html',
    };
    const config2: LocalShellConfig = {
      form: 'local',
      pluginId: 'plugin-2',
      manifest: { ...mockManifest, id: 'plugin-2' },
      installDir: '/plugins/plugin-2',
      entryPath: '/plugins/plugin-2/index.html',
    };
    manager.create(config1);
    manager.create(config2);
    await manager.clear();
    expect(manager.list().length).toBe(0);
  });

  it('throws when creating duplicate shell', () => {
    const manager = createShellManager();
    const localConfig: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };
    manager.create(localConfig);
    expect(() => manager.create(localConfig)).toThrow();
  });
});
