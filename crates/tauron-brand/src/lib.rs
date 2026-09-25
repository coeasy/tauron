// §4.16 白标：品牌清单 → tauri.conf 片段 + 图标管线 + CI 矩阵。
//
// 关键约束（计划 §4.16）：
// - identifier/协议名/自启项/数据目录/快捷键**唯一性校验**（架构 §8.4）；
// - dev 构建自动加 `.dev` 后缀；
// - 图标全平台齐备校验；
// - 产物可复现。
//
// 本 crate 不依赖 `tauri`：只操作 JSON 配置，Tauri 适配层负责
// 把合并后的配置写入 `tauri.conf.json`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub mod error;
pub mod build;

pub use error::{BrandError, BrandResult};
pub use build::{BrandBuilder, BuildOptions, BuildResult, BuildInfo, create_brand_builder, create_default_builder};

/// 平台标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Win32,
    Macos,
    Linux,
    Web,
}

impl Platform {
    pub const ALL: [Platform; 4] = [
        Platform::Win32,
        Platform::Macos,
        Platform::Linux,
        Platform::Web,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Win32 => "win32",
            Platform::Macos => "macos",
            Platform::Linux => "linux",
            Platform::Web => "web",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "win32" => Some(Platform::Win32),
            "macos" => Some(Platform::Macos),
            "linux" => Some(Platform::Linux),
            "web" => Some(Platform::Web),
            _ => None,
        }
    }
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 品牌配置。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct BrandConfig {
    /// 应用标识符（如 `com.tauron.standard`）。
    pub identifier: String,
    /// 自定义协议名（如 `tauron`）。
    pub protocol_scheme: String,
    /// 自启项名称。
    pub autostart_name: String,
    /// 数据目录（如 `tauron`）。
    pub data_dir: String,
    /// 快捷键绑定（键名 → 按键序列）。
    #[serde(default)]
    pub shortcuts: BTreeMap<String, String>,
    /// 图标路径（平台 → 相对路径）。
    #[serde(default)]
    pub icons: BTreeMap<Platform, String>,
    /// 额外 tauri.conf 覆盖（深度合并）。
    #[serde(default)]
    pub tauri_overrides: Value,
}

impl BrandConfig {
    /// dev 模式标识符：自动加 `.dev` 后缀。
    pub fn dev_identifier(&self) -> String {
        format!("{}.dev", self.identifier)
    }

    /// dev 模式协议名。
    pub fn dev_protocol_scheme(&self) -> String {
        format!("{}.dev", self.protocol_scheme)
    }

    /// dev 模式自启项名。
    pub fn dev_autostart_name(&self) -> String {
        format!("{}.dev", self.autostart_name)
    }

    /// 所有需要唯一性的标识集合。
    pub fn unique_keys(&self) -> Vec<(String, String)> {
        vec![
            ("identifier".into(), self.identifier.clone()),
            ("protocol_scheme".into(), self.protocol_scheme.clone()),
            ("autostart_name".into(), self.autostart_name.clone()),
            ("data_dir".into(), self.data_dir.clone()),
        ]
    }

    /// 校验必填字段非空。
    pub fn validate_required(&self) -> BrandResult<()> {
        let checks = [
            ("identifier", self.identifier.trim()),
            ("protocol_scheme", self.protocol_scheme.trim()),
            ("autostart_name", self.autostart_name.trim()),
            ("data_dir", self.data_dir.trim()),
        ];
        for (name, val) in checks {
            if val.is_empty() {
                return Err(BrandError::EmptyField(name.to_string()));
            }
        }
        Ok(())
    }

    /// 校验快捷键唯一性（无重复值）。
    pub fn validate_shortcuts(&self) -> BrandResult<()> {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for (key, combo) in &self.shortcuts {
            if !seen.insert(combo.clone()) {
                return Err(BrandError::DuplicateShortcut {
                    key: key.clone(),
                    combo: combo.clone(),
                });
            }
        }
        Ok(())
    }

