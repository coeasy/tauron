// §4.12 设置存储：四层叠加 + get/set/watch + 命名空间隔离 + 写前校验。
//
// 命名空间即插件 id：每个插件只拥有自己的配置子树，
// 越界写入直接拒绝（计划 §4.12 显式约束）。
//
// 四层：`builtin`（编译进二进制）< `brand`（白标）< `plugin`（随包默认）
// < `user`（用户写入）。见 {@link crate::error::LayerKind}。
//
// 持久化：本模块只管内存态与层结构；磁盘 I/O 由 §4.12 的调用方
// （`tauri-plugin-store`）承担，本 crate 只暴露 `snapshot()`/`restore()`
// 以便序列化。这样"逻辑正确性"与"存储接线"可分开测试。

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use tauron_schema::validate;

use crate::error::{LayerKind, SettingsError, SettingsResult};
use crate::merge;
use crate::registry::SchemaRegistry;

/// 一个插件的完整配置状态（四层）。
///
/// 派生 `Serialize`/`Deserialize`：本模块按设计**不做磁盘 I/O**（见模块头），
/// 但持久化的落点需要能序列化这一层——`snapshot_all()` 的结果要能写进文件、
/// 读回来喂 `restore()`。没有这两个 derive，`snapshot_all`/`restore` 就只能是
/// 测试里的玩具（本仓曾如此：设置写进去，重启即丢）。
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PluginState {
    pub builtin: Value,
    pub brand: Value,
    pub plugin: Value,
    pub user: Value,
    /// 用户层数据**已经过哪一版 schema**（空 = 未知 / 从未标注）。
    ///
    /// 这是迁移的起点：`migrate` 从它出发沿迁移链走到目标版本。它只描述
    /// `user` 层——builtin/brand/plugin 三层随二进制一起升级，磁盘上不存在
    /// 旧版本，没有可迁移的东西。
    pub schema_version: String,
}

impl PluginState {
    /// 按优先级升序产出层数组，供 `merge_layers` 使用。
    pub fn layers(&self) -> [(LayerKind, Value); 4] {
        [
            (LayerKind::Builtin, self.builtin.clone()),
            (LayerKind::Brand, self.brand.clone()),
            (LayerKind::Plugin, self.plugin.clone()),
            (LayerKind::User, self.user.clone()),
        ]
    }

    /// 合并后的最终值。
    pub fn merged(&self) -> Value {
        merge::merge_layers(&self.layers())
    }
}

/// 一次 schema 版本迁移：把**用户层**的值从 `from` 版本改写成 `to` 版本。
///
/// 用函数指针而不是闭包 trait：`Migration` 要能廉价地复制进/出注册表，
/// 且迁移函数是**纯函数**（同输入必同输出），没有捕获状态的必要。
#[derive(Clone)]
pub struct Migration {
    /// 起点版本。
    pub from: String,
    /// 终点版本。
    pub to: String,
    /// 改写函数：收旧用户层值，返回新用户层值。
    pub apply: fn(&Value) -> Value,
}

impl Migration {
    pub fn new(from: &str, to: &str, apply: fn(&Value) -> Value) -> Self {
        Self {
            from: from.to_string(),
            to: to.to_string(),
            apply,
        }
    }
}

/// 一条迁移链的步数上限（防迁移自环把启动挂死）。
pub const MIGRATION_STEP_LIMIT: usize = 16;

/// 校验用 schema：默认禁止未声明键，但**尊重** schema 自己声明的
/// `additionalProperties: true`。
///
/// 为什么需要这个例外：宿主级设置文档的键是**开放集合**（插件各自带命名空间，
/// 宿主编译期枚举不出来），它的 schema 只能声明 `additionalProperties: true`。
/// 若在这里无条件改成 `false`，这类文档一个键都写不进去。既有 schema 不带这个
/// 声明，行为与从前逐字一致（缺省仍严格）。
fn validation_schema(entry: &crate::registry::Entry) -> Value {
    let mut strict = entry.compiled.schema.clone();
    if let Value::Object(m) = &mut strict {
        if !matches!(m.get("additionalProperties"), Some(Value::Bool(true))) {
            m.insert("additionalProperties".to_string(), Value::Bool(false));
        }
    }
    strict
}

/// 用户层里是否真的有数据（`$unset` 标记不算数据）。
fn user_layer_has_data(state: Option<&PluginState>) -> bool {
    match state {
        Some(s) => !merge::split_unset(&s.user).1.is_empty(),
        None => false,
    }
}

/// 变更事件。
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeEvent {
    pub plugin_id: String,
    pub key: String,
    pub value: Value,
    pub source: LayerKind,
}

/// 单个订阅者待消费事件的**上限**（环形：超限丢最旧一条）。
///
/// 为什么需要：`broadcast` 是无条件 `push`。订阅者若从不 `drain`，队列会随每次
/// 设置写入无限增长——这是宿主内存被订阅方单方面拖垮的路径。到顶后丢最旧，
/// 与 `tauron-notify` 的环形缓冲、`MemoryWindowSink` 的留痕上限同一策略。
pub const MAX_PENDING_EVENTS: usize = 1024;

