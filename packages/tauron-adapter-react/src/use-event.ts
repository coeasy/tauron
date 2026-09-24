/**
 * useEvent 钩子（设计文档 §4.2）
 *
 * 订阅插件事件，自动清理订阅。
 */

import { useEffect, useRef, useState } from 'react';
import { listenEvent } from '@tauron/core';
import { eventNamespace } from '@tauron/types';
import { useTauronContext } from './context.js';

/** useEvent 返回值 */
export interface UseEventReturn<T = unknown> {
  /** 最新事件数据 */
  data: T | null;
  /** 是否已收到事件 */
  received: boolean;
  /** 接收次数 */
  count: number;
}

/**
 * useEvent — 事件订阅钩子
 *
 * @param pluginId - 插件 ID
 * @param eventName - 事件名
 * @param handler - 事件处理器（可选）
 */
export function useEvent<T = unknown>(
  pluginId: string,
  eventName: string,
  handler?: (payload: T) => void,
): UseEventReturn<T> {
  const { backend } = useTauronContext();
  const [data, setData] = useState<T | null>(null);
  const [received, setReceived] = useState(false);
  const [count, setCount] = useState(0);
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    const topic = eventNamespace(pluginId, eventName);
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    const handlerFn = (payload: unknown) => {
      if (cancelled) return;
      setData(payload as T);
      setReceived(true);
      setCount((c) => c + 1);
      handlerRef.current?.(payload as T);
    };

    listenEvent(backend, topic, handlerFn).then((u) => {
      if (cancelled) {
        u();
      } else {
        unlisten = u;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [backend, pluginId, eventName]);

  return { data, received, count };
}
