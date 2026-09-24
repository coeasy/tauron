/**
 * host-invoke — 宿主侧插件方法调用客户端（协议补全）。
 *
 * 与 `registerPlugin`（插件侧）共同构成完整的 `__invoke:`/`__result:` 协议：
 *
 * 1. 宿主 `callPluginMethod(bridge, method, args)`
 *    → `bridge.emitToPlugin('__invoke:<method>', { callId, args })`
 * 2. 插件（registerPlugin）执行方法
 *    → `ctx.emit('__result:<callId>', { ok, result|error })`
 * 3. 宿主收到 `__result:<callId>` → resolve/reject
 *
 * 超时兜底：插件失联时 Promise 不会永久挂起（防泄漏）。
 */

import type { PluginBridge } from './bridge.js';

/** 调用选项。 */
export interface HostInvokeOptions {
  /** 超时毫秒（默认 30000；0 = 不超时）。 */
  timeoutMs?: number;
}

/** 与 register-plugin.ts 的 InvokeResult 对应的载荷形状。 */
interface InvokeResult<T> {
  ok: boolean;
  result?: T;
  error?: { code: string; message: string };
}

/** 默认超时（毫秒），与 PluginContext 的 DEFAULT_TIMEOUT_MS 对齐。 */
const DEFAULT_HOST_INVOKE_TIMEOUT_MS = 30_000;

/**
 * 宿主侧调用 iframe 插件的方法。
 *
 * @param bridge - 已握手的宿主侧 PluginBridge
 * @param method - 插件方法名（registerPlugin 的 methods 键）
 * @param args - 方法参数
 * @param options - 调用选项
 * @returns 方法结果；插件返回错误时以 Error reject
 * @throws TimeoutError（message 含 'timed out'）当插件未在期限内响应
 */
export function callPluginMethod<T = unknown>(
  bridge: PluginBridge,
  method: string,
  args?: unknown,
  options: HostInvokeOptions = {},
): Promise<T> {
  const timeoutMs = options.timeoutMs ?? DEFAULT_HOST_INVOKE_TIMEOUT_MS;
  const callId = crypto.randomUUID();

  return new Promise<T>((resolve, reject) => {
    let settled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const finish = (fn: () => void): void => {
      if (settled) return;
      settled = true;
      if (timer !== undefined) clearTimeout(timer);
      unsubscribe();
      fn();
    };

    const unsubscribe = bridge.onEvent(`__result:${callId}`, (data) => {
      const res = data as InvokeResult<T> | null;
      if (!res || typeof res.ok !== 'boolean') {
        finish(() => reject(new Error(`Invalid plugin result envelope for method '${method}'`)));
        return;
      }
      if (res.ok) {
        finish(() => resolve(res.result as T));
      } else {
        const message = res.error?.message ?? 'Plugin method error';
        const code = res.error?.code ?? 'E_METHOD_ERROR';
        finish(() => reject(new Error(`${code}: ${message}`)));
      }
    });

    if (timeoutMs > 0) {
      timer = setTimeout(() => {
        finish(() =>
          reject(new Error(`Plugin method timed out after ${timeoutMs}ms: ${method}`)),
        );
      }, timeoutMs);
    }

    // 先发订阅再触发：onEvent 已注册，__result 无论何时到达都不丢帧。
    bridge.emitToPlugin(`__invoke:${method}`, { callId, args });
  });
}