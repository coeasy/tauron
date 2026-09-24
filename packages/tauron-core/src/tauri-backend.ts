/**
 * tauron Tauri IPC 后端（设计文档 §1.3/§2.1）
 *
 * 把 {@link TauronBackend} 接口实现为 Tauri IPC 调用：信封经 `plugin_invoke`
 * 单命令下发，取消走 `plugin_cancel`，跨插件事件走 `plugin_emit`。
 *
 * ## 为什么用动态 import
 *
 * `@tauri-apps/api` 是本包的**可选** peer dependency。静态 import 会让纯 Web
 * 构建（契约测试 / SSR / 类型检查）在缺少该依赖时直接加载失败，因此这里只在
 * 真正发起调用时按需加载，并且**失败后清空缓存**以便后续重试——否则一次失败
 * 会把缓存永久污染成 rejected promise（设计文档 §9 Web 降级）。
 *
 * 命令名与 `tauron-shell` 的命令层一一对应，由
 * `tauron-contract-tests` 的跨语言门禁锁定。
 */

import {
  buildErrorResponse,
  PluginErrorCode,
  DEFAULT_TIMEOUT_MS,
  type PluginInvokeRequest,
  type PluginInvokeResponse,
  type ProgressEvent,
} from '@tauron/types';
import type { ProgressCallback, TauronBackend } from './backend.js';

/**
 * tauron 命令名（与 Rust `tauron-shell` 命令层一致）。
 *
 * 改动这里必须同步 `crates/tauron-shell/src/commands.rs`，否则跨语言门禁失败。
 */
export const TAURON_COMMANDS = {
  invoke: 'plugin_invoke',
  cancel: 'plugin_cancel',
  emit: 'plugin_emit',
} as const;

/** 后端配置 */
export interface TauriBackendOptions {
  /** 覆盖默认请求超时（毫秒，0 = 不超时）。 */
  timeoutMs?: number;
  /**
   * 覆盖 `@tauri-apps/api` 的加载器（测试注入用）。
   *
   * 生产环境不需要设置；契约测试用它注入一个假的 IPC 层，
   * 从而在无 Tauri 运行时验证信封编解码。
   */
  loader?: () => Promise<TauriApi>;
}

/** 从 `@tauri-apps/api` 抽出的最小接口面（便于测试替身）。 */
export interface TauriApi {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen(
    event: string,
    handler: (event: { payload: unknown }) => void,
  ): Promise<() => void>;
  /** Tauri v2 流式通道；旧版本可能没有。 */
  Channel?: new <T>() => { onmessage: (message: T) => void };
}

let apiPromise: Promise<TauriApi> | null = null;

/**
 * 加载 `@tauri-apps/api`（缓存 + 失败自动重置）。
 */
async function loadTauriApi(): Promise<TauriApi> {
  if (!apiPromise) {
    apiPromise = (async (): Promise<TauriApi> => {
      const [core, event] = await Promise.all([
        import('@tauri-apps/api/core'),
        import('@tauri-apps/api/event'),
      ]);
      const api: TauriApi = {
        invoke: (cmd, args) => core.invoke(cmd, args),
        listen: (name, handler) =>
          event.listen(name, handler as (e: unknown) => void),
      };
      // `Channel` 在新版本才导出；缺失时降级为无进度推送。
      const maybeChannel = (core as { Channel?: TauriApi['Channel'] }).Channel;
      if (maybeChannel) api.Channel = maybeChannel;
      return api;
    })().catch((err: unknown) => {
      // 关键：失败不缓存，否则一次瞬时失败会永久破坏后续所有调用。
      apiPromise = null;
      throw err;
    });
  }
  return apiPromise;
}

/**
 * 创建 Tauri IPC 后端。
 *
 * ```ts
 * const backend = createTauriBackend();
 * const res = await invokePlugin(backend, 'com.example.formatter', 'format', { code });
 * ```
 */
export function createTauriBackend(options: TauriBackendOptions = {}): TauronBackend {
  const defaultTimeout = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
  const load = options.loader ?? loadTauriApi;

  return {
    async invoke(
      request: PluginInvokeRequest,
      _channel?: MessageChannel,
      onProgress?: ProgressCallback,
    ): Promise<PluginInvokeResponse> {
      const timeoutMs = request.timeoutMs ?? defaultTimeout;

      let api: TauriApi;
      try {
        api = await load();
      } catch (err) {
        return buildErrorResponse(
          request.callId,
          PluginErrorCode.CHANNEL_BROKEN,
          `Tauri IPC unavailable: ${err instanceof Error ? err.message : String(err)}`,
          true,
        );
      }

      const args: Record<string, unknown> = { request };

      // 进度通道：仅当调用方关心进度且运行时支持 Channel 时才建立。
      if (onProgress && api.Channel) {
        const channel = new api.Channel<ProgressEvent>();
        channel.onmessage = (event: ProgressEvent) => onProgress(event);
        args.channel = channel;
      }

      // JS 侧兜底超时：即使 Rust 侧未实现超时，调用方也不会永久挂起。
      const call = api.invoke<PluginInvokeResponse>(TAURON_COMMANDS.invoke, args);

      if (!timeoutMs) {
        return call.catch((err: unknown) => toBrokenChannel(request.callId, err));
      }

      let timer: ReturnType<typeof setTimeout> | undefined;
      const timeout = new Promise<PluginInvokeResponse>((resolve) => {
        timer = setTimeout(() => {
          // 超时后调用方已放弃该调用：best-effort 通知宿主中止，否则宿主侧的
          // 执行与 pending 条目只能等 TTL GC 才回收（`plugin_cancel` 会立刻
          // 结束 pending）。取消失败不放大失败：运行时可能未装载 dispatcher
          // （此时返回 false）或 pending 已被回收，故只吞不抛。
          //
          // try/catch 也是必需的：注入层若**同步抛出**（非 Tauri 的 Promise
          // 实现），异常会冒出定时器回调、令下面的 resolve 永不执行 ——
          // 那就破坏了本函数的承诺（"调用方不会永久挂起"）。
          try {
            void api
              .invoke<void>(TAURON_COMMANDS.cancel, { request: { callId: request.callId } })
              .catch(() => undefined);
          } catch {
            // 同步失败同样忽略。
          }
          resolve(
            buildErrorResponse(
              request.callId,
              PluginErrorCode.TIMEOUT,
              `Invocation timed out after ${timeoutMs}ms`,
              true,
            ),
          );
        }, timeoutMs);
      });

      try {
        return await Promise.race([
          call.catch((err: unknown) => toBrokenChannel(request.callId, err)),
          timeout,
        ]);
      } finally {
        if (timer !== undefined) clearTimeout(timer);
      }
    },

    async cancel(callId: string): Promise<void> {
      const api = await load();
      await api.invoke<void>(TAURON_COMMANDS.cancel, { request: { callId } });
    },

    async listen(
      topic: string,
      handler: (payload: unknown) => void,
    ): Promise<() => void> {
      const api = await load();
      return api.listen(topic, (event) => handler(event.payload));
    },

    async emit(topic: string, payload: unknown): Promise<void> {
      const api = await load();
      await api.invoke<void>(TAURON_COMMANDS.emit, { topic, payload });
    },
  };
}

/** IPC 层自身失败（命令缺失 / 反序列化错误）→ 结构化可重试错误。 */
function toBrokenChannel(callId: string, err: unknown): PluginInvokeResponse {
  return buildErrorResponse(
    callId,
    PluginErrorCode.CHANNEL_BROKEN,
    err instanceof Error ? err.message : String(err),
    true,
  );
}
