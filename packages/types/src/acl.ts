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
 * 这是**内层（框架 ACL）**的词表：`PluginPermissionGrant` 的 `permissions` 用它命名。
 * 轮 58 把话说实：这份词表**在仓库内没有任何执行者**——Rust 侧 `check_plugin_permission`
 * 按字符串精确比对，不校验标识是否在此表内；审批 UI 的人话文案自轮 55 起由 Rust
 * `tauron_acl::build_approval_rows` 单源生成并经线上传给前端，**不读这张表**。
 * 因此它的当前身份是「SDK 对外声明的内层词表」，不是「已被执行的能力」。
 * 另注意与应用层 manifest 权限区分——插件 manifest 的 `permissions` 只能取自随框架发版的
 * `schema/permissions.index.json`（Tauri 标识符，如 `store:allow-get`），表外即安装失败
 * （Rust `manifest.validate` 强制）。两套词表分层且互不通用；该 index 目前是**手工维护**的
 * （仓库内无生成器），仅有门禁做校验。
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

/**
 * 获取缺失的权限列表。内层词表里唯一有真实读者的工具——
 * `packages/tauron-core/src/acl.ts` 的 `getMissingPermissions` 走它。
 */
export function missingPermissions(
  grant: PluginPermissionGrant,
  requiredPermissions: string[],
): string[] {
  return requiredPermissions.filter((perm) => !grant.permissions.includes(perm));
}
