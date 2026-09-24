// §4.12/§4.13 写入前校验：按 schema 校验值（含范围/枚举）。
//
// 架构 §6.2 管线末端："写回前按 schema 校验（含范围/枚举）→ tauri-plugin-store"。
// 本模块只做**值 → schema** 的符合性判定，不产生错误文案（文案由前端
// `transformErrors` 负责，架构 §6.1）。
//
// 支持的关键字：type（含 "null" 数组形态）、enum、const、
// minLength/maxLength/pattern、minimum/maximum/multipleOf、
// required/properties/additionalProperties、items/minItems/maxItems/uniqueItems。

use std::collections::HashSet;
use std::sync::OnceLock;

use serde_json::Value;

/// 单条校验失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// JSON Pointer 形式的失败位置。
    pub path: String,
    /// 结构化的失败原因（供日志与断言，不是用户文案）。
    pub message: String,
}

impl ValidationError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self { path: path.into(), message: message.into() }
    }
}

/// 校验值是否符合 schema。成功返回 `Ok(())`，失败聚合**全部**失败点。
pub fn validate(schema: &Value, value: &Value) -> Result<(), Vec<ValidationError>> {
    let mut errs = Vec::new();
    check(schema, value, "/", &mut errs);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn check(schema: &Value, value: &Value, path: &str, errs: &mut Vec<ValidationError>) {
    let Some(m) = schema.as_object() else {
        return; // 无约束
    };

    // ── type ──────────────────────────────────────────────────────
    if let Some(t) = m.get("type") {
        let expected = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
            _ => Vec::new(),
        };
        if !expected.is_empty() && !expected.iter().any(|e| matches_type(value, e)) {
            errs.push(ValidationError::new(
                path,
                format!(
                    "期望类型 {}，实际 {}",
                    expected.join("|"),
                    actual_type(value)
                ),
            ));
            return; // 类型不符时后续关键字判定无意义
        }
    }

    // ── enum / const ──────────────────────────────────────────────
    if let Some(Value::Array(items)) = m.get("enum") {
        if !items.iter().any(|i| i == value) {
            errs.push(ValidationError::new(
                path,
                format!("值 {} 不在枚举 {items:?} 内", compact(value)),
            ));
        }
    }
    if let Some(c) = m.get("const") {
        if c != value {
            errs.push(ValidationError::new(path, format!("值 {} 不等于 const {}", compact(value), compact(c))));
        }
    }

    // ── 字符串 ────────────────────────────────────────────────────
    if let Some(s) = value.as_str() {
        if let Some(min) = m.get("minLength").and_then(|v| v.as_u64()) {
            let len = s.chars().count() as u64;
            if len < min {
                errs.push(ValidationError::new(path, format!("字符串长度 {len} 小于 minLength {min}")));
            }
        }
        if let Some(max) = m.get("maxLength").and_then(|v| v.as_u64()) {
            let len = s.chars().count() as u64;
            if len > max {
                errs.push(ValidationError::new(path, format!("字符串长度 {len} 大于 maxLength {max}")));
            }
        }
        if let Some(p) = m.get("pattern").and_then(|v| v.as_str()) {
            if !matches_pattern(p, s) {
                errs.push(ValidationError::new(path, format!("值 `{s}` 不匹配 pattern `{p}`")));
            }
        }
    }

    // ── 数字 ──────────────────────────────────────────────────────
    if let Some(n) = value.as_f64() {
        if let Some(min) = m.get("minimum").and_then(|v| v.as_f64()) {
            if n < min {
                errs.push(ValidationError::new(path, format!("值 {n} 小于 minimum {min}")));
            }
        }
        if let Some(max) = m.get("maximum").and_then(|v| v.as_f64()) {
            if n > max {
                errs.push(ValidationError::new(path, format!("值 {n} 大于 maximum {max}")));
            }
        }
        if let Some(min) = m.get("exclusiveMinimum").and_then(|v| v.as_f64()) {
            if n <= min {
                errs.push(ValidationError::new(path, format!("值 {n} 未大于 exclusiveMinimum {min}")));
            }
        }
        if let Some(max) = m.get("exclusiveMaximum").and_then(|v| v.as_f64()) {
            if n >= max {
                errs.push(ValidationError::new(path, format!("值 {n} 未小于 exclusiveMaximum {max}")));
            }
        }
        if let Some(div) = m.get("multipleOf").and_then(|v| v.as_f64()) {
            if div != 0.0 && ((n / div) % 1.0).abs() > 1e-9 {
                errs.push(ValidationError::new(path, format!("值 {n} 不是 multipleOf {div} 的整数倍")));
            }
        }
    }

    // ── 对象 ──────────────────────────────────────────────────────
    if let Some(props) = value.as_object() {
        if let Some(Value::Array(req)) = m.get("required") {
            for r in req.iter().filter_map(|v| v.as_str()) {
                if !props.contains_key(r) {
                    errs.push(ValidationError::new(path, format!("缺少必填属性 `{r}`")));
                }
            }
        }
        let schema_props = m.get("properties").and_then(Value::as_object);
        let additional = m.get("additionalProperties");
        let max_props = m.get("maxProperties").and_then(|v| v.as_u64());
        if let Some(mp) = max_props {
            if (props.len() as u64) > mp {
                errs.push(ValidationError::new(path, format!("属性数 {} 大于 maxProperties {mp}", props.len())));
            }
        }
        for (key, v) in props {
            let child_schema = schema_props.and_then(|p| p.get(key));
            match (child_schema, additional) {
                (Some(cs), _) => check(cs, v, &join(path, key), errs),
                (None, Some(Value::Bool(false))) => {
                    errs.push(ValidationError::new(path, format!("不允许额外属性 `{key}`")));
                }
                (None, Some(addl)) => check(addl, v, &join(path, key), errs),
                (None, _) => {} // 默认允许
            }
        }
    }

    // ── 数组 ──────────────────────────────────────────────────────
    if let Some(items) = value.as_array() {
        if let Some(min) = m.get("minItems").and_then(|v| v.as_u64()) {
            if (items.len() as u64) < min {
                errs.push(ValidationError::new(path, format!("数组长度 {} 小于 minItems {min}", items.len())));
            }
        }
        if let Some(max) = m.get("maxItems").and_then(|v| v.as_u64()) {
            if (items.len() as u64) > max {
                errs.push(ValidationError::new(path, format!("数组长度 {} 大于 maxItems {max}", items.len())));
            }
        }
        if m.get("uniqueItems").and_then(|v| v.as_bool()) == Some(true) {
            let mut seen: HashSet<String> = HashSet::new();
            for (i, item) in items.iter().enumerate() {
                let key = compact(item);
                if !seen.insert(key.clone()) {
                    errs.push(ValidationError::new(
                        path,
                        format!("数组第 {i} 项与前面重复：{key}"),
                    ));
                }
            }
        }
        let item_schema = m.get("items");
        if let Some(items_value) = item_schema {
            match items_value {
                // draft-07：`items` 是数组，按位置约束。
                Value::Array(prefix) => {
                    for (i, item) in items.iter().enumerate() {
                        if let Some(cs) = prefix.get(i) {
                            check(cs, item, &join(path, &i.to_string()), errs);
                        }
                    }
                }
                // 2020-12：`items` 是对象，约束每个元素。
                Value::Object(_) => {
                    for (i, item) in items.iter().enumerate() {
                        check(items_value, item, &join(path, &i.to_string()), errs);
                    }
                }
                _ => {}
            }
        }
    }

    // ── 组合 ──────────────────────────────────────────────────────
    if let Some(Value::Array(branches)) = m.get("oneOf") {
        let hits = branches.iter().filter(|b| matches_one(b, value)).count();
        if hits != 1 {
            errs.push(ValidationError::new(
                path,
                format!("oneOf 命中 {hits} 个分支（应为 1）"),
            ));
        }
    }
    if let Some(Value::Array(branches)) = m.get("anyOf") {
        if branches.is_empty() || !branches.iter().any(|b| matches_one(b, value)) {
            errs.push(ValidationError::new(path, "anyOf 未命中任何分支"));
        }
    }
    if let Some(Value::Array(branches)) = m.get("allOf") {
        for b in branches {
            let mut sub = Vec::new();
            check(b, value, path, &mut sub);
            errs.extend(sub);
        }
    }
}

