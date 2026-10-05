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

/**
 * Shell 实例状态（本包用到的 4 态）。
 *
 * ⚠️ `ready` **不等于**「已装载可用」：四条 start 路径都是延迟模拟，
 * 判据是 {@link ShellInstance.simulated}（见文件头诚实边界）。
 */
export type ShellStatus = 'loading' | 'ready' | 'error' | 'unloaded';

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
  /** 插件状态（迁移规则见 `manager.ts` 的 `SHELL_STATUS_TRANSITIONS`） */
  status: ShellStatus;
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
  /**
   * 启动 shell。
   *
   * ⚠️ 失败**一定 reject**（V7-P1-03）：此前它把异常咽成 `status: 'error'`
   * 却正常 resolve，调用方会把"没起来"当成"起来了"。被 `stop()`/`destroy()`/
   * 再次 `start()` 作废的那次启动同样 reject（`code: 'SHELL_START_ABANDONED'`），
   * 不返回旧一代的结果。
   */
  start(): Promise<void>;
  /** 停止 shell（作废在途启动并等它结束，见 `start()` 的说明） */
  stop(): Promise<void>;
  /** 获取 shell URL */
  getUrl(): string;
}

/**
 * 装载实现注入点。
 *
 * 默认实现（`manager.ts` 的四个 `start*` 路径）是 **10ms 延迟模拟**，不装载任何
 * 资源。这里留出注入位有两个理由：① 测试能把「启动何时落定」握在自己手里，
 * 从而确定性地验证陈旧启动不得复活实例；② 接真实装载时替换的就是这一层，
 * 而不必改动代际/迁移表。注入实现**不会**收到取消信号，因此它的结果同样受
 * 代际闸门约束：作废之后的完成只会被记为 `abandonedStarts`。
 */
export type ShellStartProvider = (config: ShellConfig) => Promise<void>;

/** `createShellManager()` 的可选参数 */
export interface ShellManagerOptions {
  /**
   * 按形态覆盖装载实现；未列出的形态仍走内置 10ms 模拟。
   *
   * ⚠️ 覆盖它**不等于**接上了真实装载：`simulated` 仍恒为 `true`，
   * 那要等本包真正产出文件/服务器/webview/远程装载时才改。
   */
  startProviders?: Partial<Record<ShellForm, ShellStartProvider>>;
}

/** 生命周期观测计数（供测试与泄漏门禁读取，不是业务状态）。 */
export interface ShellManagerStats {
  /** 因代际失效（stop/destroy/再次 start）而被丢弃的启动完成数 */
  abandonedStarts: number;
  /** 被状态迁移表拒绝的写入数（表外写入等于状态机说谎） */
  illegalTransitions: number;
  /** 累计创建的代际数（双 start 不重叠的判据：每次 start 恰好 +1） */
  generations: number;
}

/** Shell 管理器 */
export interface ShellManager {
  /** 创建 shell */
  create(config: ShellConfig): ShellInstance;
  /** 获取 shell */
  get(pluginId: string): ShellInstance | undefined;
  /** 列出所有 shell */
  list(): ShellInstance[];
  /**
   * 销毁 shell
   *
   * 先 `stop()`（作废在途启动并等其结束），再删表项——顺序反了就会留下
   * 「已删除但又被陈旧启动写成 ready」的实例。
   */
  destroy(pluginId: string): Promise<void>;
  /** 清理所有 shell */
  clear(): Promise<void>;
  /** 观测计数快照 */
  stats(): ShellManagerStats;
}
