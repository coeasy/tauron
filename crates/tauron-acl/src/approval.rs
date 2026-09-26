//! §4.5 审批行构造与授予集校验。
//!
//! 核心约束：**审批 UI 文案与代码共享同一常量来源**（计划 §4.5）。
//! 人话文案一律取自权限词表 [`PermissionIndex`]，不允许前端另行硬编码。

use tauron_host::authz::{approval_hints, check_grants};
use tauron_host::error::{ErrorCode, HostError, HostResult};
use tauron_host::manifest::{Permission, PermissionIndex, Risk};

use crate::grant::{GrantEntry, GrantSet};

/// 审批 UI 的一行：一条权限 + 人话文案 + 勾选状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRow {
    pub permission: String,
    pub risk: Risk,
    /// 人话描述，**来自权限词表**（与代码同一来源）。
    pub human_text: String,
    /// scope 的可读形式（无 scope 的权限为 `None`）。
    pub scope: Option<String>,
    /// 是否默认勾选。**`risk: high` 一律默认不勾**（计划 §4.5）。
    pub default_checked: bool,
}

impl ApprovalRow {
    /// 该行的确认提示（高危行要求用户显式输入确认词）。
    pub fn confirmation_hint(&self) -> Option<&'static str> {
        match self.risk {
            Risk::High => Some("我理解该权限的能力边界并显式批准"),
            _ => None,
        }
    }
}

fn scope_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|i| i.as_str().map(str::to_string).unwrap_or_else(|| i.to_string()))
            .collect::<Vec<_>>()
            .join(", "),
        _ => v.to_string(),
    }
}

/// 构造审批行。
///
/// 文案来源链：`PermissionIndex` → `authz::approval_hints` → `ApprovalRow.human_text`，
/// 中间没有任何前端自定义文案。词表查不到的权限被 [`validate_grants`] 先拦掉。
pub fn build_approval_rows(grant_set: &GrantSet, index: &PermissionIndex) -> Vec<ApprovalRow> {
    let hints = approval_hints(index, &grant_set.permissions());
    hints
        .into_iter()
        .map(|h| ApprovalRow {
            default_checked: h.risk != Risk::High,
            human_text: h.description,
            permission: h.permission.clone(),
            risk: h.risk,
            scope: grant_set.scope_of(&h.permission).map(scope_to_string),
        })
        .collect()
}

/// 授予集校验（审批落盘前的最后一道闸）。
///
/// 依次检查：
/// 1. 每条权限必须在权限词表内（表外字符串安装期即拒绝，§4.2）；
/// 2. 风险档位必须与词表一致——**风险由词表决定，插件不得自报**；
/// 3. scope 声明与 `scoped` 标志对称；
/// 4. 不违反禁止授予清单（§2.1，四条硬规则）。
pub fn validate_grants(grant_set: &GrantSet, index: &PermissionIndex) -> HostResult<()> {
    for g in &grant_set.grants {
        let p = Permission::new(g.permission.as_str());

        index.check_permitted(&p)?;

        let expected_risk = index.risk_of(&g.permission).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("权限 `{}` 不在权限词表内", g.permission),
            )
        })?;
        if expected_risk != g.risk {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "权限 `{}` 的风险档位被篡改：词表为 `{}`，授予集声明 `{}`（风险由词表决定，插件不得自报）",
                    g.permission,
                    expected_risk.as_str(),
                    g.risk.as_str()
                ),
            ));
        }

        let declared_scoped = index.entry_of(&g.permission).map(|e| e.scoped).unwrap_or(false);
        if declared_scoped != g.scoped {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "权限 `{}` 的 scope 声明不对称：词表标 `scoped={declared_scoped}`，授予集标 `{}`",
                    g.permission, g.scoped
                ),
            ));
        }
    }

    check_grants(&grant_set.scopes, &grant_set.permissions())
}

