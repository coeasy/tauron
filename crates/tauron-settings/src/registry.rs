// §4.12 schema 注册表。
//
// 插件在注册时上交原始 schema（schemars 形态，可含 `$defs`/`$ref` 与
// enum → oneOf），注册表用 §4.13 管线编译成平面 schema + uiSchema，
// 并做两道门禁：
//
// 1. **每表单 ≤40 字段**（RJSF 大 schema 会变慢，架构 §6.2）；
// 2. **平面形态**（无嵌套 oneOf/anyOf、无 `$ref`——门禁 §8-12）。
//
// 注册后不可改：schema 是插件身份的一部分，变更需升版重装（计划 §4.18）。

use std::collections::BTreeMap;

use serde_json::Value;

use tauron_schema::{compile, Compiled};

use crate::error::{SettingsError, SettingsResult};

/// 单表单字段上限。
pub const FIELD_LIMIT: usize = 40;

/// 一份已注册的 schema。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 插件 id。
    pub plugin_id: String,
    /// 插件声明的 schema 版本（用于升级判断，不参与形态判定）。
    pub schema_version: String,
    /// §4.13 管线产物。
    pub compiled: Compiled,
    /// 字段总数（用于 §8 门禁与 UI 提示）。
    pub field_count: usize,
}

/// schema 注册表。
#[derive(Debug, Default)]
pub struct SchemaRegistry {
    entries: BTreeMap<String, Entry>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一份原始 schema。编译失败或超字段上限即拒绝。
    ///
    /// 重复注册同一插件视为**升版**：只有 `schema_version` 严格递增才接受，
    /// 否则拒绝——防止"同版本覆盖"导致已渲染的表单错位。
    pub fn register(
        &mut self,
        plugin_id: &str,
        schema_version: &str,
        raw: &Value,
    ) -> SettingsResult<()> {
        if plugin_id.is_empty() || schema_version.is_empty() {
            return Err(SettingsError::InvalidPath(
                "plugin_id 与 schema_version 不能为空".into(),
            ));
        }

        // 门禁 1：编译 + 平面断言。
        let compiled = compile(raw).map_err(|e| SettingsError::SchemaCompile(e.to_string()))?;

        // 门禁 2：字段上限。
        let field_count = count_fields(&compiled.schema);
        if field_count > FIELD_LIMIT {
            return Err(SettingsError::FieldLimitExceeded {
                plugin: plugin_id.to_string(),
                fields: field_count,
                limit: FIELD_LIMIT,
            });
        }

        // 升版检查。
        if let Some(old) = self.entries.get(plugin_id) {
            if !version_greater(schema_version, &old.schema_version) {
                return Err(SettingsError::SchemaCompile(format!(
                    "插件 `{plugin_id}` 已有 schema `{}`,新版本 `{schema_version}` 不严格递增",
                    old.schema_version
                )));
            }
        }

        self.entries.insert(
            plugin_id.to_string(),
            Entry {
                plugin_id: plugin_id.to_string(),
                schema_version: schema_version.to_string(),
                compiled,
                field_count,
            },
        );
        Ok(())
    }

    /// 取已编译产物。
    pub fn get(&self, plugin_id: &str) -> Option<&Entry> {
        self.entries.get(plugin_id)
    }

    /// 已注册插件数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 产出渲染数据：`{schema, uiSchema, defaults}`。
    ///
    /// `defaults` 是 builtin 层的值（供 UI 作为表单初始值）。
    pub fn render(&self, plugin_id: &str, defaults: &Value) -> SettingsResult<Value> {
        let e = self
            .entries
            .get(plugin_id)
            .ok_or_else(|| SettingsError::SchemaNotRegistered(plugin_id.to_string()))?;
        Ok(serde_json::Map::from_iter([
            ("schema".to_string(), e.compiled.schema.clone()),
            ("uiSchema".to_string(), e.compiled.ui_schema.clone()),
            ("defaults".to_string(), defaults.clone()),
        ])
        .into())
    }
}

/// 统计 schema 的字段总数。
///
/// 字段 = `properties` 下的键（顶层 + 每个嵌套对象 + 组合成员的 object 分支）。
/// 与 RJSF 的"要渲染多少个控件"一一对应。
pub fn count_fields(schema: &Value) -> usize {
    let mut n = 0;
    count_fields_in(schema, &mut n);
    n
}

