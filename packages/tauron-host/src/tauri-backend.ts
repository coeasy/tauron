// ──────────────────────────────────────────────────────────────────────────
// Tauri 后端：**本包内唯一允许 import `@tauri-apps/api` 的文件**。
//
// 门禁 §8-1 用静态扫描断言这一点（见 `src/gates.test.ts`）。
// 其余模块只依赖 {@link Backend} 接口，因此可以在无 Tauri 依赖的
// 环境（契约测试、SSR 冒烟、类型检查）下完整运行。
// ──────────────────────────────────────────────────────────────────────────
import { invoke, Channel } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';

import type { Backend, ChannelPort, Principal } from './backend.js';

/** 插件 webview 的 label 前缀，与 Rust 侧 `IDENTITY_LABEL_PREFIX` 必须一致。 */
export const IDENTITY_LABEL_PREFIX = 'plugin-';

/**
 * 从 webview label 解析插件身份 id。
 *
 * 计划 §2.1（ADR-17）：`self` 档命令的 pluginId **只**从 label 取，
 * 调用方传入的 id 一律忽略——这是防跨插件冒充的唯一可信身份来源。
 */
export function pluginIdFromLabel(label: string): string | null {
  return label.startsWith(IDENTITY_LABEL_PREFIX)
    ? label.slice(IDENTITY_LABEL_PREFIX.length)
    : null;
}

/**
 * 从 webview label 解析身份主体（R4，与 Rust `authz::resolve_principal` 对应）。
 *
 * 畸形 `plugin-` label（前缀后为空）归 `'invalid'`——**不得**降级为
 * `'main-window'`，否则伪造畸形 label 即提权。更深层的 id 合法性（例如含 `:`）
 * 由宿主侧 `PluginId::new` 判定，宿主会拒绝并在线上返回 `E_AUTH_DENIED`。
 */
export function principalFromLabel(label: string): Principal {
  const id = pluginIdFromLabel(label);
  if (id === null) return { kind: 'main-window', origin: currentOrigin() };
  if (id.length === 0) return { kind: 'invalid', label };
  return { kind: 'plugin', id };
}

/** 当前 webview 的自报 origin（诊断用；**非**可信来源，强制校验在宿主侧）。 */
function currentOrigin(): string | null {
  try {
    return typeof location !== 'undefined' && location.origin ? location.origin : null;
  } catch {
    return null;
  }
}

/**
 * 宿主注册的**完整**命令面（与 Rust `tauron_generate_handler!` 宏一一对应，
 * 由 `@tauron/contract-tests` 的 wire-gate 锁死：宏里每条命令都必须出现在
 * 这里）。
 *
 * 断链回归：此前这里只列 10 条框架命令——生产后端的 `capabilities()` 因此
 * 对其余命令误报未注册，`available('host_window_minimize')` 等在真实宿主上
 * 全部返回 `false`，按能力矩阵做特性开关的应用会把功能全部藏掉。
 * Tauri 不提供命令注册表自省 API，这份清单就是「壳声明注册」的全集；
 * 未注册命令的实际失败会在 invoke 时以结构化错误暴露（见 normalizeError）。
 */
