// §4.13 `x-tauron` 扩展命名空间 + uiSchema 编译。
//
// 架构 §6.2：schemars 官方机制是 `#[schemars(extend("x-…"))]`，社区通行做法是
// 把 label/order/widget/条件显隐意图放 `x-` 扩展、运行时编译成 uiSchema
// （无标准，故本框架自定 `x-tauron` 命名空间并为其出版 schema）。
//
// 两条硬规则（计划 §4.13）：
// - 扩展必须带 `version`，越界即拒绝——防"旧客户端读到新键"的静默降级；
// - 未知扩展键报错，不静默忽略——否则拼写漂移会让 UI 意图悄悄失效。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{SchemaError, SchemaResult};

/// `x-tauron` 当前支持的版本。破坏性变更时自增并登记兼容窗口。
pub const XOC_VERSION: u32 = 1;

/// 已登记的扩展键。数组常量，供 CI 遍历（与门禁 §8-14 同一模式）。
pub const KNOWN_KEYS: &[&str] = &[
    "version",
    "label",
    "description",
    "order",
    "widget",
    "placeholder",
    "visibleIf",
];

/// 支持的 widget 集合——即 WC 渲染器的 6 控件（架构 §5.2 / §6.2）。
pub const ALL_WIDGETS: &[&str] = &[
    "textbox",
    "checkbox",
    "select",
    "slider",
    "switch",
    "keybind",
];

/// 扩展在 schema 文档中的键名。
pub const EXT_KEY: &str = "x-tauron";

/// 已解析的 `x-tauron` 扩展（值类型已收敛，可直接喂前端）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Extension {
    pub version: u32,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub order: Option<i32>,
    #[serde(default)]
    pub widget: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub visible_if: Option<Value>,
}

impl Extension {
    /// 从原始 JSON 解析。`path` 是出错位置（JSON Pointer），用于报错定位。
    pub fn parse(path: &str, raw: &Value) -> SchemaResult<Self> {
        let obj = raw
            .as_object()
            .ok_or_else(|| SchemaError::ExtensionType("".into(), "object".into(), value_kind(raw).into(), path.to_string()))?;

        for key in obj.keys() {
            if !KNOWN_KEYS.contains(&key.as_str()) {
                return Err(SchemaError::UnknownExtension(key.clone(), path.to_string()));
            }
        }

        let version = match obj.get("version") {
            None => XOC_VERSION, // 缺省视为当前版本（生成器总会写，手写 schema 容错）
            Some(Value::Number(n)) => n.as_u64().ok_or_else(|| {
                SchemaError::ExtensionType("version".into(), "u32".into(), value_kind(raw).into(), path.to_string())
            })?.try_into().map_err(|_| {
                SchemaError::ExtensionType("version".into(), "u32".into(), value_kind(raw).into(), path.to_string())
            })?,
            Some(other) => return Err(SchemaError::ExtensionType("version".into(), "u32".into(), value_kind(other).into(), path.to_string())),
        };
        if version != XOC_VERSION {
            return Err(SchemaError::UnsupportedVersion(version, XOC_VERSION, path.to_string()));
        }

        let ext = Self {
            version,
            label: opt_string(obj.get("label"), "label", path)?,
            description: opt_string(obj.get("description"), "description", path)?,
            order: obj
                .get("order")
                .map(|v| {
                    v.as_i64().ok_or_else(|| {
                        SchemaError::ExtensionType("order".into(), "i32".into(), value_kind(v).into(), path.to_string())
                    })
                })
                .transpose()?
                .map(|n| n.try_into())
                .transpose()
                .map_err(|_| SchemaError::ExtensionType("order".into(), "i32".into(), "out-of-range".into(), path.to_string()))?,
            widget: opt_string(obj.get("widget"), "widget", path)?,
            placeholder: opt_string(obj.get("placeholder"), "placeholder", path)?,
            visible_if: obj.get("visibleIf").cloned(),
        };

        if let Some(w) = &ext.widget {
            if !ALL_WIDGETS.contains(&w.as_str()) {
                return Err(SchemaError::ExtensionType(
                    "widget".into(),
                    format!("one of {}", ALL_WIDGETS.join("|")).into(),
                    w.clone().into(),
                    path.to_string(),
                ));
            }
        }
        Ok(ext)
    }

