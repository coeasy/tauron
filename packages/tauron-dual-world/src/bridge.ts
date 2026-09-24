/**
 * Bridge — 宿主↔沙箱通信桥
 *
 * 管理两个世界之间的消息传递，包括：
 * - 请求-响应（invoke）
 * - 事件广播（emit/on）
 * - 状态同步（getState/setState，按 key 存储于桥实例内）
 *
 * 队列采用**有界 + 丢弃最旧**的背压策略，避免长时间运行下无界增长。
 */

import type { BridgeMessage, HostFunction, SandboxContext } from './types.js';

/** Bridge 配置 */
export interface BridgeConfig {
  /**
   * 允许的宿主函数（白名单）。空数组 = 不限制。
   *
   * 非空时，注册与调用都必须命中白名单。
   */
  allowedHostFunctions: string[];
  /** 禁止的宿主函数（黑名单，优先级高于白名单） */
  deniedHostFunctions: string[];
  /** 最大消息大小（字节） */
  maxMessageSize: number;
  /** 待发送队列上限（超出时丢弃最旧消息） */
  maxQueueSize: number;
  /** 超时（毫秒） */
  timeout: number;
}

/** 默认 Bridge 配置 */
export const DEFAULT_BRIDGE_CONFIG: BridgeConfig = {
  allowedHostFunctions: [],
  deniedHostFunctions: [],
  maxMessageSize: 65536,
  maxQueueSize: 1000,
  timeout: 30000,
};

/** Bridge 状态 */
interface BridgeState {
  messageQueue: BridgeMessage[];
  droppedMessages: number;
  eventHandlers: Map<string, Set<(data: unknown) => void>>;
  hostFunctions: Map<string, HostFunction>;
  /** 沙箱侧键值状态（getState/setState 的后端） */
  sandboxState: Map<string, unknown>;
}

/**
 * 创建 Bridge 实例
 *
 * @param config - Bridge 配置
 */
