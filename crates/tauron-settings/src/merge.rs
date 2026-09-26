// §4.12 自研深合并。
//
// **为什么必须自研**（架构 §6.3）：`json_patch::merge` 的语义是 JSON
// Merge Patch——`null` 表示删除键。设置里 `null` 是合法值（例如
// "取消默认主题"就是要一个真 null），若复用会语义漂移。
// 本模块用独立的 `$unset` 列表表达"继承"，`null` 保持字面值。
//
// 两套合并语义的隔离是计划 §4.12 的显式测试项。

use serde_json::{Map, Value};

use crate::error::{SettingsError, SettingsResult};

/// 层内表达"此键继承自下层"的保留键。
///
/// 值是一个**点路径数组**（如 `["theme.color", "volume"]`）。
/// 以 `$` 开头是保留前缀——真实设置键禁止使用。
pub const UNSET_KEY: &str = "$unset";

/// 合并结果中某叶子的来源层。
pub type Source = Map<String, String>;

/// 写入决策：值是"真写入"还是"回落为继承"。
#[derive(Debug, Clone, PartialEq)]
pub enum WriteOp {
    /// 值与继承值不等：真写入。
    Write { path: String, value: Value },
    /// 值与继承值相等：回落为 `unset`（继承下层）。
    Unset { path: String },
}

/// 把一层拆成 `(unset 路径列表, 实际值对象)`。
pub fn split_unset(layer: &Value) -> (Vec<String>, Map<String, Value>) {
    let Value::Object(m) = layer else {
        return (Vec::new(), Map::new());
    };
    let mut unset = Vec::new();
    let mut values = Map::new();
    for (k, v) in m {
        if k.as_str() == UNSET_KEY {
            if let Value::Array(items) = v {
                for i in items {
                    if let Some(s) = i.as_str() {
                        unset.push(s.to_string());
                    }
                }
            }
        } else {
            values.insert(k.clone(), v.clone());
        }
    }
    (unset, values)
}

/// 深合并：把 `src` 合并进 `dst`。
///
/// - 对象：递归合并；
/// - 数组：**替换**（不逐元素合并——设置里的数组通常有顺序语义）；
/// - 标量：直接覆盖。
pub fn deep_merge(dst: &mut Value, src: &Value) {
    if !dst.is_object() || !src.is_object() {
        *dst = src.clone();
        return;
    }
    let src_map = src.as_object().expect("刚判定为对象");
    let dst_map = dst.as_object_mut().expect("刚判定为对象");
    for (k, v) in src_map {
        match dst_map.get_mut(k) {
            Some(existing) => deep_merge(existing, v),
            None => {
                dst_map.insert(k.clone(), v.clone());
            }
        }
    }
}

/// 按点路径删除 `root` 下的键（用于落实 `unset`）。
///
/// 路径段不含 `.`（`.` 是分隔符），故点路径无歧义。
pub fn delete_path(root: &mut Value, dotted: &str) {
    let parts: Vec<&str> = dotted.split('.').collect();
    if parts.is_empty() {
        return;
    }
    let mut cur = root;
    for p in &parts[..parts.len() - 1] {
        cur = match cur {
            Value::Object(m) => match m.get_mut(*p) {
                Some(v) => v,
                None => return,
            },
            _ => return,
        };
    }
    let last = *parts.last().expect("路径非空");
    if let Value::Object(m) = cur {
        m.remove(last);
    }
}

/// 按点路径读取 `root` 下的值（不存在返回 `None`）。
pub fn read_path(root: &Value, dotted: &str) -> Option<Value> {
    let mut cur = root;
    for p in dotted.split('.') {
        cur = match cur {
            Value::Object(m) => m.get(p)?,
            _ => return None,
        };
    }
    Some(cur.clone())
}