    /// 编译为 RJSF uiSchema 片段（`ui:*` 键）。
    pub fn to_ui_fragment(&self) -> Value {
        let mut m = Map::new();
        if let Some(l) = &self.label {
            m.insert("ui:label".into(), Value::String(l.clone()));
        }
        if let Some(d) = &self.description {
            m.insert("ui:help".into(), Value::String(d.clone()));
        }
        if let Some(o) = self.order {
            m.insert("ui:order".into(), Value::from(o));
        }
        if let Some(w) = &self.widget {
            m.insert("ui:widget".into(), Value::String(w.clone()));
        }
        if let Some(p) = &self.placeholder {
            m.insert("ui:placeholder".into(), Value::String(p.clone()));
        }
        if let Some(v) = &self.visible_if {
            m.insert("ui:visibleIf".into(), v.clone());
        }
        Value::Object(m)
    }
}

fn opt_string(v: Option<&Value>, key: &str, path: &str) -> SchemaResult<Option<String>> {
    match v {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(SchemaError::ExtensionType(
            key.into(),
            "string".into(),
            value_kind(other).into(),
            path.to_string(),
        )),
    }
}

fn value_kind(v: &Value) -> &'static str {
    static KINDS: OnceLock<&[&str]> = OnceLock::new();
    let _ = KINDS.get_or_init(|| &["object", "array", "string", "number", "boolean", "null"]);
    match v {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(n) => if n.is_i64() || n.is_u64() { "integer" } else { "number" },
        Value::Bool(_) => "boolean",
        Value::Null => "null",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn parses_a_full_extension() {
        let raw = j(r#"{"version":1,"label":"音量","description":"系统音量百分比","order":2,"widget":"slider","placeholder":"50","visibleIf":{"prop":"enable","value":true}}"#);
        let e = Extension::parse("/properties/volume", &raw).unwrap();
        assert_eq!(e.version, 1);
        assert_eq!(e.label.as_deref(), Some("音量"));
        assert_eq!(e.order, Some(2));
        assert_eq!(e.widget.as_deref(), Some("slider"));
        assert_eq!(e.visible_if.as_ref().unwrap()["prop"], "enable");
    }

    #[test]
    fn version_defaults_to_current() {
        let e = Extension::parse("/", &j(r#"{"label":"x"}"#)).unwrap();
        assert_eq!(e.version, XOC_VERSION);
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let err = Extension::parse("/", &j(r#"{"version":99}"#)).unwrap_err();
        assert!(matches!(err, SchemaError::UnsupportedVersion(99, 1, _)));
    }

    #[test]
    fn unknown_key_is_rejected_not_ignored() {
        let err = Extension::parse("/properties/a", &j(r#"{"version":1,"lbel":"x"}"#)).unwrap_err();
        match err {
            SchemaError::UnknownExtension(key, path) => {
                assert_eq!(key, "lbel");
                assert_eq!(path, "/properties/a");
            }
            other => panic!("期望 UnknownExtension，实际 {other:?}"),
        }
    }

    #[test]
    fn unknown_key_order_is_checked_exhaustively() {
        for bad in ["lbel", "ordr", "widgt", "plceholder", "visibleif", "descripion"] {
            let raw = Value::Object(Map::from_iter(vec![(bad.to_string(), Value::String("x".into()))]));
            assert!(Extension::parse("/", &raw).is_err(), "键 `{bad}` 必须报错");
        }
    }

    #[test]
    fn every_declared_key_is_accepted() {
        for key in KNOWN_KEYS {
            let mut raw = serde_json::Map::new();
            let value = match *key {
                "version" | "order" => Value::from(1),
                "visibleIf" => Value::Object(Map::new()),
                "widget" => Value::String("textbox".into()),
                _ => Value::String("x".into()),
            };
            raw.insert(key.to_string(), value);
            assert!(Extension::parse("/", &Value::Object(raw)).is_ok(), "键 `{key}` 必须被接受");
        }
    }

    #[test]
    fn widget_must_be_in_the_supported_set() {
        let err = Extension::parse("/", &j(r#"{"version":1,"widget":"datepicker"}"#)).unwrap_err();
        assert!(matches!(err, SchemaError::ExtensionType(k, _, _, _) if k == "widget"));
        for w in ALL_WIDGETS {
            assert!(Extension::parse("/", &j(&format!(r#"{{"version":1,"widget":"{w}"}}"#))).is_ok());
        }
    }

    #[test]
    fn wrong_types_are_reported_with_position() {
        for (obj, key, want) in [
            (r#"{"version":"one"}"#, "version", "u32"),
            (r#"{"version":1,"order":"two"}"#, "order", "i32"),
            (r#"{"version":1,"label":7}"#, "label", "string"),
            (r#"{"version":1,"placeholder":true}"#, "placeholder", "string"),
        ] {
            let err = Extension::parse("/props/x", &j(obj)).unwrap_err();
            match err {
                SchemaError::ExtensionType(k, expected, _, path) => {
                    assert_eq!(k, key);
                    assert!(expected.contains(want), "{k}: 期望含 {want}，实际 {expected}");
                    assert_eq!(path, "/props/x");
                }
                other => panic!("期望 ExtensionType，实际 {other:?}"),
            }
        }
    }

    #[test]
    fn order_out_of_i32_range_is_rejected() {
        let err = Extension::parse("/", &j(r#"{"version":1,"order":9999999999999999999}"#)).unwrap_err();
        assert!(matches!(err, SchemaError::ExtensionType(k, _, _, _) if k == "order"));
    }

    #[test]
    fn non_object_extension_is_rejected() {
        let err = Extension::parse("/", &Value::Array(Vec::new())).unwrap_err();
        assert!(matches!(err, SchemaError::ExtensionType(k, _, _, _) if k == ""));
    }

    #[test]
    fn empty_extension_is_valid_and_produces_empty_fragment() {
        let e = Extension::parse("/", &j("{}")).unwrap();
        assert_eq!(e.to_ui_fragment(), Value::Object(Map::new()));
    }

    #[test]
    fn to_ui_fragment_maps_to_rjsf_ui_keys() {
        let e = Extension::parse("/", &j(r#"{"version":1,"label":"语言","description":"界面语言","order":1,"widget":"select","placeholder":"zh-CN"}"#)).unwrap();
        let f = e.to_ui_fragment();
        assert_eq!(f["ui:label"], "语言");
        assert_eq!(f["ui:help"], "界面语言");
        assert_eq!(f["ui:order"], 1);
        assert_eq!(f["ui:widget"], "select");
        assert_eq!(f["ui:placeholder"], "zh-CN");
        // 未设置的键不得出现（否则渲染器会把 undefined 当默认值）。
        assert!(!f.as_object().unwrap().contains_key("ui:visibleIf"));
    }

    #[test]
    fn extension_roundtrips_through_json() {
        let raw = j(r#"{"version":1,"label":"音量","order":3,"widget":"slider"}"#);
        let e = Extension::parse("/", &raw).unwrap();
        let s = serde_json::to_string(&e).unwrap();
        let back: Extension = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        assert!(s.contains("\"order\":3"));
        assert!(s.contains("\"label\":\"音量\""));
    }

    #[test]
    fn visible_if_uses_camel_case_field_name() {
        let e = Extension::parse("/", &j(r#"{"version":1,"visibleIf":{"prop":"a","value":1}}"#)).unwrap();
        assert!(e.visible_if.is_some());
        assert!(serde_json::to_string(&e).unwrap().contains("\"visibleIf\""));
    }

    #[test]
    fn kinds_are_exhaustive() {
        assert_eq!(value_kind(&Value::Null), "null");
        assert_eq!(value_kind(&Value::Bool(true)), "boolean");
        assert_eq!(value_kind(&Value::Number(1.into())), "integer");
        assert_eq!(value_kind(&Value::String("x".into())), "string");
        assert_eq!(value_kind(&Value::Array(Vec::new())), "array");
        assert_eq!(value_kind(&Value::Object(Map::new())), "object");
    }

    #[test]
    fn extension_is_refused_by_serde_for_unknown_fields() {
        let s = r#"{"version":1,"nope":1}"#;
        assert!(serde_json::from_str::<Extension>(s).is_err());
    }
}
