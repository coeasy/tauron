// @tauron/app-plugin-sdk — PluginContext 实现。

import type { HostClient } from '@tauron/host';
import type { CommandHandler, EventListener, PluginContext } from './types.js';
import type { JsonValue } from '@tauron/host';

/**
 * 宿主侧事件投递入口。
 *
 * 不属于插件面向的 {@link PluginContext}：本 SDK **内置取件泵**（见
 * {@link createPluginContext}）会经 `host_events_drain` 把总线队列里的帧
 * 取回并自动分发给插件内注册的订阅者；若宿主桥接层选择自己驱动投递，
 * 也可以直接调用 {@link PluginEventSink.dispatchEvent} 注入。
 */
export interface PluginEventSink {
  /** 分发一条事件给该插件内已注册的订阅者。 */
  dispatchEvent(topic: string, payload: unknown): void;
  /** 释放全部事件订阅（插件卸载时调用）。 */
  disposeEvents(): Promise<void>;
}

/**
 * 创建 PluginContext（桥接 SDK 到宿主 RPC 面）。
 *
 * **R5**：本函数只消费 `@tauron/host` 的 `PluginRpc` 契约——事件域 4 原语
 * （`eventsSubscribe` / `eventsDrain` / `eventsUnsubscribe` / `eventsPublish`）
 * 加 `contributesRegister`；`HostClient` 结构满足它，契约漂移由编译期断言
 * （`HostClientSatisfiesPluginRpc`）+ wire-gate 拦下。SDK 因此看不见
 * `invoke` / Tauri `Channel` 等传输细节。
 *
 * 用法（宿主调用）：
 * ```typescript
 * const ctx = createPluginContext('com.example.myplugin', hostClient);
 * await plugin.activate(ctx);
 *
 * // 事件帧到达时（由内置取件泵自动调用，一般无需手动）：
 * ctx.dispatchEvent('com.example.topic', { ... });
 * ```
 */
