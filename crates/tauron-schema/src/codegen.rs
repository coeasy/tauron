// §4.13 schema 生成侧：从 Rust 类型形态产出 schemars 1.2.2 的**原始**输出。
//
// 这个模块存在的理由是：计划 §4.13 的测试要求覆盖"Rust enum / Option /
// 嵌套 struct 的生成快照"。直接手写那些 JSON 夹具既冗长又容易漂移，
// 而且写出来的形态未必真的是 schemars 的形态。本模块用一个极简的
// Rust 类型描述器，产出与 schemars 1.2.2（draft 2020-12）一致的原始
// 输出——`$defs`+`$ref`、enum → `oneOf`、`Option<T>` → `anyOf: [T, null]`
// ——作为规范化管线的输入，使"生成 → 规范化 → 平面"这条链路可快照比对。
//
// 注意：**它产出的正是需要被规范化掉的坏形态**。`compile(generate(...))`
// 之后才得到可喂渲染器的平面 schema。

use serde_json::{Map, Value};

use crate::error::{SchemaError, SchemaResult};

/// 原始（非平面）schema 的 draft 声明。
pub const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";

/// 基础类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    Str,
    Bool,
    I64,
    F64,
}

/// 一份 Rust 类型形态的描述。
///
/// `Struct`/`Enum` 带 `name`：当该名字出现在
/// [`generate`] 的命名表里时，使用点产出 `$ref`（schemars 的默认行为）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Def {
    /// `String` / `&str`。
    Str,
    /// `bool`。
    Bool,
    /// `i64` 等整数。
    Int,
    /// `f64`。
    Float,
    /// `Option<T>` → `anyOf: [T, {type:"null"}]`。
    Opt(Box<Def>),
    /// `Vec<T>`。
    List(Box<Def>),
    /// `HashMap<String, V>`。
    Map(Box<Def>),
    /// struct。字段为 `(名, 定义, 是否必填)`。
    Struct { name: String, fields: Vec<(String, Def, bool)> },
    /// enum（schemars 默认的外部标签形态）。变体为 `(名, 载荷)`。
    Enum { name: String, variants: Vec<(String, Option<Def>)> },
}

