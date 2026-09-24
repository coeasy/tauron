/**
 * Shell 矩阵类型定义（设计文档 §7.2）
 *
 * 4 种 shell 形态：
 * - local: 本地文件系统插件
 * - local-server: 本地 HTTP 服务器插件
 * - remote-url: 远程 URL 插件
 * - sub-webview: 子 Webview 插件
 *
 * ⚠️ **诚实边界（轮 11 审计）**：本包当前是**形态矩阵与生命周期骨架**，
 * `start()` **不装载任何资源**（不读文件、不起本地服务器、不建 webview），
 * 只有 `setTimeout` 模拟延迟后把状态置为 `ready`。因此
 * **`status: 'ready'` 不等于"可用"**——判据是 {@link ShellInstance.simulated}。
 * 真正的装载由宿主/适配层承担（`crates/tauron-adapter` 的窗口与 `contributes` 面）。
 * 本包目前**没有任何消费者**（无包、无 crate 依赖它）。
 */

import type { PluginManifest } from '@tauron/types';

/** Shell 形态枚举 */
export type ShellForm = 'local' | 'local-server' | 'remote-url' | 'sub-webview';

/** Shell 配置基类 */
export interface ShellConfig {
  /** Shell 形态 */
  form: ShellForm;
  /** 插件 ID */
  pluginId: string;
  /** 插件 manifest */
  manifest: PluginManifest;
  /** 额外配置 */
  options?: Record<string, unknown>;
}

/** Local shell 配置 - 本地文件系统 */
export interface LocalShellConfig extends ShellConfig {
  form: 'local';
  /** 插件安装目录 */
  installDir: string;
  /** 插件入口文件 */
  entryPath: string;
}

/** Local-server shell 配置 - 本地 HTTP 服务器 */
export interface LocalServerShellConfig extends ShellConfig {
  form: 'local-server';
  /** 本地服务器端口 */
  port: number;
  /** 服务器主机 */
  host: string;
  /** 插件资源根路径 */
  rootPath: string;
}

/** Remote-url shell 配置 - 远程 URL */
export interface RemoteUrlShellConfig extends ShellConfig {
  form: 'remote-url';
  /** 远程 URL */
  url: string;
  /** 是否启用 CSP */
  cspEnabled: boolean;
  /** 允许的域名 */
  allowedDomains: string[];
}

/** Sub-webview shell 配置 - 子 Webview */
export interface SubWebviewShellConfig extends ShellConfig {
  form: 'sub-webview';
  /** Webview 标签 */
  label: string;
  /** Webview URL */
  webviewUrl: string;
  /** 是否启用 devtools */
  devtoolsEnabled: boolean;
  /** Webview 边界 */
  bounds?: { x: number; y: number; width: number; height: number };
}

/** Shell 实例 */
export interface ShellInstance {
  /** 配置 */
  config: ShellConfig;
  /** 插件状态 */
  status: 'loading' | 'ready' | 'error' | 'unloaded';
  /**
   * 本轮启动是否为**模拟**（当前恒为 `true`）。
   *
   * 存在的理由：`status: 'ready'` 会被误读成"shell 已装载可用"，而本包
   * `start()` 只做延迟模拟。消费方据此判断"这次 ready 是不是真的"——
   * 没有这个字段时，唯一诚实的做法是把 `start()` 改成抛错，那会让整个矩阵
   * 骨架不可用。等真正接入装载实现后，这个字段应为 `false`。
   */
  simulated: boolean;
  /** 错误信息 */
  error?: string;
  /** 启动 shell */
  start(): Promise<void>;
  /** 停止 shell */
  stop(): Promise<void>;
  /** 获取 shell URL */
  getUrl(): string;
}

/** Shell 管理器 */
export interface ShellManager {
  /** 创建 shell */
  create(config: ShellConfig): ShellInstance;
  /** 获取 shell */
  get(pluginId: string): ShellInstance | undefined;
  /** 列出所有 shell */
  list(): ShellInstance[];
  /** 销毁 shell */
  destroy(pluginId: string): Promise<void>;
  /** 清理所有 shell */
  clear(): Promise<void>;
}
