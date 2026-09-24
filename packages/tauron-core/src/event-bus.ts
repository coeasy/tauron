/**
 * tauron 事件总线（设计文档 §2.3/§4.5）
 *
 * 带背压的事件总线：每插件独立队列，溢出时丢弃最旧事件。
 */

import { type PluginEvent, eventNamespace } from '@tauron/types';

/** 事件队列配置 */
export interface EventBusConfig {
  /** 每插件最大队列长度（默认 1000） */
  maxQueueSize?: number;
  /** 溢出时是否丢弃最旧事件（默认 true） */
  dropOldest?: boolean;
}

/** 事件订阅者 */
export interface EventSubscriber {
  deliver(event: PluginEvent): void;
}

/** 事件总线错误 */
export class EventBusError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'EventBusError';
  }
}

/**
 * 事件总线
 */
export class EventBus {
  private queues: Map<string, PluginEvent[]> = new Map();
  private subscribers: Map<string, Set<EventSubscriber>> = new Map();
  private config: Required<EventBusConfig>;
  private droppedCount: number = 0;

  constructor(config: EventBusConfig = {}) {
    this.config = {
      maxQueueSize: config.maxQueueSize ?? 1000,
      dropOldest: config.dropOldest ?? true,
    };
  }

  /**
   * 发布事件
   */
  emit(event: PluginEvent): EventBusError | null {
    const key = eventNamespace(event.sourcePlugin, event.eventName);

    // Backpressure check
    let queue = this.queues.get(event.sourcePlugin);
    if (!queue) {
      queue = [];
      this.queues.set(event.sourcePlugin, queue);
    }

    if (queue.length >= this.config.maxQueueSize) {
      if (this.config.dropOldest) {
        queue.shift();
        this.droppedCount++;
      } else {
        return new EventBusError('Queue overflow');
      }
    }

    queue.push(event);

    // Distribute to subscribers
    const subs = this.subscribers.get(key);
    if (subs) {
      for (const sub of subs) {
        try {
          sub.deliver(event);
        } catch (err) {
          console.error('Subscriber delivery error:', err);
        }
      }
    }

    return null;
  }

  /**
   * 订阅事件
   */
  subscribe(pluginId: string, eventName: string, subscriber: EventSubscriber): void {
    const key = eventNamespace(pluginId, eventName);
    let subs = this.subscribers.get(key);
    if (!subs) {
      subs = new Set();
      this.subscribers.set(key, subs);
    }
    subs.add(subscriber);
  }

  /**
   * 取消订阅
   */
  unsubscribe(pluginId: string, eventName: string, subscriber: EventSubscriber): void {
    const key = eventNamespace(pluginId, eventName);
    const subs = this.subscribers.get(key);
    if (subs) {
      subs.delete(subscriber);
      if (subs.size === 0) {
        this.subscribers.delete(key);
      }
    }
  }

  /**
   * 清理插件的所有队列和订阅
   */
  clearPlugin(pluginId: string): void {
    this.queues.delete(pluginId);
    for (const [key, subs] of this.subscribers) {
      if (key.startsWith(`plugin:${pluginId}:`)) {
        this.subscribers.delete(key);
      }
    }
  }

  /**
   * 获取队列统计
   */
  getQueueStats(): { queueSize: number; droppedCount: number } {
    let totalQueueSize = 0;
    for (const queue of this.queues.values()) {
      totalQueueSize += queue.length;
    }
    return { queueSize: totalQueueSize, droppedCount: this.droppedCount };
  }

  /**
   * 清空所有队列
   */
  clear(): void {
    this.queues.clear();
    this.droppedCount = 0;
  }
}
