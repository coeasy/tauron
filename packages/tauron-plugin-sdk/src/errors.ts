// 跨桥错误的带码传递（轮 35）。
//
// 桥的两侧各丢过一次码：宿主能力 handler 抛出的码被硬换成通用 `SC-9001`，
// 插件侧又只取 `error.message` 造了个裸 `Error`。轮 30 把宿主写侧的四种失败
// 按事实分好码，若读侧到这里塌回一句 "Internal error"，那条工作就白做——
// 插件无法区分「被拒」与「暂时拿不到，重连后可重试」。

import { PluginErrorCode, extractCodeLike, isCodeLike, isRetryable } from '@tauron/types';

/**
 * 跨桥失败的错误形状：`code` 是**线上原样**的码（宿主 `E_*` 或框架层 `SC-####`）。
 *
 * 不在这层做词表归一：形态判据来自 `@tauron/types`，宿主词表的合法集与重试语义
 * 留在 `@tauron/host`（插件 SDK 依赖它会整座宿主客户端进包）。因此
 * {@link PluginBridgeError.retryable} 只对**框架层码**下判断；宿主码的重试处置
 * 由调用方按 `code` 自己决定，`retryable` 在这里保持 `false` 而不是猜。
 */
export class PluginBridgeError extends Error {
  readonly code: string;
  /** `true` = 原始失败没有任何码可用，本错误落到 {@link PluginErrorCode.INTERNAL}。 */
  readonly fallbackApplied: boolean;
  readonly retryable: boolean;

  constructor(code: string, message: string, fallbackApplied = false) {
    super(message);
    this.name = 'PluginBridgeError';
    this.code = code;
    this.fallbackApplied = fallbackApplied;
    this.retryable = isAppRetryable(code);
  }
}

/**
 * 是否是"该重试"的框架层码；宿主 `E_*` 码不在此表内，一律返回 `false`（不猜）。
 */
export function isAppRetryable(code: string): boolean {
  return isCodeLike(code) && !code.startsWith('E_') ? isRetryable(code as PluginErrorCode) : false;
}

/**
 * 从任意被抛出的值里取出线上码：① 错误对象自带的 `code`（形态合法才算，
 * 避免把业务字段 `code: 500` 之类当成码）；② 消息文本里的码形态；③ 都没有则 `null`。
 */
export function codeFromThrown(err: unknown): string | null {
  if (err && typeof err === 'object') {
    const raw = (err as { code?: unknown }).code;
    if (typeof raw === 'string' && isCodeLike(raw)) return raw;
  }
  return extractCodeLike(err instanceof Error ? err.message : String(err));
}
