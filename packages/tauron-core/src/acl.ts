/**
 * tauron 内层 ACL（设计文档 §3.2）
 *
 * 框架 ACL（插件级，动态）：在安装/升级/用户授权时授予权限。
 * plugin_invoke 内的权限检查。
 */

import {
  type PluginPermissionGrant,
  hasPermission,
  hasAllPermissions,
  missingPermissions,
  PluginErrorCode,
} from '@tauron/types';

/** 权限授予表 */
export type PermissionGrants = Map<string, PluginPermissionGrant>;

/**
 * 检查插件权限（§3.2 check_plugin_permission）
 *
 * @param pluginId - 插件 ID
 * @param method - 方法名
 * @param requiredPerms - 所需权限列表
 * @param grants - 授予表
 * @returns 错误码或 null（通过）
 */
export function checkPluginPermission(
  pluginId: string,
  method: string,
  requiredPerms: string[],
  grants: PermissionGrants,
): PluginErrorCode | null {
  const grant = grants.get(pluginId);
  if (!grant) {
    return PluginErrorCode.PLUGIN_NOT_FOUND;
  }

  for (const perm of requiredPerms) {
    if (!grant.permissions.includes(perm)) {
      return PluginErrorCode.PLUGIN_PERMISSION_DENIED;
    }
  }

  return null;
}

/**
 * 授予插件权限
 */
export function grantPermissions(
  grants: PermissionGrants,
  pluginId: string,
  permissions: string[],
  grantedBy: 'install' | 'upgrade' | 'user',
): void {
  const existing = grants.get(pluginId);
  if (existing) {
    const merged = new Set([...existing.permissions, ...permissions]);
    existing.permissions = [...merged];
    existing.grantedAt = new Date().toISOString();
    existing.grantedBy = grantedBy;
  } else {
    grants.set(pluginId, {
      pluginId,
      permissions: [...permissions],
      grantedAt: new Date().toISOString(),
      grantedBy,
    });
  }
}

/**
 * 撤销插件的所有权限
 */
export function revokePermissions(grants: PermissionGrants, pluginId: string): void {
  grants.delete(pluginId);
}

/**
 * 获取插件缺失的权限
 */
export function getMissingPermissions(
  grants: PermissionGrants,
  pluginId: string,
  requiredPerms: string[],
): string[] {
  const grant = grants.get(pluginId);
  if (!grant) return requiredPerms;
  return missingPermissions(grant, requiredPerms);
}
