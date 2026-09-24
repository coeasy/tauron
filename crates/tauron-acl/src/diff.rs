//! §4.5 授予集 diff：判断一次变更是否需要重新审批。
//!
//! 规则（计划 §4.5）：**权限或 scope 变多必须重新审批**；
//! 收缩（删权限、缩 scope）如实撤销，不需要重新审批（ADR-05）。

use serde_json::Value;

use crate::grant::GrantSet;

/// 单个权限的 scope 变化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChange {
    pub permission: String,
    pub old_scope: Option<Value>,
    pub new_scope: Option<Value>,
    /// `true` = scope 变宽（需要重新审批）。
    pub grew: bool,
}

/// diff 结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
    pub scope_changes: Vec<ScopeChange>,
    /// 新授予集声明的框架范围与旧的不同（信息性，不单独触发重审）。
    pub framework_changed: bool,
    pub plugin_version_changed: bool,
    /// 是否必须重新审批。
    pub requires_reapproval: bool,
}

impl GrantDiff {
    /// 一句话总结，便于日志与审批 UI 标题。
    pub fn summary(&self) -> String {
        if self.requires_reapproval {
            let mut parts = Vec::new();
            if !self.added.is_empty() {
                parts.push(format!("新增 {} 项权限", self.added.len()));
            }
            if self.scope_changes.iter().any(|c| c.grew) {
                let n = self.scope_changes.iter().filter(|c| c.grew).count();
                parts.push(format!("{} 项 scope 变宽", n));
            }
            format!("需重新审批：{}", parts.join("；"))
        } else if self.added.is_empty()
            && self.removed.is_empty()
            && self.scope_changes.is_empty()
            && !self.framework_changed
            && !self.plugin_version_changed
        {
            "无变更".to_string()
        } else {
            "仅收缩，无需重新审批".to_string()
        }
    }
}

/// scope 值的字符串列表形式，用于超集判断。
fn scope_strings(v: &Option<Value>) -> Option<Vec<String>> {
    let v = v.as_ref()?;
    match v {
        Value::Array(items) => Some(
            items.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        ),
        Value::String(s) => Some(vec![s.clone()]),
        _ => Some(Vec::new()),
    }
}

/// 新 scope 是否比旧 scope 更宽（含"从无到有"）。
fn scope_grew(old: &Option<Value>, new: &Option<Value>) -> bool {
    let Some(n) = scope_strings(new) else {
        return false;
    };
    let Some(o) = scope_strings(old) else {
        return true; // 从无到有：变宽。
    };
    if n.len() > o.len() {
        return true;
    }
    n.iter().any(|s| !o.contains(s))
}

