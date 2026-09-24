/**
 * registerPlugin — 插件注册便捷 API（P0-4 修复）。
 *
 * 用法（与 README 示例一致）：
 * ```typescript
 * import { registerPlugin } from '@tauron/plugin-sdk';
 *
 * registerPlugin({
 *   name: 'my-plugin',
 *   version: '1.0.0',
 *   methods: {
 *     async format({ args, ctx }) {
 *       return { formatted: args.code.replace(/\s+/g, ' ') };
 *     },
 *   },
 *   events: {
 *     'data-changed': ({ payload }) => payload,
 *   },
 *   onEnable(ctx) { console.log('Plugin enabled'); },
 * });
 * ```
 *
 * 通信协议：
 * - 宿主通过 `event` 消息发送 `__invoke:<method>` 事件触发方法调用
 * - 插件通过 `emit` 消息发送 `__result:<callId>` 事件返回结果
 * - 使用事件通道复用既有 postMessage 协议，无需协议变更
 */

import { createPluginContext, type PluginContext } from './plugin-context.js';

/** 方法调用参数。 */
export interface MethodCallOpts {
  /** 调用参数。 */
  args: unknown;
  /** 插件上下文。 */
  ctx: PluginContext;
}

/** 事件触发参数。 */
export interface EventCallOpts {
  /** 事件负载。 */
  payload: unknown;
}

/** 插件注册配置。 */
export interface RegisterPluginConfig {
  /** 插件名称。 */
  name: string;
  /** 插件版本。 */
  version: string;
  /** 插件方法（宿主可调用的方法表）。 */
  methods?: Record<string, (opts: MethodCallOpts) => unknown | Promise<unknown>>;
  /** 插件事件（宿主可订阅的事件处理器）。 */
  events?: Record<string, (opts: EventCallOpts) => unknown | void>;
  /** 插件启用钩子（握手完成后调用）。 */
  onEnable?: (ctx: PluginContext) => void | Promise<void>;
  /** 插件禁用钩子。 */
  onDisable?: (ctx: PluginContext) => void | Promise<void>;
  /**
   * 覆盖握手 token（可选）。
   *
   * 缺省时由 `createPluginContext` 从 URL hash 解析（宿主
   * `PluginBridge.createIframe` 的标准注入通道）。仅非标准宿主需要显式传入。
   */
  handshakeToken?: string;
}

/** 方法调用请求载荷。 */
interface InvokeRequest {
  callId: string;
  args: unknown;
}

/** 方法调用结果载荷。 */
interface InvokeResult {
  ok: boolean;
  result?: unknown;
  error?: { code: string; message: string };
}

/**
 * 注册插件。
 *
 * 创建 PluginContext 并注册方法/事件处理器。
 * 方法通过 `__invoke:<method>` 事件触发，结果通过 `__result:<callId>` 返回。
 *
 * @param config 插件注册配置
 * @returns 插件上下文（可用于高级操作）
 */
export function registerPlugin(config: RegisterPluginConfig): PluginContext {
  const ctx = createPluginContext(
    config.handshakeToken !== undefined
      ? { handshakeToken: config.handshakeToken }
      : {},
  );

  // ── 方法注册 ──
  for (const [method, handler] of Object.entries(config.methods ?? {})) {
    ctx.onEvent(`__invoke:${method}`, (data) => {
      const req = data as InvokeRequest;
      if (!req || typeof req.callId !== 'string') {
        console.warn(`[registerPlugin:${config.name}] 无效的 invoke 请求:`, data);
        return;
      }

      Promise.resolve(handler({ args: req.args, ctx }))
        .then((result) => {
          const res: InvokeResult = { ok: true, result };
          ctx.emit(`__result:${req.callId}`, res);
        })
        .catch((err: Error) => {
          const res: InvokeResult = {
            ok: false,
            error: { code: 'E_METHOD_ERROR', message: err.message },
          };
          ctx.emit(`__result:${req.callId}`, res);
        });
    });
  }

  // ── 事件注册 ──
  for (const [event, handler] of Object.entries(config.events ?? {})) {
    ctx.onEvent(event, (payload) => handler({ payload }));
  }

  // ── 生命周期钩子 ──
  if (config.onEnable) {
    ctx.onInit(() => {
      config.onEnable!(ctx);
    });
  }

  return ctx;
}