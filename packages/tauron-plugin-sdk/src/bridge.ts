/**
 * tauron PluginBridge（设计文档 §2.4/§4.2B）
 *
 * 宿主侧（PluginBridge）：管理 iframe 沙箱通信，
 * 通过 postMessage 协议与插件通信。
 *
 * 握手流程：
 * 1. Host 创建 iframe 时注入随机 token
 * 2. 插件加载后发送 { action: 'ready', token } 验证身份
 * 3. Host 回复 { action: 'init', permissions: [...] } 授予能力
 * 4. 正式通信开始，所有消息携带 token
 */

import {
  BRIDGE_MESSAGE_TYPE,
  buildInitMessage,
  buildInvokeResultMessage,
  buildEventMessage,
  buildCancelMessage,
  isPluginToHost,
  isHostToPlugin,
  type BridgeToPluginMessage,
  type PluginToBridgeMessage,
  type BridgeMessage,
} from '@tauron/types';

// 禁用通知的保留事件名（与插件侧 `registerPlugin` 的 `onDisable` 接线共用同一常量，
// 避免两侧各写一份字面量后悄悄漂移）。
import { TAURON_DISABLE_EVENT } from './register-plugin.js';

/** 进行中的调用（用于取消语义）。 */
interface InflightCall {
  /** 已取消：结果到达后丢弃，不再回发。 */
  cancelled: boolean;
}

/** 事件处理器 */
type EventHandler = (payload: unknown) => void;

/** 权限表 */
type PermissionGrants = Map<string, string[]>;

/**
 * 握手 token 的 URL 片段键。
 *
 * 宿主 {@link PluginBridge.createIframe} 把 token 注入 iframe URL hash，
 * 插件侧 `createPluginContext` 从 hash 解析（见 plugin-context.ts 的
 * {@link readHandshakeTokenFromUrl}）。这是 token 跨越 iframe 边界的**唯一**通道。
 */
export const PLUGIN_TOKEN_FRAGMENT_KEY = 'tauron-token';

/** 把 token 以 URL hash 片段附加到 src（丢弃 src 原有 hash）。 */
function withTokenFragment(src: string, token: string): string {
  const base = src.split('#')[0] ?? src;
  return `${base}#${PLUGIN_TOKEN_FRAGMENT_KEY}=${token}`;
}

/**
 * PluginBridge — 宿主侧插件桥
 *
 * 管理 iframe 内的插件通信，包括：
 * - 握手（token 验证）
 * - invoke 调用（转发到 Rust 侧）
 * - event 发布（转发到事件总线）
 * - cancel 取消
 *
 * 安全边界：iframe 以 `sandbox="allow-scripts"` 加载（**不带**
 * `allow-same-origin`，故为跨源），因此无法用 origin 判定来源，
 * 必须用 `event.source` 与自身 iframe 的 `contentWindow` 做身份比对。
 */
export class PluginBridge {
  private iframe: HTMLIFrameElement | null = null;
  private token: string;
  /** 进行中的调用（按 callId）。 */
  private inflightCalls: Map<string, InflightCall> = new Map();
  private eventHandlers: Map<string, Set<EventHandler>> = new Map();
  private permissions: string[];
  /** 方法 → 所需权限；缺省表示该方法无需额外权限。 */
  private methodPermissions: Map<string, string>;
  private invokeHandler: (method: string, args: unknown) => Promise<unknown>;
  private cancelHandler: (callId: string) => Promise<void>;
  private ready: boolean = false;
  private onMessage: (event: MessageEvent) => void;

  constructor(
    permissions: string[],
    invokeHandler: (method: string, args: unknown) => Promise<unknown>,
    cancelHandler: (callId: string) => Promise<void>,
    methodPermissions?: Record<string, string>,
  ) {
    this.token = crypto.randomUUID();
    this.permissions = permissions;
    this.methodPermissions = new Map(Object.entries(methodPermissions ?? {}));
    this.invokeHandler = invokeHandler;
    this.cancelHandler = cancelHandler;
    this.onMessage = this.handleMessage.bind(this);
  }

  /**
   * 握手 token。
   *
   * 走 {@link createIframe} 时无需关心（自动注入 URL hash）；
   * 若自行创建 iframe，必须把该值送进插件页可读取的位置
   * （约定：URL hash `#tauron-token=<value>`）。
   */
  get handshakeToken(): string {
    return this.token;
  }

  /**
   * 创建 iframe 并初始化
   *
   * 握手 token 通过 URL hash（`#tauron-token=<uuid>`）注入插件页——
   * 这是文档承诺的"Host 创建 iframe 时注入随机 token"的实现；
   * 插件侧 `createPluginContext` 从 `location.hash` 读回同一 token。
   */
  createIframe(src: string): HTMLIFrameElement {
    const iframe = document.createElement('iframe');
    iframe.sandbox = 'allow-scripts'; // 关键：不带 allow-same-origin
    iframe.src = withTokenFragment(src, this.token);
    iframe.style.display = 'none';
    document.body.appendChild(iframe);
    this.iframe = iframe;

    window.addEventListener('message', this.onMessage);
    return iframe;
  }

  /**
   * 处理来自 iframe 的消息
   */
  private handleMessage(event: MessageEvent): void {
    // 来源校验：只接受本桥创建的那个 iframe 发来的消息。
    // iframe 为 sandbox="allow-scripts"（跨源），无法用 origin 判定，
    // 因此必须做 event.source 身份比对——否则任意窗口都能伪造信封调用宿主能力。
    if (!this.iframe || event.source !== this.iframe.contentWindow) return;

    const msg = event.data as BridgeMessage;

    // 验证消息类型
    if (!isPluginToHost(msg)) return;

    if (this.ready) {
      // 握手后处理业务消息
      if (msg.action === 'invoke') {
        void this.handleInvoke(msg);
      } else if (msg.action === 'emit') {
        this.handleEmit(msg);
      } else if (msg.action === 'error') {
        this.handleError(msg);
      }
    } else {
      // 握手前：只接受 ready 消息
      if (msg.action === 'ready') {
        this.handleReady(msg);
      }
    }
  }