    /// 校验图标全平台齐备。
    pub fn validate_icons(&self, required: &[Platform]) -> BrandResult<()> {
        for platform in required {
            if !self.icons.contains_key(platform) {
                return Err(BrandError::MissingIcon {
                    platform: platform.as_str().to_string(),
                });
            }
            if self.icons[platform].trim().is_empty() {
                return Err(BrandError::EmptyIconPath {
                    platform: platform.as_str().to_string(),
                });
            }
        }
        Ok(())
    }
}

/// 合并两个 tauri.conf 片段。
///
/// 语义（计划 §4.16 测试项）：
/// - **数组替换**：overlay 中的数组整体替换 base 的同名数组；
/// - **`null` 删键**：overlay 中值为 `null` 的键在 base 中被删除；
/// - **对象递归**：普通对象逐键递归合并；
/// - **标量覆盖**：overlay 的标量值覆盖 base 的同名值。
pub fn merge_config(base: &Value, overlay: &Value) -> Value {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            let mut result = b.clone();
            for (k, v) in o {
                match v {
                    Value::Null => {
                        result.remove(k);
                    }
                    _ => {
                        let merged = match result.get(k) {
                            Some(existing) => merge_config(existing, v),
                            None => v.clone(),
                        };
                        result.insert(k.clone(), merged);
                    }
                }
            }
            Value::Object(result)
        }
        // 非对象：overlay 整体替换 base。
        (_, Value::Array(_) | Value::String(_) | Value::Number(_) | Value::Bool(_)) => {
            overlay.clone()
        }
        _ => overlay.clone(),
    }
}

/// 应用 `TAURI_CONFIG` 环境变量覆盖。
///
/// 格式：`key=value`，多个用分号分隔。
/// 例：`productName=DevBuild;debug=true`
///
/// 返回合并后的配置。未知键静默忽略（不报错——CI 环境可能注入额外变量）。
pub fn apply_env_overrides(config: &Value, env_str: &str) -> Value {
    apply_env_overrides_reporting(config, env_str).0
}

/// [`apply_env_overrides`] 的**带诊断**版本：额外返回被**丢弃**的键。
///
/// 「丢弃」= 点路径中途撞上非对象（例如先 `productName=x` 再
/// `productName.foo=1`——`productName` 已是字符串，写不进去）。此前这条路径被
/// `let _ = set_path(...)` 静默吞掉：CI 注入的覆盖没生效，却没有任何痕迹。
/// 调用方（宿主启动装配）应把返回的键列表记进日志/诊断，而不是当作成功。
pub fn apply_env_overrides_reporting(
    config: &Value,
    env_str: &str,
) -> (Value, Vec<String>) {
    if env_str.trim().is_empty() {
        return (config.clone(), Vec::new());
    }
    let mut result = config.clone();
    let mut dropped = Vec::new();
    for pair in env_str.split(';') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((key, val)) = pair.split_once('=') else {
            // 没有 `=`：不是覆盖项（可能只是分隔符残留），不算"丢弃"。
            continue;
        };
        let key = key.trim();
        let val = val.trim();
        let json_val = match serde_json::from_str::<Value>(val).ok() {
            Some(v) => v,
            None => Value::String(val.to_string()),
        };
        if !set_path(&mut result, key, json_val) {
            dropped.push(key.to_string());
        }
    }
    (result, dropped)
}

/// 按点号路径设置值。
fn set_path(root: &mut Value, dotted: &str, value: Value) -> bool {
    let parts: Vec<&str> = dotted.split('.').collect();
    let mut cur = root;
    for part in &parts[..parts.len() - 1] {
        if let Value::Object(m) = cur {
            if !m.contains_key(*part) {
                m.insert(part.to_string(), Value::Object(Map::new()));
            }
            cur = m.get_mut(*part).unwrap();
        } else {
            return false;
        }
    }
    if let Value::Object(m) = cur {
        m.insert(parts.last().unwrap().to_string(), value);
        true
    } else {
        false
    }
}