/// 从 manifest 的权限声明生成一个"待审批"的授予集骨架（尚未签名）。
///
/// 风险档位与描述全部来自词表，因此这里不存在任何手写文案。
pub fn draft_grant_set(
    plugin_id: &str,
    plugin_version: &str,
    framework: &str,
    perms: &[Permission],
    scopes: &serde_json::Map<String, serde_json::Value>,
    index: &PermissionIndex,
    approver: &str,
    approved_at_unix: u64,
    revision: u32,
) -> HostResult<GrantSet> {
    let mut grants = Vec::new();
    for p in perms {
        let entry = index
            .entry_of(p.as_str())
            .ok_or_else(|| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("权限 `{}` 不在权限词表内（R3）", p.as_str()),
                )
            })?
            .clone();
        grants.push(GrantEntry {
            permission: p.as_str().to_string(),
            risk: entry.risk,
            description: entry.description,
            scoped: entry.scoped,
        });
    }
    Ok(GrantSet {
        schema_version: crate::grant::GRANT_SET_SCHEMA_VERSION,
        revision,
        plugin_id: plugin_id.to_string(),
        plugin_version: plugin_version.to_string(),
        framework: framework.to_string(),
        grants,
        scopes: scopes.clone(),
        approved_at_unix,
        approver: approver.to_string(),
    })
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grant::GrantEntry;

    const INDEX_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/permissions.index.json");

    fn index() -> PermissionIndex {
        PermissionIndex::load(INDEX_PATH).expect("仓库内的权限词表必须可加载")
    }

    fn gs(grants: &[(String, Risk, bool, Option<String>)]) -> GrantSet {
        let mut scopes = serde_json::Map::new();
        let mut entries = Vec::new();
        for (perm, risk, scoped, scope) in grants {
            entries.push(GrantEntry {
                permission: perm.clone(),
                risk: *risk,
                description: String::new(),
                scoped: *scoped,
            });
            if let Some(s) = scope {
                scopes.insert(
                    perm.clone(),
                    serde_json::Value::Array(vec![serde_json::Value::String(s.clone())]),
                );
            }
        }
        GrantSet {
            schema_version: crate::grant::GRANT_SET_SCHEMA_VERSION,
            revision: 1,
            plugin_id: "com.example.formatter".into(),
            plugin_version: "1.0.0".into(),
            framework: ">=2.0 <3.0".into(),
            grants: entries,
            scopes,
            approved_at_unix: 1_700_000_000,
            approver: "user".into(),
        }
    }

    #[test]
    fn rows_come_from_the_shared_index() {
        let idx = index();
        let g = gs(&[("store:allow-get".into(), Risk::Low, false, None)]);
        let rows = build_approval_rows(&g, &idx);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].permission, "store:allow-get");
        assert!(!rows[0].human_text.is_empty(), "人话文案必须来自词表");
        assert!(rows[0].default_checked, "low 档默认勾选");
        assert!(rows[0].confirmation_hint().is_none());
    }

    #[test]
    fn high_risk_rows_are_unchecked_by_default() {
        let idx = index();
        // `fs:allow-app-write-recursive` 在词表内且为 high（禁止清单内，但词表仍登记）。
        let g = gs(&[(
            "fs:allow-app-write-recursive".into(),
            Risk::High,
            true,
            Some("$APPCONFIG/**".to_string()),
        )]);
        let rows = build_approval_rows(&g, &idx);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].risk, Risk::High);
        assert!(!rows[0].default_checked, "high 档必须默认不勾");
        assert!(rows[0].confirmation_hint().is_some());
        assert_eq!(rows[0].scope.as_deref(), Some("$APPCONFIG/**"));
    }

    #[test]
    fn rows_cover_every_grant_in_order() {
        let idx = index();
        let g = gs(&[
            ("store:allow-get".into(), Risk::Low, false, None),
            (
                "http:allow-fetch".into(),
                Risk::Elevated,
                true,
                Some("https://api.example.com/**".to_string()),
            ),
        ]);
        let rows = build_approval_rows(&g, &idx);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].permission, "store:allow-get");
        assert_eq!(rows[1].permission, "http:allow-fetch");
        assert!(rows[1].default_checked);
        assert!(rows[1].scope.as_deref().unwrap().contains("api.example.com"));
    }

    #[test]
    fn validate_accepts_conformant_set() {
        let idx = index();
        let mut g = gs(&[(
            "http:allow-fetch".into(),
            Risk::Elevated,
            true,
            Some("https://api.example.com/**".to_string()),
        )]);
        g.grants[0].description = "占位".into();
        assert!(validate_grants(&g, &idx).is_ok());
    }

    #[test]
    fn validate_rejects_table_outside_permission() {
        let idx = index();
        let g = gs(&[("fs:allow-delete-everything".into(), Risk::Low, false, None)]);
        let e = validate_grants(&g, &idx).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn validate_rejects_self_reported_risk() {
        // 插件声称一个 elevated 权限是 low → 拒绝。
        let idx = index();
        let g = gs(&[(
            "http:allow-fetch".into(),
            Risk::Low,
            true,
            Some("https://api.example.com/**".to_string()),
        )]);
        let e = validate_grants(&g, &idx).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("篡改"));
        assert!(e.message.contains("不得自报"));
    }

    #[test]
    fn validate_rejects_scope_asymmetry() {
        // 词表标 scoped=true，授予集标 false。
        let idx = index();
        let g = gs(&[("http:allow-fetch".into(), Risk::Elevated, false, None)]);
        let e = validate_grants(&g, &idx).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("不对称"));
    }

    #[test]
    fn validate_rejects_forbidden_grant_with_guidance() {
        let idx = index();
        let g = gs(&[("shell:allow-execute".into(), Risk::High, false, None)]);
        let e = validate_grants(&g, &idx).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("走 A 类"));
    }

    #[test]
    fn validate_rejects_appconfig_write_recursive() {
        let idx = index();
        let g = gs(&[(
            "fs:allow-app-write-recursive".into(),
            Risk::High,
            true,
            Some("$APPCONFIG/**".to_string()),
        )]);
        let e = validate_grants(&g, &idx).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("走 A 类"));
    }

    #[test]
    fn validate_rejects_fs_scope_outside_app_dirs() {
        let idx = index();
        let g =
            gs(&[("fs:allow-app-read".into(), Risk::Low, true, Some("C:/Windows/**".to_string()))]);
        let e = validate_grants(&g, &idx).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("应用目录引用"));
    }

    #[test]
    fn draft_grant_set_takes_risk_and_text_from_index() {
        let idx = index();
        let mut scopes = serde_json::Map::new();
        scopes.insert(
            "http:allow-fetch".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String(
                "https://api.example.com/**".into(),
            )]),
        );
        let perms = vec![Permission::new("store:allow-get"), Permission::new("http:allow-fetch")];
        let draft = draft_grant_set(
            "com.example.formatter",
            "1.2.3",
            ">=2.0 <3.0",
            &perms,
            &scopes,
            &idx,
            "user",
            1_700_000_000,
            1,
        )
        .unwrap();

        assert_eq!(draft.grants.len(), 2);
        // 风险档位必须与词表一致，不允许调用方手写。
        assert_eq!(draft.grants[1].risk, idx.risk_of("http:allow-fetch").unwrap());
        assert!(!draft.grants[1].description.is_empty(), "描述必须来自词表");
        // 产物通过完整校验。
        assert!(validate_grants(&draft, &idx).is_ok());
        // 审批行数量与授予集一致。
        assert_eq!(build_approval_rows(&draft, &idx).len(), 2);
    }

    #[test]
    fn draft_grant_set_rejects_unknown_permission() {
        let idx = index();
        let e = draft_grant_set(
            "com.a",
            "1.0.0",
            ">=2.0 <3.0",
            &[Permission::new("nope:allow-nope")],
            &serde_json::Map::new(),
            &idx,
            "user",
            1,
            1,
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("不在权限词表内"));
    }

    #[test]
    fn rows_reject_unknown_permission_silently_but_validate_catches_it() {
        // build_approval_rows 会跳过词表外的权限（approval_hints 的 filter_map），
        // 因此必须靠 validate_grants 兜底，不能让"UI 少显示一行"绕过校验。
        let idx = index();
        let g = gs(&[("nope:allow-nope".into(), Risk::Low, false, None)]);
        let rows = build_approval_rows(&g, &idx);
        assert!(rows.is_empty(), "词表外权限不会产出审批行");
        assert!(validate_grants(&g, &idx).is_err(), "但校验必须拦下它");
    }
}