/// 组合分支的"是否符合"判定（递归收集失败）。
fn matches_one(branch: &Value, value: &Value) -> bool {
    let mut errs = Vec::new();
    check(branch, value, "", &mut errs);
    errs.is_empty()
}

fn matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "null" => value.is_null(),
        "string" => value.is_string(),
        "boolean" => value.as_bool().is_some(),
        "integer" => matches!(value, Value::Number(n) if n.is_i64() || n.is_u64() || is_integral_f64(n.as_f64())),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn is_integral_f64(n: Option<f64>) -> bool {
    matches!(n, Some(v) if v.is_finite() && v.fract() == 0.0)
}

fn actual_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_pattern(pattern: &str, s: &str) -> bool {
    static RE_CACHE: OnceLock<()> = OnceLock::new();
    let _ = RE_CACHE.get_or_init(|| ());
    // 每次构造代价可接受：pattern 数量远小于校验次数。
    regex::Regex::new(pattern)
        .map(|re| re.is_match(s))
        .unwrap_or(false)
}

fn compact(v: &Value) -> String {
    match v {
        Value::String(s) => format!("`{s}`"),
        other => other.to_string(),
    }
}

fn join(base: &str, key: &str) -> String {
    let escaped = key.replace('~', "~0").replace('/', "~1");
    if base.ends_with('/') {
        format!("{base}{escaped}")
    } else {
        format!("{base}/{escaped}")
    }
}