/// 校验多个品牌之间的唯一性（架构 §8.4）。
pub fn validate_uniqueness(brands: &[&BrandConfig]) -> BrandResult<()> {
    let mut seen: BTreeMap<String, (String, String)> = BTreeMap::new();
    for brand in brands {
        for (field, value) in brand.unique_keys() {
            let key = format!("{}:{}", field, value);
            if let Some((prev_brand, prev_field)) = seen.get(&key) {
                return Err(BrandError::UniquenessViolation {
                    field: field.clone(),
                    value: value.clone(),
                    brand_a: prev_brand.clone(),
                    brand_b: prev_field.clone(),
                });
            }
            seen.insert(
                key,
                (
                    format!("{}|{}", brand.identifier, brand.data_dir),
                    field,
                ),
            );
        }
    }
    Ok(())
}

/// 生成 CI 矩阵条目（品牌 × 目标平台 × 构建类型）。
pub struct MatrixEntry {
    pub brand: String,
    pub platform: Platform,
    pub target: String,
    pub build_type: BuildType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildType {
    Dev,
    Release,
}

impl BuildType {
    pub fn as_str(self) -> &'static str {
        match self {
            BuildType::Dev => "dev",
            BuildType::Release => "release",
        }
    }
}

impl std::fmt::Display for BuildType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 展开 CI 矩阵。
pub fn expand_matrix(
    brands: &[&BrandConfig],
    platforms: &[Platform],
) -> Vec<MatrixEntry> {
    let mut entries = Vec::new();
    for brand in brands {
        for platform in platforms {
            for bt in [BuildType::Dev, BuildType::Release] {
                entries.push(MatrixEntry {
                    brand: brand.identifier.clone(),
                    platform: *platform,
                    target: target_triple(*platform),
                    build_type: bt,
                });
            }
        }
    }
    entries
}