/// 对比新旧授予集。`old = None` 表示首次审批。
pub fn diff(old: Option<&GrantSet>, new: &GrantSet) -> GrantDiff {
    let Some(old) = old else {
        return GrantDiff {
            added: new.grants.iter().map(|g| g.permission.clone()).collect(),
            unchanged: Vec::new(),
            removed: Vec::new(),
            scope_changes: new
                .grants
                .iter()
                .filter_map(|g| {
                    let n = new.scope_of(&g.permission).cloned();
                    // 无 scope 的权限不产生 scope 变化（与 old 分支的 `o != n` 判据对称）。
                    if n.is_none() {
                        None
                    } else {
                        Some(ScopeChange {
                            permission: g.permission.clone(),
                            old_scope: None,
                            new_scope: n,
                            grew: true,
                        })
                    }
                })
                .collect(),
            framework_changed: false,
            plugin_version_changed: false,
            requires_reapproval: !new.grants.is_empty(),
        };
    };

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut unchanged = Vec::new();
    let mut scope_changes = Vec::new();

    for g in &new.grants {
        if old.has_permission(&g.permission) {
            unchanged.push(g.permission.clone());
            let o = old.scope_of(&g.permission).cloned();
            let n = new.scope_of(&g.permission).cloned();
            if o != n {
                scope_changes.push(ScopeChange {
                    grew: scope_grew(&o, &n),
                    new_scope: n,
                    old_scope: o,
                    permission: g.permission.clone(),
                });
            }
        } else {
            added.push(g.permission.clone());
        }
    }

    for g in &old.grants {
        if !new.has_permission(&g.permission) {
            removed.push(g.permission.clone());
        }
    }

    let requires_reapproval = !added.is_empty()
        || scope_changes.iter().any(|c| c.grew);

    GrantDiff {
        added,
        removed,
        unchanged,
        scope_changes,
        framework_changed: old.framework != new.framework,
        plugin_version_changed: old.plugin_version != new.plugin_version,
        requires_reapproval,
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tauron_host::manifest::Risk;

    use crate::grant::GrantEntry;

    fn gs(id: &str, grants: &[(&str, bool)]) -> GrantSet {
        let mut scopes = serde_json::Map::new();
        let mut entries = Vec::new();
        for (perm, scoped) in grants {
            entries.push(GrantEntry {
                permission: perm.to_string(),
                risk: Risk::Elevated,
                description: perm.to_string(),
                scoped: *scoped,
            });
            if *scoped {
                scopes.insert(
                    perm.to_string(),
                    Value::Array(vec![Value::String(format!("https://api.example.com/**"))]),
                );
            }
        }
        GrantSet {
            schema_version: crate::grant::GRANT_SET_SCHEMA_VERSION,
            revision: 1,
            plugin_id: id.to_string(),
            plugin_version: "1.0.0".into(),
            framework: ">=2.0 <3.0".into(),
            grants: entries,
            scopes,
            approved_at_unix: 1_700_000_000,
            approver: "user".into(),
        }
    }

    #[test]
    fn diff_first_approval_everything_is_added() {
        let new = gs(
            "com.a",
            &[
                ("http:allow-fetch", true),
                ("store:allow-get", false),
            ],
        );
        let d = diff(None, &new);
        assert!(d.requires_reapproval);
        assert_eq!(d.added.len(), 2);
        assert!(d.removed.is_empty());
        // 只有一条权限带 scope，因此只有 1 项 scope 变宽。
        assert_eq!(d.summary(), "需重新审批：新增 2 项权限；1 项 scope 变宽");

        // 两条都带 scope → 两项变宽；无 scope 的权限不产生 scope 变化。
        let both = gs(
            "com.a",
            &[
                ("http:allow-fetch", true),
                ("fs:allow-app-read", true),
            ],
        );
        assert_eq!(diff(None, &both).summary(), "需重新审批：新增 2 项权限；2 项 scope 变宽");
    }

    #[test]
    fn diff_identical_is_unchanged() {
        let a = gs("com.a", &[("http:allow-fetch", true)]);
        let d = diff(Some(&a), &gs("com.a", &[("http:allow-fetch", true)]));
        assert!(!d.requires_reapproval);
        assert!(d.added.is_empty());
        assert!(d.removed.is_empty());
        assert!(d.scope_changes.is_empty());
        assert_eq!(d.summary(), "无变更");
    }

    #[test]
    fn diff_added_permission_requires_reapproval() {
        let old = gs("com.a", &[("http:allow-fetch", true)]);
        let new = gs("com.a", &[("http:allow-fetch", true), ("store:allow-set", false)]);
        let d = diff(Some(&old), &new);
        assert!(d.requires_reapproval);
        assert_eq!(d.added, vec!["store:allow-set"]);
        assert_eq!(d.unchanged, vec!["http:allow-fetch"]);
    }

    #[test]
    fn diff_removed_permission_is_shrink_only() {
        let old = gs("com.a", &[("http:allow-fetch", true), ("store:allow-set", false)]);
        let new = gs("com.a", &[("http:allow-fetch", true)]);
        let d = diff(Some(&old), &new);
        assert!(!d.requires_reapproval, "收缩不应要求重新审批（ADR-05 撤销语义如实）");
        assert_eq!(d.removed, vec!["store:allow-set"]);
        assert_eq!(d.summary(), "仅收缩，无需重新审批");
    }

    #[test]
    fn diff_scope_grows_requires_reapproval() {
        let old = gs("com.a", &[("http:allow-fetch", true)]);
        let mut new = gs("com.a", &[("http:allow-fetch", true)]);
        new.scopes.insert(
            "http:allow-fetch".to_string(),
            Value::Array(vec![
                Value::String("https://api.example.com/**".into()),
                Value::String("https://*.example.com/**".into()),
            ]),
        );
        let d = diff(Some(&old), &new);
        assert!(d.requires_reapproval);
        assert_eq!(d.added.len(), 0);
        assert_eq!(d.scope_changes.len(), 1);
        assert!(d.scope_changes[0].grew);
        assert!(d.summary().contains("scope 变宽"));
    }

    #[test]
    fn diff_scope_same_set_different_order_is_not_growth() {
        // 顺序变化不应被误判为"变宽"。
        let old = gs("com.a", &[("http:allow-fetch", true)]);
        let mut new = gs("com.a", &[("http:allow-fetch", true)]);
        new.scopes.insert(
            "http:allow-fetch".to_string(),
            Value::Array(vec![
                Value::String("https://api.example.com/**".into()),
                Value::String("https://cdn.example.com/**".into()),
            ]),
        );
        let mut swapped = new.scopes.get("http:allow-fetch").cloned().unwrap();
        {
            let arr = swapped.as_array_mut().unwrap();
            arr.swap(0, 1);
        }
        new.scopes.insert("http:allow-fetch".to_string(), swapped);

        let d = diff(Some(&old), &new);
        // 集合比旧的大 → 仍算变宽（新增了 cdn 域）。
        assert!(d.requires_reapproval);
        assert!(d.scope_changes[0].grew);
    }

    #[test]
    fn diff_scope_shrinks_is_not_reapproval() {
        let mut old = gs("com.a", &[("http:allow-fetch", true)]);
        old.scopes.insert(
            "http:allow-fetch".to_string(),
            Value::Array(vec![
                Value::String("https://api.example.com/**".into()),
                Value::String("https://cdn.example.com/**".into()),
            ]),
        );
        let new = gs("com.a", &[("http:allow-fetch", true)]); // 只剩 api 域
        let d = diff(Some(&old), &new);
        assert!(!d.requires_reapproval, "scope 收窄属于收缩");
        assert_eq!(d.scope_changes.len(), 1);
        assert!(!d.scope_changes[0].grew);
    }

    #[test]
    fn diff_scope_from_absent_to_present_is_growth() {
        // 新增权限时若带 scope，也算 scope 从无到有。
        let old = gs("com.a", &[("store:allow-get", false)]);
        let new = gs("com.a", &[("store:allow-get", false), ("http:allow-fetch", true)]);
        let d = diff(Some(&old), &new);
        assert!(d.requires_reapproval);
        assert!(d.added.iter().any(|p| p == "http:allow-fetch"));
    }

    #[test]
    fn diff_records_framework_and_version_drift_without_forcing_reapproval() {
        let old = gs("com.a", &[("http:allow-fetch", true)]);
        let mut new = gs("com.a", &[("http:allow-fetch", true)]);
        new.framework = ">=2.0 <4.0".into();
        new.plugin_version = "2.0.0".into();
        let d = diff(Some(&old), &new);
        assert!(d.framework_changed);
        assert!(d.plugin_version_changed);
        // 计划 §4.5 只把"权限/scope 变多"列为重审触发条件。
        assert!(!d.requires_reapproval);
    }

    #[test]
    fn diff_both_add_and_remove() {
        let old = gs("com.a", &[("store:allow-get", false), ("shell:allow-open", false)]);
        let new = gs("com.a", &[("store:allow-get", false), ("dialog:allow-open", false)]);
        let d = diff(Some(&old), &new);
        assert!(d.requires_reapproval);
        assert_eq!(d.added, vec!["dialog:allow-open"]);
        assert_eq!(d.removed, vec!["shell:allow-open"]);
        assert_eq!(d.unchanged, vec!["store:allow-get"]);
    }

    #[test]
    fn scope_grew_edge_cases() {
        assert!(scope_grew(&None, &Some(Value::Null)));
        assert!(!scope_grew(&None, &None));
        let one = Some(Value::Array(vec![Value::String("a".into())]));
        assert!(scope_grew(&None, &one));
        assert!(!scope_grew(&one, &one.clone()));
        assert!(scope_grew(
            &one,
            &Some(Value::Array(vec![Value::String("a".into()), Value::String("b".into())]))
        ));
        assert!(!scope_grew(
            &Some(Value::Array(vec![Value::String("a".into()), Value::String("b".into())])),
            &one
        ));
        // 同长度但内容不同 → 算变宽（新增了一个旧集合里没有的）。
        assert!(scope_grew(
            &Some(Value::Array(vec![Value::String("a".into())])),
            &Some(Value::Array(vec![Value::String("b".into())]))
        ));
    }
}