impl Def {
    pub fn name(&self) -> Option<&str> {
        match self {
            Def::Struct { name, .. } | Def::Enum { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// 生成原始 schema 文档：`$schema` + `$defs` + 内联根。
///
/// `named` 是收集进 `$defs` 的命名类型（`Struct`/`Enum`），`root` 是内联在
/// 根位置的 schema（通常是顶层 struct，不建 `$ref`，与 schemars
/// `root_schema = true` 的行为一致）。
pub fn generate(named: &[Def], root: &Def, title: &str) -> SchemaResult<Value> {
    let mut defs = Map::new();
    for d in named {
        let Some(name) = d.name() else {
            return Err(SchemaError::Structure(
                "命名表里只允许 Struct / Enum（它们是唯一带名字的类型）".into(),
            ));
        };
        if defs.contains_key(name) {
            return Err(SchemaError::Structure(format!("重复的定义名 `{name}`")));
        }
        defs.insert(name.to_string(), render(d, named, false)?);
    }

    let doc = Map::from_iter([
        ("$schema".to_string(), Value::String(DRAFT_2020_12.to_string())),
        ("$defs".to_string(), Value::Object(defs)),
        ("title".to_string(), Value::String(title.to_string())),
        (
            "properties".to_string(),
            Value::Object(Map::from_iter([("value".to_string(), render(root, named, false)?)])),
        ),
        ("type".to_string(), Value::String("object".to_string())),
    ]);
    Ok(Value::Object(doc))
}

/// 渲染一个类型定义。
///
/// `allow_ref = true`：命名类型（`Struct`/`Enum`）若在命名表中则产出 `$ref`
/// ——schemars 的默认行为。
/// `allow_ref = false`：强制内联展开——用于 `$defs` 条目本体与根位置，
/// 避免自引用环。
fn render(d: &Def, named: &[Def], allow_ref: bool) -> SchemaResult<Value> {
    // 命名类型可能折成 `$ref`。
    if let Some(name) = d.name() {
        if allow_ref && named.iter().any(|x| x.name() == Some(name)) {
            return Ok(obj(&[("$ref", Value::String(format!("#/$defs/{name}")))]));
        }
        return render_body(d, named);
    }

    match d {
        Def::Str => Ok(obj(&[("type", Value::String("string".into()))])),
        Def::Bool => Ok(obj(&[("type", Value::String("boolean".into()))])),
        Def::Int => Ok(obj(&[("type", Value::String("integer".into()))])),
        Def::Float => Ok(obj(&[("type", Value::String("number".into()))])),

        Def::Opt(inner) => Ok(Value::Object(Map::from_iter([(
            "anyOf".to_string(),
            Value::Array(vec![
                render(inner, named, true)?,
                obj(&[("type", Value::String("null".into()))]),
            ]),
        )]))),

        Def::List(inner) => Ok(Value::Object(Map::from_iter([
            ("type".to_string(), Value::String("array".into())),
            ("items".to_string(), render(inner, named, true)?),
        ]))),

        Def::Map(value_def) => Ok(Value::Object(Map::from_iter([
            ("type".to_string(), Value::String("object".into())),
            ("additionalProperties".to_string(), render(value_def, named, true)?),
        ]))),

        // 命名类型已在上方处理。
        Def::Struct { .. } | Def::Enum { .. } => unreachable!("命名类型应先由 d.name() 分支处理"),
    }
}

/// 命名类型的内联本体：struct → `type: object` + `properties`；
/// enum → 外部标签 `oneOf`（**故意不带 `type`**——那是 #4666 的原始
/// 形态，留给规范化管线补）。
fn render_body(d: &Def, named: &[Def]) -> SchemaResult<Value> {
    match d {
        Def::Struct { fields, .. } => {
            let mut properties = Map::new();
            let mut required = Vec::new();
            for (fname, fdef, is_required) in fields {
                properties.insert(fname.clone(), render(fdef, named, true)?);
                if *is_required {
                    required.push(Value::String(fname.clone()));
                }
            }
            let mut out: Vec<(String, Value)> = vec![
                ("type".to_string(), Value::String("object".into())),
                ("properties".to_string(), Value::Object(properties)),
            ];
            if !required.is_empty() {
                out.push(("required".to_string(), Value::Array(required)));
            }
            Ok(Value::Object(Map::from_iter(out)))
        }
        Def::Enum { variants, .. } => {
            let branches = variants
                .iter()
                .map(|(variant, payload)| {
                    let mut props = Map::new();
                    props.insert("kind".to_string(), Value::String(variant.clone()));
                    if let Some(p) = payload {
                        props.insert("value".to_string(), render(p, named, true)?);
                    }
                    Ok::<Value, SchemaError>(obj(&[("properties", Value::Object(props))]))
                })
                .collect::<SchemaResult<Vec<_>>>()?;
            Ok(Value::Object(Map::from_iter([("oneOf".to_string(), Value::Array(branches))])))
        }
        _ => Err(SchemaError::Structure("render_body 只接受 Struct / Enum".into())),
    }
}

fn obj(kvs: &[(&str, Value)]) -> Value {
    Value::Object(Map::from_iter(kvs.iter().map(|(k, v)| (k.to_string(), v.clone()))))
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::compile;
    use serde_json::json;

    #[test]
    fn primitives_render_to_single_type_keywords() {
        let cases = [
            (Def::Str, "string"),
            (Def::Bool, "boolean"),
            (Def::Int, "integer"),
            (Def::Float, "number"),
        ];
        for (d, want) in cases {
            assert_eq!(render(&d, &[], true).unwrap(), json!({"type": want}));
        }
    }

    #[test]
    fn option_renders_as_any_of_with_null() {
        let d = Def::Opt(Box::new(Def::Str));
        let v = render(&d, &[], true).unwrap();
        assert_eq!(v["anyOf"][0], json!({"type":"string"}));
        assert_eq!(v["anyOf"][1], json!({"type":"null"}));
    }

    #[test]
    fn option_of_named_type_uses_a_ref() {
        let named =
            vec![Def::Struct { name: "N".into(), fields: vec![("a".into(), Def::Str, true)] }];
        let d = Def::Opt(Box::new(Def::Struct {
            name: "N".into(),
            fields: vec![("a".into(), Def::Str, true)],
        }));
        let v = render(&d, &named, true).unwrap();
        assert_eq!(v["anyOf"][0], json!({"$ref":"#/$defs/N"}));
    }

    #[test]
    fn list_and_map_render_to_array_and_object() {
        assert_eq!(
            render(&Def::List(Box::new(Def::Int)), &[], true).unwrap(),
            json!({"type":"array","items":{"type":"integer"}})
        );
        assert_eq!(
            render(&Def::Map(Box::new(Def::Str)), &[], true).unwrap(),
            json!({"type":"object","additionalProperties":{"type":"string"}})
        );
    }

    #[test]
    fn struct_omits_required_when_all_optional() {
        let d = Def::Struct { name: "S".into(), fields: vec![("a".into(), Def::Str, false)] };
        let v = render(&d, &[], true).unwrap();
        assert!(v.get("required").is_none());
        assert_eq!(v["type"], "object");
    }

    #[test]
    fn struct_lists_required_fields_in_declaration_order() {
        let d = Def::Struct {
            name: "S".into(),
            fields: vec![
                ("a".into(), Def::Str, true),
                ("b".into(), Def::Bool, false),
                ("c".into(), Def::Int, true),
            ],
        };
        let v = render(&d, &[], true).unwrap();
        assert_eq!(v["required"], json!(["a", "c"]));
        assert_eq!(v["properties"]["b"], json!({"type":"boolean"}));
    }

    #[test]
    fn enum_renders_as_one_of_without_type_key() {
        // 故意不写 type——这是 #4666 的原始形态。
        let d = Def::Enum {
            name: "Theme".into(),
            variants: vec![("Light".into(), None), ("Dark".into(), Some(Def::Int))],
        };
        let v = render(&d, &[], true).unwrap();
        assert_eq!(v["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(v["oneOf"][0]["properties"]["kind"], "Light");
        assert!(v["oneOf"][0].get("type").is_none(), "生成侧不加 type，留给管线补");
        assert_eq!(v["oneOf"][1]["properties"]["value"], json!({"type":"integer"}));
    }

    #[test]
    fn enum_name_is_exposed() {
        assert_eq!(Def::Enum { name: "T".into(), variants: vec![] }.name(), Some("T"));
        assert_eq!(Def::Str.name(), None);
    }

    #[test]
    fn generate_emits_schema_defs_title_and_inlined_root() {
        let named = vec![Def::Struct {
            name: "Sound".into(),
            fields: vec![("volume".into(), Def::Int, true), ("muted".into(), Def::Bool, false)],
        }];
        let root = Def::Struct {
            name: "Settings".into(),
            fields: vec![
                ("sound".into(), Def::Struct { name: "Sound".into(), fields: vec![] }, true),
                ("lang".into(), Def::Str, true),
            ],
        };
        let doc = generate(&named, &root, "设置").unwrap();

        assert_eq!(doc["$schema"], DRAFT_2020_12);
        assert_eq!(doc["title"], "设置");
        assert_eq!(doc["$defs"]["Sound"]["properties"]["volume"], json!({"type":"integer"}));
        assert_eq!(doc["$defs"]["Sound"]["required"], json!(["volume"]));
        // 根内联，且字段引用命名类型走 $ref。
        assert_eq!(doc["type"], "object");
        assert_eq!(
            doc["properties"]["value"]["properties"]["sound"],
            json!({"$ref":"#/$defs/Sound"})
        );
        assert_eq!(doc["properties"]["value"]["properties"]["lang"], json!({"type":"string"}));
    }

    #[test]
    fn generate_rejects_an_unnamed_definition() {
        let e = generate(&[Def::Str], &Def::Str, "t").unwrap_err();
        assert!(matches!(e, SchemaError::Structure(m) if m.contains("Struct / Enum")));
    }

    #[test]
    fn generate_rejects_duplicate_names() {
        let d = || Def::Struct { name: "N".into(), fields: vec![] };
        let e = generate(&[d(), d()], &Def::Str, "t").unwrap_err();
        assert!(matches!(e, SchemaError::Structure(m) if m.contains("重复的定义名")));
    }

    #[test]
    fn generated_output_is_not_flat_until_compiled() {
        // 生成侧的产物**必然**含 $ref / oneOf——这正是管线的输入。
        let named =
            vec![Def::Struct { name: "S".into(), fields: vec![("a".into(), Def::Str, true)] }];
        let root = Def::Enum {
            name: "E".into(),
            variants: vec![("A".into(), Some(Def::List(Box::new(Def::Int)))), ("B".into(), None)],
        };
        let raw = generate(&named, &root, "t").unwrap();
        assert!(raw["$defs"].get("S").is_some());
        assert!(raw["properties"]["value"]["oneOf"].is_array());

        let compiled = compile(&raw).unwrap();
        // 规范化后才变平面。
        assert!(compiled.schema.get("$defs").is_none());
        assert_eq!(compiled.schema["properties"]["value"]["oneOf"][0]["type"], "object");
        assert_eq!(
            compiled.schema["properties"]["value"]["oneOf"][0]["properties"]["value"],
            json!({"type":"array","items":{"type":"integer"}})
        );
    }

    #[test]
    fn nested_struct_produces_chained_refs_and_survives_compilation() {
        let named = vec![
            Def::Struct { name: "Inner".into(), fields: vec![("x".into(), Def::Str, true)] },
            Def::Struct {
                name: "Outer".into(),
                fields: vec![(
                    "inner".into(),
                    Def::Struct { name: "Inner".into(), fields: vec![] },
                    true,
                )],
            },
        ];
        let root = Def::Opt(Box::new(Def::Struct { name: "Outer".into(), fields: vec![] }));
        let raw = generate(&named, &root, "t").unwrap();
        assert_eq!(raw["$defs"]["Outer"]["properties"]["inner"], json!({"$ref":"#/$defs/Inner"}));

        let c = compile(&raw).unwrap();
        // Option 折叠 + 两级 $ref 展开。
        let v = &c.schema["properties"]["value"];
        assert!(v["type"].as_array().unwrap().contains(&json!("null")));
        assert_eq!(v["properties"]["inner"]["properties"]["x"], json!({"type":"string"}));
    }

    #[test]
    fn snapshot_shape_is_stable_across_runs() {
        // 快照稳定性：同输入两次生成必须逐字节相等（serde_json::Map 是 BTreeMap）。
        let named = vec![Def::Struct {
            name: "S".into(),
            fields: vec![("b".into(), Def::Bool, true), ("a".into(), Def::Str, true)],
        }];
        let root = Def::Struct {
            name: "R".into(),
            fields: vec![("s".into(), Def::Struct { name: "S".into(), fields: vec![] }, true)],
        };
        let a = generate(&named, &root, "t").unwrap();
        let b = generate(&named, &root, "t").unwrap();
        assert_eq!(serde_json::to_string(&a).unwrap(), serde_json::to_string(&b).unwrap());
        // 且字段按字典序输出，快照不依赖声明顺序。
        let props = &a["$defs"]["S"]["properties"];
        let keys: Vec<&String> = props.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec![&String::from("a"), &String::from("b")]);
    }

    #[test]
    fn generated_enum_can_carry_ui_extensions_through_the_pipeline() {
        // 生成侧不带扩展，但管线要能吃"生成 + 手工补扩展"的组合输入。
        let root =
            Def::Enum { name: "E".into(), variants: vec![("A".into(), None), ("B".into(), None)] };
        let mut raw = generate(&[], &root, "t").unwrap();
        raw["properties"]["value"]["x-tauron"] = json!({"version":1,"label":"选择"});
        let c = compile(&raw).unwrap();
        assert_eq!(c.extension_count, 1);
        assert_eq!(c.ui_schema["value"]["ui:label"], "选择");
    }
}
