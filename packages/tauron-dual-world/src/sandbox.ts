/**
 * Sandbox — 沙箱运行时（原型）
 *
 * 管理 QuickJS-WASM 沙箱实例，提供：
 * - 内存限制
 * - 执行超时
 * - 能力控制
 * - 宿主函数调用
 * - 事件通信
 */

import type {
  SandboxCapabilities,
  SandboxConfig,
  SandboxContext,
  SandboxResult,
  HostFunction,
  DEFAULT_SANDBOX_CAPABILITIES,
} from './types.js';

/** 沙箱实例接口 */
export interface SandboxInstance {
  /** 沙箱 ID */
  id: string;
  /** 插件 ID */
  pluginId: string;
  /** 配置 */
  config: SandboxConfig;
  /** 内部宿主函数映射 */
  hostFunctions: Map<string, HostFunction>;
  /** 执行代码 */
  execute: (code: string, timeout?: number) => Promise<SandboxResult>;
  /** 调用方法 */
  call: (method: string, args: unknown[]) => Promise<SandboxResult>;
  /** 发布事件 */
  emit: (event: string, data: unknown) => void;
  /** 订阅事件 */
  on: (event: string, handler: (data: unknown) => void) => () => void;
  /** 设置共享状态 */
  setState: (key: string, value: unknown) => void;
  /** 获取共享状态 */
  getState: <T = unknown>(key: string) => T | undefined;
  /** 销毁沙箱 */
  destroy: () => Promise<void>;
}

/** 事件处理器映射 */
type EventHandlers = Map<string, Set<(data: unknown) => void>>;

/**
 * 创建沙箱实例
 *
 * @param config - 沙箱配置
 */
export function createSandbox(config: SandboxConfig): SandboxInstance {
  const id = generateId();
  const state = new Map<string, unknown>();
  const eventHandlers: EventHandlers = new Map();
  const hostFunctions = new Map<string, HostFunction>();
  const pendingCalls = new Map<string, { resolve: (v: unknown) => void; reject: (e: Error) => void; timeout: number }>();

  const sandboxContext: SandboxContext = {
    pluginId: config.pluginId,
    sandboxId: id,
    invoke: async (method, args) => {
      const fn = hostFunctions.get(method);
      if (!fn) {
        throw new Error(`Host function '${method}' not found or not allowed`);
      }
      // Check denied functions
      if (config.deniedHostFunctions.includes(method)) {
        throw new Error(`Host function '${method}' is denied`);
      }
      // Check allowed functions
      if (config.allowedHostFunctions.length > 0 && !config.allowedHostFunctions.includes(method)) {
        throw new Error(`Host function '${method}' is not in allowed list`);
      }
      return fn(args, sandboxContext);
    },
    emit: (event, data) => {
      const handlers = eventHandlers.get(event);
      if (handlers) {
        for (const handler of handlers) {
          try {
            handler(data);
          } catch (err) {
            console.error(`[sandbox:${id}] Event handler error:`, err);
          }
        }
      }
    },
    on: (event, handler) => {
      let handlers = eventHandlers.get(event);
      if (!handlers) {
        handlers = new Set();
        eventHandlers.set(event, handlers);
      }
      handlers.add(handler);
      return () => {
        handlers!.delete(handler);
        if (handlers!.size === 0) {
          eventHandlers.delete(event);
        }
      };
    },
    getState: <T = unknown>(key: string) => state.get(key) as T | undefined,
    setState: (key, value) => {
      state.set(key, value);
    },
    log: (...args) => {
      console.log(`[sandbox:${id}]`, ...args);
    },
  };

  return {
    id,
    pluginId: config.pluginId,
    config,
    hostFunctions,

    async execute(code: string, _timeout = config.timeout): Promise<SandboxResult> {
      // Check memory limit (simplified - real implementation would track WASM memory)
      if (config.memoryLimit <= 0) {
        return { ok: false, error: { code: 'MEM_LIMIT', message: 'Memory limit exceeded' } };
      }

      // ⚠️ **fail closed（轮 11 审计修正）**：此前这里 `setTimeout` 随机延迟后返回
      // `{ ok: true, result: { executed: true } }`——**一行代码都没执行**，却对外声称
      // 执行成功。这比"未实现"更坏：调用方会据此认为插件代码已跑过并通过。
      //
      // 真实现需要 QuickJS-WASM 之类的运行时，而依赖闭包里没有、硬约束禁止新增依赖，
      // 因此现在的正确行为是**拒绝执行**：拿到 `ok: false` 的调用方不会误判。
      return {
        ok: false,
        error: {
          code: 'SANDBOX_UNAVAILABLE',
          message:
            'sandbox 未接入任何 JS 运行时（本包不执行代码，也不伪报执行成功）；' +
            `拒绝执行 ${code.length} 字符的代码。接入 QuickJS-WASM 后此路径应改为真实执行。`,
        },
      };
    },

    async call(method: string, args: unknown[]): Promise<SandboxResult> {
      try {
        const result = await sandboxContext.invoke(method, args);
        return { ok: true, result };
      } catch (err) {
        return {
          ok: false,
          error: {
            code: 'CALL_ERROR',
            message: err instanceof Error ? err.message : String(err),
          },
        };
      }
    },

    emit: sandboxContext.emit,
    on: sandboxContext.on,
    setState: sandboxContext.setState,
    getState: sandboxContext.getState,

    async destroy(): Promise<void> {
      // Clean up pending calls
      for (const [id, pending] of pendingCalls) {
        clearTimeout(pending.timeout);
        pending.reject(new Error('Sandbox destroyed'));
      }
      pendingCalls.clear();
      eventHandlers.clear();
      state.clear();
    },
  };
}

/**
 * 注册宿主函数
 */
export function registerHostFunction(
  sandbox: SandboxInstance,
  name: string,
  fn: HostFunction,
): void {
  sandbox.hostFunctions.set(name, fn);
}

/**
 * 生成唯一 ID
 */
function generateId(): string {
  return Math.random().toString(36).substring(2, 11);
}