/// 变更订阅者（简单广播：每个订阅者一条独立队列）。
#[derive(Default)]
pub struct Watcher {
    /// 订阅者 id → 待消费事件。
    queues: BTreeMap<u64, Vec<ChangeEvent>>,
    next_id: u64,
}

impl Watcher {
    pub fn subscribe(&mut self) -> u64 {
        let id = self.next_id;
        self.queues.insert(id, Vec::new());
        self.next_id += 1;
        id
    }

    pub fn unsubscribe(&mut self, id: u64) {
        self.queues.remove(&id);
    }

    pub fn broadcast(&mut self, event: &ChangeEvent) {
        for q in self.queues.values_mut() {
            if q.len() >= MAX_PENDING_EVENTS {
                q.remove(0);
            }
            q.push(event.clone());
        }
    }

    /// 取出某订阅者的全部待消费事件（消费即清空）。
    pub fn drain(&mut self, id: u64) -> Vec<ChangeEvent> {
        self.queues.get_mut(&id).map(|q| std::mem::take(q)).unwrap_or_default()
    }

    /// 活跃订阅者数（用于"卸载零悬挂"类门禁）。
    pub fn subscriber_count(&self) -> usize {
        self.queues.len()
    }
}

/// 设置存储。
pub struct SettingsStore {
    registry: SchemaRegistry,
    states: BTreeMap<String, PluginState>,
    watcher: Watcher,
    /// 命名空间 → 已注册的迁移步骤。
    migrations: BTreeMap<String, Vec<Migration>>,
}

impl SettingsStore {
    pub fn new() -> Self {
        Self {
            registry: SchemaRegistry::new(),
            states: BTreeMap::new(),
            watcher: Watcher::default(),
            migrations: BTreeMap::new(),
        }
    }

    pub fn registry(&self) -> &SchemaRegistry {
        &self.registry
    }

    /// 注册 schema（见 {@link SchemaRegistry::register}）。
    pub fn register(&mut self, plugin_id: &str, version: &str, raw: &Value) -> SettingsResult<()> {
        self.registry.register(plugin_id, version, raw)
    }

    /// 设置某层的值（整层替换）。用于启动时注入 builtin/brand/plugin 层。
    pub fn set_layer(&mut self, plugin_id: &str, kind: LayerKind, value: Value) {
        let s = self.state(plugin_id);
        match kind {
            LayerKind::Builtin => s.builtin = value,
            LayerKind::Brand => s.brand = value,
            LayerKind::Plugin => s.plugin = value,
            LayerKind::User => s.user = value,
        }
    }

    /// 取合并后的当前值。
    pub fn get(&self, plugin_id: &str) -> SettingsResult<Value> {
        Ok(self
            .state_ref(plugin_id)
            .map(PluginState::merged)
            .unwrap_or(Value::Object(Map::new())))
    }

    /// 取某键的当前值（点路径）。
    pub fn get_key(&self, plugin_id: &str, path: &str) -> SettingsResult<Option<Value>> {
        Ok(merge::read_path(&self.get(plugin_id)?, path))
    }

    /// 取合并值及其来源标注（每个叶子键来自哪一层）。
    pub fn get_with_source(
        &self,
        plugin_id: &str,
    ) -> SettingsResult<(Value, BTreeMap<String, String>)> {
        let s = self
            .state_ref(plugin_id)
            .ok_or_else(|| SettingsError::SchemaNotRegistered(plugin_id.to_string()))?;
        let merged = s.merged();
        Ok((merged, source_map(s)))
    }

    /// 写入一个键。**先校验、再判等值回落、最后落盘并广播**。
    ///
    /// 返回写入了什么：真值或 unset（继承）。
    pub fn set(
        &mut self,
        writer: &str,
        plugin_id: &str,
        path: &str,
        value: &Value,
    ) -> SettingsResult<merge::WriteOp> {
        // 门禁 1：schema 已注册。
        let entry = self.registry.get(plugin_id).ok_or_else(|| {
            SettingsError::SchemaNotRegistered(plugin_id.to_string())
        })?;

        // 门禁 2：命名空间隔离。
        if writer != plugin_id {
            return Err(SettingsError::NamespaceViolation {
                writer: writer.to_string(),
                target: plugin_id.to_string(),
            });
        }

        // 门禁 3：路径合法。
        merge::validate_path(path)?;

        // 门禁 4：写前校验。校验对象是"写入后的完整值"。
        // 设置 schema 默认禁止未声明键（additionalProperties: false）；
        // 显式声明开放键集合的 schema 例外（见 `validation_schema`）。
        let strict = validation_schema(entry);
        let schema_version = entry.schema_version.clone();
        let mut next = self.get(plugin_id)?;
        merge::write_path(&mut next, path, value.clone());
        let errs = validate(&strict, &next);
        if let Err(v) = errs {
            return Err(SettingsError::Validation {
                key: path.to_string(),
                detail: v.iter().map(|e| format!("{} {}", e.path, e.message)).collect(),
            });
        }

        // 门禁 5：等值写回落 unset。
        let layers = self.state(plugin_id).layers();
        let inherited = merge::inherited_at(&layers, path);
        let op = merge::decide_write(path, value, &inherited);

        // 落实。
        let s = self.state(plugin_id);
        if !s.user.is_object() {
            s.user = Value::Object(Map::new());
        }
        let map = s.user.as_object_mut().expect("刚确认为对象");
        merge::apply_op(map, &op);
        // 用户层现在含一条按**本版** schema 校验过的值 → 标注数据版本。
        // 迁移的起点由此明确，不需要猜。
        s.schema_version = schema_version;

        // 广播（合并后的实际值，而非用户写的那层）。
        let final_value = self.get_key(plugin_id, path)?.unwrap_or(Value::Null);
        self.watcher.broadcast(&ChangeEvent {
            plugin_id: plugin_id.to_string(),
            key: path.to_string(),
            value: final_value,
            source: LayerKind::User,
        });

        Ok(op)
    }

