/**
 * tauron 错误码规范（设计文档 §2.2）
 *
 * SC-xxxx 格式：
 * - 0xxx: 插件系统错误
 * - 1xxx: 权限错误
 * - 2xxx: 通信错误
 * - 3xxx: 插件实现错误
 * - 9xxx: 系统错误
 */

export enum PluginErrorCode {
  // ---- 插件系统错误（0xxx）----
  PLUGIN_NOT_FOUND = 'SC-0001',
  PLUGIN_DISABLED = 'SC-0002',
  PLUGIN_ERRORED = 'SC-0003',

  // ---- 权限错误（1xxx）----
  PERMISSION_DENIED = 'SC-1001',
  PLUGIN_PERMISSION_DENIED = 'SC-1002',
  CAPABILITY_REQUIRED = 'SC-1003',

  // ---- 通信错误（2xxx）----
  TIMEOUT = 'SC-2001',
  CANCELLED = 'SC-2002',
  INVALID_PAYLOAD = 'SC-2003',
  CHANNEL_BROKEN = 'SC-2004',

  // ---- 插件实现错误（3xxx）----
  PLUGIN_PANIC = 'SC-3001',
  PLUGIN_OOM = 'SC-3002',
  PLUGIN_EXITED = 'SC-3003',

  // ---- 系统错误（9xxx）----
  INTERNAL = 'SC-9001',
}

/** 可重试错误码集合 */
export const RETRYABLE_ERROR_CODES: ReadonlySet<PluginErrorCode> = new Set([
  PluginErrorCode.TIMEOUT,
  PluginErrorCode.CHANNEL_BROKEN,
  PluginErrorCode.INTERNAL,
]);

/** 错误码分类 */
export type ErrorCategory = 'plugin' | 'permission' | 'communication' | 'implementation' | 'system';

export const ERROR_CATEGORIES: Record<PluginErrorCode, ErrorCategory> = {
  [PluginErrorCode.PLUGIN_NOT_FOUND]: 'plugin',
  [PluginErrorCode.PLUGIN_DISABLED]: 'plugin',
  [PluginErrorCode.PLUGIN_ERRORED]: 'plugin',
  [PluginErrorCode.PERMISSION_DENIED]: 'permission',
  [PluginErrorCode.PLUGIN_PERMISSION_DENIED]: 'permission',
  [PluginErrorCode.CAPABILITY_REQUIRED]: 'permission',
  [PluginErrorCode.TIMEOUT]: 'communication',
  [PluginErrorCode.CANCELLED]: 'communication',
  [PluginErrorCode.INVALID_PAYLOAD]: 'communication',
  [PluginErrorCode.CHANNEL_BROKEN]: 'communication',
  [PluginErrorCode.PLUGIN_PANIC]: 'implementation',
  [PluginErrorCode.PLUGIN_OOM]: 'implementation',
  [PluginErrorCode.PLUGIN_EXITED]: 'implementation',
  [PluginErrorCode.INTERNAL]: 'system',
};

/** 错误码 → 人类可读描述 */
export const ERROR_MESSAGES: Record<PluginErrorCode, string> = {
  [PluginErrorCode.PLUGIN_NOT_FOUND]: 'Plugin not found or not registered',
  [PluginErrorCode.PLUGIN_DISABLED]: 'Plugin is disabled',
  [PluginErrorCode.PLUGIN_ERRORED]: 'Plugin is in error state',
  [PluginErrorCode.PERMISSION_DENIED]: 'Capability not authorized',
  [PluginErrorCode.PLUGIN_PERMISSION_DENIED]: 'Plugin-level permission not granted',
  [PluginErrorCode.CAPABILITY_REQUIRED]: 'Required capability not available',
  [PluginErrorCode.TIMEOUT]: 'Call timed out',
  [PluginErrorCode.CANCELLED]: 'Call was cancelled',
  [PluginErrorCode.INVALID_PAYLOAD]: 'Parameter validation failed',
  [PluginErrorCode.CHANNEL_BROKEN]: 'IPC channel broken',
  [PluginErrorCode.PLUGIN_PANIC]: 'Plugin internal panic/crash',
  [PluginErrorCode.PLUGIN_OOM]: 'Plugin out of memory',
  [PluginErrorCode.PLUGIN_EXITED]: 'Process plugin exited unexpectedly',
  [PluginErrorCode.INTERNAL]: 'Internal error',
};

/**
 * 判断错误码是否可重试
 */
export function isRetryable(code: PluginErrorCode): boolean {
  return RETRYABLE_ERROR_CODES.has(code);
}

/**
 * 获取错误分类
 */
export function errorCategory(code: PluginErrorCode): ErrorCategory {
  return ERROR_CATEGORIES[code];
}

/**
 * 获取错误的人类可读描述
 */
export function errorMessage(code: PluginErrorCode): string {
  return ERROR_MESSAGES[code];
}

/**
 * 验证是否为有效的错误码
 */
export function isValidErrorCode(code: string): code is PluginErrorCode {
  return Object.values(PluginErrorCode).includes(code as PluginErrorCode);
}

// ──────────────────────────────────────────────────────────────────────────
// 码形态（轮 35）：仓里有**两套**错误词表（宿主 `E_*` / 框架层 `SC-####`），
// 穿越 iframe 桥时最常见的破坏是"把带码的失败换成通用码"——码一丢，调用方
// 就只剩一句 "Internal error"，宿主侧按事实分码的工作全部白做。
//
// 层归属按 `docs/architecture/overview.md` 的口径：`SC-####` 属**框架层**（由
// `tauron-shell` 与 `@tauron/types` 共用），`E_*` 属**应用层**宿主底座。符号名里
// 的 `APP_LAYER_` 前缀是 1.0 公开面遗留命名——注释按文档口径改，导出名不改。
//
// 形态识别放在这一层，因为 `@tauron/types` 是两套词表都能触到的最低依赖点：
// `@tauron/host`（宿主词表）与 `@tauron/plugin-sdk`（框架层词表、且**不得**
// 依赖 host，否则整个宿主客户端会打进插件包）都用这同一份判据。判据只认
// **形态**，不认"这条码在不在某套词表里"——后者仍是各层自己的清单。
// ──────────────────────────────────────────────────────────────────────────

/** 框架层错误码形态（`SC-####`）。 */
export const APP_LAYER_ERROR_CODE_PATTERN = /^SC-\d{4}$/;

/** 是否为框架层错误码形态（识别形态 ≠ 接受语义）。 */
export function isAppLayerErrorCode(v: string): boolean {
  return APP_LAYER_ERROR_CODE_PATTERN.test(v);
}

/**
 * 是否"像错误码"的串：宿主 `E_*` 前缀或框架层 `SC-####`。
 *
 * 只做形态判断：宿主码的**合法集**在 `@tauron/host` 的 `HOST_ERROR_CODES`，
 * 框架层码的合法集是上面的 {@link PluginErrorCode}——这里都不重复。
 */
export function isCodeLike(v: string): boolean {
  return v.startsWith('E_') || isAppLayerErrorCode(v);
}

/**
 * 从任意串里抽出第一个码形态（宿主 `E_*` 优先匹配整词，再试 `SC-####`）。
 * 找不到返回 `null`——调用方据此决定"保码"还是"落到通用码"。
 */
export function extractCodeLike(s: string): string | null {
  return s.match(/\b(E_[A-Z0-9_]+|SC-\d{4})\b/)?.[1] ?? null;
}
