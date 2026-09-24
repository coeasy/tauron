// ──────────────────────────────────────────────────────────────────────────
// HostRpc（R5）：宿主能力的**传输无关面**。
//
// 方案 R5 的目标：`Tauri invoke` 与事件总线不再是两个世界——上层只见一个
// RPC 面（request / stream / event / subscribe），换传输（移动 gateway、测试
// mock、未来的 IPC 后端）只需换实现，SDK 与业务代码不动。
//
// 现状与关系（诚实说明）：
// - {@link HostRpc} = 方案定义的规范面，由 {@link toHostRpc} 从 `HostClient` 适配；
// - {@link PluginRpc} = `PluginContext` 目前**实际**消费的事件子集（它自带取件泵，
//   因此需要 drain/unsubscribe 原语）。两者都由 `HostClient` 结构满足，SDK 侧
//   只 import 类型，不 import 任何 Tauri 细节。
// ──────────────────────────────────────────────────────────────────────────

import type { Unlisten } from './backend.js';
import type { EventFrame, EventSelector, Subscription, JsonValue } from './events.js';
import type { ContributeEntryInput, HostClient } from './host.js';
import type { StreamFrame } from './stream.js';

/**
 * 传输无关的宿主 RPC 面（方案 R5）。
 *
 * 插件 SDK 与业务代码只依赖本接口：换成非 Tauri 传输（移动 gateway、测试 mock）
 * 时，实现这 4 个方法即可，不需要知道 `invoke`/`Channel` 的存在。
 */
export interface HostRpc {
  /** 发一条请求，拿一次应答。 */
  request<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;

  /**
   * 订阅一次调用的流式帧。
   *
   * 返回的 `Unlisten` 会**关流**（发终帧 + 释放句柄），而不是只摘掉本地回调——
   * 只摘回调会让宿主侧留下一条永远没人收的流。
   */
  stream(callId: string, onFrame: (frame: StreamFrame) => void): Unlisten;

  /** 发布事件（走总线，可靠语义）。 */
  event(topic: string, payload: unknown): Promise<void>;

  /**
   * 订阅事件 topic。
   *
   * 事件总线是**拉取**模型，本实现自带节拍泵（`pumpIntervalMs` 默认 100ms，
   * 无订阅时收泵）。需要自己控制节拍的宿主可注入 {@link PumpScheduler}。
   */
  subscribe(topic: string, handler: (payload: unknown) => void): Promise<Unlisten>;
}

/**
 * `PluginContext` 消费的宿主 RPC 子集。
 *
 * SDK 内置取件泵（`host_events_drain` 驱动），因此它需要的是原语而不是
 * {@link HostRpc.subscribe} 封装；此外它还要注册贡献（设置 Tab 等）。
 * `HostClient` 结构上满足本接口——契约漂移由
 * {@link HostClientSatisfiesPluginRpc} 编译期断言 + wire-gate 拦下。
 */
export interface PluginRpc {
  eventsSubscribe(subs: EventSelector[]): Promise<Subscription>;
  eventsDrain(kind?: 'event' | 'request' | 'state'): Promise<EventFrame[]>;
  eventsUnsubscribe(token: string): Promise<void>;
  eventsPublish(evt: { topic: string; payload: JsonValue }): Promise<unknown>;
  contributesRegister(entry: ContributeEntryInput): Promise<void>;
}

/**
 * 编译期断言：`HostClient` 必须满足 SDK 消费的事件子集契约。
 *
 * 这条断言是**接口漂移的报警器**：`PluginContext` 只用 4 个事件方法，若哪天
 * `HostClient` 改了签名而 SDK 没跟上，这里会直接编译失败（而不是等运行时）。
 */
export type HostClientSatisfiesPluginRpc = HostClient extends PluginRpc ? true : never;

/** 泵节拍器：默认用真实定时器；测试注入手动节拍以避免依赖真实时间。 */
export interface PumpScheduler {
  every(ms: number, tick: () => void): Unlisten;
}

export interface HostRpcOptions {
  /** 取件泵节拍（毫秒，默认 100）。 */
  pumpIntervalMs?: number;
  /** 自定义节拍器（测试用）。 */
  scheduler?: PumpScheduler;
}

const defaultScheduler: PumpScheduler = {
  every(ms, tick) {
    const timer = setInterval(tick, ms);
    return () => clearInterval(timer);
  },
};

/** 从 `HostClient` 适配出 {@link HostRpc}。 */
export function toHostRpc(client: HostClient, options: HostRpcOptions = {}): HostRpc {
  const handlers = new Map<string, Set<(payload: unknown) => void>>();
  const tokens = new Map<string, string>();
  let stopPump: Unlisten | null = null;
  let ticking = false;

  const tick = async (): Promise<void> => {
    // 单拍不重入：慢 IPC 下不让取件请求堆积。
    if (ticking) return;
    ticking = true;
    try {
      const frames = await client.eventsDrain('event');
      for (const frame of frames) {
        handlers.get(frame.topic)?.forEach((h) => h(frame.payload));
      }
    } catch {
      // 单拍失败**不打断**泵：断一拍就是永久断链，下一拍重试。
    } finally {
      ticking = false;
    }
  };

  return {
    request: <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
      client.request<T>(cmd, args),

    stream: (callId, onFrame) => client.openStream(callId, onFrame),

    event: async (topic, payload) => {
      await client.eventsPublish({ topic, payload: payload as JsonValue });
    },

    subscribe: async (topic, handler) => {
      let set = handlers.get(topic);
      if (!set) {
        set = new Set();
        handlers.set(topic, set);
        // 先建立宿主订阅，再挂本地回调：反过来的话第一帧可能丢在建立之前。
        const sub = await client.eventsSubscribe([{ topic }]);
        tokens.set(topic, sub.token);
      }
      set.add(handler);
      if (!stopPump) {
        stopPump = (options.scheduler ?? defaultScheduler).every(
          options.pumpIntervalMs ?? 100,
          () => void tick(),
        );
      }
      let active = true;
      return () => {
        if (!active) return; // 幂等：重复退订不得误伤同 topic 的其他订阅者
        active = false;
        const current = handlers.get(topic);
        if (!current) return;
        current.delete(handler);
        if (current.size > 0) return;
        handlers.delete(topic);
        const token = tokens.get(topic);
        tokens.delete(topic);
        if (token !== undefined) void client.eventsUnsubscribe(token);
        if (handlers.size === 0 && stopPump) {
          stopPump();
          stopPump = null;
        }
      };
    },
  };
}
