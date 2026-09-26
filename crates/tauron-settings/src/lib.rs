// §4.12 设置中心（`tauron-settings`）。
//
// 职责（计划 §4.12）：schema 注册表 + 四层合并 + get/set/watch + 渲染数据产出。
//
// ```text
// 插件 schema（schemars 形态）
//   │ §4.13 管线：$ref 展开 / enum 规范化 / x-tauron → uiSchema
//   ▼
// SchemaRegistry（平面 schema + uiSchema + 字段数）
//   │ 每表单 ≤40 字段、平面形态（门禁 §8-12）
//   ▼
// SettingsStore：builtin < brand < plugin < user
//   │ 写入前按 schema 校验（含范围/枚举）
//   │ 等值写回落 unset（继承下层）
//   │ 插件命名空间隔离，禁止越写
//   ▼
// {schema, uiSchema, defaults} → RJSF / WC 渲染器
// ```
//
// 两套合并语义隔离（架构 §6.3）：本 crate 的深合并**不使用**
// `json_patch::merge`。JSON Merge Patch 用 `null` 删键，而设置里
// `null` 是合法值——本模块用独立的 `$unset` 列表表达"继承"。
// `merge::tests::differs_from_json_merge_patch_on_null` 逐条证明差异。
//
// 本 crate 的错误不跨 IPC：由 §4.10 UI 翻译成
// `tauron_host::error::HostError`（复用 `E_INVALID_MANIFEST`，
// 不新增 `ErrorCode`——TS 门禁要求两侧同序）。

pub mod error;
pub mod merge;
pub mod registry;
pub mod store;

pub use error::{LayerKind, SettingsError, SettingsResult};
pub use merge::{
    apply_op, decide_write, deep_merge, delete_path, inherited_at, merge_layers, read_path,
    split_unset, validate_path, write_path, WriteOp, UNSET_KEY,
};
pub use registry::{count_fields, Entry, SchemaRegistry, FIELD_LIMIT};
pub use store::{ChangeEvent, Migration, PluginState, SettingsStore, MIGRATION_STEP_LIMIT};

#[cfg(test)]
mod integration_tests {
    use super::*;
    use serde_json::{json, Value};
    use tauron_schema::validate;

    /// 端到端：schema 注册 → 四层注入 → 用户写入 → 等值回落 → unset →
    /// 渲染数据产出。覆盖计划 §4.12 的全部关键约束。
    #[test]
    fn end_to_end_settings_flow() {
        let mut store = SettingsStore::new();

        // 1. 注册 schema（含 x-tauron 扩展）。
        let raw = json!({
            "type": "object",
            "properties": {
                "volume": {"type":"integer","minimum":0,"maximum":100,
                    "x-tauron":{"version":1,"label":"音量","widget":"slider","order":1}},
                "muted": {"type":"boolean",
                    "x-tauron":{"version":1,"label":"静音","widget":"switch","order":2}},
                "language": {"enum":["zh","en"],
                    "x-tauron":{"version":1,"label":"语言","widget":"select","order":3}}
            },
            "required":["volume"]
        });
        store.register("p.app", "1.0.0", &raw).unwrap();
        assert_eq!(store.registry().get("p.app").unwrap().field_count, 3);

        // 2. 四层注入。
        store.set_layer(
            "p.app",
            LayerKind::Builtin,
            json!({"volume": 10, "muted": false, "language": "zh"}),
        );
        store.set_layer("p.app", LayerKind::Brand, json!({"language": "en"}));
        store.set_layer("p.app", LayerKind::Plugin, json!({"volume": 20}));
        let got = store.get("p.app").unwrap();
        assert_eq!(got, json!({"volume": 20, "muted": false, "language": "en"}));

        // 3. 用户写入 → 校验通过 → 值不同于继承值 → 真写入。
        let op = store.set("p.app", "p.app", "volume", &json!(66)).unwrap();
        assert!(matches!(op, WriteOp::Write { .. }));
        assert_eq!(store.get_key("p.app", "volume").unwrap(), Some(json!(66)));

        // 4. 等值写回落 unset：把 volume 写回 plugin 层的 20。
        let op = store.set("p.app", "p.app", "volume", &json!(20)).unwrap();
        assert!(matches!(op, WriteOp::Unset { .. }), "{op:?}");
        assert_eq!(store.get_key("p.app", "volume").unwrap(), Some(json!(20)));
        // user 层没有 volume 值。
        assert!(store.snapshot("p.app").unwrap().user.get("volume").is_none());

        // 5. 显式 unset：language 回落到 builtin 的 "zh"。
        store.unset("p.app", "p.app", "language").unwrap();
        // 注意：brand 层仍有 "en"，unset user 层只影响 user 层。
        assert_eq!(store.get_key("p.app", "language").unwrap(), Some(json!("en")));

        // 6. 越界写入被拒。
        let e = store.set("p.other", "p.app", "volume", &json!(1)).unwrap_err();
        assert!(matches!(e, SettingsError::NamespaceViolation { .. }));

        // 7. 校验拦截超范围写入。
        let e = store.set("p.app", "p.app", "volume", &json!(1000)).unwrap_err();
        assert!(matches!(e, SettingsError::Validation { .. }));

        // 8. 渲染数据产出。
        let render = store.registry().render("p.app", &json!({"volume": 20})).unwrap();
        assert_eq!(render["schema"]["properties"]["volume"]["minimum"], 0);
        assert_eq!(render["uiSchema"]["volume"]["ui:widget"], "slider");
        assert_eq!(render["uiSchema"]["muted"]["ui:label"], "静音");
        assert_eq!(render["defaults"]["volume"], 20);
    }

