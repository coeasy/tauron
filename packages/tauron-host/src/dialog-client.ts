// ──────────────────────────────────────────────────────────────────────────
// DialogClient — 对话框与剪贴板客户端（P2-13）。
//
// ⚠️ 接入状态（读这一节再调用）：宿主**尚未**接入原生对话框与系统剪贴板，
//    因此当前语义如下——它们是**契约**，不是待修 bug：
//    - `openFile` / `openDirectory` / `saveFile` 恒返回 `null`（等同"用户取消"）；
//    - `message` / `error` / `warn` 不弹窗，直接返回（`Ok`）；
//    - `confirm` 恒返回 `false`——**它绝不表示用户点了取消，也永不为 true**：
//      不得作为真实用户确认（destructive 操作前请自行设计确认 UI）；
//    - 剪贴板是**进程内字符串**，不读写系统剪贴板：`clipboardRead()` 只能读回
//      本 App 自己 `clipboardWrite()` 写过的内容。
//
//    接入方式（宿主侧）：加 `tauri-plugin-dialog` /
//    `tauri-plugin-clipboard-manager`，在 `tauron-adapter` 对应命令里转调原生 API。
//    窗口命令已是真实调用，可作为接线参照（`host_window_set_position` 等）。
//
// 职责：
// 1. 打开文件对话框（openFile/openDirectory）
// 2. 保存文件对话框（saveFile）
// 3. 消息对话框（message/error/confirm/warn）
// 4. 剪贴板操作（read/write）
//
// 用法：
// ```typescript
// const dialog = new DialogClient({ backend });
// const path = await dialog.openFile({ filters: [{ name: 'Images', extensions: ['png', 'jpg'] }] });
// await dialog.message({ title: 'Info', message: 'Hello!' });
// const confirmed = await dialog.confirm({ title: 'Confirm', message: 'Are you sure?' });
// ```
// ──────────────────────────────────────────────────────────────────────────

import type { Backend } from './backend.js';

/** 文件过滤器 */
export interface FileFilter {
  /** 过滤器名称 */
  name: string;
  /** 扩展名列表 */
  extensions: string[];
}

/** 打开文件对话框选项 */
export interface OpenFileOptions {
  /** 文件过滤器 */
  filters?: FileFilter[];
  /** 是否允许多选 */
  multiple?: boolean;
  /** 是否允许目录 */
  directory?: boolean;
  /** 初始路径 */
  defaultPath?: string;
}

/** 保存文件对话框选项 */
export interface SaveFileOptions {
  /** 文件过滤器 */
  filters?: FileFilter[];
  /** 默认文件名 */
  defaultName?: string;
  /** 初始路径 */
  defaultPath?: string;
}

/** 消息对话框选项 */
export interface MessageOptions {
  /** 标题 */
  title: string;
  /** 消息内容 */
  message: string;
  /** 是否显示 OK 按钮 */
  ok?: boolean;
  /** 是否显示 Cancel 按钮 */
  cancel?: boolean;
}

/** 确认对话框选项 */
export interface ConfirmOptions {
  /** 标题 */
  title: string;
  /** 消息内容 */
  message: string;
  /** 确认按钮文本 */
  confirmLabel?: string;
  /** 取消按钮文本 */
  cancelLabel?: string;
}

/**
 * DialogClient — 系统对话框管理器。
 */
export class DialogClient {
  private readonly _backend: Backend;

  constructor(options: { backend: Backend }) {
    this._backend = options.backend;
  }

  /**
   * 打开文件对话框。
   *
   * 返回选中文件路径，取消返回 null。
   *
   * ⚠️ 原生对话框尚未接入：**恒返回 `null`**（见文件头接入状态）。
   */
  async openFile(options: OpenFileOptions = {}): Promise<string | null> {
    return this._backend.invoke<string | null>('host_dialog_open', {
      multiple: options.multiple ?? false,
      directory: options.directory ?? false,
      filters: options.filters,
      defaultPath: options.defaultPath,
    });
  }

  /**
   * 打开目录对话框。
   *
   * 返回选中目录路径，取消返回 null。
   *
   * ⚠️ 原生对话框尚未接入：**恒返回 `null`**（见文件头接入状态）。
   */
  async openDirectory(options: { defaultPath?: string } = {}): Promise<string | null> {
    return this._backend.invoke<string | null>('host_dialog_open', {
      directory: true,
      defaultPath: options.defaultPath,
    });
  }

  /**
   * 保存文件对话框。
   *
   * 返回选中文件路径，取消返回 null。
   *
   * ⚠️ 原生对话框尚未接入：**恒返回 `null`**（见文件头接入状态）。
   */
  async saveFile(options: SaveFileOptions = {}): Promise<string | null> {
    return this._backend.invoke<string | null>('host_dialog_save', {
      filters: options.filters,
      defaultName: options.defaultName,
      defaultPath: options.defaultPath,
    });
  }

  /**
   * 消息对话框（仅 OK 按钮）。
   *
   * ⚠️ 原生对话框尚未接入：**不弹窗**，直接返回（见文件头接入状态）。
   */
  async message(options: MessageOptions): Promise<void> {
    await this._backend.invoke('host_dialog_message', {
      title: options.title,
      message: options.message,
    });
  }

  /**
   * 错误对话框。
   *
   * ⚠️ 原生对话框尚未接入：**不弹窗**，直接返回（见文件头接入状态）。
   */
  async error(options: MessageOptions): Promise<void> {
    await this._backend.invoke('host_dialog_message', {
      title: options.title,
      message: options.message,
      kind: 'error',
    });
  }

  /**
   * 警告对话框。
   *
   * ⚠️ 原生对话框尚未接入：**不弹窗**，直接返回（见文件头接入状态）。
   */
  async warn(options: MessageOptions): Promise<void> {
    await this._backend.invoke('host_dialog_message', {
      title: options.title,
      message: options.message,
      kind: 'warning',
    });
  }

  /**
   * 确认对话框。
   *
   * 返回 true 表示确认，false 表示取消。
   *
   * ⚠️ 原生对话框尚未接入：**恒返回 `false`**。它**不代表用户取消**，更不会
   * 为 `true`——不得用它做真实用户确认（destructive 操作请自备确认 UI）。
   */
  async confirm(options: ConfirmOptions): Promise<boolean> {
    return this._backend.invoke<boolean>('host_dialog_confirm', {
      title: options.title,
      message: options.message,
      confirmLabel: options.confirmLabel ?? 'OK',
      cancelLabel: options.cancelLabel ?? 'Cancel',
    });
  }

  /**
   * 读取剪贴板文本。
   *
   * ⚠️ **不是系统剪贴板**：只读回本 App 自己 `clipboardWrite()` 写过的进程内
   * 字符串（未写过则为 `''`）；用户在其它 App 里复制的内容读不到。
   */
  async clipboardRead(): Promise<string> {
    return this._backend.invoke<string>('host_clipboard_read');
  }

  /**
   * 写入剪贴板文本。
   *
   * ⚠️ **不写入系统剪贴板**：只写进程内字符串，其它 App 粘贴不到。
   */
  async clipboardWrite(text: string): Promise<void> {
    await this._backend.invoke('host_clipboard_write', { text });
  }

  /**
   * 清理资源。
   */
  destroy(): void {
    // 无需清理（无状态）
  }
}

/**
 * 创建 DialogClient 实例。
 *
 * 便捷工厂函数。
 */
export function createDialogClient(options: { backend: Backend }): DialogClient {
  return new DialogClient(options);
}