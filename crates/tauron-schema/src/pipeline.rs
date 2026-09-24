// §4.13 schema 规范化管线（架构 §6.2 定稿管线）。
//
// 三步：① `$ref` 本地展开 ② enum / Option 规范化（内部标签化 + 去 anyOf）
// ③ `x-tauron` → uiSchema。产物是无嵌套 oneOf/anyOf、无 `$ref` 的平面
// schema，同一份喂 RJSF（React）与 WC 声明式渲染器（架构 §6.2）。
//
// 为什么必须做：schemars 1.2.2 默认 draft 2020-12，必然产出 `$defs`+`$ref`
// 且 Rust enum 默认产出 oneOf——正撞 RJSF #4666（嵌套 oneOf 不支持）与
// #4505（bundled `$ref` 未支持），表现为"表单渲染不出来"且报错难懂（ADR-12）。

use serde_json::{Map, Value};

use crate::error::{SchemaError, SchemaResult};
use crate::gate;
use crate::xoc::{self, Extension};

/// `$ref` 展开最大深度。超限即视为环（schema 自引用渲染不出来，拒绝而非截断）。
pub const MAX_REF_DEPTH: usize = 32;

/// 规范化产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// 平面 schema：无嵌套 oneOf/anyOf、无 `$ref`、无 `$defs`、无 `x-tauron`。
    pub schema: Value,
    /// `x-tauron` 编译出的 uiSchema（`ui:*` 键，路径嵌套）。
    pub ui_schema: Value,
    /// 遇到的扩展节点数。
    pub extension_count: usize,
}

impl Compiled {
    pub fn is_empty_ui(&self) -> bool {
        matches!(&self.ui_schema, Value::Object(m) if m.is_empty())
    }
}

/// 规范化一份 schema。
pub fn compile(raw: &Value) -> SchemaResult<Compiled> {
    let mut refs = Vec::new();
    let schema = inline(raw, raw, &mut Vec::new(), &mut refs, 0, "/", &[], false)?;
    gate::assert_flat(&schema)?;

    let extension_count = refs.len();
    let mut ui = Map::new();
    for (path, ext) in refs {
        insert_at(&mut ui, &path, &ext.to_ui_fragment());
    }

    Ok(Compiled { schema, ui_schema: Value::Object(ui), extension_count })
}

/// 编译并序列化为 `{schema, uiSchema}` 文档（供前端消费）。
pub fn compile_to_doc(raw: &Value) -> SchemaResult<Value> {
    let c = compile(raw)?;
    Ok(Value::Object(Map::from_iter([
        ("schema".into(), c.schema),
        ("uiSchema".into(), c.ui_schema),
    ])))
}

// ──────────────────────────────────────────────────────────────────────────
// ① $ref 展开 + ② 组合规范化
// ──────────────────────────────────────────────────────────────────────────