    /// 计划 §4.12 测试项："两套合并语义的隔离用例"——集成层再验一次。
    #[test]
    fn merge_semantics_are_isolated_from_json_patch() {
        let raw = json!({
            "type": "object",
            "properties": { "v": { "type": ["number", "null"] } }
        });
        let mut store = SettingsStore::new();
        store.register("p.x", "1.0.0", &raw).unwrap();
        store.set_layer("p.x", LayerKind::Builtin, json!({"v": 1}));
        // 写入真 null：JSON Merge Patch 会删键，本模块保留为字面值。
        store.set("p.x", "p.x", "v", &Value::Null).unwrap();
        let merged = store.get("p.x").unwrap();
        assert!(merged.get("v").is_some(), "null 必须保留为字面值");
        assert_eq!(merged["v"], Value::Null);
    }

    /// 计划 §4.12 测试项："等值写回不产生变更"。
    #[test]
    fn equal_writes_do_not_produce_changes() {
        let mut store = SettingsStore::new();
        store
            .register(
                "p.x",
                "1.0.0",
                &json!({"type":"object","properties":{"a":{"type":"integer"}}}),
            )
            .unwrap();
        store.set_layer("p.x", LayerKind::Builtin, json!({"a": 1}));
        let sub = store.watch("p.x");

        store.set("p.x", "p.x", "a", &json!(1)).unwrap(); // 等值
        let events = store.drain(sub);
        // 等值写回落 unset，但仍广播（调用方可能想知道"我这次写没生效"）。
        // 关键断言：user 层没有 a。
        assert!(store.snapshot("p.x").unwrap().user.get("a").is_none());
        assert!(events.iter().all(|e| e.value == json!(1)), "合并值不变");
    }

    /// 计划 §4.12 测试项："坏配置回退 + 报告路径"。
    #[test]
    fn bad_config_reports_path() {
        let mut store = SettingsStore::new();
        store
            .register(
                "p.x",
                "1.0.0",
                &json!({"type":"object","properties":{"v":{"type":"integer","minimum":0}}}),
            )
            .unwrap();
        store.set_layer("p.x", LayerKind::Builtin, json!({"v": 5}));
        // 直接注入坏的用户层（模拟磁盘损坏）。
        store.set_layer("p.x", LayerKind::User, json!({"v": -10}));
        let merged = store.get("p.x").unwrap();
        let errs =
            validate(&store.registry().get("p.x").unwrap().compiled.schema, &merged).unwrap_err();
        assert!(!errs.is_empty());
        assert!(errs.iter().all(|e| e.path.starts_with("/")), "{errs:?}");
    }
}