const FRAMEWORK_COMMANDS = [
  // 框架服务（插件 webview 调用的 self/admin 档）
  'host_plugin_call',
  'host_call_end',
  'host_cancel',
  'host_lifecycle_report',
  'host_contributes_register',
  'host_events_publish',
  'host_events_subscribe',
  'host_events_unsubscribe',
  'host_events_drain',
  'host_registry_list',
  'host_registry_admin',
  // 流式帧（R5/P0-1）：三命令成组——缺一条就会让某条流永远等不到终帧。
  'host_stream_open',
  'host_stream_write',
  'host_stream_close',
  // 进程插件运行时（P0-2）：spawn + 健康/租约查询配对。
  'host_runtime_spawn',
  'host_runtime_health',
  // 注册表（主窗）
  'host_registry_list_all',
  // 设置
  'host_settings_get',
  'host_settings_set',
  // 设置迁移（R7）：承接旧文档 + 显式迁移。此前是**没有线上入口**的纯函数，
  // 只留在 Rust 单测里；这两个名字与 Rust 宏、ShellClient 方法三处必须一致。
  'host_settings_adopt_legacy',
  'host_settings_migrate',
  // 通知（写 + 读配对）
  'host_notify',
  'host_notifications_list',
  'host_notifications_read',
  // 启动恢复
  'host_recover_boot',
  'host_recover_report',
  'host_recover_trial_enable',
  // 商城 / 品牌
  'host_market_check',
  'host_market_download',
  'host_market_install',
  'host_brand_info',
  // i18n
  'host_i18n_t',
  'host_i18n_t_params',
  'host_i18n_set_locale',
  'host_i18n_load',
  'host_i18n_stats',
  'host_i18n_cleanup_plugin',
  // 贡献（读取）
  'host_contributes_list',
  // 窗口
  'host_window_minimize',
  'host_window_maximize',
  'host_window_restore',
  'host_window_close',
  'host_window_quit',
  'host_window_set_position',
  'host_window_set_size',
  // 窗口（R8）：relaunch = 先对账恢复阶段再重启；create 必须 `plugin-<id>` 且
  // 该 id 已在注册表内（故它绑 PluginRuntimeState，属插件域命令集）。
  'host_window_relaunch',
  'host_window_create',
  // 剪贴板
  'host_clipboard_write',
  'host_clipboard_read',
  // 深链
  'host_deep_link_register',
  // 对话框
  'host_dialog_open',
  'host_dialog_save',
  'host_dialog_message',
  'host_dialog_confirm',
] as const;

export type FrameworkCommand = (typeof FRAMEWORK_COMMANDS)[number];

/**
 * 真实 Tauri 后端。
 *
 * `commandPrefix` 默认 `'plugin:tauron|'`——Tauri 的命令级 ACL 通常以
 * `plugin:<name>|<cmd>` 形式注册。若宿主把命令注册在根命名空间，传 `''`。
 */
export class TauriBackend implements Backend {
  private readonly prefix: string;
  private readonly caps: ReadonlySet<string>;

  constructor(options: { commandPrefix?: string } = {}) {
    this.prefix = options.commandPrefix ?? 'plugin:tauron|';
    // Tauri 不提供命令注册表自省 API，这里报"壳声明注册"的全集。
    // 命令未注册的实际失败会在 invoke 时以结构化错误暴露（见 normalizeError）。
    this.caps = new Set<string>(FRAMEWORK_COMMANDS);
  }

  private full(cmd: string): string {
    return cmd.startsWith(this.prefix) ? cmd : this.prefix + cmd;
  }

  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    return invoke<T>(this.full(cmd), args);
  }

  async listen(event: string, handler: (payload: unknown) => void): Promise<() => void> {
    return listen(event, (e) => handler(e.payload));
  }

  channel<T = unknown>(): ChannelPort<T> {
    // R5：不再需要 `as unknown as` 掩盖不一致——`ChannelPort` 现在只声明真实
    // Tauri `Channel` 拥有的成员（`onmessage` / `id`），因此这个赋值是**可检查**的。
    // 之前那个 cast 正是「类型上成立、线上不成立」的来源。
    return new Channel<T>();
  }

  principal(): Principal {
    try {
      return principalFromLabel(getCurrentWebview().label);
    } catch {
      // 不在 webview 上下文（SSR / 主进程 / 测试）：无 webview 身份，按主窗主体
      // 返回且 origin 未知；宿主侧仍按真实 label 独立判定，不受此自报影响。
      return { kind: 'main-window', origin: null };
    }
  }

  pluginId(): string | null {
    const p = this.principal();
    return p.kind === 'plugin' ? p.id : null;
  }

  capabilities(): ReadonlySet<string> {
    return this.caps;
  }
}