/// 递归展开 `$ref`、收集 `x-tauron`、规范化组合节点。
///
/// `stack` 记录正在解析的 `$ref` 链用于环检测；`refs` 收集扩展节点（按
/// **uiSchema 路径**而非 JSON Pointer 定位——RJSF 的 uiSchema 键是字段名
/// 与下标，不含 `properties`/`items` 这类 schema 关键字）；
/// `in_combo` 标记是否已在组合分支内（组合内禁止再出现组合）。
fn inline(
    node: &Value,
    doc: &Value,
    stack: &mut Vec<String>,
    refs: &mut Vec<(Vec<String>, Extension)>,
    depth: usize,
    path: &str,
    ui_path: &[String],
    in_combo: bool,
) -> SchemaResult<Value> {
    let Value::Object(m) = node else { return Ok(node.clone()) };

    // ── `$ref` 展开 ────────────────────────────────────────────────
    if let Some(r) = m.get("$ref") {
        let ptr = r.as_str().ok_or_else(|| SchemaError::Structure(format!("`{path}/$ref` 不是字符串")))?;
        let target = resolve_pointer(doc, ptr)?;
        if depth + 1 > MAX_REF_DEPTH || stack.contains(&ptr.to_string()) {
            return Err(SchemaError::RefCycle(ptr.to_string()));
        }
        stack.push(ptr.to_string());
        let expanded = inline(&target, doc, stack, refs, depth + 1, path, ui_path, in_combo)?;
        stack.pop();

        // 与 `$ref` 并列的扩展属于当前位置，不是被引用节点。
        if let Some(e) = m.get(xoc::EXT_KEY) {
            refs.push((ui_path.to_vec(), Extension::parse(path, e)?));
        }

        // 2020-12 允许 `$ref` 与兄弟键并列：先放展开结果，兄弟键可覆盖。
        let mut merged = expanded;
        for (k, v) in m {
            match k.as_str() {
                "$ref" | xoc::EXT_KEY => continue,
                _ => {
                    if let Value::Object(obj) = &mut merged {
                        obj.insert(
                            k.clone(),
                            inline(v, doc, stack, refs, depth + 1, path, ui_path, in_combo)?,
                        );
                    }
                }
            }
        }
        return Ok(merged);
    }

    let mut out = Map::new();

    // ── 扩展收集 ────────────────────────────────────────────────────
    if let Some(e) = m.get(xoc::EXT_KEY) {
        refs.push((ui_path.to_vec(), Extension::parse(path, e)?));
    }

    for (k, v) in m {
        match k.as_str() {
            "$defs" | "definitions" | xoc::EXT_KEY => {}

            // ── 组合节点 ────────────────────────────────────────────
            key if gate::COMBO_KEYS.contains(&key) => {
                let Value::Array(members) = v else {
                    return Err(SchemaError::Structure(format!("`{path}/{key}` 不是数组")));
                };
                if in_combo {
                    return Err(SchemaError::NestedCombo(key.to_string(), join_ptr(path, key)));
                }
                let mut resolved: Vec<Value> = Vec::with_capacity(members.len());
                for (i, mem) in members.iter().enumerate() {
                    let mp = format!("{}/{}", join_ptr(path, key), i);
                    let child_ui = push(ui_path, i.to_string());
                    let mut r = inline(mem, doc, stack, refs, depth, &mp, &child_ui, true)?;
                    // #4666 缓解：object 形态的成员补 `type`（维护者建议的 workaround）。
                    if let Value::Object(obj) = &mut r {
                        if !obj.contains_key("type")
                            && (obj.contains_key("properties") || obj.contains_key("required"))
                        {
                            obj.insert("type".into(), Value::String("object".into()));
                        }
                    }
                    resolved.push(r);
                }
                if key == "anyOf" {
                    // Option<T> 折叠：把 `anyOf` 整个键换成合并后的 schema。
                    if let Some(collapsed) = collapse_any_of_null(&resolved) {
                        if let Value::Object(parts) = collapsed {
                            out.extend(parts);
                        }
                        continue;
                    }
                }
                out.insert(k.clone(), Value::Array(resolved));
            }

            // ── properties / patternProperties ──────────────────────
            "properties" | "patternProperties" => {
                let Value::Object(props) = v else { continue };
                let mut child = Map::new();
                for (name, sub) in props {
                    let child_path = format!("{}/{}", join_ptr(path, k), escape_pointer(name));
                    let child_ui = push(ui_path, name.clone());
                    child.insert(
                        name.clone(),
                        inline(sub, doc, stack, refs, depth, &child_path, &child_ui, in_combo)?,
                    );
                }
                out.insert(k.clone(), Value::Object(child));
            }

            // ── items（2020-12 对象 / draft-07 前缀数组）────────────
            "items" => {
                let child = match v {
                    Value::Array(prefix) => {
                        let mut arr = Map::new();
                        for (i, sub) in prefix.iter().enumerate() {
                            let child_path = format!("{}/{}", join_ptr(path, "items"), i);
                            let child_ui = push(ui_path, format!("[{i}]"));
                            arr.insert(
                                i.to_string(),
                                inline(sub, doc, stack, refs, depth, &child_path, &child_ui, in_combo)?,
                            );
                        }
                        Value::Object(arr)
                    }
                    other => inline(other, doc, stack, refs, depth, path, ui_path, in_combo)?,
                };
                out.insert(k.to_string(), child);
            }

            // ── 其余关键字：原样递归，uiSchema 路径不变 ────────────────
            _ => {
                let vp = join_ptr(path, k);
                out.insert(k.clone(), inline(v, doc, stack, refs, depth, &vp, ui_path, in_combo)?);
            }
        }
    }

    Ok(Value::Object(out))
}