export function createPluginContext(
  pluginId: string,
  host: HostClient,
): PluginContext & PluginEventSink {
  const commands = new Map<string, CommandHandler>();
  const settingsTabs = new Map<string, { id: string; title: string; schema?: Record<string, unknown> }>();

  /** 本地订阅者（按 topic 分组）。 */
  const eventListeners = new Map<string, Set<EventListener>>();
  /** 已建立的宿主订阅 token。 */
  const eventTokens = new Map<string, string>();
  /** 正在建立中的宿主订阅（防重复订阅）。 */
  const subscribing = new Set<string>();
  /** 建立过程中已被退订的 topic（回调到达时需立即回收）。 */
  const cancelled = new Set<string>();

  // ── 取件泵 ────────────────────────────────────────────────────────────
  //
  // 宿主总线是**拉取**模型（`host_events_drain`）：帧只进队列、没人取就
  // 永远到不了订阅者（「只订阅不取件 = 没订阅」，队列只进不出，发布方最终
  // 溢满报错）。订阅建立后由本泵周期取回并分发——缺了它，subscribe 侧看似
  // 成功、投递侧永远断链。单拍失败不打断泵（下一拍重试），断一拍就是
  // 永久断链。
  const PUMP_INTERVAL_MS = 100;
  let pumpTimer: ReturnType<typeof setTimeout> | null = null;
  let pumpActive = false;

  const stopPump = (): void => {
    pumpActive = false;
    if (pumpTimer !== null) {
      clearTimeout(pumpTimer);
      pumpTimer = null;
    }
  };

  /** 没有任何在途/存量宿主订阅时收泵（退订干净后不再空转 IPC）。 */
  const stopPumpIfIdle = (): void => {
    if (eventTokens.size === 0 && subscribing.size === 0) stopPump();
  };

  const runPump = async (): Promise<void> => {
    pumpTimer = null;
    if (!pumpActive) return;
    try {
      // 发布走可靠通道（`host_events_publish` → request），深链等框架投递走
      // event 通道——两路都取，取回即按 topic 分发给本地订阅者。
      const batches = await Promise.all([
        host.eventsDrain('request').catch(() => undefined),
        host.eventsDrain('event').catch(() => undefined),
      ]);
      if (!pumpActive) return;
      for (const frames of batches) {
        for (const frame of frames ?? []) {
          ctx.dispatchEvent(frame.topic, frame.payload);
        }
      }
    } catch {
      // 兜底：任何取件异常都不得杀死泵（下一拍重试）。
    }
    if (pumpActive) {
      pumpTimer = setTimeout(() => {
        void runPump();
      }, PUMP_INTERVAL_MS);
    }
  };

  const startPump = (): void => {
    if (pumpActive) return;
    pumpActive = true;
    void runPump();
  };

  /** 惰性建立宿主订阅：同一 topic 只订阅一次。 */
  const ensureSubscription = (topic: string): void => {
    if (eventTokens.has(topic) || subscribing.has(topic)) return;
    subscribing.add(topic);
    void host
      .eventsSubscribe([{ topic }])
      .then((sub) => {
        subscribing.delete(topic);
        if (cancelled.delete(topic)) {
          // 订阅途中所有订阅者都已退订：立即回收，避免宿主侧悬挂订阅
          void host.eventsUnsubscribe(sub.token).catch(() => undefined);
          stopPumpIfIdle();
          return;
        }
        eventTokens.set(topic, sub.token);
        startPump();
      })
      .catch(() => {
        // 订阅失败（如未获授权）：不抛给调用方，静默降级为本地无投递
        subscribing.delete(topic);
        cancelled.delete(topic);
        stopPumpIfIdle();
      });
  };

  /** 释放宿主订阅。 */
  const releaseSubscription = (topic: string): void => {
    const token = eventTokens.get(topic);
    if (token !== undefined) {
      eventTokens.delete(topic);
      void host.eventsUnsubscribe(token).catch(() => undefined);
      stopPumpIfIdle();
      return;
    }
    if (subscribing.has(topic)) {
      cancelled.add(topic);
    }
  };

  const ctx: PluginContext & PluginEventSink = {
    pluginId,

    get host() {
      return host;
    },

    commands: {
      register<TArgs = unknown, TResult = unknown>(
        id: string,
        handler: CommandHandler<TArgs, TResult>,
      ): { ok: boolean; error?: string } {
        if (commands.has(id)) {
          return { ok: false, error: `命令 "${id}" 已存在` };
        }
        commands.set(id, handler as CommandHandler);
        return { ok: true };
      },

      unregister(id: string): boolean {
        return commands.delete(id);
      },

      has(id: string): boolean {
        return commands.has(id);
      },

      list(): string[] {
        return [...commands.keys()];
      },

      async execute<TArgs = unknown, TResult = unknown>(
        id: string,
        args?: TArgs,
      ): Promise<TResult> {
        const handler = commands.get(id);
        if (!handler) {
          throw new Error(`命令 "${id}" 不存在`);
        }
        return (await handler(args as TArgs, ctx)) as TResult;
      },
    },

    settings: {
      /**
       * 注册设置 Tab。
       *
       * 本地 Map 只是去重账本；应用的设置中心读的是**宿主贡献表**
       * （`host_contributes_list`，kind `settings`）——不同步喂给宿主就是
       * 「注册了但永远不可见」。因此注册成功后 best-effort 上报一条贡献：
       * 宿主拒绝（重复 id/未授权）只告警，不推翻本地注册结果。
       * 卸载时由宿主按插件 id 统一回收（没有单条注销命令，`unregisterTab`
       * 只影响本 webview）。
       */
      registerTab(config: { id: string; title: string; schema?: Record<string, unknown>; component?: string }): { ok: boolean; error?: string } {
        if (settingsTabs.has(config.id)) {
          return { ok: false, error: `设置 Tab "${config.id}" 已存在` };
        }
        settingsTabs.set(config.id, {
          id: config.id,
          title: config.title,
          // exactOptionalPropertyTypes：仅在提供了 schema 时才带上该字段
          ...(config.schema !== undefined ? { schema: config.schema } : {}),
        });
        ctx.host
          .contributesRegister({ kind: 'settings', id: config.id, label: config.title })
          .catch((err: unknown) => {
            ctx.log.warn('settings Tab 注册宿主贡献失败（不影响本地注册）', config.id, err);
          });
        return { ok: true };
      },

      unregisterTab(id: string): boolean {
        return settingsTabs.delete(id);
      },
    },

    events: {
      async publish(topic: string, payload: unknown): Promise<void> {
        // 跨界转换：payload 在 Rust 侧由 serde_json 校验为合法 JSON，
        // 因此这里的断言是 IPC 边界上的必要放宽，而非类型谎言。
        await host.eventsPublish({ topic, payload: payload as JsonValue });
      },

      subscribe(topic: string, listener: EventListener): () => void {
        let listeners = eventListeners.get(topic);
        if (!listeners) {
          listeners = new Set();
          eventListeners.set(topic, listeners);
        }
        listeners.add(listener);

        // 首次订阅该 topic 时向宿主申请订阅（惰性、去重）
        ensureSubscription(topic);

        return () => {
          const current = eventListeners.get(topic);
          if (!current) return;
          current.delete(listener);
          if (current.size === 0) {
            eventListeners.delete(topic);
            releaseSubscription(topic);
          }
        };
      },
    },

    log: {
      info(message: string, ...args: unknown[]): void {
        console.log(`[${pluginId}] ${message}`, ...args);
      },
      warn(message: string, ...args: unknown[]): void {
        console.warn(`[${pluginId}] ${message}`, ...args);
      },
      error(message: string, ...args: unknown[]): void {
        console.error(`[${pluginId}] ${message}`, ...args);
      },
    },

    // ── 宿主侧事件投递（PluginEventSink）──────────────────────────

    dispatchEvent(topic: string, payload: unknown): void {
      const listeners = eventListeners.get(topic);
      if (!listeners || listeners.size === 0) return;
      // 复制一份：订阅者在回调中退订不应影响本轮分发
      for (const listener of [...listeners]) {
        try {
          listener(payload, { topic, pluginId });
        } catch {
          // 单个订阅者异常不影响其他订阅者
        }
      }
    },

    async disposeEvents(): Promise<void> {
      const tokens = [...eventTokens.values()];
      eventTokens.clear();
      eventListeners.clear();
      subscribing.clear();
      cancelled.clear();
      stopPump();
      await Promise.all(
        tokens.map((token) => host.eventsUnsubscribe(token).catch(() => undefined)),
      );
    },
  };

  return ctx;
}