fn target_triple(p: Platform) -> String {
    match p {
        Platform::Win32 => "x86_64-pc-windows-msvc".into(),
        Platform::Macos => "aarch64-apple-darwin".into(),
        Platform::Linux => "x86_64-unknown-linux-gnu".into(),
        Platform::Web => "wasm32-unknown-unknown".into(),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_brand(id: &str, proto: &str, autostart: &str, dir: &str) -> BrandConfig {
        BrandConfig {
            identifier: id.into(),
            protocol_scheme: proto.into(),
            autostart_name: autostart.into(),
            data_dir: dir.into(),
            shortcuts: BTreeMap::new(),
            icons: BTreeMap::new(),
            tauri_overrides: Value::Null,
        }
    }

    fn full_brand() -> BrandConfig {
        let mut b = test_brand("com.oc.standard", "ocstd", "tauronStd", "oc-std");
        b.shortcuts.insert("open_app".into(), "Ctrl+Shift+O".into());
        b.icons.insert(Platform::Win32, "icons/win32/icon.ico".into());
        b.icons.insert(Platform::Macos, "icons/macos/icon.png".into());
        b.icons.insert(Platform::Linux, "icons/linux/icon.png".into());
        b.icons.insert(Platform::Web, "icons/web/icon.png".into());
        b
    }

    // ── Platform ─────────────────────────────────────────────────────

    #[test]
    fn platform_serde_roundtrip() {
        for p in Platform::ALL {
            let v = serde_json::to_value(p).unwrap();
            assert_eq!(v, serde_json::json!(p.as_str()));
            assert_eq!(serde_json::from_value::<Platform>(v).unwrap(), p);
        }
    }

    #[test]
    fn platform_rejects_unknown() {
        assert!(serde_json::from_value::<Platform>(serde_json::json!("solaris")).is_err());
        assert!(Platform::parse("solaris").is_none());
    }

    // ── BrandConfig 校验 ─────────────────────────────────────────────

    #[test]
    fn validate_required_rejects_empty() {
        let b = test_brand("", "proto", "autostart", "dir");
        assert!(matches!(b.validate_required(), Err(BrandError::EmptyField(ref f)) if f == "identifier"));
    }

    #[test]
    fn validate_required_accepts_valid() {
        assert!(full_brand().validate_required().is_ok());
    }

    #[test]
    fn dev_identifier_appends_dev_suffix() {
        let b = test_brand("com.oc.standard", "ocstd", "Autostart", "dir");
        assert_eq!(b.dev_identifier(), "com.oc.standard.dev");
        assert_eq!(b.dev_protocol_scheme(), "ocstd.dev");
        assert_eq!(b.dev_autostart_name(), "Autostart.dev");
    }

    #[test]
    fn dev_identifier_only_applied_in_dev_mode() {
        // release 模式不用后缀。
        let b = test_brand("com.oc.standard", "ocstd", "Autostart", "dir");
        assert_ne!(b.identifier, b.dev_identifier());
    }

    #[test]
    fn unique_keys_contains_all_four() {
        let b = full_brand();
        let keys = b.unique_keys();
        assert_eq!(keys.len(), 4);
        assert!(keys.iter().any(|(f, _)| f == "identifier"));
        assert!(keys.iter().any(|(f, _)| f == "protocol_scheme"));
        assert!(keys.iter().any(|(f, _)| f == "autostart_name"));
        assert!(keys.iter().any(|(f, _)| f == "data_dir"));
    }

    #[test]
    fn validate_shortcuts_rejects_duplicates() {
        let mut b = full_brand();
        b.shortcuts.insert("a".into(), "Ctrl+1".into());
        b.shortcuts.insert("b".into(), "Ctrl+1".into());
        assert!(matches!(
            b.validate_shortcuts(),
            Err(BrandError::DuplicateShortcut { .. })
        ));
    }

    #[test]
    fn validate_shortcuts_accepts_unique() {
        let mut b = full_brand();
        b.shortcuts.insert("a".into(), "Ctrl+1".into());
        b.shortcuts.insert("b".into(), "Ctrl+2".into());
        assert!(b.validate_shortcuts().is_ok());
    }

    #[test]
    fn validate_icons_rejects_missing_platform() {
        let mut b = full_brand();
        b.icons.remove(&Platform::Web);
        let err = b.validate_icons(&Platform::ALL).unwrap_err();
        assert!(matches!(err, BrandError::MissingIcon { ref platform } if platform == "web"));
    }

    #[test]
    fn validate_icons_accepts_all_present() {
        assert!(full_brand().validate_icons(&Platform::ALL).is_ok());
    }

    #[test]
    fn validate_icons_rejects_empty_path() {
        let mut b = full_brand();
        b.icons.insert(Platform::Web, "".into());
        assert!(matches!(b.validate_icons(&Platform::ALL), Err(BrandError::EmptyIconPath { .. })));
    }

    // ── 合并语义 ─────────────────────────────────────────────────────

    #[test]
    fn merge_scalars_overlay_wins() {
        let base = serde_json::json!({"a": 1, "b": "old"});
        let overlay = serde_json::json!({"b": "new", "c": 3});
        let result = merge_config(&base, &overlay);
        assert_eq!(result["a"], 1, "base 独有键保留");
        assert_eq!(result["b"], "new", "overlay 覆盖");
        assert_eq!(result["c"], 3, "overlay 独有键加入");
    }

    #[test]
    fn merge_null_deletes_key() {
        let base = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let overlay = serde_json::json!({"b": null});
        let result = merge_config(&base, &overlay);
        assert!(result.get("b").is_none(), "null 应删除键");
        assert_eq!(result["a"], 1);
        assert_eq!(result["c"], 3);
    }

    #[test]
    fn merge_null_on_missing_key_is_noop() {
        let base = serde_json::json!({"a": 1});
        let overlay = serde_json::json!({"b": null});
        let result = merge_config(&base, &overlay);
        assert_eq!(result, serde_json::json!({"a": 1}));
    }

    #[test]
    fn merge_arrays_replace_not_concat() {
        let base = serde_json::json!({"plugins": ["a", "b", "c"]});
        let overlay = serde_json::json!({"plugins": ["x"]});
        let result = merge_config(&base, &overlay);
        assert_eq!(result["plugins"], serde_json::json!(["x"]), "数组应整体替换");
    }

    #[test]
    fn merge_nested_objects_recurse() {
        let base = serde_json::json!({
            "window": {
                "width": 800,
                "height": 600,
                "title": "Base"
            }
        });
        let overlay = serde_json::json!({
            "window": {
                "width": 1024,
                "title": "Overlay"
            }
        });
        let result = merge_config(&base, &overlay);
        assert_eq!(result["window"]["width"], 1024);
        assert_eq!(result["window"]["height"], 600, "未覆盖的嵌套键保留");
        assert_eq!(result["window"]["title"], "Overlay");
    }

    #[test]
    fn merge_deep_nested_null() {
        let base = serde_json::json!({"a": {"b": {"c": 1, "d": 2}}});
        let overlay = serde_json::json!({"a": {"b": {"c": null}}});
        let result = merge_config(&base, &overlay);
        assert!(result["a"]["b"].get("c").is_none());
        assert_eq!(result["a"]["b"]["d"], 2);
    }

    #[test]
    fn merge_array_replaces_even_if_base_is_object() {
        let base = serde_json::json!({"items": {"a": 1}});
        let overlay = serde_json::json!({"items": [1, 2, 3]});
        let result = merge_config(&base, &overlay);
        assert_eq!(result["items"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn merge_object_over_array_replaces() {
        let base = serde_json::json!({"items": [1, 2]});
        let overlay = serde_json::json!({"items": {"x": 1}});
        let result = merge_config(&base, &overlay);
        assert_eq!(result["items"], serde_json::json!({"x": 1}));
    }

    // ── TAURI_CONFIG 环境变量覆盖 ────────────────────────────────────

    #[test]
    fn apply_env_overrides_single_key() {
        let base = serde_json::json!({"productName": "Base", "debug": false});
        let result = apply_env_overrides(&base, "productName=DevBuild");
        assert_eq!(result["productName"], "DevBuild");
        assert_eq!(result["debug"], false);
    }

    #[test]
    fn apply_env_overrides_multiple_keys() {
        let base = serde_json::json!({"a": 1, "b": 2});
        let result = apply_env_overrides(&base, "a=10;b=20");
        assert_eq!(result["a"], 10);
        assert_eq!(result["b"], 20);
    }

    #[test]
    fn apply_env_overrides_nested_path() {
        let base = serde_json::json!({"window": {"width": 800}});
        let result = apply_env_overrides(&base, "window.width=1024");
        assert_eq!(result["window"]["width"], 1024);
    }

    #[test]
    fn apply_env_overrides_boolean() {
        let base = serde_json::json!({"debug": false});
        let result = apply_env_overrides(&base, "debug=true");
        assert_eq!(result["debug"], true);
    }

    #[test]
    fn apply_env_overrides_ignores_malformed() {
        let base = serde_json::json!({"a": 1});
        let result = apply_env_overrides(&base, "no_equals;ok=5");
        assert_eq!(result["a"], 1);
        assert_eq!(result["ok"], 5);
    }

    #[test]
    fn apply_env_overrides_empty_string_is_noop() {
        let base = serde_json::json!({"a": 1});
        let result = apply_env_overrides(&base, "");
        assert_eq!(result, base);
    }

    #[test]
    fn apply_env_overrides_reports_dropped_paths_instead_of_swallowing_them() {
        // `productName` 已是字符串，`productName.foo=1` 写不进去。此前被
        // `let _ =` 静默吞掉——覆盖没生效却毫无痕迹。诊断版必须如实报告。
        let base = serde_json::json!({"productName": "Base"});
        let (result, dropped) =
            apply_env_overrides_reporting(&base, "productName.foo=1;ok=5");
        assert_eq!(dropped, vec!["productName.foo".to_string()]);
        assert_eq!(result["ok"], 5);
        // 兼容版行为不变（返回值仍是合并后的配置）。
        let compat = apply_env_overrides(&base, "productName.foo=1;ok=5");
        assert_eq!(compat, result);
    }

    #[test]
    fn apply_env_overrides_reports_nothing_for_clean_input() {
        let base = serde_json::json!({"window": {"width": 800}});
        let (_, dropped) = apply_env_overrides_reporting(&base, "window.width=1024;debug=true");
        assert!(dropped.is_empty(), "全部覆盖生效时不该报告任何丢弃");
    }

    // ── 唯一性校验 ───────────────────────────────────────────────────

    #[test]
    fn uniqueness_allows_distinct_brands() {
        let a = test_brand("com.oc.std", "std", "Std", "std");
        let b = test_brand("com.oc.pro", "pro", "Pro", "pro");
        assert!(validate_uniqueness(&[&a, &b]).is_ok());
    }

    #[test]
    fn uniqueness_rejects_duplicate_identifier() {
        let a = test_brand("com.oc.same", "a", "A", "aa");
        let b = test_brand("com.oc.same", "b", "B", "bb");
        assert!(matches!(
            validate_uniqueness(&[&a, &b]),
            Err(BrandError::UniquenessViolation { ref field, .. }) if field == "identifier"
        ));
    }

    #[test]
    fn uniqueness_rejects_duplicate_protocol() {
        let a = test_brand("com.a", "same", "A", "aa");
        let b = test_brand("com.b", "same", "B", "bb");
        assert!(matches!(
            validate_uniqueness(&[&a, &b]),
            Err(BrandError::UniquenessViolation { ref field, .. }) if field == "protocol_scheme"
        ));
    }

    #[test]
    fn uniqueness_rejects_duplicate_data_dir() {
        let a = test_brand("com.a", "a", "A", "same");
        let b = test_brand("com.b", "b", "B", "same");
        assert!(matches!(
            validate_uniqueness(&[&a, &b]),
            Err(BrandError::UniquenessViolation { ref field, .. }) if field == "data_dir"
        ));
    }

    #[test]
    fn uniqueness_single_brand_is_ok() {
        let a = test_brand("com.a", "a", "A", "aa");
        assert!(validate_uniqueness(&[&a]).is_ok());
    }

    // ── CI 矩阵 ─────────────────────────────────────────────────────

    #[test]
    fn expand_matrix_produces_expected_count() {
        let a = test_brand("com.a", "a", "A", "aa");
        let entries = expand_matrix(&[&a], &Platform::ALL);
        // 1 brand × 4 platforms × 2 build types = 8
        assert_eq!(entries.len(), 8);
    }

    #[test]
    fn expand_matrix_two_brands() {
        let a = test_brand("com.a", "a", "A", "aa");
        let b = test_brand("com.b", "b", "B", "bb");
        let entries = expand_matrix(&[&a, &b], &[Platform::Win32, Platform::Macos]);
        // 2 brands × 2 platforms × 2 build types = 8
        assert_eq!(entries.len(), 8);
    }

    #[test]
    fn expand_matrix_target_triples() {
        let a = test_brand("com.a", "a", "A", "aa");
        let entries = expand_matrix(&[&a], &Platform::ALL);
        let triples: Vec<String> = entries.iter().map(|e| e.target.clone()).collect();
        assert!(triples.contains(&"x86_64-pc-windows-msvc".into()));
        assert!(triples.contains(&"aarch64-apple-darwin".into()));
        assert!(triples.contains(&"x86_64-unknown-linux-gnu".into()));
        assert!(triples.contains(&"wasm32-unknown-unknown".into()));
    }

    #[test]
    fn build_type_as_str() {
        assert_eq!(BuildType::Dev.as_str(), "dev");
        assert_eq!(BuildType::Release.as_str(), "release");
    }

    // ── 端到端 ──────────────────────────────────────────────────────

    /// 计划 §4.16 测试项：合并语义 + 唯一性 + dev/release 并存。
    #[test]
    fn end_to_end_brand_flow() {
        // 1. 创建两个品牌。
        let mut std_brand = full_brand();
        std_brand.tauri_overrides = serde_json::json!({
            "productName": "tauron",
            "window": {"width": 1024, "height": 768}
        });

        let mut pro_brand = full_brand();
        pro_brand.identifier = "com.oc.pro".into();
        pro_brand.protocol_scheme = "ocpro".into();
        pro_brand.autostart_name = "tauronPro".into();
        pro_brand.data_dir = "oc-pro".into();
        pro_brand.tauri_overrides = serde_json::json!({
            "productName": "tauron Pro",
            "window": {"width": 1920, "height": 1080}
        });

        // 2. 唯一性校验。
        assert!(validate_uniqueness(&[&std_brand, &pro_brand]).is_ok());

        // 3. 合并配置：base tauri.conf + 品牌覆盖。
        let base = serde_json::json!({
            "productName": "Base",
            "version": "0.1.0",
            "window": {"width": 800, "height": 600, "title": "Base"}
        });
        let std_merged = merge_config(&base, &std_brand.tauri_overrides);
        assert_eq!(std_merged["productName"], "tauron");
        assert_eq!(std_merged["version"], "0.1.0", "未覆盖字段保留");
        assert_eq!(std_merged["window"]["width"], 1024);
        assert_eq!(std_merged["window"]["title"], "Base", "未覆盖嵌套字段保留");

        // 4. dev 后缀。
        assert!(std_brand.dev_identifier().ends_with(".dev"));
        assert!(!std_brand.identifier.ends_with(".dev"));

        // 5. TAURI_CONFIG 覆盖。
        let with_env = apply_env_overrides(&std_merged, "window.width=1280;debug=true");
        assert_eq!(with_env["window"]["width"], 1280);
        assert_eq!(with_env["debug"], true);

        // 6. CI 矩阵。
        let entries = expand_matrix(&[&std_brand, &pro_brand], &Platform::ALL);
        assert_eq!(entries.len(), 16);
    }

    /// 计划 §4.16 测试项：null 删键。
    #[test]
    fn end_to_end_null_deletion() {
        let base = serde_json::json!({
            "plugins": ["a", "b", "c"],
            "window": {"width": 800, "height": 600},
            "features": {"a": true, "b": false}
        });
        let overlay = serde_json::json!({
            "plugins": ["x"],
            "window": {"height": null},
            "features": {"b": true, "c": true}
        });
        let result = merge_config(&base, &overlay);
        assert_eq!(result["plugins"], serde_json::json!(["x"]), "数组替换");
        assert!(result["window"].get("height").is_none(), "null 删键");
        assert_eq!(result["window"]["width"], 800);
        assert_eq!(result["features"]["a"], true);
        assert_eq!(result["features"]["b"], true);
        assert_eq!(result["features"]["c"], true);
    }

    #[test]
    fn brand_config_serde_roundtrip() {
        let b = full_brand();
        let v = serde_json::to_value(&b).unwrap();
        let b2 = serde_json::from_value::<BrandConfig>(v).unwrap();
        assert_eq!(b.identifier, b2.identifier);
        assert_eq!(b.icons.len(), b2.icons.len());
    }

    #[test]
    fn brand_config_rejects_unknown_fields() {
        let json = serde_json::json!({
            "identifier": "com.a",
            "protocol_scheme": "a",
            "autostart_name": "A",
            "data_dir": "a",
            "unknown_field": "x"
        });
        assert!(serde_json::from_value::<BrandConfig>(json).is_err());
    }
}
