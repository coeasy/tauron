/**
 * useInvoke 钩子（设计文档 §4.2）
 *
 * 提供插件方法调用的 React 状态管理：
 * - loading: 是否正在调用
 * - data: 调用结果
 * - error: 调用错误
 * - invoke: 发起调用
 * - reset: 重置状态
 */

import { useCallback, useEffect, useState, useRef } from 'react';
import { invokePlugin, type InvokeOptions } from '@tauron/core';
import type { PluginInvokeResponse } from '@tauron/types';
import { useTauronContext } from './context.js';

/** useInvoke 状态 */
export interface InvokeState<T = unknown> {
  /** 是否正在调用 */
  loading: boolean;
  /** 调用结果 */
  data: T | null;
  /** 调用错误 */
  error: { code: string; message: string } | null;
  /** 上一次调用的请求参数 */
  lastArgs: unknown | null;
}

/** useInvoke 返回值 */
export interface UseInvokeReturn<T = unknown> {
  /** 状态 */
  state: InvokeState<T>;
  /** 发起调用 */
  invoke: (method: string, payload?: unknown, options?: InvokeOptions) => Promise<T | null>;
  /** 重置状态 */
  reset: () => void;
}

/**
 * useInvoke — 插件方法调用钩子
 *
 * 并发语义：**最后一次调用胜出**。旧调用即使后返回也不会覆盖新调用的状态
 * （否则快速连续调用会出现结果错乱）；组件卸载后不再写入状态。
 *
 * 需要真正中止进行中的调用时，请使用 `@tauron/core` 的 `cancelPlugin`
 * （信封带 `callId`），本钩子不伪造取消语义。
 *
 * @param pluginId - 插件 ID
 */
export function useInvoke<T = unknown>(pluginId: string): UseInvokeReturn<T> {
  const { backend } = useTauronContext();
  const [state, setState] = useState<InvokeState<T>>({
    loading: false,
    data: null,
    error: null,
    lastArgs: null,
  });

  /** 调用序号：只有最新一次调用可以写状态。 */
  const seqRef = useRef(0);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      // 使所有在途调用失效，避免卸载后 setState
      seqRef.current += 1;
    };
  }, []);

  const invoke = useCallback(
    async (method: string, payload?: unknown, options?: InvokeOptions): Promise<T | null> => {
      const seq = (seqRef.current += 1);
      /** 仅当仍挂载且仍是最新调用时才提交状态。 */
      const commit = (next: InvokeState<T>): void => {
        if (mountedRef.current && seq === seqRef.current) {
          setState(next);
        }
      };

      commit({ loading: true, data: null, error: null, lastArgs: payload ?? null });

      try {
        const response: PluginInvokeResponse = await invokePlugin(
          backend,
          pluginId,
          method,
          payload,
          options,
        );

        if (response.ok) {
          const data = response.result as T;
          commit({ loading: false, data, error: null, lastArgs: payload ?? null });
          return data;
        } else {
          const error = response.error!;
          commit({
            loading: false,
            data: null,
            error: { code: error.code, message: error.message },
            lastArgs: payload ?? null,
          });
          return null;
        }
      } catch (err) {
        const error = {
          code: 'SC-2004',
          message: err instanceof Error ? err.message : String(err),
        };
        commit({ loading: false, data: null, error, lastArgs: payload ?? null });
        return null;
      }
    },
    [backend, pluginId],
  );

  const reset = useCallback(() => {
    // 使在途调用失效，避免 reset 之后被旧响应覆盖
    seqRef.current += 1;
    setState({ loading: false, data: null, error: null, lastArgs: null });
  }, []);

  return { state, invoke, reset };
}
