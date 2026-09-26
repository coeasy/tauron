use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 权限授予记录（设计文档 §3.2）
///
/// 线上字段为 camelCase，与 TS `PluginPermissionGrant`
/// （`pluginId` / `grantedAt` / `grantedBy`）保持一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginPermissionGrant {
    /// 插件 ID
    pub plugin_id: String,
    /// 权限列表
    pub permissions: Vec<String>,
    /// 授予时间（ISO 8601）
    pub granted_at: String,
    /// 授予方式
    pub granted_by: GrantType,
}

/// 授予方式
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GrantType {
    Install,
    Upgrade,
    User,
}

/// 权限授予表
pub type PermissionGrants = HashMap<String, PluginPermissionGrant>;

/// 内层 ACL 权限检查（设计文档 §3.2）
///
/// **嵌入式扩展点**：默认分发主链（`HostState::handle_invoke`）不调用本函数——
/// 权限强制由应用层宿主执行（manifest 词表校验 `PermissionIndex::check_permitted`
/// + `authz` 档位判定）。嵌入式宿主可在自己的分发链上调用本函数启用内层检查。
///
/// @param plugin_id 插件 ID
/// @param method 方法名
/// @param required_perms 所需权限列表
/// @param grants 授予表
/// @returns 错误或 Ok(())
pub fn check_plugin_permission(
    plugin_id: &str,
    _method: &str,
    required_perms: &[String],
    grants: &PermissionGrants,
) -> Result<(), crate::PluginErrorCode> {
    let grant = grants.get(plugin_id).ok_or(crate::PluginErrorCode::PluginNotFound)?;

    for perm in required_perms {
        if !grant.permissions.contains(perm) {
            return Err(crate::PluginErrorCode::PluginPermissionDenied);
        }
    }

    Ok(())
}

/// 授予插件权限
pub fn grant_permissions(
    grants: &mut PermissionGrants,
    plugin_id: &str,
    permissions: Vec<String>,
    granted_by: GrantType,
) {
    use chrono::Utc;
    let granted_at = Utc::now().to_rfc3339();
    let existing = grants.get_mut(plugin_id);
    match existing {
        Some(grant) => {
            let mut merged = grant.permissions.clone();
            for perm in permissions {
                if !merged.contains(&perm) {
                    merged.push(perm);
                }
            }
            grant.permissions = merged;
            grant.granted_at = granted_at;
            grant.granted_by = granted_by;
        }
        None => {
            grants.insert(
                plugin_id.to_string(),
                PluginPermissionGrant {
                    plugin_id: plugin_id.to_string(),
                    permissions,
                    granted_at,
                    granted_by,
                },
            );
        }
    }
}

/// 撤销插件的所有权限
pub fn revoke_permissions(grants: &mut PermissionGrants, plugin_id: &str) {
    grants.remove(plugin_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_grants() -> PermissionGrants {
        let mut grants = PermissionGrants::new();
        grant_permissions(
            &mut grants,
            "com.example.test",
            vec![
                "store:read".to_string(),
                "http:fetch".to_string(),
                "fs:read-app-dirs".to_string(),
            ],
            GrantType::Install,
        );
        grants
    }

    #[test]
    fn test_check_permission_pass() {
        let grants = make_grants();
        let result = check_plugin_permission(
            "com.example.test",
            "format",
            &["store:read".to_string()],
            &grants,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_permission_plugin_not_found() {
        let grants = make_grants();
        let result = check_plugin_permission("com.unknown.test", "format", &[], &grants);
        assert_eq!(result.unwrap_err(), crate::PluginErrorCode::PluginNotFound);
    }

    #[test]
    fn test_check_permission_denied() {
        let grants = make_grants();
        let result = check_plugin_permission(
            "com.example.test",
            "format",
            &["clipboard:read".to_string()],
            &grants,
        );
        assert_eq!(result.unwrap_err(), crate::PluginErrorCode::PluginPermissionDenied);
    }

    #[test]
    fn test_check_permission_empty_required() {
        let grants = make_grants();
        let result = check_plugin_permission("com.example.test", "format", &[], &grants);
        assert!(result.is_ok());
    }

    #[test]
    fn test_grant_permissions_merge() {
        let mut grants = PermissionGrants::new();
        grant_permissions(&mut grants, "test", vec!["store:read".to_string()], GrantType::Install);
        grant_permissions(
            &mut grants,
            "test",
            vec!["http:fetch".to_string(), "store:read".to_string()],
            GrantType::Upgrade,
        );

        let grant = grants.get("test").unwrap();
        assert_eq!(grant.permissions.len(), 2); // No duplicates
        assert_eq!(grant.granted_by, GrantType::Upgrade);
    }

    #[test]
    fn test_revoke_permissions() {
        let mut grants = make_grants();
        revoke_permissions(&mut grants, "com.example.test");
        assert!(!grants.contains_key("com.example.test"));
    }

    #[test]
    fn test_grant_serde_roundtrip() {
        let grant = PluginPermissionGrant {
            plugin_id: "test".to_string(),
            permissions: vec!["store:read".to_string()],
            granted_at: "2026-01-01T00:00:00Z".to_string(),
            granted_by: GrantType::Install,
        };
        let json = serde_json::to_string(&grant).unwrap();
        let deserialized: PluginPermissionGrant = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.plugin_id, grant.plugin_id);
        assert_eq!(deserialized.granted_by, GrantType::Install);
    }

    /// 线上字段必须是 camelCase，与 TS `PluginPermissionGrant` 对齐。
    #[test]
    fn test_grant_wire_format_is_camel_case() {
        let grant = PluginPermissionGrant {
            plugin_id: "com.example.p".to_string(),
            permissions: vec!["store:read".to_string()],
            granted_at: "2026-01-01T00:00:00Z".to_string(),
            granted_by: GrantType::User,
        };
        let value = serde_json::to_value(&grant).unwrap();

        assert_eq!(value["pluginId"], "com.example.p");
        assert_eq!(value["grantedAt"], "2026-01-01T00:00:00Z");
        assert_eq!(value["grantedBy"], "user");
        assert!(value.get("plugin_id").is_none());

        // TS 侧产出的 camelCase 载荷必须可反序列化
        let back: PluginPermissionGrant = serde_json::from_value(value).unwrap();
        assert_eq!(back.granted_by, GrantType::User);
    }
}
