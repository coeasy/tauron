/**
 * @tauron/core/testing — 测试工具
 *
 * 提供 MockBackend 用于测试信封调用、事件订阅等。
 */

import type {
  PluginInvokeRequest,
  PluginInvokeResponse,
  ProgressEvent,
} from '@tauron/types';
import {
  buildOkResponse,
  buildErrorResponse,
  PluginErrorCode,
} from '@tauron/types';
import type { TauronBackend, ProgressCallback } from './backend.js';

/** 调用记录 */
export interface InvocationRecord {
  request: PluginInvokeRequest;
  response: PluginInvokeResponse;
}

/** MockBackend 配置 */
export interface MockBackendConfig {
  /** 调用处理器（可覆盖默认行为） */
  onInvoke?: (request: PluginInvokeRequest) => Promise<PluginInvokeResponse>;
  /** 事件处理器 */
  onEmit?: (topic: string, payload: unknown) => void;
}

/**
 * MockBackend — 用于测试的 IPC 后端模拟
 */
export class MockBackend implements TauronBackend {
  invocations: InvocationRecord[] = [];
  private listeners: Map<string, Array<(payload: unknown) => void>> = new Map();
  private config: MockBackendConfig;

  constructor(config: MockBackendConfig = {}) {
    this.config = config;
  }

  async invoke(
    request: PluginInvokeRequest,
    _channel?: MessageChannel,
    _onProgress?: ProgressCallback,
  ): Promise<PluginInvokeResponse> {
    let response: PluginInvokeResponse;

    if (this.config.onInvoke) {
      response = await this.config.onInvoke(request);
    } else {
      // Default: return success
      response = buildOkResponse(request.callId, {
        pluginId: request.pluginId,
        method: request.method,
        payload: request.payload,
      });
    }

    this.invocations.push({ request, response });
    return response;
  }

  async cancel(_callId: string): Promise<void> {
    // Mock: no-op
  }

  async listen(topic: string, handler: (payload: unknown) => void): Promise<() => void> {
    let listeners = this.listeners.get(topic);
    if (!listeners) {
      listeners = [];
      this.listeners.set(topic, listeners);
    }
    listeners.push(handler);

    return () => {
      const ls = this.listeners.get(topic);
      if (ls) {
        const idx = ls.indexOf(handler);
        if (idx !== -1) ls.splice(idx, 1);
        if (ls.length === 0) this.listeners.delete(topic);
      }
    };
  }

  async emit(topic: string, payload: unknown): Promise<void> {
    if (this.config.onEmit) {
      this.config.onEmit(topic, payload);
    }
    const listeners = this.listeners.get(topic);
    if (listeners) {
      for (const listener of [...listeners]) {
        listener(payload);
      }
    }
  }

  /**
   * 重置所有记录
   */
  reset(): void {
    this.invocations = [];
    this.listeners.clear();
  }
}

/**
 * 创建一个简单的测试夹具
 */
export function createTestBackend(handler?: (req: PluginInvokeRequest) => PluginInvokeResponse): MockBackend {
  return new MockBackend({
    onInvoke: async (request) => {
      if (handler) {
        return handler(request);
      }
      return buildOkResponse(request.callId, { ok: true });
    },
  });
}