fn count_fields_in(node: &Value, n: &mut usize) {
    let serde_json::Value::Object(m) = node else { return };

    if let Some(Value::Object(props)) = m.get("properties") {
        *n += props.len();
        for sub in props.values() {
            count_fields_in(sub, n);
        }
    }
    if let Some(Value::Array(items)) = m.get("oneOf") {
        for branch in items {
            count_fields_in(branch, n);
        }
    }
    if let Some(Value::Array(items)) = m.get("anyOf") {
        for branch in items {
            count_fields_in(branch, n);
        }
    }
    if let Some(v) = m.get("items") {
        count_fields_in(v, n);
    }
}

/// 语义化版本比较（`a > b`）。支持 `MAJOR.MINOR.PATCH` 与可选前缀 `v`。
fn version_greater(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    match (pa, pb) {
        (Some(x), Some(y)) => x > y,
        // 不可解析时按字典序（保底，不 panic）。
        _ => a > b,
    }
}

fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let t = s.strip_prefix('v').unwrap_or(s);
    let mut parts = t.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn simple_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "volume": {"type": "integer", "minimum": 0, "maximum": 100,
                    "x-tauron": {"version": 1, "label": "音量", "widget": "slider"}},
                "muted": {"type": "boolean", "x-tauron": {"version": 1, "label": "静音"}}
            },
            "required": ["volume"]
        })
    }

    // ── 注册 ────────────────────────────────────────────────────────

    #[test]
    fn register_success_compiles_and_counts_fields() {
        let mut r = SchemaRegistry::new();
        r.register("p.audio", "1.0.0", &simple_schema()).unwrap();
        let e = r.get("p.audio").unwrap();
        assert_eq!(e.field_count, 2);
        assert_eq!(e.compiled.extension_count, 2);
        assert_eq!(e.compiled.schema["properties"]["volume"]["type"], "integer");
        assert_eq!(e.compiled.ui_schema["volume"]["ui:label"], "音量");
    }

    #[test]
    fn register_rejects_empty_ids() {
        let mut r = SchemaRegistry::new();
        assert!(r.register("", "1.0.0", &simple_schema()).is_err());
        assert!(r.register("p", "", &simple_schema()).is_err());
    }

    #[test]
    fn register_rejects_a_nonflat_schema() {
        // 嵌套 oneOf → §8-12 门禁拒绝。
        let bad = json!({"oneOf":[{"oneOf":[{"type":"string"}]}]});
        let mut r = SchemaRegistry::new();
        let e = r.register("p.bad", "1.0.0", &bad).unwrap_err();
        assert!(matches!(e, SettingsError::SchemaCompile(_)));
    }

    #[test]
    fn register_rejects_unknown_extension_key() {
        let bad = json!({"type":"object","properties":{"a":{"type":"string","x-tauron":{"version":1,"lbel":"x"}}}});
        let mut r = SchemaRegistry::new();
        let e = r.register("p.bad", "1.0.0", &bad).unwrap_err();
        assert!(matches!(e, SettingsError::SchemaCompile(m) if m.contains("UnknownExtension") || m.contains("lbel") || m.contains("未知")));
    }

    #[test]
    fn register_rejects_out_of_range_extension_version() {
        let bad = json!({"type":"object","properties":{"a":{"type":"string","x-tauron":{"version":999,"label":"x"}}}});
        let mut r = SchemaRegistry::new();
        assert!(r.register("p.bad", "1.0.0", &bad).is_err());
    }

    // ── 字段上限 ────────────────────────────────────────────────────

    #[test]
    fn field_limit_boundary_is_enforced() {
        // 正好 40 个字段：允许。
        let mut r = SchemaRegistry::new();
        let props = (0..=FIELD_LIMIT - 1)
            .map(|i| (format!("f{i}"), json!({"type": "string"})))
            .collect::<serde_json::Map<_, _>>();
        r.register("p.40", "1.0.0", &json!({"type":"object","properties":props})).unwrap();

        // 41 个：拒绝。
        let props = (0..=FIELD_LIMIT)
            .map(|i| (format!("f{i}"), json!({"type": "string"})))
            .collect::<serde_json::Map<_, _>>();
        let e = r.register("p.41", "1.0.0", &json!({"type":"object","properties":props})).unwrap_err();
        match e {
            SettingsError::FieldLimitExceeded { fields, limit, .. } => {
                assert_eq!(fields, 41);
                assert_eq!(limit, FIELD_LIMIT);
            }
            other => panic!("期望 FieldLimitExceeded，实际 {other:?}"),
        }
    }

    #[test]
    fn nested_objects_count_towards_the_limit() {
        let mut r = SchemaRegistry::new();
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "object", "properties": {"x": {"type": "string"}, "y": {"type": "string"}}},
                "b": {"type": "object", "properties": {"z": {"type": "string"}}}
            }
        });
        r.register("p.nest", "1.0.0", &schema).unwrap();
        assert_eq!(r.get("p.nest").unwrap().field_count, 5, "顶层 2 + 嵌套 3");
    }

    #[test]
    fn one_of_branch_fields_count() {
        let mut r = SchemaRegistry::new();
        let schema = json!({
            "type": "object",
            "properties": {
                "kind": {"oneOf": [
                    {"type": "object", "properties": {"a": {"type": "string"}}},
                    {"type": "object", "properties": {"b": {"type": "string"}, "c": {"type": "string"}}}
                ]}
            }
        });
        r.register("p.enum", "1.0.0", &schema).unwrap();
        // 顶层 kind(1) + 分支字段(1+2) = 4
        assert_eq!(r.get("p.enum").unwrap().field_count, 4);
    }

    // ── 升版 ────────────────────────────────────────────────────────

    #[test]
    fn re_register_requires_strictly_greater_version() {
        let mut r = SchemaRegistry::new();
        r.register("p", "1.0.0", &simple_schema()).unwrap();

        for same_or_lower in ["1.0.0", "1.0.0", "0.9.9", "1.0"] {
            let e = r.register("p", same_or_lower, &simple_schema()).unwrap_err();
            assert!(matches!(e, SettingsError::SchemaCompile(ref m) if m.contains("递增")), "{same_or_lower}: {e}");
        }
    }

    #[test]
    fn re_register_with_higher_version_replaces() {
        let mut r = SchemaRegistry::new();
        r.register("p", "1.0.0", &simple_schema()).unwrap();
        let props = (0..5).map(|i| (format!("f{i}"), json!({"type":"string"}))).collect::<serde_json::Map<_, _>>();
        r.register("p", "1.1.0", &json!({"type":"object","properties":props})).unwrap();
        assert_eq!(r.get("p").unwrap().schema_version, "1.1.0");
        assert_eq!(r.get("p").unwrap().field_count, 5);
    }

    #[test]
    fn version_comparison_handles_v_prefix_and_patch() {
        assert!(version_greater("1.1.0", "1.0.0"));
        assert!(version_greater("1.0.1", "1.0.0"));
        assert!(version_greater("2.0.0", "1.9.9"));
        assert!(version_greater("v2.0.0", "1.0.0"));
        assert!(!version_greater("1.0.0", "1.0.0"));
        assert!(version_greater("1.0.0", "0.9.0"));
    }

    #[test]
    fn version_falls_back_to_lexicographic_for_unparseable() {
        assert!(version_greater("abd", "abc"));
        // 不可解析不 panic。
        assert!(!version_greater("x", "x"));
    }

    // ── 查询 / 渲染 ──────────────────────────────────────────────────

    #[test]
    fn get_on_missing_returns_none() {
        let r = SchemaRegistry::new();
        assert!(r.get("nope").is_none());
        assert_eq!(r.len(), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn render_returns_schema_uischema_and_defaults() {
        let mut r = SchemaRegistry::new();
        r.register("p", "1.0.0", &simple_schema()).unwrap();
        let out = r.render("p", &json!({"volume": 10, "muted": false})).unwrap();
        assert!(out["schema"]["properties"]["volume"].is_object());
        assert_eq!(out["uiSchema"]["volume"]["ui:label"], "音量");
        assert_eq!(out["defaults"]["volume"], 10);
    }

    #[test]
    fn render_on_unregistered_errors() {
        let r = SchemaRegistry::new();
        assert!(matches!(
            r.render("nope", &json!({})).unwrap_err(),
            SettingsError::SchemaNotRegistered(ref p) if p == "nope"
        ));
    }

    #[test]
    fn register_is_idempotent_for_distinct_plugins() {
        let mut r = SchemaRegistry::new();
        r.register("p.a", "1.0.0", &simple_schema()).unwrap();
        r.register("p.b", "1.0.0", &simple_schema()).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.get("p.a").is_some() && r.get("p.b").is_some());
    }

    #[test]
    fn count_fields_ignores_non_object_nodes() {
        assert_eq!(count_fields(&json!("plain")), 0);
        assert_eq!(count_fields(&json!(42)), 0);
        assert_eq!(count_fields(&json!({"type":"string"})), 0);
        assert_eq!(count_fields(&json!({"properties": {"a": 1}})), 1, "非对象属性也计入控件数");
    }

    #[test]
    fn count_fields_is_stable_across_runs() {
        // serde_json::Map 是 BTreeMap，字段数与遍历顺序无关。
        let a = json!({"type":"object","properties":{"b":{"type":"string"},"a":{"type":"string"}}});
        let b = json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}}});
        assert_eq!(count_fields(&a), count_fields(&b));
    }
}
