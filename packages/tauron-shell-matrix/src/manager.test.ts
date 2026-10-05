import { describe, it, expect } from 'vitest';
import { createShellManager, ShellStartAbandonedError } from './manager.js';
import type {
  LocalShellConfig,
  LocalServerShellConfig,
  RemoteUrlShellConfig,
  SubWebviewShellConfig,
} from './types.js';
import type { PluginManifest } from '@tauron/types';

/**
 * 手动落定的门：代际竞态的判据必须是「测试说了算的完成时机」。
 *
 * 内置的 10ms 模拟路径无法在**确定的那一时刻**落定，所以时序类断言一律走
 * `startProviders` 注入（注入的语义与内置路径一致：不可取消，落定后仍过代际闸门）。
 */
function deferred<T = void>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** 取 reject 原因（同时把 promise 标记为已处理，避免未处理 reject）。 */
function rejection(p: Promise<unknown>): Promise<unknown> {
  return p.then(
    (value) => {
      throw new Error(`expected rejection, but resolved with: ${String(value)}`);
    },
    (reason: unknown) => reason,
  );
}

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

  // ────────────────────────────────────────────────────────────────────────
  // V7-P1-03：代际、状态迁移表与错误传播。
  //
  // 下面的时序断言全部由手动门驱动，没有任何 sleep 参与判据。
  // ⚠️ 这些测试证明的是**时序正确性**，不是装载真实化：`simulated` 仍恒为 true。
  // ────────────────────────────────────────────────────────────────────────
  describe('代际与生命周期（V7-P1-03）', () => {
    const localConfig: LocalShellConfig = {
      form: 'local',
      pluginId: 'test-plugin',
      manifest: mockManifest,
      installDir: '/plugins/test-plugin',
      entryPath: '/plugins/test-plugin/index.html',
    };

    it('start 在途时 stop：迟到的启动不得把实例写回 ready', async () => {
      const gate = deferred();
      const manager = createShellManager({
        startProviders: { local: () => gate.promise },
      });
      const shell = manager.create(localConfig);

      const starting = rejection(shell.start());
      expect(shell.status).toBe('loading');

      const stopping = shell.stop();
      gate.resolve(); // start 落定：此刻代际已作废，结果只能丢弃
      await stopping;

      expect(shell.status, 'stop 之后必须是 unloaded').toBe('unloaded');
      expect(await starting).toBeInstanceOf(ShellStartAbandonedError);
      expect(manager.stats()).toEqual({
        abandonedStarts: 1,
        illegalTransitions: 0,
        generations: 1,
      });
    });

    it('装载失败：start() reject 原始 cause，状态落 error（不再 resolve 却留 error）', async () => {
      const boom = new Error('boom');
      let fail = true;
      const manager = createShellManager({
        startProviders: {
          local: async () => {
            if (fail) throw boom;
          },
        },
      });
      const shell = manager.create(localConfig);

      await expect(shell.start()).rejects.toBe(boom);
      expect(shell.status).toBe('error');
      expect(shell.error).toBe('boom');
      expect(shell.simulated).toBe(true);
      expect(manager.stats()).toMatchObject({ abandonedStarts: 0, illegalTransitions: 0 });

      // 失败后重启是表内迁移（error → loading → ready），不是绕过状态机。
      fail = false;
      await shell.start();
      expect(shell.status).toBe('ready');
      expect(manager.stats()).toMatchObject({ generations: 2, illegalTransitions: 0 });
    });

    it('destroy 期间 start 在途：表项删除后没有迟到写入', async () => {
      const gate = deferred();
      const manager = createShellManager({
        startProviders: { local: () => gate.promise },
      });
      const shell = manager.create(localConfig);
      const starting = rejection(shell.start());

      const destroying = manager.destroy('test-plugin');
      gate.resolve();
      await destroying;

      expect(manager.get('test-plugin'), '表项必须已删除').toBeUndefined();
      expect(shell.status, '销毁后的实例不得被写成 ready').toBe('unloaded');
      expect(await starting).toBeInstanceOf(ShellStartAbandonedError);
      expect(manager.stats()).toMatchObject({ abandonedStarts: 1, illegalTransitions: 0 });
    });

    it('双 start 不重叠：第一代被判废弃，只有新一代提交', async () => {
      const first = deferred();
      const second = deferred();
      let calls = 0;
      const manager = createShellManager({
        startProviders: {
          local: () => {
            calls += 1;
            return calls === 1 ? first.promise : second.promise;
          },
        },
      });
      const shell = manager.create(localConfig);

      const firstStart = rejection(shell.start());
      const secondStart = shell.start();

      second.resolve();
      await secondStart;
      expect(shell.status).toBe('ready');
      expect(manager.stats().abandonedStarts, '新一代提交时不该有废弃计数').toBe(0);

      first.resolve(); // 第一代迟到
      expect(await firstStart).toBeInstanceOf(ShellStartAbandonedError);
      expect(shell.status, '迟到的一代不得改写 ready').toBe('ready');
      expect(manager.stats()).toMatchObject({ generations: 2, abandonedStarts: 1 });
    });

    it('stop() 幂等：重复 stop 不产生表外迁移', async () => {
      const manager = createShellManager();
      const shell = manager.create(localConfig);
      await shell.start();

      await shell.stop();
      await shell.stop();

      expect(shell.status).toBe('unloaded');
      expect(manager.stats()).toMatchObject({ abandonedStarts: 0, illegalTransitions: 0 });
    });

    it('clear() 打断全部在途启动：无残留可写状态的异步任务', async () => {
      const gate = deferred();
      const manager = createShellManager({
        startProviders: { local: () => gate.promise },
      });
      const a = manager.create({ ...localConfig, pluginId: 'a' });
      const b = manager.create({ ...localConfig, pluginId: 'b' });
      const startingA = rejection(a.start());
      const startingB = rejection(b.start());

      const clearing = manager.clear();
      gate.resolve();
      await clearing;

      expect(manager.list()).toHaveLength(0);
      expect(a.status).toBe('unloaded');
      expect(b.status).toBe('unloaded');
      expect(await startingA).toBeInstanceOf(ShellStartAbandonedError);
      expect(await startingB).toBeInstanceOf(ShellStartAbandonedError);
      expect(manager.stats()).toMatchObject({ abandonedStarts: 2, illegalTransitions: 0 });
    });

    it('未注入 provider 时走内置 10ms 模拟（ready 必须带 simulated，不得被读成可用）', async () => {
      const manager = createShellManager();
      const shell = manager.create(localConfig);
      await shell.start();

      expect(shell.status).toBe('ready');
      expect(shell.simulated).toBe(true);
      expect(manager.stats()).toMatchObject({ abandonedStarts: 0, illegalTransitions: 0 });
    });
  });
});
