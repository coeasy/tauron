/**
 * eventStore — Svelte store for event subscription
 *
 * 订阅插件事件，自动清理订阅。
 */

import { writable, type Readable } from 'svelte/store';
import { listenEvent } from '@tauron/core';
import { eventNamespace } from '@tauron/types';
import { getTauronContext } from './context.js';

export interface EventState<T = unknown> {
  data: T | null;
  received: boolean;
  count: number;
}

export interface EventStore<T = unknown> extends Readable<EventState<T>> {
  onEvent: (handler: (payload: T) => void) => () => void;
  destroy: () => void;
}

export function createEventStore<T = unknown>(
  pluginId: string,
  eventName: string,
  handler?: (payload: T) => void,
): EventStore<T> {
  const { backend } = getTauronContext();

  const { subscribe, update } = writable<EventState<T>>({
    data: null,
    received: false,
    count: 0,
  });

  let unlisten: (() => void) | undefined;
  const handlers: Array<(payload: T) => void> = handler ? [handler] : [];

  const topic = eventNamespace(pluginId, eventName);

  listenEvent(backend, topic, (payload) => {
    const data = payload as T;
    for (const h of handlers) {
      h(data);
    }
    update((state) => ({
      data,
      received: true,
      count: state.count + 1,
    }));
  }).then((u) => {
    unlisten = u;
  });

  const onEvent = (newHandler: (payload: T) => void) => {
    handlers.push(newHandler);
    return () => {
      const idx = handlers.indexOf(newHandler);
      if (idx !== -1) handlers.splice(idx, 1);
    };
  };

  const destroy = () => {
    unlisten?.();
    handlers.length = 0;
  };

  return { subscribe, onEvent, destroy };
}
