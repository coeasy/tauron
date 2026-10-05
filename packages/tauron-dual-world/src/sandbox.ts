/**
 * Sandbox — 沙箱运行时（原型）
 *
 * 管理 QuickJS-WASM 沙箱实例，提供：
 * - 内存限制
 * - 执行超时
 * - 能力控制
 * - 宿主函数调用
 * - 事件通信
 *
 * ⚠️ **destroy 是一次性终态（V7-P1-04）**：`destroy()` 之后 `execute()` /
 * `call()` 返回 `code: 'SANDBOX_DESTROYED'` 的失败结果，`setState` / `getState` /
 * `emit` / `on` / 上下文 `invoke` / `registerHostFunction()` 抛同一个码——
 * 已销毁的沙箱不执行任何宿主函数、不写状态、不派发事件、不再登记处理器。
 * （此前 destroy 只清表不设旗，之后所有入口照常工作。）
 *
 * `execute()` 未接运行时时的 fail-closed 行为（`SANDBOX_UNAVAILABLE`）保持不变。
 */

import type {
  HostFunction,
  SandboxConfig,
  SandboxContext,
  SandboxErrorCode,
  SandboxResult,
} from './types.js';

/** 沙箱实例接口 */
export interface SandboxInstance {
  /** 沙箱 ID（单调计数 + UUID，仅用于诊断关联，**不是**安全身份） */
  id: string;
  /** 插件 ID */
  pluginId: string;
  /** 配置 */
  config: SandboxConfig;
  /** 内部宿主函数映射 */
  hostFunctions: Map<string, HostFunction>;
  /**
   * 是否已销毁（一次性终态，{@link SandboxDestroyedError}）。
   *
   * 必须是可读的：调用方要能分辨「这个沙箱还能不能用」，而不是靠试调用猜。
   */
  readonly destroyed: boolean;
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
  /** 销毁沙箱（幂等） */
  destroy: () => Promise<void>;
}

/** 事件处理器映射 */
type EventHandlers = Map<string, Set<(data: unknown) => void>>;

/**
 * destroy 之后的统一拒绝。
 *
 * 只有一个码：`SANDBOX_DESTROYED`（码表见 `types.ts` 的 {@link SANDBOX_ERROR_CODES}）。
 * ⚠️ 包内码，不是 wire 码：不进 `contracts/error/error-codes.json`。
 */
export class SandboxDestroyedError extends Error {
  readonly code: SandboxErrorCode = 'SANDBOX_DESTROYED';

  constructor(sandboxId: string, pluginId: string) {
    super(`沙箱 '${sandboxId}'（插件 '${pluginId}'）已销毁，拒绝任何调用`);
    this.name = 'SandboxDestroyedError';
  }
}

/** 用本包码表构造失败结果（码写错会在类型层就红，不会漂成另一个词表）。 */
function sandboxFailure(code: SandboxErrorCode, message: string): SandboxResult {
  return { ok: false, error: { code, message } };
}

/**
 * 模块级单调计数：即使 `crypto.randomUUID` 不可用，同模块内创建的沙箱 ID
 * 也两两不同。
 */
let idSequence = 0;

/**
 * 生成沙箱 ID：单调计数 + `crypto.randomUUID()`。
 *
 * ⚠️ 唯一性的用途是**诊断关联**（日志与事件归属），不是安全边界——本包的
 * 进程内沙箱不提供隔离性。没有 `crypto.randomUUID` 的环境退化为
 * `sbx-<计数>-noncrypto`：同模块内仍唯一，但**可预测**，因此这条 ID 绝不
 * 得被当成安全身份。
 * ⚠️ 不用 `Math.random()`（V7-P1-04）：既可预测又不保证唯一。
 */
function generateId(): string {
  const sequence = (idSequence += 1);
  const uuid =
    typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
      ? crypto.randomUUID()
      : 'noncrypto';
  return `sbx-${sequence}-${uuid}`;
}

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
  /** 一次性终态旗（V7-P1-04）：置位后所有入口按同一码拒绝。 */
  let destroyed = false;

  const destroyedFailure = (): SandboxResult => {
    const err = new SandboxDestroyedError(id, config.pluginId);
    return sandboxFailure(err.code, err.message);
  };

  const assertAlive = (): void => {
    if (destroyed) {
      throw new SandboxDestroyedError(id, config.pluginId);
    }
  };

  const sandboxContext: SandboxContext = {
    pluginId: config.pluginId,
    sandboxId: id,
    invoke: async (method, args) => {
      // 终态检查在**任何宿主函数被调用之前**：destroy 之后连函数表都不再读。
      assertAlive();
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
      // destroy 若发生在宿主函数 await 期间，结果同样作废（不对外可见）。
      const result = await fn(args, sandboxContext);
      assertAlive();
      return result;
    },
    emit: (event, data) => {
      assertAlive();
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
      assertAlive();
      let handlers = eventHandlers.get(event);
      if (!handlers) {
        handlers = new Set();
        eventHandlers.set(event, handlers);
      }
      handlers.add(handler);
      return () => {
        // 退订在 destroy 之后仍要安全：清表不能让已登记的 handler 泄漏出来，
        // 但也不该抛错——调用方只是在收尾。
        handlers?.delete(handler);
        if (handlers && handlers.size === 0) {
          eventHandlers.delete(event);
        }
      };
    },
    getState: <T = unknown>(key: string) => {
      assertAlive();
      return state.get(key) as T | undefined;
    },
    setState: (key, value) => {
      assertAlive();
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

    get destroyed(): boolean {
      return destroyed;
    },

    async execute(code: string, _timeout = config.timeout): Promise<SandboxResult> {
      if (destroyed) return destroyedFailure();

      // Check memory limit (simplified - real implementation would track WASM memory)
      if (config.memoryLimit <= 0) {
        return sandboxFailure('MEM_LIMIT', 'Memory limit exceeded');
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
      // 终态优先：destroy 之后的调用不是「宿主函数报错」，而是「沙箱已销毁」。
      if (destroyed) return destroyedFailure();

      try {
        const result = await sandboxContext.invoke(method, args);
        if (destroyed) return destroyedFailure();
        return { ok: true, result };
      } catch (err) {
        if (err instanceof SandboxDestroyedError) return destroyedFailure();
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
      // 幂等：重复 destroy 不再清第二次，也不抛错。
      if (destroyed) return;
      destroyed = true;
      // 本包没有任何在途调用可登记（此前的 `pendingCalls` 从无写入方，是孤儿
      // 清理结构，V7-P1-04 已删除）。接入真实异步 transport 时，由那时的请求
      // 登记并在这里 reject + 清定时器。
      eventHandlers.clear();
      state.clear();
      hostFunctions.clear();
    },
  };
}

/**
 * 注册宿主函数
 *
 * 已销毁的沙箱拒绝登记（否则销毁点后仍能长出新的可调用面）。
 */
export function registerHostFunction(
  sandbox: SandboxInstance,
  name: string,
  fn: HostFunction,
): void {
  if (sandbox.destroyed) {
    throw new SandboxDestroyedError(sandbox.id, sandbox.pluginId);
  }
  sandbox.hostFunctions.set(name, fn);
}
