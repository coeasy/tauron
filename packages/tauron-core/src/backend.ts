/**
 * tauron Backend 抽象（设计文档 §1.3 数据流）
 *
 * 抽象 IPC 层，允许在不同环境（Tauri/Web/Electron）中切换。
 */

import type {
  PluginInvokeRequest,
  PluginInvokeResponse,
  ProgressEvent,
} from '@tauron/types';

/** 进度事件回调 */
export type ProgressCallback = (event: ProgressEvent) => void;

/** IPC Backend 接口 */
export interface TauronBackend {
  /**
   * 调用插件方法。
   *
   * @param request - 信封请求
   * @param channel - **保留参数**：仅当后端自行完成 `port2` 转移（即真正会把
   *   `port2` 交给对端）时才有意义。`invokePlugin` 不会传入该参数，进度统一
   *   通过 `onProgress` 投递。
   * @param onProgress - 进度回调（后端实现进度的主通道）
   */
  invoke(
    request: PluginInvokeRequest,
    channel?: MessageChannel,
    onProgress?: ProgressCallback,
  ): Promise<PluginInvokeResponse>;

  /** 取消进行中的调用 */
  cancel(callId: string): Promise<void>;

  /** 订阅事件 */
  listen(
    topic: string,
    handler: (payload: unknown) => void,
  ): Promise<() => void>;

  /** 发布事件 */
  emit(topic: string, payload: unknown): Promise<void>;
}