/// `anyOf: [T, {type:"null"}]` → `T` 合并 `type` 数组（Option<T> 规范化）。
///
/// 返回 `None` 表示不折叠（非二元、非 null 形态、或非 null 分支缺 `type`）——
/// 此时保持原样，由门禁决定是否放行。
fn collapse_any_of_null(members: &[Value]) -> Option<Value> {
    if members.len() != 2 {
        return None;
    }
    let null_idx = members.iter().position(is_null_schema)?;
    let Value::Object(mut obj) = members[1 - null_idx].clone() else {
        return None;
    };

    let mut types = match obj.remove("type") {
        Some(Value::String(s)) => vec![Value::String(s)],
        Some(Value::Array(a)) => a,
        _ => return None,
    };
    if !types.contains(&Value::String("null".into())) {
        types.push(Value::String("null".into()));
    }
    let value = if types.len() == 1 { types.into_iter().next().unwrap() } else { Value::Array(types) };
    obj.insert("type".into(), value);
    Some(Value::Object(obj))
}

fn is_null_schema(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.get("type").and_then(Value::as_str) == Some("null"))
}

// ──────────────────────────────────────────────────────────────────────────
// JSON Pointer
// ──────────────────────────────────────────────────────────────────────────

/// 解析本地 JSON Pointer（RFC 6901）。只接受 `#...` 形式的文档内引用。
fn resolve_pointer<'a>(doc: &'a Value, ref_: &str) -> SchemaResult<&'a Value> {
    let Some(ptr) = ref_.strip_prefix('#') else {
        return Err(SchemaError::UnresolvedRef(ref_.to_string()));
    };
    if ptr.is_empty() {
        return Ok(doc);
    }
    let mut cur = doc;
    for raw_token in ptr.split('/').skip(1) {
        let token = raw_token.replace("~1", "/").replace("~0", "~");
        let next = match cur {
            Value::Object(m) => m.get(&token),
            Value::Array(a) => {
                let idx: usize =
                    token.parse().map_err(|_| SchemaError::UnresolvedRef(ref_.to_string()))?;
                a.get(idx)
            }
            _ => None,
        };
        match next {
            Some(v) => cur = v,
            None => return Err(SchemaError::UnresolvedRef(ref_.to_string())),
        }
    }
    Ok(cur)
}

fn join_ptr(base: &str, key: &str) -> String {
    if base.ends_with('/') {
        format!("{base}{key}")
    } else {
        format!("{base}/{key}")
    }
}

fn push(base: &[String], part: String) -> Vec<String> {
    let mut v = base.to_vec();
    v.push(part);
    v
}

fn escape_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// 在嵌套对象树里按 path 写入 fragment（路径不存在则创建）。
fn insert_at(root: &mut Map<String, Value>, path: &[String], fragment: &Value) {
    if path.is_empty() {
        merge_fragment(root, fragment);
        return;
    }
    let mut cur = root;
    for key in path {
        cur = descend(cur, key);
    }
    merge_fragment(cur, fragment);
}

/// 下潜一级：目标不存在则创建空对象。
fn descend<'a>(m: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let v = m.entry(key.to_string()).or_insert_with(|| Value::Object(Map::new()));
    v.as_object_mut().expect("路径节点必须是对象")
}