/// 按点路径写入（必要时创建中间对象）。
///
/// **中间节点不是对象时替换它，而不是断言失败**。这条路径是可达的：跨层
/// 叠加时下层 `{"a":{"b":1}}`、上层 `{"a":"x","$unset":["a.b"]}` 会让
/// [`apply_unset`] 带着 `before` 里的 `a.b` 去写 `acc`，而 `acc` 里的 `a`
/// 已被上层覆盖成标量。原先的 `expect` 会把一份**畸形但可接受**的配置升级成
/// 宿主 panic（被 `guard` 兜成 `E_HOST_PANIC`，用户拿不到可读原因）。
///
/// 覆盖标量是唯一自洽的选择：「写入深层路径」的语义本就是建立结构，且与下方
/// `or_insert_with` 对**缺失**中间节点的既有行为一致（缺失→建对象，
/// 类型不符→也建对象），调用方不必区分"没写过"和"写成了别的类型"。
pub fn write_path(root: &mut Value, dotted: &str, value: Value) {
    let parts: Vec<&str> = dotted.split('.').collect();
    let mut cur = root;
    for p in &parts[..parts.len() - 1] {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        // 保持链式写法：借用在表达式结束即释放，随后才把结果赋回 `cur`
        // （拆成 `let slot = ...` 会让借用跨越赋值语句，借用检查不通过）。
        cur = cur
            .as_object_mut()
            .expect("上一行已保证是对象")
            .entry(p.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    let last = parts.last().expect("路径非空").to_string();
    if !cur.is_object() {
        *cur = Value::Object(Map::new());
    }
    cur.as_object_mut().expect("上一行已保证是对象").insert(last, value);
}

/// 校验点路径合法性：非空、无空段、不以 `.` 开头/结尾、无 `$` 保留前缀。
pub fn validate_path(dotted: &str) -> SettingsResult<()> {
    if dotted.is_empty() {
        return Err(SettingsError::InvalidPath(dotted.to_string()));
    }
    if dotted.starts_with('.') || dotted.ends_with('.') || dotted.contains("..") {
        return Err(SettingsError::InvalidPath(dotted.to_string()));
    }
    for seg in dotted.split('.') {
        if seg.starts_with('$') {
            return Err(SettingsError::InvalidPath(format!("路径段 `{seg}` 以保留前缀 `$` 开头")));
        }
    }
    Ok(())
}

/// 自底向上叠加四层。`layers` 按 `LayerKind::priority` 升序传入。
///
/// `unset` 语义：本层标记为 unset 的键**回退到本层写入前的值**（即继承
/// 下层），而不是被删除。同层内"写了又 unset"等于没写。
pub fn merge_layers(layers: &[(crate::error::LayerKind, Value)]) -> Value {
    let mut acc: Value = Value::Object(Map::new());
    for (_kind, layer) in layers {
        let (unset, values) = split_unset(layer);
        let before = acc.clone();
        deep_merge(&mut acc, &Value::Object(values));
        apply_unset(&mut acc, &before, &unset);
    }
    acc
}

/// 把 unset 路径回退到 `before` 的状态。
fn apply_unset(acc: &mut Value, before: &Value, unset: &[String]) {
    for p in unset {
        match read_path(before, p) {
            Some(v) => write_path(acc, p, v),
            None => delete_path(acc, p),
        }
    }
}

/// 计算"某层某路径"的**继承值**——即该层以下各层叠加后的值。
/// 用于实现"等值写回落 unset"。
pub fn inherited_at(layers: &[(crate::error::LayerKind, Value)], path: &str) -> Value {
    // 排除当前层及以上，只叠下层。
    let mut acc: Value = Value::Object(Map::new());
    for (kind, layer) in layers {
        if *kind == crate::error::LayerKind::User {
            continue;
        }
        let (unset, values) = split_unset(layer);
        let before = acc.clone();
        deep_merge(&mut acc, &Value::Object(values));
        apply_unset(&mut acc, &before, &unset);
    }
    read_path(&acc, path).unwrap_or(Value::Null)
}

/// 决定一次写入是真写入还是回落 unset。
pub fn decide_write(path: &str, value: &Value, inherited: &Value) -> WriteOp {
    if value == inherited {
        WriteOp::Unset { path: path.to_string() }
    } else {
        WriteOp::Write { path: path.to_string(), value: value.clone() }
    }
}

/// 把一次写入决策落实到用户层对象上。
pub fn apply_op(user_layer: &mut Map<String, Value>, op: &WriteOp) {
    match op {
        WriteOp::Write { path, value } => {
            // 真写入：先清除该路径的 unset 标记（之前可能标记过）。
            remove_unset(user_layer, path);
            // 再写入值。
            let mut host = Value::Object(Map::new());
            write_path(&mut host, path, value.clone());
            let Value::Object(h) = host else { return };
            for (k, v) in h {
                deep_merge_in_map(user_layer, k, v);
            }
        }
        WriteOp::Unset { path } => {
            // 1. 移除已写入的值（如果有），使合并结果回退到下层。
            let mut host = Value::Object(std::mem::take(user_layer));
            delete_path(&mut host, path);
            *user_layer = match host {
                Value::Object(m) => m,
                _ => unreachable!("host 刚由 Map 构造"),
            };
            // 2. 登记 unset 路径。
            let mut list = match user_layer.get(UNSET_KEY) {
                Some(Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            if !list.iter().any(|v| v.as_str() == Some(path.as_str())) {
                list.push(Value::String(path.clone()));
            }
            user_layer.insert(UNSET_KEY.to_string(), Value::Array(list));
        }
    }
}

/// 从用户层的 unset 列表中移除某路径。
fn remove_unset(user_layer: &mut Map<String, Value>, path: &str) {
    let Some(Value::Array(list)) = user_layer.get(UNSET_KEY) else {
        return;
    };
    let filtered: Vec<Value> = list.iter().filter(|v| v.as_str() != Some(path)).cloned().collect();
    if filtered.is_empty() {
        user_layer.remove(UNSET_KEY);
    } else {
        user_layer.insert(UNSET_KEY.to_string(), Value::Array(filtered));
    }
}

/// 把一个顶层键的值深合并进目标 map。
fn deep_merge_in_map(target: &mut Map<String, Value>, key: String, value: Value) {
    match target.get_mut(&key) {
        Some(existing) => deep_merge(existing, &value),
        None => {
            target.insert(key, value);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::LayerKind;
    use serde_json::json;

    fn layer(kind: LayerKind, v: Value) -> (LayerKind, Value) {
        (kind, v)
    }

    // ── 深合并基础语义 ──────────────────────────────────────────────

    #[test]
    fn deep_merge_recurses_into_objects() {
        let mut a = json!({"a": {"x": 1, "y": 2}});
        deep_merge(&mut a, &json!({"a": {"y": 20}, "b": 3}));
        assert_eq!(a, json!({"a": {"x": 1, "y": 20}, "b": 3}));
    }

    #[test]
    fn deep_merge_replaces_arrays_whole() {
        let mut a = json!({"list": [1, 2, 3]});
        deep_merge(&mut a, &json!({"list": [9]}));
        assert_eq!(a["list"], json!([9]));
    }

    #[test]
    fn deep_merge_replaces_when_types_differ() {
        let mut a = json!({"a": {"x": 1}});
        deep_merge(&mut a, &json!({"a": 5}));
        assert_eq!(a["a"], json!(5));
    }

    #[test]
    fn deep_merge_null_is_a_literal_value() {
        // 与 JSON Merge Patch 的关键区别：null 不删键。
        let mut a = json!({"volume": 5});
        deep_merge(&mut a, &json!({"volume": null}));
        assert!(a.get("volume").is_some(), "null 必须保留为字面值");
        assert_eq!(a["volume"], Value::Null);
    }

    // ── unset 语义 ────────────────────────────────────────────────

    #[test]
    fn unset_removes_the_key_so_the_lower_layer_wins() {
        let builtin = json!({"volume": 10, "muted": false});
        let user = json!({"volume": 100, "$unset": ["muted"]});
        let out = merge_layers(&[layer(LayerKind::Builtin, builtin), layer(LayerKind::User, user)]);
        assert_eq!(out["volume"], 100);
        assert_eq!(out["muted"], false, "unset 后应继承 builtin 的 false");
    }

    #[test]
    fn unset_of_nested_path_works() {
        let builtin = json!({"theme": {"color": "blue", "size": 14}});
        let user = json!({"theme": {"size": 20}, "$unset": ["theme.color"]});
        let out = merge_layers(&[layer(LayerKind::Builtin, builtin), layer(LayerKind::User, user)]);
        assert_eq!(out["theme"], json!({"color": "blue", "size": 20}));
    }

    #[test]
    fn unset_beats_write_within_the_same_layer() {
        // 同层内 unset 优先：写了再 unset 等于没写。
        let builtin = json!({"volume": 10});
        let user = json!({"volume": 100, "$unset": ["volume"]});
        let out = merge_layers(&[layer(LayerKind::Builtin, builtin), layer(LayerKind::User, user)]);
        assert_eq!(out["volume"], 10);
    }

    #[test]
    fn unset_key_is_never_leaked_into_the_result() {
        let builtin = json!({"volume": 1});
        let user = json!({"$unset": ["x"]});
        let out = merge_layers(&[layer(LayerKind::Builtin, builtin), layer(LayerKind::User, user)]);
        assert!(out.get(UNSET_KEY).is_none());
    }

    #[test]
    fn non_array_unset_list_is_ignored_not_panicked() {
        let builtin = json!({"volume": 1});
        let user = json!({"volume": 2, "$unset": "not-an-array"});
        let out = merge_layers(&[layer(LayerKind::Builtin, builtin), layer(LayerKind::User, user)]);
        assert_eq!(out["volume"], 2);
    }

    #[test]
    fn non_object_layer_is_a_noop() {
        let builtin = json!({"volume": 1});
        let out = merge_layers(&[
            layer(LayerKind::Builtin, builtin.clone()),
            layer(LayerKind::User, json!(42)),
        ]);
        assert_eq!(out, builtin);
    }

    // ── 四层优先级 ──────────────────────────────────────────────────

    #[test]
    fn user_beats_plugin_beats_brand_beats_builtin() {
        let out = merge_layers(&[
            layer(LayerKind::Builtin, json!({"volume": 1})),
            layer(LayerKind::Brand, json!({"volume": 2})),
            layer(LayerKind::Plugin, json!({"volume": 3})),
            layer(LayerKind::User, json!({"volume": 4})),
        ]);
        assert_eq!(out["volume"], 4);
    }

    #[test]
    fn missing_upper_layers_fall_through() {
        let out = merge_layers(&[
            layer(LayerKind::Builtin, json!({"volume": 1})),
            layer(LayerKind::Brand, json!({})),
            layer(LayerKind::Plugin, json!({})),
            layer(LayerKind::User, json!({})),
        ]);
        assert_eq!(out["volume"], 1);
    }

    #[test]
    fn all_four_layers_combined_full_matrix() {
        // 计划 §4.12："四层优先级全组合"。
        // 键 a：四层都有值 → user 赢。
        // 键 b：builtin + plugin → plugin 赢。
        // 键 c：仅 brand → brand。
        // 键 d：仅 builtin。
        // 键 e：user unset → 继承 plugin。
        // 键 f：brand unset → 继承 builtin。
        let out = merge_layers(&[
            layer(LayerKind::Builtin, json!({"a": 1, "b": 1, "d": 1, "f": "builtin"})),
            layer(LayerKind::Brand, json!({"a": 2, "c": 2, "$unset": ["f"]})),
            layer(LayerKind::Plugin, json!({"a": 3, "b": 3, "e": 3})),
            layer(LayerKind::User, json!({"a": 4, "$unset": ["e"]})),
        ]);
        assert_eq!(out["a"], 4);
        assert_eq!(out["b"], 3);
        assert_eq!(out["c"], 2);
        assert_eq!(out["d"], 1);
        assert_eq!(out["e"], 3, "user unset 后继承 plugin");
        assert_eq!(out["f"], "builtin", "brand unset 后继承 builtin");
    }

    #[test]
    fn nested_objects_merge_across_layers() {
        let out = merge_layers(&[
            layer(LayerKind::Builtin, json!({"theme": {"color": "blue", "size": 14}})),
            layer(LayerKind::User, json!({"theme": {"size": 20}})),
        ]);
        assert_eq!(out["theme"], json!({"color": "blue", "size": 20}));
    }

    // ── 等值写回落 unset ────────────────────────────────────────────

    #[test]
    fn equal_value_writes_unset_not_the_value() {
        let layers = [
            layer(LayerKind::Builtin, json!({"volume": 10})),
            layer(LayerKind::Plugin, json!({"volume": 10})),
        ];
        let op = decide_write("volume", &json!(10), &inherited_at(&layers, "volume"));
        assert!(matches!(op, WriteOp::Unset { .. }), "{op:?}");
    }

    #[test]
    fn different_value_writes_the_value() {
        let layers = [layer(LayerKind::Builtin, json!({"volume": 10}))];
        let op = decide_write("volume", &json!(11), &inherited_at(&layers, "volume"));
        assert!(matches!(op, WriteOp::Write { .. }), "{op:?}");
    }

    #[test]
    fn writing_default_back_to_default_is_a_noop_semantically() {
        // builtin=10，plugin=10，user 写 10 → unset → 合并结果仍是 10。
        let builtin = json!({"volume": 10});
        let plugin = json!({"volume": 10});
        let mut user = Map::new();
        let layers =
            [layer(LayerKind::Builtin, builtin.clone()), layer(LayerKind::Plugin, plugin.clone())];
        let op = decide_write("volume", &json!(10), &inherited_at(&layers, "volume"));
        apply_op(&mut user, &op);
        let out = merge_layers(&[
            layer(LayerKind::Builtin, builtin),
            layer(LayerKind::Plugin, plugin),
            layer(LayerKind::User, Value::Object(user)),
        ]);
        assert_eq!(out["volume"], 10);
    }

    #[test]
    fn unset_then_set_again_is_idempotent() {
        let mut user = Map::new();
        apply_op(&mut user, &WriteOp::Unset { path: "a".into() });
        apply_op(&mut user, &WriteOp::Unset { path: "a".into() });
        let unset = user.get(UNSET_KEY).unwrap().as_array().unwrap();
        assert_eq!(unset.len(), 1, "重复 unset 同一键不应重复记录");
    }

    // ── 路径 ────────────────────────────────────────────────────────

    #[test]
    fn validate_path_accepts_flat_and_dotted() {
        assert!(validate_path("volume").is_ok());
        assert!(validate_path("theme.color").is_ok());
        assert!(validate_path("a.b.c.d").is_ok());
    }

    #[test]
    fn validate_path_rejects_empty_and_ambiguous() {
        assert!(validate_path("").is_err());
        assert!(validate_path(".a").is_err());
        assert!(validate_path("a.").is_err());
        assert!(validate_path("a..b").is_err());
    }

    #[test]
    fn validate_path_rejects_dollar_prefix() {
        for p in ["$unset", "a.$x", "$x.y"] {
            assert!(validate_path(p).is_err(), "{p}");
        }
    }

    #[test]
    fn read_path_missing_returns_none() {
        let v = json!({"a": {"b": 1}});
        assert!(read_path(&v, "a.missing").is_none());
        assert!(read_path(&v, "c.b").is_none());
        assert_eq!(read_path(&v, "a.b"), Some(json!(1)));
    }

    #[test]
    fn write_path_creates_intermediate_objects() {
        let mut v = json!({});
        write_path(&mut v, "a.b.c", json!(7));
        assert_eq!(v, json!({"a": {"b": {"c": 7}}}));
    }

    /// 中间节点是标量时**替换**为对象，而不是 panic。
    #[test]
    fn write_path_replaces_scalar_intermediate_node() {
        let mut v = json!({"a": "scalar"});
        write_path(&mut v, "a.b", json!(7));
        assert_eq!(v, json!({"a": {"b": 7}}));
    }

    /// 端到端：下层有 `a.b`，上层把 `a` 覆盖成标量并 `$unset` 掉 `a.b`。
    ///
    /// 修复前这条链会走到 `write_path` 的中间节点断言 → panic →
    /// `E_HOST_PANIC`：一份畸形但可接受的配置把整次合并炸掉，用户只看到
    /// 「宿主内部错误」而不是可读原因。
    #[test]
    fn merge_layers_scalar_and_unset_conflict_does_not_panic() {
        let layers = vec![
            layer(LayerKind::Brand, json!({"a": {"b": 1}})),
            layer(LayerKind::User, json!({"a": "x", "$unset": ["a.b"]})),
        ];
        let merged = merge_layers(&layers);
        assert!(merged.is_object(), "合并必须产出对象而不是 panic");
        assert_eq!(merged["a"]["b"], json!(1), "unset 回退应重建 a.b");
    }

    #[test]
    fn delete_path_on_missing_is_a_noop() {
        let mut v = json!({"a": {"b": 1}});
        delete_path(&mut v, "a.nope");
        assert_eq!(v, json!({"a": {"b": 1}}));
        delete_path(&mut v, "z.q");
        assert_eq!(v, json!({"a": {"b": 1}}));
    }

    #[test]
    fn delete_path_nested() {
        let mut v = json!({"a": {"b": {"c": 1, "d": 2}}});
        delete_path(&mut v, "a.b.c");
        assert_eq!(v, json!({"a": {"b": {"d": 2}}}));
    }

    // ── 与 JSON Merge Patch 的语义隔离 ──────────────────────────────

    /// 计划 §4.12 显式测试项："两套合并语义的隔离用例"。
    ///
    /// JSON Merge Patch 的规则：`null` 删除键、对象递归、数组替换。
    /// 本模块的规则：`null` 是字面值、删除走 `$unset`、对象递归、数组替换。
    /// 下列断言逐条证明差异点。
    #[test]
    fn differs_from_json_merge_patch_on_null() {
        // JSON Merge Patch 下：{"a":1} ⊕ {"a":null} = {}
        // 本模块下：{"a":1} ⊕ {"a":null} = {"a": null}
        let mut a = json!({"a": 1});
        deep_merge(&mut a, &json!({"a": null}));
        assert_eq!(a, json!({"a": null}), "null 必须保留");
    }

    #[test]
    fn differs_from_json_merge_patch_on_deletion_mechanism() {
        // JSON Merge Patch: 删除用 null。本系统: null 是字面值，
        // "不设此键"用 $unset 表达（继承下层值）。
        let builtin = json!({"a": 1, "b": 2});
        let user = json!({"a": null});
        let out = merge_layers(&[layer(LayerKind::Builtin, builtin), layer(LayerKind::User, user)]);
        assert_eq!(out["a"], Value::Null, "null 保留为字面值，不删键");
        assert_eq!(out["b"], 2);
    }

    #[test]
    fn unset_inherits_from_below_layer() {
        // unset 语义是"继承下层"，不是"删除"。
        let builtin = json!({"a": 1, "b": 2});
        let plugin = json!({"a": 3});
        let user = json!({"$unset": ["a"]});
        let out = merge_layers(&[
            layer(LayerKind::Builtin, builtin),
            layer(LayerKind::Plugin, plugin),
            layer(LayerKind::User, user),
        ]);
        assert_eq!(out["a"], 3, "user unset 后 a 继承 plugin 层的 3");
        assert_eq!(out["b"], 2);
    }

    #[test]
    fn no_json_patch_dependency_is_used() {
        // 结构性证明：本模块的"删除"入口是 delete_path + UNSET_KEY，
        // 不通过 null 表达删除。
        let mut acc = json!({"a": 1});
        deep_merge(&mut acc, &json!({"a": null}));
        assert!(acc.get("a").is_some(), "deep_merge 从不因 null 删键");
    }
}
