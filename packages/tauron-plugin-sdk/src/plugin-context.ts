/**
 * tauron PluginContext（设计文档 §2.4/§4.2B）—— **legacy / 过渡实现**。
 *
 * 插件侧（iframe 内）：通过 postMessage 与宿主通信。
 * 插件调用 createPluginBridge() 获取上下文，
 * 通过 ctx.invoke() 调用宿主能力，ctx.emit() 发布事件。
 *
 * ⚠️ 方案 R2：新的插件代码应当使用**共享契约形状**（`@tauron/plugin-context-contract`
 * 的 `PluginContext`）。本文件的形状是 iframe 一代的传输/握手细节，属过渡 API，
 * 不再承接新能力；需要在本 SDK 下跑契约形状时用 `createContractContext`
 * （见 `./contract-context.ts`）适配。
 */

import {
  DEFAULT_TIMEOUT_MS,
  buildReadyMessage,
  buildInvokeMessage,
  buildEmitMessage,
  isHostToPlugin,
  type BridgeToPluginMessage,
} from '@tauron/types';

/** 待处理调用 */
interface PendingCall {
  resolve: (result: unknown) => void;
  reject: (error: Error) => void;
  /** 超时定时器；`0` 表示不超时 */
  timer: ReturnType<typeof setTimeout> | undefined;
}

/** 事件处理器 */
type EventHandler = (payload: unknown) => void;

/** 插件上下文选项 */
export interface PluginContextOptions {
  /**
   * `invoke()` 的默认超时（毫秒，0 = 不超时）。
   *
   * 必须存在：宿主无响应时若永不超时，`pendingCalls` 会持续泄漏且调用方永久挂起。
   */
  timeoutMs?: number;
  /**
   * 覆盖握手 token。
   *
   * 缺省时从当前 URL hash 的 `tauron-token=` 片段解析（宿主
   * `PluginBridge.createIframe` 注入的标准通道）。仅测试或非标准宿主需要显式传入。
   */
  handshakeToken?: string;
}

/** 握手 token 的 URL 片段键（与 bridge.ts 的 `PLUGIN_TOKEN_FRAGMENT_KEY` 一致）。 */
export const PLUGIN_TOKEN_FRAGMENT_KEY = 'tauron-token';

/**
 * 从 URL 解析宿主注入的握手 token。
 *
 * 形如 `.../plugin.html#tauron-token=<uuid>`。解析不到返回 `''`
 * （此时握手会失败并在宿主侧留下 "Token mismatch" 日志——宁可显式失败，
 * 不静默用随机值假成功）。
 */
export function readHandshakeTokenFromUrl(url?: string): string {
  const href =
    url ??
    (typeof window !== 'undefined' && window.location ? window.location.href : '');
  if (!href) return '';
  try {
    const hash = new URL(href).hash; // "#tauron-token=..."
    const raw = hash.startsWith('#') ? hash.slice(1) : hash;
    for (const part of raw.split('&')) {
      const eq = part.indexOf('=');
      if (eq < 0) continue;
      if (decodeURIComponent(part.slice(0, eq)) === PLUGIN_TOKEN_FRAGMENT_KEY) {
        return decodeURIComponent(part.slice(eq + 1));
      }
    }
  } catch {
    // 非法 URL（非浏览器环境）：按无 token 处理。
  }
  return '';
}

/**
 * 插件上下文接口（**legacy / 过渡形状**）。
 *
 * ⚠️ **新代码不要用这个形状。** 方案 R2 之后，插件上下文的事实源是
 * `@tauron/plugin-context-contract` 的 `PluginContext`
 * （`pluginId` / `host` / `commands` / `events` / `settings` / `log`）。
 *
 * 本接口是 **iframe bridge 一代** 的插件侧形状，与契约形状**没有一条成员同名
 * 同义**（`ready` / `permissions` / `invoke` / `emit` / `onEvent` / `onInit` /
 * `destroy` 都是 postMessage 传输与握手细节，不是所有插件都必须接受的契约）。
 * 它之所以保留，是因为 `registerPlugin` 与既有测试建立在它之上——属**过渡**
 * API，不再承接新能力。
 *
 * 迁移路径：用 {@link createContractContext} 把本形状适配成契约形状，
 * 插件定义即可与 `@tauron/app-plugin-sdk` 共用（互操作测试见
 * `@tauron/app-plugin-sdk/src/interop.test.ts`）。
 */
export interface PluginContext {
  /** 插件已就绪（init 消息已收到） */
  ready: boolean;
  /** 已授予的权限 */
  permissions: string[];
  /** 调用宿主能力 */
  invoke(method: string, args: unknown): Promise<unknown>;
  /** 发布事件 */
  emit(eventName: string, payload: unknown): void;
  /** 订阅宿主事件 */
  onEvent(eventName: string, handler: EventHandler): () => void;
  /** 订阅权限变更 */
  onInit(callback: (permissions: string[]) => void): void;
  /** 关闭连接 */
  destroy(): void;
}