  /**
   * 处理 ready 消息（插件加载完成）
   */
  private handleReady(msg: PluginToBridgeMessage): void {
    const payload = msg.payload as { token: string };
    if (payload.token !== this.token) {
      console.error('Token mismatch: bridge handshake failed');
      return;
    }

    // 回复 init 消息
    this.sendMessage(buildInitMessage(this.token, this.permissions));
    this.ready = true;
  }

  /**
   * 处理 invoke 消息（插件请求宿主能力）
   */
  private async handleInvoke(msg: PluginToBridgeMessage): Promise<void> {
    const callId = msg.callId!;
    const payload = msg.payload as { method: string; args: unknown };

    // 权限检查（快速失败层；权威判定在 Rust 侧 ACL）
    const requiredPerm = this.getMethodPermission(payload.method);
    if (requiredPerm && !this.permissions.includes(requiredPerm)) {
      this.sendResult(callId, {
        ok: false,
        error: { code: 'SC-1002', message: `Permission denied: ${requiredPerm}`, retryable: false },
      });
      return;
    }

    // 登记在途调用：cancel() 可将其标记为已取消，结果到达后丢弃
    const inflight: InflightCall = { cancelled: false };
    this.inflightCalls.set(callId, inflight);

    try {
      const result = await this.invokeHandler(payload.method, payload.args);
      if (inflight.cancelled) return;
      this.sendResult(callId, { ok: true, result });
    } catch (err) {
      if (inflight.cancelled) return;
      this.sendResult(callId, {
        ok: false,
        error: {
          code: 'SC-9001',
          message: err instanceof Error ? err.message : String(err),
          retryable: false,
        },
      });
    } finally {
      this.inflightCalls.delete(callId);
    }
  }

  /**
   * 处理 emit 消息（插件发布事件）
   */
  private handleEmit(msg: PluginToBridgeMessage): void {
    const payload = msg.payload as { eventName: string; payload: unknown };
    const handlers = this.eventHandlers.get(payload.eventName);
    if (handlers) {
      for (const handler of handlers) {
        try {
          handler(payload.payload);
        } catch (err) {
          console.error('Event handler error:', err);
        }
      }
    }
  }

  /**
   * 处理 error 消息
   */
  private handleError(msg: PluginToBridgeMessage): void {
    console.error('Plugin error:', msg.payload);
  }

  /**
   * 发送结果到 iframe
   */
  private sendResult(callId: string, result: unknown): void {
    this.sendMessage(buildInvokeResultMessage(this.token, callId, result));
  }

  /**
   * 发送消息到 iframe
   */
  private sendMessage(msg: BridgeToPluginMessage): void {
    if (this.iframe?.contentWindow) {
      this.iframe.contentWindow.postMessage(msg, '*');
    }
  }

  /**
   * 获取方法所需的权限（未配置的方法返回 null，表示无需额外权限）。
   *
   * 这是宿主侧快速失败层；权威判定仍在 Rust 侧 ACL。
   */
  private getMethodPermission(method: string): string | null {
    return this.methodPermissions.get(method) ?? null;
  }

  /**
   * 订阅插件事件
   */
  onEvent(eventName: string, handler: EventHandler): () => void {
    let handlers = this.eventHandlers.get(eventName);
    if (!handlers) {
      handlers = new Set();
      this.eventHandlers.set(eventName, handlers);
    }
    handlers.add(handler);

    return () => {
      handlers.delete(handler);
      if (handlers.size === 0) {
        this.eventHandlers.delete(eventName);
      }
    };
  }

  /**
   * 向插件推送事件（Host → 插件）。
   *
   * 与 {@link onEvent}（插件 → Host）方向相反，构成完整事件回路。
   */
  emitToPlugin(eventName: string, payload: unknown): void {
    this.sendMessage(buildEventMessage(this.token, eventName, payload));
  }

  /**
   * 通知插件「你已被禁用 / 卸载」（触发插件的 `onDisable`）。
   *
   * 宿主在 `host_registry_admin` 的 `disable` / `uninstall` 成功之后调它。
   * 此前这条通知**不存在**：`RegisterPluginConfig.onDisable` 声明了却永不触发，
   * 插件的清理逻辑（停定时器、断开连接、释放资源）是死代码。
   */
  notifyDisabled(reason = 'disabled'): void {
    this.emitToPlugin(TAURON_DISABLE_EVENT, { reason });
  }

  /**
   * 取消进行中的调用。
   *
   * 标记在途调用并把取消通知发给插件与后端；已在途的结果到达后被丢弃。
   */
  cancel(callId: string): void {
    const inflight = this.inflightCalls.get(callId);
    if (inflight) {
      inflight.cancelled = true;
    }
    this.sendMessage(buildCancelMessage(this.token, callId));
    void this.cancelHandler(callId).catch(() => undefined);
  }

  /**
   * 销毁桥
   */
  destroy(): void {
    window.removeEventListener('message', this.onMessage);
    if (this.iframe) {
      if (this.iframe.parentNode) {
        this.iframe.parentNode.removeChild(this.iframe);
      }
      this.iframe = null;
    }

    // 丢弃所有在途调用的结果，避免销毁后仍回发消息
    for (const inflight of this.inflightCalls.values()) {
      inflight.cancelled = true;
    }
    this.inflightCalls.clear();
    this.eventHandlers.clear();
    this.ready = false;
  }
}
