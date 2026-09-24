/**
 * @tauron/dual-world 共享类型
 *
 * B+ 双世界架构：
 * - World A: 宿主 JS 环境（Node.js/Browser）
 * - World B: QuickJS-WASM 沙箱（受限执行环境）
 * - Bridge: 两个世界之间的通信桥
 */

/** 沙箱能力集 */
export interface SandboxCapabilities {
  /** 允许文件系统访问 */
  fs: boolean;
  /** 允许网络请求 */
  network: boolean;
  /** 允许定时器 */
  timers: boolean;
  /** 允许 DOM 操作 */
  dom: boolean;
  /** 允许 Worker 创建 */
  workers: boolean;
  /** 允许 WASM 模块加载 */
  wasm: boolean;
}

/** 默认沙箱能力（最小权限） */
export const DEFAULT_SANDBOX_CAPABILITIES: SandboxCapabilities = {
  fs: false,
  network: false,
  timers: true,
  dom: false,
  workers: false,
  wasm: false,
};

/** 沙箱配置 */
export interface SandboxConfig {
  /** 插件 ID */
  pluginId: string;
  /** 沙箱名称 */
  name: string;
  /** 内存限制（MB） */
  memoryLimit: number;
  /** 执行超时（毫秒） */
  timeout: number;
  /** 允许的能力集 */
  capabilities: SandboxCapabilities;
  /** 允许的宿主函数 */
  allowedHostFunctions: string[];
  /** 禁止的宿主函数 */
  deniedHostFunctions: string[];
  /** 环境变量 */
  env: Record<string, string>;
  /** 初始状态 */
  initialState?: Record<string, unknown>;
}

/** 宿主函数签名 */
export type HostFunction = (args: unknown[], sandbox: SandboxContext) => Promise<unknown>;

/** 沙箱上下文 */
export interface SandboxContext {
  /** 插件 ID */
  pluginId: string;
  /** 沙箱 ID */
  sandboxId: string;
  /** 调用宿主函数 */
  invoke: (method: string, args: unknown[]) => Promise<unknown>;
  /** 发布事件 */
  emit: (event: string, data: unknown) => void;
  /** 订阅事件 */
  on: (event: string, handler: (data: unknown) => void) => () => void;
  /** 获取共享状态 */
  getState: <T = unknown>(key: string) => T | undefined;
  /** 设置共享状态 */
  setState: (key: string, value: unknown) => void;
  /** 日志 */
  log: (...args: unknown[]) => void;
}

/** Bridge 消息 */
export interface BridgeMessage {
  /** 消息 ID */
  id: string;
  /** 方向: host→sandbox 或 sandbox→host */
  direction: 'host-to-sandbox' | 'sandbox-to-host';
  /** 动作 */
  action: string;
  /** 数据 */
  data: unknown;
}

/** 沙箱执行结果 */
export interface SandboxResult {
  /** 是否成功 */
  ok: boolean;
  /** 结果数据 */
  result?: unknown;
  /** 错误信息 */
  error?: { code: string; message: string; stack?: string };
}

/** 插件生命周期钩子 */
export interface PluginLifecycleHooks {
  /** 沙箱创建 */
  onSandboxCreate?: (ctx: SandboxContext) => Promise<void>;
  /** 插件加载 */
  onLoad?: (ctx: SandboxContext) => Promise<void>;
  /** 插件启用 */
  onEnable?: (ctx: SandboxContext) => Promise<void>;
  /** 插件禁用 */
  onDisable?: (ctx: SandboxContext) => Promise<void>;
  /** 插件卸载 */
  onUnload?: (ctx: SandboxContext) => Promise<void>;
  /** 沙箱销毁 */
  onSandboxDestroy?: (ctx: SandboxContext) => Promise<void>;
}
