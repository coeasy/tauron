/**
 * @tauron/plugin-sdk/testing — 测试工具
 *
 * 提供 MockBridge 和 MockPluginContext 用于测试。
 */

import type { PluginContext } from './plugin-context.js';

/** Mock 插件上下文 */
export class MockPluginContext implements PluginContext {
  ready: boolean = false;
  permissions: string[] = [];
  invoked: Array<{ method: string; args: unknown }> = [];
  emitted: Array<{ eventName: string; payload: unknown }> = [];
  private eventHandlers: Map<string, Array<EventHandler>> = new Map();
  private initCallbacks: Array<(permissions: string[]) => void> = [];

  constructor(permissions: string[] = []) {
    this.permissions = permissions;
    this.ready = true;
  }

  async invoke(method: string, args: unknown): Promise<unknown> {
    this.invoked.push({ method, args });
    return { ok: true, result: { method, args } };
  }

  emit(eventName: string, payload: unknown): void {
    this.emitted.push({ eventName, payload });
    const handlers = this.eventHandlers.get(eventName);
    if (handlers) {
      for (const handler of handlers) {
        handler(payload);
      }
    }
  }

  onEvent(eventName: string, handler: (payload: unknown) => void): () => void {
    let handlers = this.eventHandlers.get(eventName);
    if (!handlers) {
      handlers = [];
      this.eventHandlers.set(eventName, handlers);
    }
    handlers.push(handler);

    return () => {
      const hs = this.eventHandlers.get(eventName);
      if (hs) {
        const idx = hs.indexOf(handler);
        if (idx !== -1) hs.splice(idx, 1);
      }
    };
  }

  onInit(callback: (permissions: string[]) => void): void {
    this.initCallbacks.push(callback);
    if (this.ready) {
      callback(this.permissions);
    }
  }

  destroy(): void {
    this.eventHandlers.clear();
    this.initCallbacks = [];
  }

  /**
   * 模拟收到 invoke-result
   */
  simulateInvokeResult(callId: string, result: unknown): void {
    // No-op in mock: the mock context doesn't use real callId tracking
    void callId;
    void result;
  }
}

/** 测试用事件处理器类型 */
type EventHandler = (payload: unknown) => void;