export function createBridge(config: Partial<BridgeConfig> = {}): Bridge {
  const cfg: BridgeConfig = { ...DEFAULT_BRIDGE_CONFIG, ...config };
  const state: BridgeState = {
    messageQueue: [],
    droppedMessages: 0,
    eventHandlers: new Map(),
    hostFunctions: new Map(),
    sandboxState: new Map(),
  };

  /** 宿主函数是否被允许调用（黑名单优先于白名单）。 */
  const isHostFunctionAllowed = (name: string): boolean => {
    if (cfg.deniedHostFunctions.includes(name)) return false;
    if (cfg.allowedHostFunctions.length === 0) return true;
    return cfg.allowedHostFunctions.includes(name);
  };

  return {
    config: cfg,

    /**
     * 注册宿主函数
     */
    registerHostFunction(name: string, fn: HostFunction): void {
      if (!isHostFunctionAllowed(name)) {
        throw new Error(`Cannot register host function not in allowlist: ${name}`);
      }
      state.hostFunctions.set(name, fn);
    },

    /**
     * 注销宿主函数
     */
    unregisterHostFunction(name: string): void {
      state.hostFunctions.delete(name);
    },

    /**
     * 发送消息到沙箱
     *
     * 队列满时丢弃**最旧**消息并递增 `droppedMessages`（有界背压）。
     */
    sendMessage(message: BridgeMessage): void {
      // Check message size
      const size = JSON.stringify(message).length;
      if (size > cfg.maxMessageSize) {
        throw new Error(`Message too large: ${size} > ${cfg.maxMessageSize}`);
      }

      if (message.direction !== 'host-to-sandbox') {
        throw new Error('Bridge.sendMessage only accepts host-to-sandbox messages');
      }

      if (state.messageQueue.length >= cfg.maxQueueSize) {
        state.messageQueue.shift();
        state.droppedMessages += 1;
      }
      state.messageQueue.push(message);
    },

    /**
     * 接收来自沙箱的消息
     */
    receiveMessage(message: BridgeMessage): unknown {
      if (message.direction !== 'sandbox-to-host') {
        throw new Error('Bridge.receiveMessage only accepts sandbox-to-host messages');
      }

      // Handle request-response pattern
      if (message.action === 'invoke' && typeof message.data === 'object' && message.data !== null) {
        const data = message.data as { requestId: string; method: string; args: unknown[] };
        const fn = state.hostFunctions.get(data.method);

        if (!fn) {
          return { ok: false, error: { code: 'NOT_FOUND', message: `Host function '${data.method}' not found` } };
        }
        // 白名单/黑名单已在 registerHostFunction 处强制；
        // 未注册的函数不可能进入 hostFunctions，因此这里无需重复校验。

        // Execute async and resolve the pending request
        const ctx: SandboxContext = {
          pluginId: 'unknown',
          sandboxId: 'unknown',
          invoke: async (method, args) => {
            const fn2 = state.hostFunctions.get(method);
            if (!fn2) throw new Error(`Host function '${method}' not found`);
            return fn2(args, ctx);
          },
          emit: (event, data) => {
            const handlers = state.eventHandlers.get(event);
            if (handlers) {
              for (const h of handlers) h(data);
            }
          },
          on: (event, handler) => {
            let handlers = state.eventHandlers.get(event);
            if (!handlers) {
              handlers = new Set();
              state.eventHandlers.set(event, handlers);
            }
            handlers.add(handler);
            return () => {
              handlers!.delete(handler);
              if (handlers!.size === 0) state.eventHandlers.delete(event);
            };
          },
          getState: <T = unknown>(key: string) => state.sandboxState.get(key) as T | undefined,
          setState: (key: string, value: unknown) => {
            state.sandboxState.set(key, value);
          },
          log: (...args) => console.log('[bridge]', ...args),
        };

        return fn(data.args, ctx)
          .then((result) => ({ ok: true, result }))
          .catch((err) => ({
            ok: false,
            error: {
              code: 'EXECUTION_ERROR',
              message: err instanceof Error ? err.message : String(err),
            },
          }));
      }

      return message.data;
    },

    /**
     * 订阅事件
     */
    on(event: string, handler: (data: unknown) => void): () => void {
      let handlers = state.eventHandlers.get(event);
      if (!handlers) {
        handlers = new Set();
        state.eventHandlers.set(event, handlers);
      }
      handlers.add(handler);
      return () => {
        handlers!.delete(handler);
        if (handlers!.size === 0) state.eventHandlers.delete(event);
      };
    },

    /**
     * 发布事件
     */
    emit(event: string, data: unknown): void {
      const handlers = state.eventHandlers.get(event);
      if (handlers) {
        for (const handler of handlers) {
          try {
            handler(data);
          } catch (err) {
            console.error(`[bridge] Event handler error:`, err);
          }
        }
      }
    },

    /**
     * 获取待处理消息数
     */
    getQueueSize(): number {
      return state.messageQueue.length;
    },

    /**
     * 取出并清空待发送队列。
     *
     * 这是队列的**唯一消费入口**；调用方（沙箱泵）应在每次 tick 调用它，
     * 否则队列只会在达到上限后开始丢弃最旧消息。
     */
    drainQueue(): BridgeMessage[] {
      const drained = state.messageQueue.slice();
      state.messageQueue.length = 0;
      return drained;
    },

    /**
     * 因背压被丢弃的消息总数
     */
    getDroppedCount(): number {
      return state.droppedMessages;
    },

    /**
     * 清空消息队列
     */
    clearQueue(): void {
      state.messageQueue.length = 0;
    },

    /**
     * 销毁 Bridge
     */
    destroy(): void {
      state.eventHandlers.clear();
      state.hostFunctions.clear();
      state.sandboxState.clear();
      state.messageQueue.length = 0;
    },
  };
}

/** Bridge 接口 */
export interface Bridge {
  config: BridgeConfig;
  registerHostFunction(name: string, fn: HostFunction): void;
  unregisterHostFunction(name: string): void;
  sendMessage(message: BridgeMessage): void;
  receiveMessage(message: BridgeMessage): unknown | Promise<unknown>;
  on(event: string, handler: (data: unknown) => void): () => void;
  emit(event: string, data: unknown): void;
  getQueueSize(): number;
  drainQueue(): BridgeMessage[];
  getDroppedCount(): number;
  clearQueue(): void;
  destroy(): void;
}
