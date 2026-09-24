// §8-12 门禁：schema 产物形态静态断言。
//
// 计划 §8-12："§4.13 产物中禁止出现嵌套 oneOf/anyOf 与未展开 $ref；
// Rust enum 必须内部标签化（防 #4666/#4505 回归）。"
//
// 判定口径：组合键（oneOf/anyOf/allOf）**只允许出现在非组合位置**——
// 任何组合键出现在另一个组合的成员列表内即判违规。顶层（含 `properties`
// 之下）的组合是允许的，enum 标签化后必须留在这一层。
//
// 本模块是唯一的判定点，供管线调用、供测试断言、供 CI 复用。

use serde_json::Value;

use crate::error::{SchemaError, SchemaResult};

/// 全部组合键。
pub const COMBO_KEYS: &[&str] = &["oneOf", "anyOf", "allOf"];

/// 断言整份 schema 是"平面"的：
/// - 不允许任何 `$ref`；
/// - 不允许 `$defs`/`definitions` 残留；
/// - 不允许组合键出现在组合成员内。
pub fn assert_flat(doc: &Value) -> SchemaResult<()> {
    walk(doc, "/", false)
}

fn walk(node: &Value, path: &str, in_combo: bool) -> SchemaResult<()> {
    match node {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk(item, &join_ptr(path, &i.to_string()), in_combo)?;
            }
            Ok(())
        }
        Value::Object(m) => {
            if let Some(r) = m.get("$ref") {
                return Err(SchemaError::UnexpandedRef(
                    r.as_str().unwrap_or_default().to_string(),
                ));
            }

            for k in m.keys() {
                let kpath = join_ptr(path, k);
                if COMBO_KEYS.contains(&k.as_str()) {
                    let Value::Array(members) = &m[k] else {
                        return Err(SchemaError::Structure(format!("`{kpath}` 不是数组")));
                    };
                    if in_combo {
                        return Err(SchemaError::NestedCombo(k.clone(), kpath));
                    }
                    for (i, member) in members.iter().enumerate() {
                        let mp = join_ptr(&kpath, &i.to_string());
                        walk(member, &mp, true)?;
                    }
                } else if k == "$defs" || k == "definitions" {
                    return Err(SchemaError::UnexpandedRef(kpath));
                } else {
                    walk(&m[k], &kpath, in_combo)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 拼 JSON Pointer 段，避免根路径 `/` 产生 `//`。
fn join_ptr(base: &str, key: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{key}")
    } else {
        format!("{base}/{key}")
    }
}

/// 报告 schema 的形态指标，供测试与 CI 快照。
#[derive(Debug, Default, Clone)]
pub struct ShapeReport {
    /// 出现在非组合位置的组合键次数（enum 标签化后的形态）。
    pub top_level_combos: usize,
    /// 组合成员总数。
    pub combo_members: usize,
    /// 组合成员的 JSON Pointer 路径（供快照比对）。
    pub combo_paths: Vec<String>,
    /// 剩余 `$ref` 数（必须为 0）。
    pub remaining_refs: usize,
    /// 剩余 `x-tauron` 扩展数（必须为 0，已编译进 uiSchema）。
    pub remaining_extensions: usize,
    /// 节点总数。
    pub nodes: usize,
}

/// 统计形态。不做判定（判定由 {@link assert_flat}）。
pub fn report(doc: &Value) -> ShapeReport {
    let mut r = ShapeReport::default();
    walk_report(doc, "/", false, &mut r);
    r
}

fn walk_report(node: &Value, path: &str, in_combo: bool, r: &mut ShapeReport) {
    r.nodes += 1;
    match node {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk_report(item, &join_ptr(path, &i.to_string()), in_combo, r);
            }
        }
        Value::Object(m) => {
            if m.get("$ref").is_some() {
                r.remaining_refs += 1;
            }
            if m.get(crate::xoc::EXT_KEY).is_some() {
                r.remaining_extensions += 1;
            }

            for (k, v) in m {
                let kpath = join_ptr(path, k);
                if COMBO_KEYS.contains(&k.as_str()) {
                    if !in_combo {
                        r.top_level_combos += 1;
                    }
                    if let Value::Array(members) = v {
                        for (i, member) in members.iter().enumerate() {
                            let mp = join_ptr(&kpath, &i.to_string());
                            r.combo_members += 1;
                            r.combo_paths.push(mp.clone());
                            walk_report(member, &mp, true, r);
                        }
                    }
                } else {
                    walk_report(v, &kpath, in_combo, r);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flat_schema_passes() {
        let doc = json!({"type":"object","properties":{"a":{"type":"string"}}});
        assert!(assert_flat(&doc).is_ok());
        let r = report(&doc);
        assert_eq!(r.remaining_refs, 0);
        assert!(r.nodes >= 3);
    }

    #[test]
    fn top_level_combo_is_allowed() {
        let doc = json!({"oneOf":[
            {"type":"object","properties":{"kind":{"enum":["a"]}}},
            {"type":"object","properties":{"combo":{"type":"string"}}}
        ]});
        assert!(assert_flat(&doc).is_ok());
        let r = report(&doc);
        assert_eq!(r.top_level_combos, 1);
        assert_eq!(r.combo_members, 2);
        assert_eq!(r.combo_paths.len(), 2);
    }

    #[test]
    fn combo_inside_properties_is_allowed() {
        let doc = json!({"type":"object","properties":{"theme":{"oneOf":[{"const":"a"},{"const":"b"}]}}});
        assert!(assert_flat(&doc).is_ok());
        let r = report(&doc);
        assert_eq!(r.top_level_combos, 1);
        assert_eq!(r.combo_paths[0], "/properties/theme/oneOf/0");
    }

    #[test]
    fn nested_combo_is_rejected() {
        let doc = json!({"oneOf":[
            {"oneOf":[{"type":"string"},{"type":"number"}]},
            {"type":"object"}
        ]});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::NestedCombo(k, p) => {
                assert_eq!(k, "oneOf");
                assert!(p.starts_with("/oneOf/0"), "{p}");
            }
            other => panic!("期望 NestedCombo，实际 {other:?}"),
        }
    }

    #[test]
    fn any_of_nesting_is_also_rejected() {
        let doc = json!({"anyOf":[{"anyOf":[{"type":"string"}]}]});
        let e = assert_flat(&doc).unwrap_err();
        assert!(matches!(e, SchemaError::NestedCombo(ref k, _) if k == "anyOf"));
    }

    #[test]
    fn combo_inside_all_of_is_rejected() {
        let doc = json!({"allOf":[{"oneOf":[{"type":"string"}]}]});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::NestedCombo(ref k, p) => {
                assert_eq!(k, "oneOf");
                assert_eq!(p, "/allOf/0/oneOf");
            }
            other => panic!("期望 NestedCombo，实际 {other:?}"),
        }
    }

    #[test]
    fn all_of_with_plain_members_is_allowed() {
        let doc = json!({"allOf":[{"type":"object"},{"minimum":1}]});
        assert!(assert_flat(&doc).is_ok());
    }

    #[test]
    fn combo_inside_array_is_traversed() {
        let doc = json!({"oneOf":[{"anyOf":[{"type":"string"}]}]});
        let e = assert_flat(&doc).unwrap_err();
        assert!(matches!(e, SchemaError::NestedCombo(ref k, _) if k == "anyOf"));
    }

    #[test]
    fn remaining_ref_is_rejected() {
        let doc = json!({"$ref":"#/$defs/Missing"});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::UnexpandedRef(p) => assert_eq!(p, "#/$defs/Missing"),
            other => panic!("期望 UnexpandedRef，实际 {other:?}"),
        }
    }

    #[test]
    fn leftover_defs_block_is_rejected() {
        let doc = json!({"$defs":{"A":{"type":"string"}},"type":"string"});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::UnexpandedRef(p) => assert!(p.contains("$defs"), "{p}"),
            other => panic!("期望 UnexpandedRef，实际 {other:?}"),
        }
    }

    #[test]
    fn unexpanded_ref_inside_properties_is_rejected() {
        let doc = json!({"type":"object","properties":{"x":{"$ref":"#/$defs/A"}}});
        assert!(matches!(assert_flat(&doc).unwrap_err(), SchemaError::UnexpandedRef(_)));
    }

    #[test]
    fn combo_member_with_ref_is_rejected() {
        let doc = json!({"oneOf":[{"$ref":"#/$defs/A"},{"type":"null"}]});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::UnexpandedRef(p) => assert_eq!(p, "#/$defs/A"),
            other => panic!("期望 UnexpandedRef，实际 {other:?}"),
        }
    }

    #[test]
    fn report_counts_extensions() {
        let doc = json!({"type":"object","properties":{"a":{"type":"string","x-tauron":{"version":1}}}});
        let r = report(&doc);
        assert_eq!(r.remaining_extensions, 1);
    }

    #[test]
    fn report_counts_refs_and_nodes() {
        let doc = json!({"$ref":"#/$defs/A","type":"object","properties":{"a":1}});
        let r = report(&doc);
        assert_eq!(r.remaining_refs, 1);
        assert!(r.nodes >= 3);
    }

    #[test]
    fn nested_combo_is_not_counted_as_top_level() {
        let doc = json!({"oneOf":[{"oneOf":[{"type":"string"}]}]});
        let r = report(&doc);
        assert_eq!(r.top_level_combos, 1, "内层组合不计入顶层");
        assert_eq!(r.combo_members, 2);
    }

    #[test]
    fn primitive_values_are_not_reported_as_structure_errors() {
        let doc = json!({"type":"string","enum":[1,2,3],"description":"x"});
        assert!(assert_flat(&doc).is_ok());
    }

    #[test]
    fn combo_must_be_an_array() {
        let doc = json!({"oneOf":{"type":"string"}});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::Structure(m) => assert!(m.contains("oneOf"), "{m}"),
            other => panic!("期望 Structure，实际 {other:?}"),
        }
    }

    #[test]
    fn deeply_nested_combo_reports_the_full_path() {
        let doc = json!({"type":"object","properties":{"a":{"oneOf":[{"anyOf":[{"type":"string"}]}]}}});
        match assert_flat(&doc).unwrap_err() {
            SchemaError::NestedCombo(k, p) => {
                assert_eq!(k, "anyOf");
                assert_eq!(p, "/properties/a/oneOf/0/anyOf");
            }
            other => panic!("期望 NestedCombo，实际 {other:?}"),
        }
    }

    #[test]
    fn join_ptr_avoids_double_slash_at_root() {
        assert_eq!(join_ptr("/", "a"), "/a");
        assert_eq!(join_ptr("/a", "b"), "/a/b");
        assert_eq!(join_ptr("", "a"), "/a");
    }
}
