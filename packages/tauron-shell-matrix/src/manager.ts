/**
 * Shell 管理器（设计文档 §7.2）
 *
 * 管理 4 种 shell 形态的插件加载与生命周期。
 */

import type {
  ShellConfig,
  ShellInstance,
  ShellManager,
  LocalShellConfig,
  LocalServerShellConfig,
  RemoteUrlShellConfig,
  SubWebviewShellConfig,
} from './types.js';

/**
 * 创建 Shell 实例
 */
function createShell(config: ShellConfig): ShellInstance {
  const instance: ShellInstance = {
    config,
    status: 'loading',
    // 诚实标记：本包 `start()` 只做模拟加载（见文件头注释），恒为 true。
    simulated: true,

    async start(): Promise<void> {
      try {
        switch (config.form) {
          case 'local':
            await startLocalShell(config as LocalShellConfig, instance);
            break;
          case 'local-server':
            await startLocalServerShell(config as LocalServerShellConfig, instance);
            break;
          case 'remote-url':
            await startRemoteUrlShell(config as RemoteUrlShellConfig, instance);
            break;
          case 'sub-webview':
            await startSubWebviewShell(config as SubWebviewShellConfig, instance);
            break;
        }
        instance.status = 'ready';
      } catch (err) {
        instance.status = 'error';
        instance.error = err instanceof Error ? err.message : String(err);
      }
    },

    async stop(): Promise<void> {
      instance.status = 'unloaded';
    },

    getUrl(): string {
      switch (config.form) {
        case 'local':
          return `file://${(config as LocalShellConfig).entryPath}`;
        case 'local-server':
          return `http://${(config as LocalServerShellConfig).host}:${(config as LocalServerShellConfig).port}/`;
        case 'remote-url':
          return (config as RemoteUrlShellConfig).url;
        case 'sub-webview':
          return (config as SubWebviewShellConfig).webviewUrl;
      }
    },
  };

  return instance;
}

async function startLocalShell(config: LocalShellConfig, instance: ShellInstance): Promise<void> {
  // Simulate loading local file
  await new Promise((resolve) => setTimeout(resolve, 10));
  instance.status = 'ready';
}

async function startLocalServerShell(config: LocalServerShellConfig, instance: ShellInstance): Promise<void> {
  // Simulate starting local server
  await new Promise((resolve) => setTimeout(resolve, 10));
  instance.status = 'ready';
}

async function startRemoteUrlShell(config: RemoteUrlShellConfig, instance: ShellInstance): Promise<void> {
  // Simulate loading remote URL
  await new Promise((resolve) => setTimeout(resolve, 10));
  instance.status = 'ready';
}

async function startSubWebviewShell(config: SubWebviewShellConfig, instance: ShellInstance): Promise<void> {
  // Simulate creating sub-webview
  await new Promise((resolve) => setTimeout(resolve, 10));
  instance.status = 'ready';
}

/**
 * 创建 Shell 管理器
 */
export function createShellManager(): ShellManager {
  const shells = new Map<string, ShellInstance>();

  return {
    create(config: ShellConfig): ShellInstance {
      if (shells.has(config.pluginId)) {
        throw new Error(`Shell for plugin '${config.pluginId}' already exists`);
      }
      const instance = createShell(config);
      shells.set(config.pluginId, instance);
      return instance;
    },

    get(pluginId: string): ShellInstance | undefined {
      return shells.get(pluginId);
    },

    list(): ShellInstance[] {
      return Array.from(shells.values());
    },

    async destroy(pluginId: string): Promise<void> {
      const instance = shells.get(pluginId);
      if (instance) {
        await instance.stop();
        shells.delete(pluginId);
      }
    },

    async clear(): Promise<void> {
      for (const [pluginId, instance] of shells) {
        await instance.stop();
        shells.delete(pluginId);
      }
    },
  };
}
