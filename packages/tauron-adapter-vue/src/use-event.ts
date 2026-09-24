/**
 * useEvent composable（设计文档 §4.2）
 *
 * 订阅插件事件，自动清理订阅。
 */

import { shallowRef, onUnmounted, type Ref } from 'vue';
import { listenEvent } from '@tauron/core';
import { eventNamespace } from '@tauron/types';
import { injectTauronContext } from './context.js';

export interface UseEventReturn<T = unknown> {
  data: Ref<T | null>;
  received: Ref<boolean>;
  count: Ref<number>;
}

export function useEvent<T = unknown>(
  pluginId: string,
  eventName: string,
  handler?: (payload: T) => void,
): UseEventReturn<T> {
  const { backend } = injectTauronContext();

  const data = shallowRef<T | null>(null);
  const received = shallowRef(false);
  const count = shallowRef(0);

  let unlisten: (() => void) | undefined;

  const topic = eventNamespace(pluginId, eventName);

  listenEvent(backend, topic, (payload) => {
    data.value = payload as T;
    received.value = true;
    count.value = count.value + 1;
    handler?.(payload as T);
  }).then((u) => {
    unlisten = u;
  });

  onUnmounted(() => {
    unlisten?.();
  });

  return { data, received, count };
}