    /// 显式设为继承（不落值）。
    pub fn unset(&mut self, writer: &str, plugin_id: &str, path: &str) -> SettingsResult<()> {
        if writer != plugin_id {
            return Err(SettingsError::NamespaceViolation {
                writer: writer.to_string(),
                target: plugin_id.to_string(),
            });
        }
        merge::validate_path(path)?;
        let s = self.state(plugin_id);
        if !s.user.is_object() {
            s.user = Value::Object(Map::new());
        }
        let map = s.user.as_object_mut().expect("刚确认为对象");
        merge::apply_op(map, &merge::WriteOp::Unset { path: path.to_string() });
        Ok(())
    }

    /// 订阅变更。
    ///
    /// # 诚实边界
    ///
    /// - 本 API 目前**没有生产调用点**（仓内只有单测用）。`tauron-adapter` 的
    ///   设置命令走的是"写即落盘"，没有订阅回推路径。要用它需要先在宿主侧
    ///   接一条事件出口。
    /// - `plugin_id` 参数当前**被忽略**：`broadcast` 会把所有命名空间的变更都投给
    ///   每个订阅者。真要做按插件隔离的订阅，得先让 `ChangeEvent` 的过滤落到
    ///   队列分发处，而不是在这里加一个没人读的参数。
    /// - 队列有上限（[`MAX_PENDING_EVENTS`]）：订阅者不 `drain` 时丢最旧，不无限增长。
    pub fn watch(&mut self, _plugin_id: &str) -> u64 {
        self.watcher.subscribe()
    }

    /// 取消订阅。
    pub fn unwatch(&mut self, id: u64) {
        self.watcher.unsubscribe(id);
    }

    /// 取出某订阅者的全部待消费事件。
    pub fn drain(&mut self, id: u64) -> Vec<ChangeEvent> {
        self.watcher.drain(id)
    }

    /// 活跃订阅者数。
    pub fn subscriber_count(&self) -> usize {
        self.watcher.subscriber_count()
    }

    /// 取某插件的状态快照（可序列化）。
    pub fn snapshot(&self, plugin_id: &str) -> Option<PluginState> {
        self.states.get(plugin_id).cloned()
    }