/**
 * 创建插件上下文（iframe 内使用）
 *
 * 握手流程：
 * 1. 插件加载后发送 { action: 'ready', token }
 * 2. 宿主回复 { action: 'init', permissions }
 * 3. 正式通信开始
 */
export function createPluginContext(options: PluginContextOptions = {}): PluginContext {
  const defaultTimeout = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const pendingCalls: Map<string, PendingCall> = new Map();
  const eventHandlers: Map<string, Set<EventHandler>> = new Map();
  const initCallbacks: Set<(permissions: string[]) => void> = new Set();

  let ready = false;
  let permissions: string[] = [];

  /** 取出一条待处理调用并清理其定时器。 */
  const takePending = (callId: string): PendingCall | undefined => {
    const pending = pendingCalls.get(callId);
    if (!pending) return undefined;
    pendingCalls.delete(callId);
    if (pending.timer !== undefined) clearTimeout(pending.timer);
    return pending;
  };

  // 消息处理器
  const handleMessage = (event: MessageEvent): void => {
    const msg = event.data as BridgeToPluginMessage;

    if (!isHostToPlugin(msg)) return;

    if (msg.action === 'init') {
      // 握手完成
      ready = true;
      permissions = (msg.payload as { permissions: string[] }).permissions;
      initCallbacks.forEach((cb) => cb(permissions));
    } else if (msg.action === 'invoke-result') {
      // 调用结果
      const payload = msg.payload as { callId: string; result: unknown };
      const pending = takePending(payload.callId);
      if (pending) {
        const result = payload.result as { ok: boolean; result?: unknown; error?: { code: string; message: string } };
        if (result.ok) {
          pending.resolve(result.result);
        } else {
          pending.reject(new Error(result.error?.message ?? 'Unknown error'));
        }
      }
    } else if (msg.action === 'event') {
      // 宿主推送事件 → 分发给 onEvent 订阅者
      const payload = msg.payload as { eventName: string; payload: unknown };
      const handlers = eventHandlers.get(payload.eventName);
      if (handlers) {
        for (const handler of handlers) {
          try {
            handler(payload.payload);
          } catch (err) {
            console.error('Plugin event handler error:', err);
          }
        }
      }
    } else if (msg.action === 'cancel') {
      // 取消调用
      const payload = msg.payload as { callId: string };
      const pending = takePending(payload.callId);
      if (pending) {
        pending.reject(new Error('Cancelled'));
      }
    }
  };

  window.addEventListener('message', handleMessage);

  // 发送 ready 消息（握手）：token 由宿主经 iframe URL hash 注入
  //（PluginBridge.createIframe 自动完成；解析不到则握手显式失败，见上）。
  const token = options.handshakeToken ?? readHandshakeTokenFromUrl();
  window.parent.postMessage(buildReadyMessage(token), '*');

  const context: PluginContext = {
    get ready() {
      return ready;
    },
    get permissions() {
      return permissions;
    },

    async invoke(method: string, args: unknown): Promise<unknown> {
      const callId = crypto.randomUUID();
      const msg = buildInvokeMessage(callId, method, args);

      return new Promise((resolve, reject) => {
        const timer =
          defaultTimeout > 0
            ? setTimeout(() => {
                // 超时后必须移除，否则 pendingCalls 无界增长
                pendingCalls.delete(callId);
                reject(new Error(`Invoke timed out after ${defaultTimeout}ms: ${method}`));
              }, defaultTimeout)
            : undefined;

        pendingCalls.set(callId, { resolve, reject, timer });
        window.parent.postMessage(msg, '*');
      });
    },

    emit(eventName: string, payload: unknown): void {
      const msg = buildEmitMessage(eventName, payload);
      window.parent.postMessage(msg, '*');
    },

    onEvent(eventName: string, handler: EventHandler): () => void {
      let handlers = eventHandlers.get(eventName);
      if (!handlers) {
        handlers = new Set();
        eventHandlers.set(eventName, handlers);
      }
      handlers.add(handler);

      return () => {
        handlers.delete(handler);
        if (handlers.size === 0) {
          eventHandlers.delete(eventName);
        }
      };
    },

    onInit(callback: (permissions: string[]) => void): void {
      initCallbacks.add(callback);
    },

    destroy(): void {
      window.removeEventListener('message', handleMessage);
      for (const [, pending] of pendingCalls) {
        if (pending.timer !== undefined) clearTimeout(pending.timer);
        pending.reject(new Error('Context destroyed'));
      }
      pendingCalls.clear();
      eventHandlers.clear();
      initCallbacks.clear();
    },
  };

  return context;
}