fn merge_fragment(target: &mut Map<String, Value>, fragment: &Value) {
    if let Value::Object(frag) = fragment {
        for (k, v) in frag {
            target.insert(k.clone(), v.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::report;
    use serde_json::json;

    fn compile_ok(raw: Value) -> Compiled {
        compile(&raw).expect("规范化应成功")
    }

    // ── $ref 展开 ────────────────────────────────────────────────

    #[test]
    fn resolves_local_ref_from_defs() {
        let raw = json!({
            "$defs": {"Name": {"type":"string","minLength":1}},
            "type":"object",
            "properties":{"name":{"$ref":"#/$defs/Name"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["name"], json!({"type":"string","minLength":1}));
        assert!(c.schema.get("$defs").is_none(), "产物不应残留 $defs");
        assert!(gate::assert_flat(&c.schema).is_ok());
    }

    #[test]
    fn resolves_draft7_definitions_alias() {
        let raw = json!({
            "definitions": {"a":{"b":{"type":"number"}}},
            "type":"object",
            "properties":{"x":{"$ref":"#/definitions/a/b"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["x"], json!({"type":"number"}));
    }

    #[test]
    fn resolves_array_pointer() {
        let raw = json!({
            "definitions": ["str", {"type":"number"}],
            "type":"object",
            "properties":{"x":{"$ref":"#/definitions/1"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["x"], json!({"type":"number"}));
    }

    #[test]
    fn resolves_tilde_escapes() {
        let raw = json!({
            "$defs": {"a/b~c": {"type":"string"}},
            "type":"object",
            "properties":{"x":{"$ref":"#/$defs/a~1b~0c"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["x"], json!({"type":"string"}));
    }

    #[test]
    fn merges_ref_siblings() {
        let raw = json!({
            "$defs": {"Name": {"type":"string"}},
            "type":"object",
            "properties":{"name":{"$ref":"#/$defs/Name","description":"姓名"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["name"]["type"], "string");
        assert_eq!(c.schema["properties"]["name"]["description"], "姓名");
    }

    #[test]
    fn sibling_overrides_ref_key() {
        let raw = json!({
            "$defs": {"Name": {"type":"string"}},
            "type":"object",
            "properties":{"name":{"$ref":"#/$defs/Name","type":"number"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["name"]["type"], "number");
    }

    #[test]
    fn chained_refs_expand() {
        let raw = json!({
            "$defs": {"A": {"$ref":"#/$defs/B"}, "B": {"type":"string"}},
            "type":"object",
            "properties":{"x":{"$ref":"#/$defs/A"}}
        });
        let c = compile_ok(raw);
        assert_eq!(c.schema["properties"]["x"], json!({"type":"string"}));
    }

    #[test]
    fn cycle_is_rejected() {
        let raw = json!({
            "$defs": {"A": {"type":"object","properties":{"next":{"$ref":"#/$defs/A"}}}},
            "type":"object",
            "properties":{"x":{"$ref":"#/$defs/A"}}
        });
        match compile(&raw).unwrap_err() {
            SchemaError::RefCycle(p) => assert_eq!(p, "#/$defs/A"),
            other => panic!("期望 RefCycle，实际 {other:?}"),
        }
    }

    #[test]
    fn direct_self_ref_is_rejected() {
        let raw = json!({"$defs":{"X":{"$ref":"#/$defs/X"}},"type":"object","properties":{"x":{"$ref":"#/$defs/X"}}});
        assert!(matches!(compile(&raw).unwrap_err(), SchemaError::RefCycle(_)));
    }

    #[test]
    fn too_deep_ref_chain_is_rejected() {
        let mut defs = Map::new();
        for i in 0..60 {
            defs.insert(
                format!("D{i}"),
                Value::Object(Map::from_iter([
                    ("type".into(), Value::String("object".into())),
                    ("properties".into(), Value::Object(Map::from_iter([(
                        "next".into(),
                        Value::Object(Map::from_iter([("$ref".into(), Value::String(
                            format!("#/$defs/D{}", i + 1),
                        ))])),
                    )]))),
                ])),
            );
        }
        let raw = json!({"$defs":defs,"type":"object","properties":{"x":{"$ref":"#/$defs/D0"}}});
        assert!(matches!(compile(&raw).unwrap_err(), SchemaError::RefCycle(_)));
    }

    #[test]
    fn remote_ref_is_rejected() {
        let raw = json!({"type":"object","properties":{"x":{"$ref":"https://example.com/schema"}}});
        match compile(&raw).unwrap_err() {
            SchemaError::UnresolvedRef(p) => assert!(p.starts_with("https://"), "{p}"),
            other => panic!("期望 UnresolvedRef，实际 {other:?}"),
        }
    }

    #[test]
    fn missing_ref_is_rejected() {
        let raw = json!({"type":"object","properties":{"x":{"$ref":"#/$defs/Missing"}}});
        match compile(&raw).unwrap_err() {
            SchemaError::UnresolvedRef(p) => assert_eq!(p, "#/$defs/Missing"),
            other => panic!("期望 UnresolvedRef，实际 {other:?}"),
        }
    }

    #[test]
    fn non_string_ref_is_rejected() {
        let raw = json!({"$ref":42});
        match compile(&raw).unwrap_err() {
            SchemaError::Structure(m) => assert!(m.contains("$ref"), "{m}"),
            other => panic!("期望 Structure，实际 {other:?}"),
        }
    }

    #[test]
    fn empty_pointer_resolves_to_root() {
        let raw = json!({"$ref":"#"});
        // `#` 指向文档根自身 → 自引用，按环处理。
        assert!(matches!(compile(&raw).unwrap_err(), SchemaError::RefCycle(p) if p == "#"));
    }

    // ── Option<T> 折叠 ────────────────────────────────────────────

    #[test]
    fn collapses_option_string() {
        let c = compile_ok(json!({"anyOf":[{"type":"string"},{"type":"null"}]}));
        assert_eq!(c.schema, json!({"type":["string","null"]}));
    }

    #[test]
    fn collapses_option_with_null_first() {
        let c = compile_ok(json!({"anyOf":[{"type":"null"},{"type":"string"}]}));
        assert_eq!(c.schema, json!({"type":["string","null"]}));
    }

    #[test]
    fn collapses_option_object() {
        let c = compile_ok(json!({
            "anyOf":[
                {"type":"object","properties":{"a":{"type":"string"}}},
                {"type":"null"}
            ]
        }));
        let t = c.schema["type"].as_array().unwrap();
        assert!(t.contains(&json!("object")));
        assert!(t.contains(&json!("null")));
        assert!(c.schema["properties"]["a"].is_object());
    }

    #[test]
    fn collapses_option_with_existing_null() {
        let c = compile_ok(json!({"anyOf":[{"type":["string","null"]},{"type":"null"}]}));
        let t = c.schema["type"].as_array().unwrap();
        assert_eq!(t.len(), 2, "null 不应重复：{t:?}");
    }

    #[test]
    fn collapses_option_deep_in_properties() {
        let c = compile_ok(json!({"type":"object","properties":{"opt":{"anyOf":[{"type":"integer"},{"type":"null"}]}}}));
        assert_eq!(c.schema["properties"]["opt"], json!({"type":["integer","null"]}));
    }

    #[test]
    fn any_of_without_null_is_untouched() {
        let c = compile_ok(json!({"anyOf":[{"type":"string"},{"type":"number"}]}));
        assert!(c.schema["anyOf"].is_array(), "无 null 分支的 anyOf 保持原样");
        assert!(gate::assert_flat(&c.schema).is_ok());
    }

    #[test]
    fn three_way_any_of_is_untouched() {
        let c = compile_ok(json!({"anyOf":[{"type":"string"},{"type":"number"},{"type":"null"}]}));
        assert_eq!(c.schema["anyOf"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn any_of_with_two_nulls_collapses_to_null() {
        let c = compile_ok(json!({"anyOf":[{"type":"null"},{"type":"null"}]}));
        assert_eq!(c.schema, json!({"type":"null"}));
    }

    #[test]
    fn any_of_with_typeless_branch_is_left_alone() {
        let c = compile_ok(json!({"anyOf":[{"minLength":1},{"type":"null"}]}));
        assert!(c.schema["anyOf"].is_array(), "缺 type 的分支无法安全折叠");
    }

    #[test]
    fn any_of_with_scalar_branch_is_left_alone() {
        let c = compile_ok(json!({"anyOf":["plain", {"type":"null"}]}));
        assert!(c.schema["anyOf"].is_array());
    }

    // ── enum 规范化（#4666 防回归）───────────────────────────────

    #[test]
    fn enum_object_branch_gets_type_added() {
        let c = compile_ok(json!({
            "oneOf":[
                {"properties":{"kind":{"enum":["a"]},"val":{"type":"number"}}},
                {"properties":{"kind":{"enum":["b"]}}}
            ]
        }));
        assert_eq!(c.schema["oneOf"][0]["type"], "object");
        assert_eq!(c.schema["oneOf"][1]["type"], "object");
        assert!(gate::assert_flat(&c.schema).is_ok());
    }

    #[test]
    fn enum_branches_without_type_are_untouched() {
        let c = compile_ok(json!({"oneOf":[{"const":"a"},{"const":"b"}]}));
        assert!(c.schema["oneOf"][0].get("type").is_none());
        assert!(gate::assert_flat(&c.schema).is_ok());
    }

    #[test]
    fn nested_enum_combo_is_rejected() {
        let raw = json!({"oneOf":[{"oneOf":[{"type":"string"}]}]});
        match compile(&raw).unwrap_err() {
            SchemaError::NestedCombo(k, p) => {
                assert_eq!(k, "oneOf");
                assert!(p.starts_with("/oneOf/0"), "{p}");
            }
            other => panic!("期望 NestedCombo，实际 {other:?}"),
        }
    }

    #[test]
    fn option_inside_enum_branch_is_rejected() {
        // 组合分支内禁止再出现组合：Option<enum> 需先展平。
        let raw = json!({"oneOf":[{"anyOf":[{"type":"string"},{"type":"null"}]}]});
        assert!(matches!(compile(&raw).unwrap_err(), SchemaError::NestedCombo(_, _)));
    }

    // ── x-tauron → uiSchema ──────────────────────────────────

    #[test]
    fn extensions_are_extracted_into_ui_schema() {
        let c = compile_ok(json!({
            "type":"object",
            "properties":{
                "volume":{"type":"integer","x-tauron":{"version":1,"label":"音量","order":2,"widget":"slider"}}
            }
        }));
        assert_eq!(c.extension_count, 1);
        assert!(c.schema["properties"]["volume"].get("x-tauron").is_none());
        assert_eq!(c.ui_schema["volume"]["ui:label"], "音量");
        assert_eq!(c.ui_schema["volume"]["ui:widget"], "slider");
        assert_eq!(c.ui_schema["volume"]["ui:order"], 2);
    }

    #[test]
    fn root_extension_is_at_ui_root() {
        let c = compile_ok(json!({"type":"object","x-tauron":{"version":1,"label":"设置"},"properties":{}}));
        assert_eq!(c.ui_schema["ui:label"], "设置");
    }

    #[test]
    fn nested_property_extensions_are_nested_in_ui() {
        let c = compile_ok(json!({
            "type":"object",
            "properties":{
                "sound":{"type":"object","x-tauron":{"version":1,"label":"声音"},"properties":{
                    "volume":{"type":"integer","x-tauron":{"version":1,"label":"音量","widget":"slider"}}
                }}
            }
        }));
        assert_eq!(c.extension_count, 2);
        assert_eq!(c.ui_schema["sound"]["ui:label"], "声音");
        assert_eq!(c.ui_schema["sound"]["volume"]["ui:label"], "音量");
    }

    #[test]
    fn extension_inside_one_of_branch_reaches_ui() {
        let c = compile_ok(json!({
            "oneOf":[
                {"type":"object","properties":{"kind":{"enum":["a"]}},"x-tauron":{"version":1,"label":"甲"}},
                {"type":"object","properties":{"kind":{"enum":["b"]}},"x-tauron":{"version":1,"label":"乙"}}
            ]
        }));
        assert_eq!(c.extension_count, 2);
        assert_eq!(c.ui_schema["0"]["ui:label"], "甲");
        assert_eq!(c.ui_schema["1"]["ui:label"], "乙");
    }

    #[test]
    fn extension_inside_items_is_at_the_container_path() {
        // JSON Schema 2020-12 的 `items` 是对象，路径不增加段。
        let c = compile_ok(json!({"type":"array","items":{"type":"string","x-tauron":{"version":1,"label":"项"}}}));
        assert_eq!(c.ui_schema["ui:label"], "项");
    }

    #[test]
    fn extension_inside_ref_target_is_collected() {
        let c = compile_ok(json!({
            "$defs":{"Name":{"type":"string","x-tauron":{"version":1,"label":"姓名"}}},
            "type":"object","properties":{"name":{"$ref":"#/$defs/Name"}}
        }));
        assert_eq!(c.extension_count, 1);
        assert_eq!(c.ui_schema["name"]["ui:label"], "姓名");
    }

    #[test]
    fn empty_extensions_yield_empty_ui() {
        let c = compile_ok(json!({"type":"object","properties":{"a":{"type":"string"}}}));
        assert_eq!(c.extension_count, 0);
        assert!(c.is_empty_ui());
    }

    #[test]
    fn unknown_extension_key_fails_the_build() {
        let raw = json!({"type":"object","properties":{"a":{"type":"string","x-tauron":{"version":1,"lbel":"x"}}}});
        match compile(&raw).unwrap_err() {
            SchemaError::UnknownExtension(k, p) => {
                assert_eq!(k, "lbel");
                assert!(p.starts_with("/properties/a"), "{p}");
            }
            other => panic!("期望 UnknownExtension，实际 {other:?}"),
        }
    }

    #[test]
    fn ui_compile_is_idempotent() {
        let raw = json!({
            "type":"object",
            "properties":{
                "a":{"type":"string","x-tauron":{"version":1,"label":"甲","order":1}},
                "b":{"type":"integer","x-tauron":{"version":1,"label":"乙","widget":"slider","order":2}}
            }
        });
        let a = compile(&raw).unwrap();
        let b = compile(&raw).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.schema, b.schema);
        assert_eq!(a.ui_schema, b.ui_schema);
    }

    // ── 端到端：schemars 形态 → 平面 ────────────────────────────

    #[test]
    fn full_schemars_shape_produces_flat_output() {
        let raw = json!({
            "$schema":"https://json-schema.org/draft/2020-12/schema",
            "title":"设置",
            "$defs":{
                "Language":{"type":"string"},
                "SoundConfig":{
                    "type":"object",
                    "properties":{
                        "volume":{"type":"integer","minimum":0,"maximum":100},
                        "muted":{"type":["boolean","null"]}
                    },
                    "required":["volume"]
                }
            },
            "type":"object",
            "properties":{
                "language":{"$ref":"#/$defs/Language","x-tauron":{"version":1,"label":"语言","widget":"select","order":1}},
                "sound":{"$ref":"#/$defs/SoundConfig","x-tauron":{"version":1,"label":"声音","order":2}},
                "theme":{"oneOf":[
                    {"type":"object","properties":{"kind":{"const":"light"}},"x-tauron":{"version":1,"label":"浅色"}},
                    {"type":"object","properties":{"kind":{"const":"dark"}},"x-tauron":{"version":1,"label":"深色"}}
                ]}
            }
        });
        let c = compile(&raw).unwrap();

        assert!(gate::assert_flat(&c.schema).is_ok());
        let r = report(&c.schema);
        assert_eq!(r.remaining_refs, 0);
        assert_eq!(r.remaining_extensions, 0);
        assert_eq!(r.top_level_combos, 1, "theme 的 oneOf 保留在顶层");

        assert_eq!(c.schema["properties"]["language"]["type"], "string");
        assert_eq!(c.schema["properties"]["sound"]["type"], "object");
        assert_eq!(c.schema["properties"]["sound"]["required"], json!(["volume"]));
        assert!(c.schema.get("$defs").is_none());

        assert_eq!(c.schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
        assert_eq!(c.schema["title"], "设置");

        assert_eq!(c.extension_count, 4);
        assert_eq!(c.ui_schema["language"]["ui:label"], "语言");
        assert_eq!(c.ui_schema["sound"]["ui:label"], "声音");
        assert_eq!(c.ui_schema["theme"]["0"]["ui:label"], "浅色");
        assert_eq!(c.ui_schema["theme"]["1"]["ui:label"], "深色");
    }

    #[test]
    fn compile_to_doc_shape() {
        let doc = compile_to_doc(&json!({"type":"object","x-tauron":{"version":1,"label":"根"}})).unwrap();
        assert!(doc.get("schema").is_some());
        assert!(doc.get("uiSchema").is_some());
        assert_eq!(doc["uiSchema"]["ui:label"], "根");
        assert_eq!(doc.as_object().unwrap().len(), 2, "产物只有两键");
    }

    #[test]
    fn primitive_document_passes_through() {
        let c = compile_ok(json!("plain string"));
        assert_eq!(c.schema, json!("plain string"));
        assert!(c.is_empty_ui());
    }

    #[test]
    fn helpers_are_correct() {
        let base = vec!["a".to_string()];
        assert_eq!(push(&[], "x".into()), vec!["x".to_string()]);
        assert_eq!(push(&base, "b".into()), vec!["a".to_string(), "b".to_string()]);
        // 不得原地修改基路径。
        assert_eq!(base, vec!["a".to_string()]);

        assert_eq!(escape_pointer("plain"), "plain");
        assert_eq!(escape_pointer("a/b~c"), "a~1b~0c");
        assert_eq!(escape_pointer("a~1b~0c"), "a~01b~00c");
    }

    #[test]
    fn is_null_schema_detects_only_object_form() {
        assert!(is_null_schema(&json!({"type":"null"})));
        assert!(!is_null_schema(&json!({"type":"string"})));
        assert!(!is_null_schema(&json!("null")));
    }
}