    /// 全量快照（用于持久化）。
    pub fn snapshot_all(&self) -> Vec<(String, PluginState)> {
        self.states.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// 从快照恢复。
    pub fn restore(&mut self, entries: &[(String, PluginState)]) {
        self.states.clear();
        for (k, v) in entries {
            self.states.insert(k.clone(), v.clone());
        }
    }

    // ── 版本迁移 ────────────────────────────────────────────────────

    /// 注册一条迁移步骤（`from` → `to`）。
    ///
    /// 同一 `(from, to)` 重复注册会**替换**：调用方反复装配时不该把步骤越堆越多。
    /// 迁移是显式的——不会在 `set` 里偷偷改写用户数据。理由：写入路径上的隐式
    /// 迁移意味着「用户改一个无关的键」也可能重写整层数据，出错时无从审计；
    /// 显式调用只有一处，可测可回滚。
    pub fn register_migration(&mut self, plugin_id: &str, migration: Migration) {
        let list = self.migrations.entry(plugin_id.to_string()).or_default();
        list.retain(|m| !(m.from == migration.from && m.to == migration.to));
        list.push(migration);
    }

    /// 已注册的迁移步数（诊断 / 门禁用）。
    pub fn migration_count(&self, plugin_id: &str) -> usize {
        self.migrations.get(plugin_id).map_or(0, Vec::len)
    }

    /// 标注该命名空间**已存数据**的 schema 版本。
    ///
    /// 宿主从磁盘载入旧版配置后必须调它，否则 [`Self::migrate`] 无从知道起点。
    pub fn set_data_version(&mut self, plugin_id: &str, version: &str) {
        self.state(plugin_id).schema_version = version.to_string();
    }

    /// 已存数据的 schema 版本（`None` = 既无数据也无标注）。
    pub fn data_version(&self, plugin_id: &str) -> Option<&str> {
        self.state_ref(plugin_id)
            .map(|s| s.schema_version.as_str())
            .filter(|v| !v.is_empty())
    }

    /// 沿迁移链把该命名空间的**用户层**从已标注版本迁到 `to_version`。
    ///
    /// **全有或全无**：链路缺失、出现自环、或迁移结果过不了 schema 校验时，
    /// 用户层一个字节都不改，返回错误。迁移途中改坏数据再报错，等于让调用方
    /// 在「旧数据」和「坏数据」之间二选一——两边都不能接受。
    ///
    /// # 返回
    ///
    /// 实际应用的迁移步数（版本已相等或没有数据时为 `0`）。
    pub fn migrate(&mut self, plugin_id: &str, to_version: &str) -> SettingsResult<usize> {
        let from = self.data_version(plugin_id).unwrap_or("").to_string();
        if from == to_version {
            return Ok(0);
        }
        if from.is_empty() {
            if user_layer_has_data(self.state_ref(plugin_id)) {
                // 有数据却没有起点版本：**不猜**。猜错就是静默数据损坏。
                return Err(SettingsError::SchemaCompile(format!(
                    "命名空间 `{plugin_id}` 的用户层有数据但未标注 schema 版本，\
                     无法迁到 `{to_version}`（先用 set_data_version 标注来源版本）"
                )));
            }
            self.set_data_version(plugin_id, to_version);
            return Ok(0);
        }

        // 在副本上走完整条链。
        let mut current = from.clone();
        let mut data = self
            .state_ref(plugin_id)
            .map(|s| s.user.clone())
            .unwrap_or(Value::Null);
        let mut applied = 0usize;
        while current != to_version {
            let step = self
                .migrations
                .get(plugin_id)
                .and_then(|list| list.iter().find(|m| m.from == current).cloned());
            let Some(step) = step else {
                return Err(SettingsError::SchemaCompile(format!(
                    "命名空间 `{plugin_id}` 缺少 `{current}` → `{to_version}` 的迁移步骤"
                )));
            };
            data = (step.apply)(&data);
            current = step.to.clone();
            applied += 1;
            if applied > MIGRATION_STEP_LIMIT {
                // 自环（`to == from`）会让上面的循环永不收敛。
                return Err(SettingsError::SchemaCompile(format!(
                    "命名空间 `{plugin_id}` 的迁移链超过 {MIGRATION_STEP_LIMIT} 步（疑似自环），已中止"
                )));
            }
        }

        // 落新值前先校验：迁移不该造出 schema 不接受的数据。
        if let Some(entry) = self.registry.get(plugin_id) {
            let strict = validation_schema(entry);
            let mut candidate = self.state_ref(plugin_id).cloned().unwrap_or_default();
            candidate.user = data.clone();
            let errs = validate(&strict, &candidate.merged());
            if let Err(v) = errs {
                return Err(SettingsError::Validation {
                    key: format!("$migrate:{from}→{to_version}"),
                    detail: v.iter().map(|e| format!("{} {}", e.path, e.message)).collect(),
                });
            }
        }

        let s = self.state(plugin_id);
        s.user = data;
        s.schema_version = to_version.to_string();
        Ok(applied)
    }

    fn state(&mut self, plugin_id: &str) -> &mut PluginState {
        self.states.entry(plugin_id.to_string()).or_default()
    }

    fn state_ref(&self, plugin_id: &str) -> Option<&PluginState> {
        self.states.get(plugin_id)
    }
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// 计算每个叶子键的来源层（自顶向下：最高层赢）。
fn source_map(s: &PluginState) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    collect_source(s.builtin.clone(), LayerKind::Builtin, &mut out);
    collect_source(s.brand.clone(), LayerKind::Brand, &mut out);
    collect_source(s.plugin.clone(), LayerKind::Plugin, &mut out);
    collect_source(s.user.clone(), LayerKind::User, &mut out);
    out
}

fn collect_source(node: Value, kind: LayerKind, out: &mut BTreeMap<String, String>) {
    let Value::Object(m) = node else { return };
    for (k, v) in m {
        if k.as_str() == merge::UNSET_KEY {
            continue;
        }
        if v.is_object() && !v.as_object().unwrap().is_empty() {
            collect_source(v, kind, out);
        } else {
            out.insert(k, kind.as_str().to_string());
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn audio_schema() -> Value {
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

    fn store_with_audio() -> SettingsStore {
        let mut s = SettingsStore::new();
        s.register("p.audio", "1.0.0", &audio_schema()).unwrap();
        s
    }

    // ── 四层优先级 ──────────────────────────────────────────────────

    #[test]
    fn four_layer_priority() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set_layer("p.audio", LayerKind::Brand, json!({"volume": 20}));
        s.set_layer("p.audio", LayerKind::Plugin, json!({"volume": 30}));
        s.set_layer("p.audio", LayerKind::User, json!({"volume": 40}));
        assert_eq!(s.get("p.audio").unwrap()["volume"], 40);
    }

    #[test]
    fn get_key_reads_the_merged_value() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10, "muted": false}));
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(10)));
        assert_eq!(s.get_key("p.audio", "muted").unwrap(), Some(json!(false)));
        assert_eq!(s.get_key("p.audio", "nope").unwrap(), None);
    }

    #[test]
    fn set_requires_registered_schema() {
        let mut s = SettingsStore::new();
        s.set_layer("p.x", LayerKind::Builtin, json!({"a": 1}));
        let e = s.set("p.x", "p.x", "a", &json!(2)).unwrap_err();
        assert!(matches!(e, SettingsError::SchemaNotRegistered(ref p) if p == "p.x"));
    }

    // ── 命名空间隔离 ────────────────────────────────────────────────

    #[test]
    fn namespace_isolation_rejects_cross_write() {
        let mut s = store_with_audio();
        let e = s.set("p.other", "p.audio", "volume", &json!(50)).unwrap_err();
        match e {
            SettingsError::NamespaceViolation { writer, target } => {
                assert_eq!(writer, "p.other");
                assert_eq!(target, "p.audio");
            }
            other => panic!("期望 NamespaceViolation，实际 {other:?}"),
        }
    }

    #[test]
    fn namespace_isolation_rejects_cross_unset() {
        let mut s = store_with_audio();
        let e = s.unset("p.other", "p.audio", "volume").unwrap_err();
        assert!(matches!(e, SettingsError::NamespaceViolation { .. }));
    }

    #[test]
    fn owner_can_write() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let op = s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        assert!(matches!(op, merge::WriteOp::Write { .. }));
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(50)));
    }

    // ── 写前校验 ────────────────────────────────────────────────────

    #[test]
    fn set_rejects_out_of_range_value() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let e = s.set("p.audio", "p.audio", "volume", &json!(999)).unwrap_err();
        match e {
            SettingsError::Validation { key, detail } => {
                assert_eq!(key, "volume");
                assert!(!detail.is_empty());
            }
            other => panic!("期望 Validation，实际 {other:?}"),
        }
    }

    #[test]
    fn set_rejects_wrong_type() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let e = s.set("p.audio", "p.audio", "volume", &json!("loud")).unwrap_err();
        assert!(matches!(e, SettingsError::Validation { .. }));
    }

    #[test]
    fn set_rejects_unknown_key() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let e = s.set("p.audio", "p.audio", "brightness", &json!(1)).unwrap_err();
        assert!(matches!(e, SettingsError::Validation { .. }));
    }

    #[test]
    fn set_rejects_invalid_path() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        for p in ["", "$unset", ".a", "a.", "a..b"] {
            assert!(s.set("p.audio", "p.audio", p, &json!(1)).is_err(), "{p}");
        }
    }

    // ── 等值写回落 unset ────────────────────────────────────────────

    #[test]
    fn equal_write_falls_back_to_unset() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set_layer("p.audio", LayerKind::Plugin, json!({"volume": 10}));
        let op = s.set("p.audio", "p.audio", "volume", &json!(10)).unwrap();
        assert!(matches!(op, merge::WriteOp::Unset { .. }), "{op:?}");
        // 合并值不变。
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(10)));
    }

    #[test]
    fn equal_write_to_builtin_produces_no_user_value() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set("p.audio", "p.audio", "volume", &json!(10)).unwrap();
        let user = s.snapshot("p.audio").unwrap().user;
        assert!(user.get("volume").is_none(), "等值写不应产生 user 层的 volume");
        assert!(user.get(merge::UNSET_KEY).is_some());
    }

    #[test]
    fn equal_write_then_different_write_is_real() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set("p.audio", "p.audio", "volume", &json!(10)).unwrap();
        s.set("p.audio", "p.audio", "volume", &json!(55)).unwrap();
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(55)));
        let user = s.snapshot("p.audio").unwrap().user;
        assert_eq!(user["volume"], 55);
    }

    #[test]
    fn unset_then_read_inherits() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set_layer("p.audio", LayerKind::User, json!({"volume": 90}));
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(90)));
        s.unset("p.audio", "p.audio", "volume").unwrap();
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(10)));
    }

    // ── watch ───────────────────────────────────────────────────────

    #[test]
    fn watch_receives_events() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let sub = s.watch("p.audio");
        s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        let events = s.drain(sub);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].plugin_id, "p.audio");
        assert_eq!(events[0].key, "volume");
        assert_eq!(events[0].value, json!(50));
        assert_eq!(events[0].source, LayerKind::User);
    }

    #[test]
    fn watch_queue_is_per_subscriber() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let a = s.watch("p.audio");
        let b = s.watch("p.audio");
        s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        assert_eq!(s.drain(a).len(), 1);
        assert_eq!(s.drain(b).len(), 1, "每个订阅者独立收一份");
    }

    #[test]
    fn watcher_queue_is_bounded_so_a_silent_subscriber_cannot_grow_forever() {
        // `broadcast` 是无条件 push：订阅者从不 drain 时队列会随每次写入无限增长。
        // 到顶后丢最旧，稳定在 MAX_PENDING_EVENTS。
        let mut w = Watcher::default();
        let id = w.subscribe();
        for i in 0..(MAX_PENDING_EVENTS + 25) {
            w.broadcast(&ChangeEvent {
                plugin_id: "p".to_string(),
                key: format!("k{i}"),
                value: json!(i),
                source: LayerKind::User,
            });
        }
        let drained = w.drain(id);
        assert_eq!(drained.len(), MAX_PENDING_EVENTS, "队列必须有上限");
        // 丢的是**最旧**的：留下的是最后 MAX_PENDING_EVENTS 条。
        assert_eq!(drained[0].key, format!("k{}", 25));
        assert_eq!(
            drained[MAX_PENDING_EVENTS - 1].key,
            format!("k{}", MAX_PENDING_EVENTS + 24)
        );
    }

    #[test]
    fn drain_twice_yields_nothing_the_second_time() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let sub = s.watch("p.audio");
        s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        assert_eq!(s.drain(sub).len(), 1);
        assert_eq!(s.drain(sub).len(), 0);
    }

    #[test]
    fn unwatch_removes_the_subscriber() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let sub = s.watch("p.audio");
        assert_eq!(s.subscriber_count(), 1);
        s.unwatch(sub);
        assert_eq!(s.subscriber_count(), 0);
        s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        assert!(s.drain(sub).is_empty());
    }

    #[test]
    fn unsubscribed_subscriber_is_not_leaked() {
        // §8-3 类门禁：卸载后零悬挂。
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        let sub = s.watch("p.audio");
        s.unwatch(sub);
        s.drain(sub);
        assert_eq!(s.subscriber_count(), 0);
    }

    // ── 来源标注 ────────────────────────────────────────────────────

    #[test]
    fn source_map_reports_the_winning_layer() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10, "muted": false}));
        s.set_layer("p.audio", LayerKind::Plugin, json!({"volume": 30}));
        let (_merged, src) = s.get_with_source("p.audio").unwrap();
        assert_eq!(src["volume"], "plugin");
        assert_eq!(src["muted"], "builtin");
    }

    // ── 快照 / 恢复 ────────────────────────────────────────────────

    #[test]
    fn snapshot_restore_roundtrip() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        let snap = s.snapshot_all();

        let mut s2 = store_with_audio();
        s2.restore(&snap);
        assert_eq!(s2.get("p.audio").unwrap(), s.get("p.audio").unwrap());
    }

    #[test]
    fn snapshot_is_none_for_missing_plugin() {
        let s = store_with_audio();
        assert!(s.snapshot("nope").is_none());
    }

    // ── 状态跨插件隔离 ──────────────────────────────────────────────

    #[test]
    fn plugins_do_not_share_state() {
        let mut s = store_with_audio();
        s.register("p.video", "1.0.0", &json!({
            "type":"object","properties":{"brightness":{"type":"integer","minimum":0,"maximum":100}}
        })).unwrap();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set_layer("p.video", LayerKind::Builtin, json!({"brightness": 100}));
        assert_eq!(s.get_key("p.audio", "volume").unwrap(), Some(json!(10)));
        assert_eq!(s.get_key("p.video", "brightness").unwrap(), Some(json!(100)));
        assert_eq!(s.get_key("p.audio", "brightness").unwrap(), None);
    }

    // ── 坏配置回退 ────────────────────────────────────────────────

    #[test]
    fn bad_user_layer_is_reported_and_falls_back() {
        // 用户层里的值不合法：合并仍给出结果，但 get 前的校验能捕获。
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        // 直接塞非法值（绕过 set 的门禁）——模拟"磁盘上的坏配置"。
        s.set_layer("p.audio", LayerKind::User, json!({"volume": 999999}));
        let merged = s.get("p.audio").unwrap();
        let errs = validate(&s.registry().get("p.audio").unwrap().compiled.schema, &merged);
        let errs = errs.unwrap_err();
        assert!(errs.iter().any(|e| e.path.contains("volume")));
        assert!(!errs.iter().all(|e| e.path.is_empty()), "错误需带定位路径");
    }

    #[test]
    fn bad_config_in_plugin_layer_does_not_break_merge() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        s.set_layer("p.audio", LayerKind::Plugin, json!({"volume": "not-a-number"}));
        let merged = s.get("p.audio").unwrap();
        assert_eq!(merged["volume"], "not-a-number");
        // 校验能定位到具体路径。
        let errs = validate(&s.registry().get("p.audio").unwrap().compiled.schema, &merged).unwrap_err();
        assert_eq!(errs[0].path, "/volume");
    }

    #[test]
    fn empty_store_get_is_empty_object() {
        let s = store_with_audio();
        assert_eq!(s.get("p.audio").unwrap(), json!({}));
    }

    #[test]
    fn layered_defaults_are_mergeable() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10, "muted": false}));
        s.set_layer("p.audio", LayerKind::Brand, json!({"volume": 25}));
        assert_eq!(s.get("p.audio").unwrap(), json!({"volume": 25, "muted": false}));
    }

    #[test]
    fn many_writes_produce_the_right_final_state() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10, "muted": false}));
        s.set("p.audio", "p.audio", "volume", &json!(20)).unwrap();
        s.set("p.audio", "p.audio", "muted", &json!(true)).unwrap();
        s.set("p.audio", "p.audio", "volume", &json!(30)).unwrap();
        s.unset("p.audio", "p.audio", "muted").unwrap();
        assert_eq!(s.get("p.audio").unwrap(), json!({"volume": 30, "muted": false}));
    }

    // ── 开放键集合 schema（additionalProperties 显式 true）────────────

    #[test]
    fn explicit_open_object_schema_accepts_undeclared_keys() {
        // 宿主级设置文档的键是开放集合；schema 显式声明 true 时必须被尊重。
        let mut s = SettingsStore::new();
        s.register(
            "host",
            "1.0.0",
            &json!({"type": "object", "additionalProperties": true}),
        )
        .unwrap();
        s.set("host", "host", "plugin%3Ap.theme", &json!("dark")).unwrap();
        s.set("host", "host", "other-key", &json!(1)).unwrap();
        assert_eq!(
            s.get_key("host", "plugin%3Ap.theme").unwrap(),
            Some(json!("dark"))
        );
        assert_eq!(s.get_key("host", "other-key").unwrap(), Some(json!(1)));
    }

    #[test]
    fn undeclared_keys_are_still_rejected_without_the_explicit_opt_in() {
        // 缺省仍严格——上面的例外不能顺手把门禁拆了。
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        assert!(s.set("p.audio", "p.audio", "brightness", &json!(1)).is_err());
    }

    // ── 数据版本标注 ────────────────────────────────────────────────

    #[test]
    fn writes_stamp_the_data_version() {
        let mut s = store_with_audio();
        s.set_layer("p.audio", LayerKind::Builtin, json!({"volume": 10}));
        assert_eq!(s.data_version("p.audio"), None, "没有数据就没有版本标注");
        s.set("p.audio", "p.audio", "volume", &json!(50)).unwrap();
        assert_eq!(s.data_version("p.audio"), Some("1.0.0"));
    }

    // ── v1 → v2 迁移 ────────────────────────────────────────────────

    /// 计划 R7「验证」项：settings v1→v2 迁移。
    fn v1_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": true,
            "properties": {"legacyTheme": {"type": "string"}}
        })
    }

    fn v2_schema() -> Value {
        json!({
            "type": "object",
            "additionalProperties": true,
            "properties": {"appearance": {"type": "string"}}
        })
    }

    /// v1 → v2 的改写：`legacyTheme` 改名为 `appearance`。
    fn migrate_v1_to_v2(user: &Value) -> Value {
        let mut out = user.clone();
        if let Value::Object(m) = &mut out {
            if let Some(v) = m.remove("legacyTheme") {
                m.insert("appearance".to_string(), v);
            }
        }
        out
    }

    #[test]
    fn settings_v1_to_v2_migration_rewrites_the_user_layer() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        assert_eq!(s.data_version("p.app"), Some("1.0.0"));

        // 升版 + 注册迁移。
        s.register("p.app", "2.0.0", &v2_schema()).unwrap();
        s.register_migration("p.app", Migration::new("1.0.0", "2.0.0", migrate_v1_to_v2));
        assert_eq!(s.migration_count("p.app"), 1);

        let steps = s.migrate("p.app", "2.0.0").unwrap();
        assert_eq!(steps, 1, "应恰好走一步");
        assert_eq!(s.data_version("p.app"), Some("2.0.0"));
        assert_eq!(s.get_key("p.app", "appearance").unwrap(), Some(json!("dark")));
        assert_eq!(s.get_key("p.app", "legacyTheme").unwrap(), None, "旧键必须被搬走");
    }

    #[test]
    fn migration_is_idempotent_and_can_be_re_run() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        s.register("p.app", "2.0.0", &v2_schema()).unwrap();
        s.register_migration("p.app", Migration::new("1.0.0", "2.0.0", migrate_v1_to_v2));
        assert_eq!(s.migrate("p.app", "2.0.0").unwrap(), 1);
        // 第二次调用：版本已相等 → 0 步、数据不变（不会把 appearance 再搬一次）。
        assert_eq!(s.migrate("p.app", "2.0.0").unwrap(), 0);
        assert_eq!(s.get_key("p.app", "appearance").unwrap(), Some(json!("dark")));
    }

    #[test]
    fn migration_with_a_gap_changes_nothing() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        s.register("p.app", "3.0.0", &v2_schema()).unwrap();
        // 只注册 1 → 2，目标是 3：链断了。
        s.register_migration("p.app", Migration::new("1.0.0", "2.0.0", migrate_v1_to_v2));
        let e = s.migrate("p.app", "3.0.0").unwrap_err();
        assert!(matches!(e, SettingsError::SchemaCompile(ref m) if m.contains("缺少")), "{e}");
        // 全有或全无：版本与数据都没动。
        assert_eq!(s.data_version("p.app"), Some("1.0.0"));
        assert_eq!(s.get_key("p.app", "legacyTheme").unwrap(), Some(json!("dark")));
    }

    #[test]
    fn migration_rejects_data_the_new_schema_refuses() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        // v2 要求 appearance 是整数——迁移函数产出字符串，必须被拦下。
        let strict_v2 = json!({
            "type": "object",
            "additionalProperties": true,
            "properties": {"appearance": {"type": "integer"}}
        });
        s.register("p.app", "2.0.0", &strict_v2).unwrap();
        s.register_migration("p.app", Migration::new("1.0.0", "2.0.0", migrate_v1_to_v2));
        let e = s.migrate("p.app", "2.0.0").unwrap_err();
        assert!(matches!(e, SettingsError::Validation { .. }), "{e}");
        // 校验失败 → 数据与版本都不动。
        assert_eq!(s.data_version("p.app"), Some("1.0.0"));
        assert_eq!(s.get_key("p.app", "legacyTheme").unwrap(), Some(json!("dark")));
    }

    #[test]
    fn migration_without_a_starting_version_is_refused_when_data_exists() {
        let mut s = SettingsStore::new();
        s.register("p.app", "2.0.0", &v2_schema()).unwrap();
        // 绕过 set 直接注入用户层（模拟磁盘上的数据），且不标注版本。
        s.set_layer("p.app", LayerKind::User, json!({"legacyTheme": "dark"}));
        let e = s.migrate("p.app", "2.0.0").unwrap_err();
        assert!(matches!(e, SettingsError::SchemaCompile(ref m) if m.contains("未标注")), "{e}");
    }

    #[test]
    fn migration_on_empty_data_just_stamps_the_version() {
        let mut s = SettingsStore::new();
        s.register("p.app", "2.0.0", &v2_schema()).unwrap();
        assert_eq!(s.migrate("p.app", "2.0.0").unwrap(), 0);
        assert_eq!(s.data_version("p.app"), Some("2.0.0"));
    }

    #[test]
    fn migration_chain_walks_multiple_steps() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        s.register(
            "p.app",
            "3.0.0",
            &json!({"type":"object","additionalProperties":true,
                    "properties":{"palette":{"type":"string"}}}),
        )
        .unwrap();
        s.register_migration("p.app", Migration::new("1.0.0", "2.0.0", migrate_v1_to_v2));
        s.register_migration("p.app", Migration::new("2.0.0", "3.0.0", |user| {
            let mut out = user.clone();
            if let Value::Object(m) = &mut out {
                if let Some(v) = m.remove("appearance") {
                    m.insert("palette".to_string(), v);
                }
            }
            out
        }));
        assert_eq!(s.migrate("p.app", "3.0.0").unwrap(), 2, "两步链应走两步");
        assert_eq!(s.get_key("p.app", "palette").unwrap(), Some(json!("dark")));
        assert_eq!(s.data_version("p.app"), Some("3.0.0"));
    }

    #[test]
    fn self_looping_migration_is_aborted() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        s.register("p.app", "2.0.0", &v2_schema()).unwrap();
        // 自环：1.0.0 → 1.0.0。
        s.register_migration("p.app", Migration::new("1.0.0", "1.0.0", |v| v.clone()));
        let e = s.migrate("p.app", "2.0.0").unwrap_err();
        assert!(matches!(e, SettingsError::SchemaCompile(ref m) if m.contains("自环")), "{e}");
    }

    #[test]
    fn snapshot_restore_keeps_the_data_version() {
        let mut s = SettingsStore::new();
        s.register("p.app", "1.0.0", &v1_schema()).unwrap();
        s.set("p.app", "p.app", "legacyTheme", &json!("dark")).unwrap();
        let snap = s.snapshot_all();

        let mut s2 = SettingsStore::new();
        s2.register("p.app", "2.0.0", &v2_schema()).unwrap();
        s2.restore(&snap);
        assert_eq!(s2.data_version("p.app"), Some("1.0.0"), "恢复后仍知道数据是 v1");
        s2.register_migration("p.app", Migration::new("1.0.0", "2.0.0", migrate_v1_to_v2));
        assert_eq!(s2.migrate("p.app", "2.0.0").unwrap(), 1);
        assert_eq!(s2.get_key("p.app", "appearance").unwrap(), Some(json!("dark")));
    }
}
