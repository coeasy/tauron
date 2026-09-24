/**
 * tauron 信封调用层（设计文档 §2.1/§1.3）
 *
 * 业务代码 → adapter hook → @tauron/core → invoke('plugin_invoke', envelope) → Tauri IPC
 */

import {
  buildRequest,
  buildErrorResponse,
  generateCallId,
  DEFAULT_TIMEOUT_MS,
  PluginErrorCode,
  type PluginInvokeRequest,
  type PluginInvokeResponse,
  type ProgressEvent,
} from '@tauron/types';
import type { TauronBackend, ProgressCallback } from './backend.js';

/** 调用选项 */
export interface InvokeOptions {
  /** 超时毫秒（默认 30000，0=不超时） */
  timeoutMs?: number;
  /** 进度回调 */
  onProgress?: ProgressCallback;
}

/**
 * 调用插件方法
 *
 * @param backend - IPC 后端
 * @param pluginId - 插件 ID
 * @param method - 方法名
 * @param payload - 方法参数
 * @param options - 调用选项
 */
export async function invokePlugin(
  backend: TauronBackend,
  pluginId: string,
  method: string,
  payload?: unknown,
  options: InvokeOptions = {},
): Promise<PluginInvokeResponse> {
  const request: PluginInvokeRequest = buildRequest({
    pluginId,
    method,
    payload,
    timeoutMs: options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
  });

  // 进度回调直接交给后端。
  //
  // 这里**不**自行构造 MessageChannel：若不同时把 `port2` 转移给对端，
  // `port1.onmessage` 永远不会触发，是一条死链路；而真正实现进度的后端
  // （如 Tauri 后端）使用运行时自己的 Channel 并只消费 `onProgress`。
  // 二者同时存在还会导致进度被投递两次。
  try {
    return await backend.invoke(request, undefined, options.onProgress);
  } catch (err) {
    return buildErrorResponse(
      request.callId,
      PluginErrorCode.CHANNEL_BROKEN,
      `IPC channel error: ${err instanceof Error ? err.message : String(err)}`,
      false,
    );
  }
}

/**
 * 取消进行中的调用
 */
export async function cancelPlugin(backend: TauronBackend, callId: string): Promise<void> {
  return backend.cancel(callId);
}

/**
 * 订阅事件
 */
export async function listenEvent(
  backend: TauronBackend,
  topic: string,
  handler: (payload: unknown) => void,
): Promise<() => void> {
  return backend.listen(topic, handler);
}

/**
 * 发布事件
 */
export async function emitEvent(
  backend: TauronBackend,
  topic: string,
  payload: unknown,
): Promise<void> {
  return backend.emit(topic, payload);
}
