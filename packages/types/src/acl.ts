/**
 * tauron 权限模型（设计文档 §3）
 *
 * 双层 ACL：
 * - 外层：Tauri ACL（命令级，静态）— 在 capabilities/default.json 中声明
 * - 内层：框架 ACL（插件级，动态）— 在安装/升级/用户授权时授予
 */

/** 权限授予记录 */
export interface PluginPermissionGrant {
  /** 插件 ID */
  pluginId: string;
  /** 权限列表（如 ["store:read", "fs:read-app-dirs", "http:fetch"]） */
  permissions: string[];
  /** 授予时间（ISO 8601） */
  grantedAt: string;
  /** 授予方式 */
  grantedBy: 'install' | 'upgrade' | 'user';
}

/**
 * 权限粒度清单（设计文档 §3.3）。
 *
 * 这是**内层（框架 ACL）**的描述词表：`PluginPermissionGrant` 的 permissions、
 * 审批 UI 文案与 `isValidPermission` 都用它。注意与应用层 manifest 权限区分——
 * 插件 manifest 的 `permissions` 只能取自机器生成的
 * `schema/permissions.index.json`（Tauri 标识符，如 `store:allow-get`），
 * 表外即安装失败（Rust `manifest.validate` 强制）。两套词表分层且互不通用。
 */
export const PERMISSION_GRANULARITY: Readonly<Record<string, string>> = {
  // 存储
  'store:read': '读取键值存储',
  'store:write': '写入键值存储',
  // 文件
  'fs:read-app-dirs': '读取应用目录',
  'fs:read-documents': '读取文档目录',
  'fs:write-app-dirs': '写入应用目录',
  // 网络
  'http:fetch': '发起 HTTP 请求（受 URL 白名单限制）',
  'websocket:connect': '建立 WebSocket 连接',
  // 系统
  'clipboard:read': '读取剪贴板',
  'clipboard:write': '写入剪贴板',
  'notification:send': '发送系统通知',
  'shell:execute': '执行命令（受白名单限制）',
  'dialog:open': '打开文件对话框',
  'dialog:save': '保存文件对话框',
  'opener:open-url': '打开 URL',
  'opener:open-path': '打开文件路径',
  // 宿主 API
  'host:emit-event': '发布事件',
  'host:get-settings': '读取宿主设置',
  'host:get-system-info': '读取系统信息',
  // 进程插件专属
  'process:spawn': '启动子进程',
  // WASM 专属
  'wasm:memory-limit': 'WASM 内存上限（MB）',
  'wasm:timeout': 'WASM 执行超时（ms）',
};

/** 权限风险等级 */
export type PermissionRisk = 'low' | 'elevated' | 'high';

/** 权限元数据 */
export interface PermissionMeta {
  description: string;
  risk: PermissionRisk;
}

/**
 * 检查插件是否拥有指定权限
 */
export function hasPermission(
  grant: PluginPermissionGrant,
  permission: string,
): boolean {
  return grant.permissions.includes(permission);
}

/**
 * 检查插件是否拥有所需的所有权限
 */
export function hasAllPermissions(
  grant: PluginPermissionGrant,
  requiredPermissions: string[],
): boolean {
  return requiredPermissions.every((perm) => grant.permissions.includes(perm));
}

/**
 * 获取缺失的权限列表
 */
export function missingPermissions(
  grant: PluginPermissionGrant,
  requiredPermissions: string[],
): string[] {
  return requiredPermissions.filter((perm) => !grant.permissions.includes(perm));
}

/**
 * 验证权限标识是否有效
 */
export function isValidPermission(permission: string): boolean {
  return permission in PERMISSION_GRANULARITY;
}
