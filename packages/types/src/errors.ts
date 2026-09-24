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