/// 把校验失败聚合成单条 [`crate::error::SchemaError::Validation`]。
pub fn into_schema_error(errs: &[ValidationError]) -> crate::error::SchemaError {
    let text = errs
        .iter()
        .map(|e| format!("{} {}", e.path, e.message))
        .collect::<Vec<_>>()
        .join("；");
    crate::error::SchemaError::Validation(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serde_json::Map;

    fn ok(schema: Value, value: Value) {
        validate(&schema, &value).unwrap_or_else(|e| panic!("应通过：{e:?}"));
    }

    fn fail(schema: Value, value: Value, expect: &str) {
        let errs = validate(&schema, &value).expect_err("应失败");
        assert!(!errs.is_empty());
        assert!(
            errs.iter().any(|e| e.message.contains(expect)),
            "期望含 `{expect}`，实际 {errs:?}"
        );
    }

    // ── type ──────────────────────────────────────────────────────

    #[test]
    fn type_string_passes_and_rejects() {
        ok(json!({"type":"string"}), json!("hi"));
        fail(json!({"type":"string"}), json!(1), "期望类型 string");
    }

    #[test]
    fn type_integer_accepts_integral_f64() {
        ok(json!({"type":"integer"}), json!(1));
        ok(json!({"type":"integer"}), json!(1.0));
        fail(json!({"type":"integer"}), json!(1.5), "期望类型 integer");
    }

    #[test]
    fn type_number_accepts_int_and_float() {
        ok(json!({"type":"number"}), json!(1));
        ok(json!({"type":"number"}), json!(1.5));
        fail(json!({"type":"number"}), json!("1"), "期望类型 number");
    }

    #[test]
    fn type_array_includes_null_for_optional() {
        ok(json!({"type":["string","null"]}), json!("a"));
        ok(json!({"type":["string","null"]}), json!(Value::Null));
        fail(json!({"type":["string","null"]}), json!(3), "期望类型 string|null");
    }

    #[test]
    fn type_mismatch_skips_deeper_keywords() {
        let errs = validate(&json!({"type":"string","minLength":5}), &json!(1)).unwrap_err();
        assert_eq!(errs.len(), 1, "类型不符后不应再报 minLength：{errs:?}");
    }

    #[test]
    fn all_json_types_are_classified() {
        for (v, name) in [
            (json!(Value::Null), "null"),
            (json!(true), "boolean"),
            (json!(1), "integer"),
            (json!(1.5), "number"),
            (json!("s"), "string"),
            (json!([]), "array"),
            (json!({}), "object"),
        ] {
            assert_eq!(actual_type(&v), name);
        }
    }

    // ── enum / const ──────────────────────────────────────────────

    #[test]
    fn enum_passes_and_rejects() {
        ok(json!({"enum":["zh","en"]}), json!("zh"));
        fail(json!({"enum":["zh","en"]}), json!("ja"), "不在枚举");
    }

    #[test]
    fn enum_compares_structurally() {
        ok(json!({"enum":[{"a":1}]}), json!({"a":1}));
        fail(json!({"enum":[{"a":1}]}), json!({"a":2}), "不在枚举");
    }

    #[test]
    fn const_passes_and_rejects() {
        ok(json!({"const":"light"}), json!("light"));
        fail(json!({"const":"light"}), json!("dark"), "不等于 const");
    }

    // ── 字符串 ────────────────────────────────────────────────────

    #[test]
    fn min_max_length() {
        ok(json!({"type":"string","minLength":1,"maxLength":3}), json!("ab"));
        fail(json!({"minLength":2}), json!("a"), "小于 minLength");
        fail(json!({"maxLength":2}), json!("abc"), "大于 maxLength");
    }

    #[test]
    fn length_counts_chars_not_bytes() {
        // 中文 3 字 = 3 chars。
        ok(json!({"minLength":3,"maxLength":3}), json!("中文三"));
        fail(json!({"maxLength":2}), json!("中文三"), "大于 maxLength");
    }

    #[test]
    fn pattern_matches() {
        ok(json!({"pattern":"^com\\.[a-z]+$"}), json!("com.example"));
        fail(json!({"pattern":"^com\\.[a-z]+$"}), json!("COM.example"), "不匹配 pattern");
    }

    #[test]
    fn invalid_regex_is_treated_as_no_match() {
        fail(json!({"pattern":"("}), json!("anything"), "不匹配 pattern");
    }

    // ── 数字 ──────────────────────────────────────────────────────

    #[test]
    fn minimum_maximum() {
        ok(json!({"minimum":0,"maximum":100}), json!(50));
        fail(json!({"minimum":0}), json!(-1), "小于 minimum");
        fail(json!({"maximum":100}), json!(101), "大于 maximum");
    }

    #[test]
    fn exclusive_bounds() {
        ok(json!({"exclusiveMinimum":0,"exclusiveMaximum":10}), json!(5));
        fail(json!({"exclusiveMinimum":0}), json!(0), "未大于 exclusiveMinimum");
        fail(json!({"exclusiveMaximum":10}), json!(10), "未小于 exclusiveMaximum");
    }

    #[test]
    fn multiple_of() {
        ok(json!({"multipleOf":5}), json!(10));
        fail(json!({"multipleOf":5}), json!(12), "不是 multipleOf 5 的整数倍");
    }

    #[test]
    fn boundary_values_are_inclusive() {
        ok(json!({"minimum":0,"maximum":100}), json!(0));
        ok(json!({"minimum":0,"maximum":100}), json!(100));
    }

    // ── 对象 ──────────────────────────────────────────────────────

    #[test]
    fn required_collects_all_missing() {
        let errs = validate(
            &json!({"type":"object","required":["a","b","c"],"properties":{"a":{"type":"string"},"b":{"type":"string"},"c":{"type":"string"}}}),
            &json!({}),
        )
        .unwrap_err();
        assert_eq!(errs.len(), 3, "三个缺失都应报出：{errs:?}");
    }

    #[test]
    fn property_values_are_validated_recursively() {
        let errs = validate(
            &json!({"type":"object","properties":{"n":{"type":"integer","minimum":10}}}),
            &json!({"n":1}),
        )
        .unwrap_err();
        assert_eq!(errs[0].path, "/n");
    }

    #[test]
    fn pointer_escapes_slash_and_tilde() {
        let errs = validate(
            &json!({"type":"object","properties":{"a/b":{"type":"integer"}}}),
            &json!({"a/b":"x"}),
        )
        .unwrap_err();
        assert_eq!(errs[0].path, "/a~1b");

        let errs2 = validate(
            &json!({"type":"object","properties":{"c~d":{"type":"integer"}}}),
            &json!({"c~d":"x"}),
        )
        .unwrap_err();
        assert_eq!(errs2[0].path, "/c~0d");
    }

    #[test]
    fn additional_properties_false_rejects_extras() {
        let errs = validate(
            &json!({"type":"object","additionalProperties":false,"properties":{"a":{"type":"string"}}}),
            &json!({"a":"x","b":"y"}),
        )
        .unwrap_err();
        assert!(errs.iter().any(|e| e.message.contains("不允许额外属性 `b`")));
    }

    #[test]
    fn additional_properties_schema_validates_extras() {
        validate(
            &json!({"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":{"type":"integer"}}),
            &json!({"a":"x","b":1}),
        )
        .unwrap();
        let errs = validate(
            &json!({"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":{"type":"integer"}}),
            &json!({"a":"x","b":"wrong"}),
        )
        .unwrap_err();
        assert_eq!(errs[0].path, "/b");
    }

    #[test]
    fn max_properties() {
        fail(json!({"maxProperties":1}), json!({"a":1,"b":2}), "大于 maxProperties");
        ok(json!({"maxProperties":2}), json!({"a":1,"b":2}));
    }

    // ── 数组 ──────────────────────────────────────────────────────

    #[test]
    fn items_schema_applies_to_every_element() {
        let errs = validate(&json!({"type":"array","items":{"type":"integer"}}), &json!(["a", 1])).unwrap_err();
        assert_eq!(errs[0].path, "/0");
    }

    #[test]
    fn min_max_items() {
        ok(json!({"minItems":1,"maxItems":2}), json!([1, 2]));
        fail(json!({"minItems":2}), json!([1]), "小于 minItems");
        fail(json!({"maxItems":1}), json!([1, 2]), "大于 maxItems");
    }

    #[test]
    fn unique_items_detects_duplicates() {
        ok(json!({"uniqueItems":true}), json!([1, 2, 3]));
        let errs = validate(&json!({"uniqueItems":true}), &json!(["a", "a"])).unwrap_err();
        assert!(errs[0].message.contains("重复"));
    }

    #[test]
    fn draft7_prefix_items_are_checked_positionally() {
        let errs = validate(
            &json!({"items":[{"type":"integer"},{"type":"string"}]}),
            &json!(["x", 1]),
        )
        .unwrap_err();
        assert_eq!(errs[0].path, "/0");
        assert_eq!(errs[1].path, "/1");
    }

    // ── 组合 ──────────────────────────────────────────────────────

    #[test]
    fn one_of_requires_exactly_one() {
        ok(json!({"oneOf":[{"const":"a"},{"const":"b"}]}), json!("a"));
        // 两个分支都命中 → 违规。
        fail(
            json!({"oneOf":[{"type":"string","minLength":1},{"type":"string","maxLength":3}]}),
            json!("ab"),
            "oneOf 命中",
        );
        fail(json!({"oneOf":[{"type":"object"}]}), json!("s"), "oneOf 命中");
    }

    #[test]
    fn any_of_requires_at_least_one() {
        ok(json!({"anyOf":[{"type":"string"},{"type":"null"}]}), json!("s"));
        ok(json!({"anyOf":[{"type":"string"},{"type":"null"}]}), json!(Value::Null));
        fail(json!({"anyOf":[{"type":"string"},{"type":"null"}]}), json!(1), "anyOf 未命中");
    }

    #[test]
    fn any_of_with_empty_array_is_a_failure() {
        fail(json!({"anyOf":[]}), json!("x"), "anyOf 未命中");
    }

    #[test]
    fn all_of_requires_all() {
        ok(json!({"allOf":[{"type":"integer"},{"minimum":1}]}), json!(5));
        fail(json!({"allOf":[{"type":"integer"},{"minimum":1}]}), json!(0), "小于 minimum");
    }

    // ── 聚合与工具 ────────────────────────────────────────────────

    #[test]
    fn errors_aggregate_across_branches() {
        let errs = validate(
            &json!({"type":"object","required":["a","b"],"properties":{"a":{"type":"integer"},"b":{"type":"integer"}}}),
            &json!({"a":"x"}),
        )
        .unwrap_err();
        assert!(errs.iter().any(|e| e.path == "/a"));
        assert!(errs.iter().any(|e| e.message.contains("缺少必填属性 `b`")));
    }

    #[test]
    fn into_schema_error_joins_all() {
        let errs = validate(&json!({"type":"integer"}), &json!("x")).unwrap_err();
        let e = into_schema_error(&errs);
        assert!(matches!(e, crate::error::SchemaError::Validation(ref m) if m.contains("期望类型 integer")));
    }

    #[test]
    fn empty_constraints_always_pass() {
        for v in [json!(42), json!("s"), json!(Value::Null), json!({}), json!([])] {
            assert!(validate(&Value::Object(Map::new()), &v).is_ok());
        }
    }

    #[test]
    fn non_object_schema_is_unconstrained() {
        assert!(validate(&Value::Null, &json!(42)).is_ok());
        assert!(validate(&json!("a string schema"), &json!(42)).is_ok());
    }

    #[test]
    fn unconstrained_schema_passes_everything() {
        let schema = json!({});
        assert!(validate(&schema, &json!(42)).is_ok());
        assert!(validate(&schema, &json!("x")).is_ok());
        assert!(validate(&schema, &json!(Value::Null)).is_ok());
    }
}
