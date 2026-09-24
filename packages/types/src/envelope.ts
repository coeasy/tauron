/**
 * tauron 信封协议（设计文档 §2.1）
 *
 * plugin_invoke 单命令信封：前端通过 Tauri IPC 调用 `plugin_invoke`，
 * Rust 壳层根据信封内容分发到对应插件。
 */

import type { PluginErrorCode } from './errors.js';

/** 调用 ID 生成器（UUID v4） */
export function generateCallId(): string {
  if (typeof crypto !== 'undefined' && crypto.randomUUID) {
    return crypto.randomUUID();
  }
  // Fallback for non-browser environments
  return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (c) => {
    const r = (Math.random() * 16) | 0;
    const v = c === 'x' ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

/** 请求信封（前端 → Rust） */
export interface PluginInvokeRequest {
  /** 插件 ID（反域名：com.example.formatter） */
  pluginId: string;
  /** 方法名（如 "format", "load", "save"） */
  method: string;
  /** 方法参数（JSON 序列化） */
  payload?: unknown;
  /** 调用 ID（UUID，用于关联请求-响应/取消） */
  callId: string;
  /** 超时毫秒（默认 30000，0=不超时） */
  timeoutMs?: number;
}

/** 响应信封（Rust → 前端） */
export interface PluginInvokeResponse {
  /** 与请求匹配 */
  callId: string;
  /** 成功/失败 */
  ok: boolean;
  /** 成功时的结果 */
  result?: unknown;
  /** 失败时的错误 */
  error?: {
    code: PluginErrorCode;
    message: string;
    retryable: boolean;
  };
}

/** 流式进度事件（通过 Tauri Channel） */
export interface ProgressEvent {
  callId: string;
  step: number;
  total: number;
  message: string;
  percentage: number;
}

/** 取消请求 */
export interface PluginCancelRequest {
  callId: string;
}

/** 默认超时毫秒 */
export const DEFAULT_TIMEOUT_MS = 30_000;

/**
 * 构建请求信封（便利工厂）
 */
export function buildRequest(params: {
  pluginId: string;
  method: string;
  payload?: unknown;
  timeoutMs?: number;
}): PluginInvokeRequest {
  return {
    pluginId: params.pluginId,
    method: params.method,
    callId: generateCallId(),
    ...(params.payload !== undefined ? { payload: params.payload } : {}),
    ...(params.timeoutMs !== undefined ? { timeoutMs: params.timeoutMs } : {}),
  };
}

/**
 * 构建成功响应
 */
export function buildOkResponse(callId: string, result: unknown): PluginInvokeResponse {
  return { callId, ok: true, result };
}

/**
 * 构建错误响应
 */
export function buildErrorResponse(
  callId: string,
  code: PluginErrorCode,
  message: string,
  retryable: boolean = false,
): PluginInvokeResponse {
  return {
    callId,
    ok: false,
    error: { code, message, retryable },
  };
}

/**
 * 验证请求信封的完整性
 */
export function validateRequest(req: PluginInvokeRequest): string | null {
  if (!req.pluginId || typeof req.pluginId !== 'string') return 'pluginId is required';
  if (!req.method || typeof req.method !== 'string') return 'method is required';
  if (!req.callId || typeof req.callId !== 'string') return 'callId is required';
  if (req.timeoutMs !== undefined && (typeof req.timeoutMs !== 'number' || req.timeoutMs < 0)) {
    return 'timeoutMs must be a non-negative number';
  }
  return null;
}
