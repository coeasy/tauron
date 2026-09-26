// §4.15 通知中心。
//
// 职责（计划 §4.15）：统一出口分发应用内/系统通知 + 历史持久化
// （未读/分组/清空）。
//
// 关键约束：
// - 历史条目上限 + **环形裁剪**（满时丢最旧，不阻塞写入）；
// - 系统通知未授权/不支持时**静默降级**（不报错、不重试、只记录）；
// - 插件通知按 `plugin:<id>` 归组并**随卸载清理**；
// - 跨框架契约由 WC 承担，sonner 只是 React 侧可选实现（T1）——
//   本 crate 只产出一条框架无关的事件，渲染由前端决定。
//
// 本 crate 不依赖 `tauri`：分发通道是抽象的 `DispatchSink` trait，
// 系统通知由 Tauri 适配层实现，单元测试用 Mock。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod error;

pub use error::{NotifyError, NotifyResult};

/// 通知类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    Info,
    Warning,
    Error,
}

impl NotifyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyKind::Info => "info",
            NotifyKind::Warning => "warning",
            NotifyKind::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "info" => Some(NotifyKind::Info),
            "warning" => Some(NotifyKind::Warning),
            "error" => Some(NotifyKind::Error),
            _ => None,
        }
    }
}

impl std::fmt::Display for NotifyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单条通知。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyEntry {
    /// 稳定 id（调用方生成，建议 `uuid`）。
    pub id: String,
    /// 发起插件 id。
    pub plugin_id: String,
    /// 通知类型。
    pub kind: NotifyKind,
    /// 标题。
    pub title: String,
    /// 正文。
    pub message: String,
    /// 时间戳（epoch 毫秒）。
    pub ts: u64,
    /// 是否已读。
    pub read: bool,
    /// 附带数据（如跳转链接、操作按钮）。
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl NotifyEntry {
    /// 分组键：`plugin:<id>`。
    pub fn group_key(&self) -> String {
        format!("plugin:{}", self.plugin_id)
    }
}

/// 分发结果：系统通知是否成功。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// 系统通知已送达。
    System,
    /// 系统通知失败，已降级为应用内。
    Degraded,
    /// 两者都失败（已记录日志）。
    Failed,
}

impl DispatchOutcome {
    /// 线名（供适配层 `dispatchLog` 直接输出）。
    pub fn as_str(self) -> &'static str {
        match self {
            DispatchOutcome::System => "system",
            DispatchOutcome::Degraded => "degraded",
            DispatchOutcome::Failed => "failed",
        }
    }
}

/// 系统通知分发通道（抽象）。
pub trait DispatchSink: Send + Sync {
    /// 返回 `Ok(true)` 表示系统通知成功；`Ok(false)` 表示不支持/未授权
    /// （应静默降级）；`Err` 表示致命错误（也应降级）。
    fn send(&self, entry: &NotifyEntry) -> Result<bool, String>;
}

/// 分发记录（降级矩阵的可观测证据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRecord {
    pub outcome: DispatchOutcome,
    pub entry_id: String,
    pub ts: u64,
}

/// 通知存储：环形缓冲 + 分组索引 + 未读计数。
pub struct NotifyStore {
    capacity: usize,
    /// 单插件通知历史上限；插件超限时仅裁剪该插件最旧记录。
    plugin_capacity: usize,
    /// 按 id 索引（id → 条目）。BTreeMap 保证顺序稳定、快照可复现。
    entries: BTreeMap<String, NotifyEntry>,
    /// 插入顺序（最旧在前）。用于环形裁剪。
    order: Vec<String>,
    /// 分组计数（分组键 → 条目数）。用于快速归组统计。
    groups: BTreeMap<String, usize>,
    /// 当前安装会话中因容量限制被裁剪的通知数（插件 id → 累计数）。
    evictions: BTreeMap<String, u64>,
    /// 未读计数。
    unread: usize,
    /// 分发记录（最旧在前，用于降级矩阵审计）。
    dispatch_log: Vec<DispatchRecord>,
    /// 分发日志上限。
    dispatch_log_capacity: usize,
}

impl Default for NotifyStore {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY).expect("DEFAULT_CAPACITY 恒 > 0")
    }
}

/// 默认历史条目上限。
pub const DEFAULT_CAPACITY: usize = 500;

/// 单插件通知历史上限。
pub const DEFAULT_PLUGIN_CAPACITY: usize = 64;

/// 默认分发日志上限。
pub const DEFAULT_DISPATCH_LOG_CAPACITY: usize = 200;

impl NotifyStore {
    pub fn new(capacity: usize) -> NotifyResult<Self> {
        Self::with_plugin_capacity(capacity, DEFAULT_PLUGIN_CAPACITY)
    }

    /// 设置全局与单插件通知历史上限。
    pub fn with_plugin_capacity(capacity: usize, plugin_capacity: usize) -> NotifyResult<Self> {
        if capacity == 0 {
            return Err(NotifyError::ZeroCapacity);
        }
        if plugin_capacity == 0 {
            return Err(NotifyError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            plugin_capacity,
            entries: BTreeMap::new(),
            order: Vec::new(),
            groups: BTreeMap::new(),
            evictions: BTreeMap::new(),
            unread: 0,
            dispatch_log: Vec::new(),
            dispatch_log_capacity: DEFAULT_DISPATCH_LOG_CAPACITY,
        })
    }

    /// 当前条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 容量。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 单插件历史容量。
    pub fn plugin_capacity(&self) -> usize {
        self.plugin_capacity
    }

    /// 每个插件因容量限制被裁剪的历史条数。
    pub fn eviction_counts(&self) -> BTreeMap<String, u64> {
        self.evictions.clone()
    }

    /// 容量裁剪总次数。
    pub fn eviction_total(&self) -> u64 {
        self.evictions.values().copied().fold(0, u64::saturating_add)
    }

    /// 未读条目数。
    pub fn unread_count(&self) -> usize {
        self.unread
    }

    /// 写入一条通知。单插件满时裁剪该插件最旧条目；全局满时先裁剪占用最多的
    /// 插件，再裁剪该插件最旧条目，避免单个高频插件挤掉所有邻居的历史。
    ///
    /// 循环带**收敛保护**：`evict_one` 只能移除同时存在于 `order` 与
    /// `entries` 的条目，一旦两者失同步（`order` 里的 id 在 `entries` 中已不存在，
    /// 或 `order` 已空而 `entries` 仍超限），本循环就会空转——那不是"多驱逐几条"，
    /// 而是宿主被挂死。因此每次迭代校验 `entries` 是否真的缩小，没有就中止。
    /// 宁可让通知短暂超容，也不让整个客户端失去响应。
    pub fn push(&mut self, entry: NotifyEntry) -> NotifyResult<Vec<String>> {
        // 重复 ID 必须在任何容量裁剪前拒绝；否则满环上重放一条已存在的通知
        // 会先驱逐真实历史，再由 `insert_no_evict` 报重复，失败操作却产生副作用。
        if self.entries.contains_key(&entry.id) {
            return Err(NotifyError::DuplicateId(entry.id));
        }
        let mut evicted = Vec::new();
        let plugin_id = entry.plugin_id.clone();
        while self.groups.get(&entry.group_key()).copied().unwrap_or_default()
            >= self.plugin_capacity
        {
            let before = self.entries.len();
            evicted.extend(self.evict_oldest_for(&plugin_id));
            if self.entries.len() == before {
                break;
            }
        }
        while self.entries.len() >= self.capacity {
            let before = self.entries.len();
            evicted.extend(self.evict_one());
            if self.entries.len() == before {
                break;
            }
        }
        self.insert_no_evict(entry)?;
        Ok(evicted)
    }

    /// 写入且不裁剪（仅当未满时合法）。
    fn insert_no_evict(&mut self, entry: NotifyEntry) -> NotifyResult<Vec<String>> {
        if self.entries.contains_key(&entry.id) {
            return Err(NotifyError::DuplicateId(entry.id.clone()));
        }
        let group = entry.group_key();
        if !entry.read {
            self.unread += 1;
        }
        let slot = self.groups.entry(group).or_insert(0);
        *slot = slot.checked_add(1).ok_or_else(|| NotifyError::Store("分组计数溢出".into()))?;
        self.order.push(entry.id.clone());
        self.entries.insert(entry.id.clone(), entry);
        Ok(Vec::new())
    }

    /// 丢最旧一条，返回被丢的 id 列表。
    fn evict_one(&mut self) -> Vec<String> {
        // 分组选出当前占用最多的插件，平手时按 BTreeMap 的稳定字典序选择；
        // 再沿全局插入顺序移除该插件最旧的一条。每次都从真实索引重算，
        // 因此损坏/失同步时仍由外层 entries 长度收敛保护兜底。
        let group = self
            .groups
            .iter()
            .max_by(|(left_key, left_count), (right_key, right_count)| {
                left_count.cmp(right_count).then_with(|| right_key.cmp(left_key))
            })
            .map(|(key, _)| key.clone());
        let Some(group) = group else { return Vec::new() };
        let plugin_id = group.strip_prefix("plugin:").unwrap_or(&group);
        let oldest = self
            .order
            .iter()
            .find(|id| self.entries.get(*id).is_some_and(|entry| entry.plugin_id == plugin_id))
            .cloned();
        oldest.map_or_else(Vec::new, |id| self.remove_entry_from_ring(&id))
    }

    fn evict_oldest_for(&mut self, plugin_id: &str) -> Vec<String> {
        let oldest = self
            .order
            .iter()
            .find(|id| self.entries.get(*id).is_some_and(|entry| entry.plugin_id == plugin_id));
        match oldest {
            Some(id) => {
                let id = id.clone();
                self.remove_entry_from_ring(&id)
            }
            None => Vec::new(),
        }
    }

    fn remove_entry_from_ring(&mut self, id: &str) -> Vec<String> {
        if let Some(index) = self.order.iter().position(|ordered_id| ordered_id == id) {
            self.order.remove(index);
        }
        let Some(entry) = self.entries.remove(id) else {
            return Vec::new();
        };
        let evictions = self.evictions.entry(entry.plugin_id.clone()).or_default();
        *evictions = evictions.saturating_add(1);
        if !entry.read {
            self.unread = self.unread.saturating_sub(1);
        }
        let group = entry.group_key();
        if let Some(count) = self.groups.get_mut(&group) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.groups.remove(&group);
            }
        }
        vec![id.to_string()]
    }

    /// 批量裁剪到目标容量（供外部缩容使用）。
    pub fn trim_to(&mut self, capacity: usize) -> NotifyResult<Vec<String>> {
        if capacity == 0 {
            return Err(NotifyError::ZeroCapacity);
        }
        self.capacity = capacity;
        let mut evicted = Vec::new();
        // 同 `push` 的收敛保护：`order` 与 `entries` 失同步时空转会挂死宿主。
        while self.entries.len() > self.capacity {
            let before = self.entries.len();
            evicted.extend(self.evict_one());
            if self.entries.len() == before {
                break;
            }
        }
        Ok(evicted)
    }

    /// 取单条。
    pub fn get(&self, id: &str) -> Option<&NotifyEntry> {
        self.entries.get(id)
    }

    /// 标记已读。返回是否真的发生了变化。
    pub fn mark_read(&mut self, id: &str) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            return false;
        };
        if entry.read {
            return false;
        }
        entry.read = true;
        self.unread = self.unread.saturating_sub(1);
        true
    }

    /// 全部标记已读。返回被标记的条目数。
    pub fn mark_all_read(&mut self) -> usize {
        let n = self.unread;
        for entry in self.entries.values_mut() {
            entry.read = true;
        }
        self.unread = 0;
        n
    }

    /// 按分组取条目。
    pub fn by_group(&self, group_key: &str) -> Vec<&NotifyEntry> {
        self.entries.values().filter(|e| e.group_key() == group_key).collect()
    }

    /// 分组统计：分组键 → 条目数。
    pub fn group_counts(&self) -> BTreeMap<String, usize> {
        self.groups.clone()
    }

    /// **卸载清理**：删除某插件的所有通知。返回被删的条目数。
    pub fn cleanup_plugin(&mut self, plugin_id: &str) -> usize {
        self.evictions.remove(plugin_id);
        let group = format!("plugin:{plugin_id}");
        let ids: Vec<String> = self
            .entries
            .values()
            .filter(|e| e.group_key() == group)
            .map(|e| e.id.clone())
            .collect();
        let mut removed = 0;
        for id in &ids {
            if let Some(entry) = self.entries.remove(id) {
                if !entry.read {
                    self.unread = self.unread.saturating_sub(1);
                }
                removed += 1;
            }
            self.order.retain(|x| x != id);
        }
        if let Some(count) = self.groups.get_mut(&group) {
            *count = count.saturating_sub(removed);
            if *count == 0 {
                self.groups.remove(&group);
            }
        }
        removed
    }

    /// 清空全部。
    pub fn clear(&mut self) -> usize {
        let n = self.entries.len();
        self.entries.clear();
        self.order.clear();
        self.groups.clear();
        self.evictions.clear();
        self.unread = 0;
        n
    }

    /// 最近 N 条（按时间倒序）。
    pub fn recent(&self, n: usize) -> Vec<&NotifyEntry> {
        self.order.iter().rev().take(n).filter_map(|id| self.entries.get(id)).collect()
    }

    /// 记录一次分发（供降级矩阵审计）。
    pub fn log_dispatch(&mut self, record: DispatchRecord) {
        if self.dispatch_log.len() >= self.dispatch_log_capacity {
            self.dispatch_log.remove(0);
        }
        self.dispatch_log.push(record);
    }

    /// 分发日志。
    pub fn dispatch_log(&self) -> &[DispatchRecord] {
        &self.dispatch_log
    }

    /// 降级率：降级次数 / 总分发次数。
    pub fn degradation_ratio(&self) -> f64 {
        let total = self.dispatch_log.len();
        if total == 0 {
            return 0.0;
        }
        let degraded =
            self.dispatch_log.iter().filter(|r| r.outcome == DispatchOutcome::Degraded).count();
        degraded as f64 / total as f64
    }
}

/// 分发入口：尝试系统通知，失败则静默降级为应用内。
///
/// **静默降级**（计划 §4.15 关键约束）：系统通知未授权/不支持时
/// 不报错、不重试、不弹窗打扰用户——只记录一条分发日志。
///
/// **顺序不可反**（R7 关键不变量）：先入环形缓冲，再 `sink.send`，最后记日志。
/// 反过来的话，`send` 一旦失败（或 panic）通知就永远进不了历史——
/// 「系统通知失败不阻断存储」就成了空话。顺序也因此是**可观测**的：
/// `send` 里能看见的 store 状态已经是「写完之后」的状态。
///
/// 缓冲与分发**互相独立失败**：
/// - `send` 返回 `Ok(false)`（不支持/未授权）或 `Err`（致命错误）→ `Degraded`，
///   通知仍在缓冲里，本函数**不返回错误**；
/// - 入缓冲失败（重复 id）不阻断分发，也不改判结果。
pub fn dispatch(
    store: &mut NotifyStore,
    sink: &dyn DispatchSink,
    entry: NotifyEntry,
) -> DispatchOutcome {
    // 1) 先入缓冲。`push` 的错误（重复 id）不阻断分发——缓冲是尽力而为的
    //    历史，系统通知是即时通道，二者不该互相拖垮。
    let _ = store.push(entry.clone());
    // 2) 再尝试系统通知。
    let outcome = match sink.send(&entry) {
        Ok(true) => DispatchOutcome::System,
        // 静默降级：不支持/未授权/致命错误都走这条。
        Ok(false) | Err(_) => DispatchOutcome::Degraded,
    };
    // 3) 最后落一条分发记录（是「尝试」的日志，不是通知本身）。
    store.log_dispatch(DispatchRecord { outcome, entry_id: entry.id.clone(), ts: entry.ts });
    outcome
}

/// 校验分组键合法性。
pub fn validate_group_key(key: &str) -> NotifyResult<()> {
    if key.is_empty() {
        return Err(NotifyError::InvalidGroup(key.to_string()));
    }
    if key.starts_with('$') || key.contains("..") {
        return Err(NotifyError::InvalidGroup(key.to_string()));
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, plugin: &str, kind: NotifyKind) -> NotifyEntry {
        NotifyEntry {
            id: id.into(),
            plugin_id: plugin.into(),
            kind,
            title: "标题".into(),
            message: "正文".into(),
            ts: 1_000_000,
            read: false,
            data: Value::Null,
        }
    }

    fn entry_with_ts(id: &str, plugin: &str, ts: u64) -> NotifyEntry {
        let mut e = entry(id, plugin, NotifyKind::Info);
        e.ts = ts;
        e
    }

    // ── 基础写入 / 读取 ──────────────────────────────────────────────

    #[test]
    fn push_and_get() {
        let mut s = NotifyStore::new(10).unwrap();
        let e = entry("n1", "p.audio", NotifyKind::Info);
        let evicted = s.push(e).unwrap();
        assert!(evicted.is_empty());
        assert_eq!(s.len(), 1);
        assert!(s.get("n1").is_some());
        assert!(s.get("n2").is_none());
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.a", NotifyKind::Info)).unwrap();
        let e = s.push(entry("n1", "p.b", NotifyKind::Info)).unwrap_err();
        assert!(matches!(e, NotifyError::DuplicateId(ref id) if id == "n1"));
    }

    #[test]
    fn duplicate_id_at_capacity_does_not_evict_existing_history() {
        let mut store = NotifyStore::with_plugin_capacity(2, 2).unwrap();
        store.push(entry("a-1", "p.a", NotifyKind::Info)).unwrap();
        store.push(entry("b-1", "p.b", NotifyKind::Info)).unwrap();

        let error = store.push(entry("a-1", "p.a", NotifyKind::Error)).unwrap_err();
        assert!(matches!(error, NotifyError::DuplicateId(_)));
        assert!(store.get("a-1").is_some());
        assert!(store.get("b-1").is_some());
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn unread_count_tracks_pushes() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.a", NotifyKind::Info)).unwrap();
        let mut read_e = entry("n2", "p.a", NotifyKind::Info);
        read_e.read = true;
        s.push(read_e).unwrap();
        assert_eq!(s.unread_count(), 1, "已读条目不计入未读");
    }

    // ── 环形裁剪 ────────────────────────────────────────────────────

    #[test]
    fn ring_eviction_drops_oldest() {
        let mut s = NotifyStore::new(3).unwrap();
        s.push(entry_with_ts("n1", "p.a", 1)).unwrap();
        s.push(entry_with_ts("n2", "p.a", 2)).unwrap();
        s.push(entry_with_ts("n3", "p.a", 3)).unwrap();
        assert_eq!(s.len(), 3);
        // 第 4 条触发裁剪。
        let evicted = s.push(entry_with_ts("n4", "p.a", 4)).unwrap();
        assert_eq!(evicted, vec!["n1".to_string()]);
        assert!(s.get("n1").is_none());
        assert!(s.get("n4").is_some());
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn ring_eviction_respects_unread_count() {
        let mut s = NotifyStore::new(2).unwrap();
        s.push(entry_with_ts("n1", "p.a", 1)).unwrap();
        s.push(entry_with_ts("n2", "p.a", 2)).unwrap();
        assert_eq!(s.unread_count(), 2);
        // 裁剪未读条目 → 未读计数减 1。
        let _ = s.push(entry_with_ts("n3", "p.a", 3)).unwrap();
        assert_eq!(s.unread_count(), 2, "裁剪未读后仍剩 2 条未读");
    }

    #[test]
    fn ring_eviction_of_read_entry_does_not_affect_unread() {
        let mut s = NotifyStore::new(2).unwrap();
        s.push(entry_with_ts("n1", "p.a", 1)).unwrap();
        s.mark_read("n1");
        s.push(entry_with_ts("n2", "p.a", 2)).unwrap();
        assert_eq!(s.unread_count(), 1);
        // 写入一条已读条目触发裁剪：裁掉的是已读的 n1，未读数不变。
        let mut n3 = entry_with_ts("n3", "p.a", 3);
        n3.read = true;
        let _ = s.push(n3).unwrap();
        assert_eq!(s.unread_count(), 1, "裁剪已读条目不影响未读");
    }

    #[test]
    fn trim_to_shrinks() {
        let mut s = NotifyStore::new(10).unwrap();
        for i in 1..=8 {
            s.push(entry_with_ts(&format!("n{i}"), "p.a", i)).unwrap();
        }
        let evicted = s.trim_to(3).unwrap();
        assert_eq!(evicted.len(), 5);
        assert_eq!(s.len(), 3);
        assert_eq!(s.capacity(), 3);
        assert!(s.get("n6").is_some(), "应保留最新的 3 条");
        assert!(s.get("n8").is_some());
        assert!(s.get("n5").is_none(), "最旧的应被裁掉");
    }

    /// 收敛保护：`order` 与 `entries` 失同步时 `evict_one` 无法再减少条目，
    /// 没有保护的话 `push` 会空转把宿主挂死——那不是"多裁几条"，是活锁。
    /// 本用例在修复前永不返回，因此是判别性的（不是"改完也能过"的测试）。
    #[test]
    fn push_terminates_when_order_and_entries_desync() {
        let mut s = NotifyStore::new(2).unwrap();
        s.push(entry("n1", "p.a", NotifyKind::Info)).unwrap();
        s.push(entry("n2", "p.a", NotifyKind::Info)).unwrap();
        assert_eq!(s.len(), 2);

        // 人为失同步：只清顺序表，保留条目表。
        s.order.clear();

        let evicted = s.push(entry("n3", "p.a", NotifyKind::Info)).unwrap();
        assert!(evicted.is_empty(), "失同步下不存在可驱逐的条目");
        assert_eq!(s.len(), 3, "宁可短暂超容也不挂死宿主");
    }

    #[test]
    fn trim_to_terminates_when_order_and_entries_desync() {
        let mut s = NotifyStore::new(5).unwrap();
        for i in 1..=5 {
            s.push(entry(&format!("n{i}"), "p.a", NotifyKind::Info)).unwrap();
        }
        s.order.clear();
        let evicted = s.trim_to(2).unwrap();
        assert!(evicted.is_empty(), "失同步下无法裁减任何条目");
        assert_eq!(s.capacity(), 2, "容量仍按请求收缩");
    }

    #[test]
    fn trim_to_zero_is_rejected() {
        let mut s = NotifyStore::new(10).unwrap();
        assert!(s.trim_to(0).is_err());
    }

    #[test]
    fn new_with_zero_capacity_is_rejected() {
        assert!(matches!(NotifyStore::new(0), Err(NotifyError::ZeroCapacity)));
    }

    #[test]
    fn capacity_boundary_is_exact() {
        // 容量 5：第 5 条不裁剪，第 6 条裁剪。
        let mut s = NotifyStore::new(5).unwrap();
        for i in 1..=5 {
            let e = s.push(entry_with_ts(&format!("n{i}"), "p.a", i)).unwrap();
            assert!(e.is_empty(), "第 {i} 条不应裁剪");
        }
        assert_eq!(s.len(), 5);
        let e = s.push(entry_with_ts("n6", "p.a", 6)).unwrap();
        assert_eq!(e, vec!["n1".to_string()]);
        assert_eq!(s.len(), 5);
    }

    // ── 已读 / 未读 ────────────────────────────────────────────────

    #[test]
    fn mark_read_is_idempotent() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.a", NotifyKind::Info)).unwrap();
        assert!(s.mark_read("n1"));
        assert!(!s.mark_read("n1"), "二次标记不产生变化");
        assert_eq!(s.unread_count(), 0);
    }

    #[test]
    fn mark_read_on_missing_is_noop() {
        let mut s = NotifyStore::new(10).unwrap();
        assert!(!s.mark_read("nope"));
    }

    #[test]
    fn mark_all_read_returns_count() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.a", NotifyKind::Info)).unwrap();
        s.push(entry("n2", "p.a", NotifyKind::Info)).unwrap();
        let mut read = entry("n3", "p.a", NotifyKind::Info);
        read.read = true;
        s.push(read).unwrap();
        let n = s.mark_all_read();
        assert_eq!(n, 2);
        assert_eq!(s.unread_count(), 0);
    }

    // ── 分组 ────────────────────────────────────────────────────────

    #[test]
    fn group_key_is_plugin_prefixed() {
        let e = entry("n1", "p.audio", NotifyKind::Info);
        assert_eq!(e.group_key(), "plugin:p.audio");
    }

    #[test]
    fn by_group_returns_only_that_plugins_entries() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.audio", NotifyKind::Info)).unwrap();
        s.push(entry("n2", "p.video", NotifyKind::Info)).unwrap();
        s.push(entry("n3", "p.audio", NotifyKind::Warning)).unwrap();
        let audio = s.by_group("plugin:p.audio");
        assert_eq!(audio.len(), 2);
        assert!(audio.iter().all(|e| e.plugin_id == "p.audio"));
    }

    #[test]
    fn group_counts_track_pushes_and_evictions() {
        let mut s = NotifyStore::new(2).unwrap();
        s.push(entry_with_ts("n1", "p.a", 1)).unwrap();
        s.push(entry_with_ts("n2", "p.a", 2)).unwrap();
        assert_eq!(s.group_counts()["plugin:p.a"], 2);
        let _ = s.push(entry_with_ts("n3", "p.b", 3)).unwrap();
        assert_eq!(s.group_counts()["plugin:p.a"], 1, "裁剪后 p.a 只剩 1 条");
        assert_eq!(s.group_counts()["plugin:p.b"], 1);
    }

    #[test]
    fn per_plugin_ring_does_not_evict_neighbor_history() {
        let mut store = NotifyStore::with_plugin_capacity(128, 64).unwrap();
        for i in 0..65 {
            store.push(entry(&format!("a-{i}"), "p.a", NotifyKind::Info)).unwrap();
        }
        assert!(store.get("a-0").is_none(), "p.a 只裁剪自己的最旧历史");
        assert!(store.get("a-64").is_some());
        store.push(entry("b-0", "p.b", NotifyKind::Warning)).unwrap();

        assert_eq!(store.group_counts()["plugin:p.a"], 64);
        assert_eq!(store.group_counts()["plugin:p.b"], 1);
        assert!(store.get("b-0").is_some(), "A 的配额裁剪不得影响 B");
        assert_eq!(store.capacity(), 128);
        assert_eq!(store.plugin_capacity(), 64);
    }

    #[test]
    fn global_overflow_evicts_oldest_entry_from_noisiest_plugin() {
        let mut store = NotifyStore::with_plugin_capacity(6, 6).unwrap();
        for i in 0..4 {
            store.push(entry(&format!("a-{i}"), "p.a", NotifyKind::Info)).unwrap();
        }
        store.push(entry("b-0", "p.b", NotifyKind::Warning)).unwrap();
        store.push(entry("c-0", "p.c", NotifyKind::Info)).unwrap();

        let evicted = store.push(entry("d-0", "p.d", NotifyKind::Info)).unwrap();
        assert_eq!(evicted, vec!["a-0"]);
        assert!(store.get("b-0").is_some(), "较安静插件的历史必须保留");
        assert_eq!(store.group_counts()["plugin:p.a"], 3);
        assert_eq!(store.len(), 6);
        assert_eq!(store.eviction_counts()["p.a"], 1);
        assert_eq!(store.eviction_total(), 1);
    }

    #[test]
    fn validate_group_key_rejects_empty_and_dollar() {
        assert!(validate_group_key("").is_err());
        assert!(validate_group_key("$bad").is_err());
        assert!(validate_group_key("a..b").is_err());
        assert!(validate_group_key("plugin:p.a").is_ok());
    }

    // ── 卸载清理 ────────────────────────────────────────────────────

    #[test]
    fn cleanup_plugin_removes_only_that_plugin() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.audio", NotifyKind::Info)).unwrap();
        s.push(entry("n2", "p.video", NotifyKind::Info)).unwrap();
        s.push(entry("n3", "p.audio", NotifyKind::Warning)).unwrap();
        let removed = s.cleanup_plugin("p.audio");
        assert_eq!(removed, 2);
        assert_eq!(s.len(), 1);
        assert!(s.get("n2").is_some());
        assert!(s.get("n1").is_none());
        assert!(s.get("n3").is_none());
    }

    #[test]
    fn cleanup_plugin_updates_unread_count() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.audio", NotifyKind::Info)).unwrap();
        s.push(entry("n2", "p.audio", NotifyKind::Info)).unwrap();
        s.push(entry("n3", "p.video", NotifyKind::Info)).unwrap();
        s.mark_read("n3");
        assert_eq!(s.unread_count(), 2);
        s.cleanup_plugin("p.audio");
        assert_eq!(s.unread_count(), 0, "清理的未读条目应从不读数中扣除");
    }

    #[test]
    fn cleanup_plugin_removes_group_from_counts() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.audio", NotifyKind::Info)).unwrap();
        assert!(s.group_counts().contains_key("plugin:p.audio"));
        s.cleanup_plugin("p.audio");
        assert!(!s.group_counts().contains_key("plugin:p.audio"), "清空后分组键应消失");
    }

    #[test]
    fn cleanup_plugin_on_missing_is_noop() {
        let mut s = NotifyStore::new(10).unwrap();
        assert_eq!(s.cleanup_plugin("p.nope"), 0);
    }

    #[test]
    fn cleanup_then_push_same_plugin_works() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.audio", NotifyKind::Info)).unwrap();
        s.cleanup_plugin("p.audio");
        s.push(entry("n2", "p.audio", NotifyKind::Info)).unwrap();
        assert_eq!(s.len(), 1);
        assert!(s.get("n2").is_some());
    }

    // ── 分发与降级 ────────────────────────────────────────────────

    struct MockSink {
        /// 模拟系统通知能力：true = 支持且已授权。
        supported: bool,
        /// 模拟发送失败。
        fail: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl MockSink {
        fn new(supported: bool, fail: bool) -> Self {
            Self { supported, fail, calls: std::sync::Mutex::new(Vec::new()) }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl DispatchSink for MockSink {
        fn send(&self, entry: &NotifyEntry) -> Result<bool, String> {
            self.calls.lock().unwrap().push(entry.id.clone());
            if self.fail {
                return Err("system error".into());
            }
            Ok(self.supported)
        }
    }

    #[test]
    fn dispatch_system_success() {
        let mut s = NotifyStore::new(10).unwrap();
        let sink = MockSink::new(true, false);
        let outcome = dispatch(&mut s, &sink, entry("n1", "p.a", NotifyKind::Info));
        assert_eq!(outcome, DispatchOutcome::System);
        assert_eq!(s.dispatch_log().len(), 1);
        assert_eq!(s.dispatch_log()[0].outcome, DispatchOutcome::System);
    }

    #[test]
    fn dispatch_silently_degrades_when_unsupported() {
        let mut s = NotifyStore::new(10).unwrap();
        let sink = MockSink::new(false, false);
        let outcome = dispatch(&mut s, &sink, entry("n1", "p.a", NotifyKind::Info));
        assert_eq!(outcome, DispatchOutcome::Degraded, "未授权应静默降级");
        // 不报错、不重试。
        assert_eq!(sink.call_count(), 1, "降级不重试");
        assert_eq!(s.dispatch_log()[0].outcome, DispatchOutcome::Degraded);
    }

    #[test]
    fn dispatch_silently_degrades_on_error() {
        let mut s = NotifyStore::new(10).unwrap();
        let sink = MockSink::new(true, true);
        let outcome = dispatch(&mut s, &sink, entry("n1", "p.a", NotifyKind::Error));
        assert_eq!(outcome, DispatchOutcome::Degraded, "系统错误也静默降级");
    }

    #[test]
    fn dispatch_stores_the_entry() {
        let mut s = NotifyStore::new(10).unwrap();
        let sink = MockSink::new(true, false);
        let e = entry("n1", "p.a", NotifyKind::Info);
        dispatch(&mut s, &sink, e);
        assert!(s.get("n1").is_some(), "分发后通知应入库");
    }

    #[test]
    fn degradation_ratio_computes_correctly() {
        let mut s = NotifyStore::new(10).unwrap();
        let ok_sink = MockSink::new(true, false);
        let bad_sink = MockSink::new(false, false);
        dispatch(&mut s, &ok_sink, entry("n1", "p.a", NotifyKind::Info));
        dispatch(&mut s, &ok_sink, entry("n2", "p.a", NotifyKind::Info));
        dispatch(&mut s, &bad_sink, entry("n3", "p.a", NotifyKind::Info));
        dispatch(&mut s, &bad_sink, entry("n4", "p.a", NotifyKind::Info));
        assert!((s.degradation_ratio() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn degradation_ratio_empty_store_is_zero() {
        let s = NotifyStore::new(10).unwrap();
        assert_eq!(s.degradation_ratio(), 0.0);
    }

    #[test]
    fn dispatch_log_bounded() {
        let mut s = NotifyStore::new(1000).unwrap();
        let sink = MockSink::new(false, false);
        for i in 0..300 {
            dispatch(&mut s, &sink, entry_with_ts(&format!("n{i}"), "p.a", i as u64));
        }
        assert!(s.dispatch_log().len() <= DEFAULT_DISPATCH_LOG_CAPACITY);
    }

    /// 顺序探针（R7 不变量「先入环形缓冲再 dispatch」）。
    ///
    /// sink 在 `send` 里 panic —— 通知**仍然**必须已经在缓冲里。这条断言只有
    /// 「先 push 再 send」才成立；把顺序反过来（先 send 再 push）本用例立刻变红。
    /// 用 panic 而不是 `Err` 是因为 `Err` 在两种顺序下的可观测结果相同，
    /// 只有「send 根本没返回」才能把顺序钉死。
    #[test]
    fn dispatch_buffers_the_entry_before_calling_the_sink() {
        struct PanickingSink;
        impl DispatchSink for PanickingSink {
            fn send(&self, _entry: &NotifyEntry) -> Result<bool, String> {
                panic!("sink 在 send 里炸了");
            }
        }

        let mut s = NotifyStore::new(10).unwrap();
        let sink = PanickingSink;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatch(&mut s, &sink, entry("n1", "p.a", NotifyKind::Info))
        }));
        assert!(result.is_err(), "sink 的 panic 必须冒泡，不被 dispatch 吞掉");
        assert!(s.get("n1").is_some(), "先入缓冲：sink 还没返回时通知就必须已经入库");
        assert_eq!(s.len(), 1);
    }

    /// 降级路径的完整断言：通知仍在缓冲里、无返回值可报错、dispatchLog 有记录。
    #[test]
    fn degraded_dispatch_still_keeps_the_notification() {
        let mut s = NotifyStore::new(10).unwrap();
        // 致命错误（`Err`）与「不支持」（`Ok(false)`）走同一条降级路径。
        let hard_fail = MockSink::new(true, true);
        let outcome = dispatch(&mut s, &hard_fail, entry("n1", "p.a", NotifyKind::Error));
        assert_eq!(outcome, DispatchOutcome::Degraded);
        assert!(s.get("n1").is_some(), "系统通知失败后通知仍须入库");
        assert_eq!(s.len(), 1);

        let unsupported = MockSink::new(false, false);
        let outcome = dispatch(&mut s, &unsupported, entry("n2", "p.a", NotifyKind::Info));
        assert_eq!(outcome, DispatchOutcome::Degraded);
        assert!(s.get("n2").is_some());
        assert_eq!(s.len(), 2, "两条降级通知都在缓冲里");

        // dispatchLog 是**尝试**的日志：每次 host_notify 一条，与通知是否入库无关。
        assert_eq!(s.dispatch_log().len(), 2);
        assert_eq!(s.dispatch_log()[0].entry_id, "n1");
        assert_eq!(s.dispatch_log()[0].outcome.as_str(), "degraded");
        assert_eq!(s.dispatch_log()[0].ts, 1_000_000, "ts 取自通知自身");
        assert_eq!(s.dispatch_log()[1].entry_id, "n2");
        assert_eq!(s.degradation_ratio(), 1.0);
    }

    #[test]
    fn dispatch_outcome_wire_names() {
        assert_eq!(DispatchOutcome::System.as_str(), "system");
        assert_eq!(DispatchOutcome::Degraded.as_str(), "degraded");
        assert_eq!(DispatchOutcome::Failed.as_str(), "failed");
    }

    /// 分发日志是**独立**的环形：通知被裁掉后，它的分发记录仍在。
    #[test]
    fn dispatch_log_outlives_evicted_entries() {
        let mut s = NotifyStore::new(2).unwrap();
        let sink = MockSink::new(false, false);
        dispatch(&mut s, &sink, entry_with_ts("n1", "p.a", 1));
        dispatch(&mut s, &sink, entry_with_ts("n2", "p.a", 2));
        dispatch(&mut s, &sink, entry_with_ts("n3", "p.a", 3)); // 裁掉 n1
        assert!(s.get("n1").is_none(), "n1 已被环形裁剪");
        assert_eq!(s.dispatch_log().len(), 3, "分发日志不被条目裁剪影响");
        assert!(s.dispatch_log().iter().any(|r| r.entry_id == "n1"));
    }

    // ── 清空 ────────────────────────────────────────────────────────

    #[test]
    fn clear_removes_everything() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry("n1", "p.a", NotifyKind::Info)).unwrap();
        s.push(entry("n2", "p.b", NotifyKind::Info)).unwrap();
        let n = s.clear();
        assert_eq!(n, 2);
        assert!(s.is_empty());
        assert_eq!(s.unread_count(), 0);
        assert!(s.group_counts().is_empty());
    }

    // ── 最近 N 条 ──────────────────────────────────────────────────

    #[test]
    fn recent_returns_newest_first() {
        let mut s = NotifyStore::new(10).unwrap();
        s.push(entry_with_ts("n1", "p.a", 1)).unwrap();
        s.push(entry_with_ts("n2", "p.a", 2)).unwrap();
        s.push(entry_with_ts("n3", "p.a", 3)).unwrap();
        let r = s.recent(2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id, "n3");
        assert_eq!(r[1].id, "n2");
    }

    #[test]
    fn recent_handles_evicted_ids() {
        let mut s = NotifyStore::new(2).unwrap();
        s.push(entry_with_ts("n1", "p.a", 1)).unwrap();
        s.push(entry_with_ts("n2", "p.a", 2)).unwrap();
        s.push(entry_with_ts("n3", "p.a", 3)).unwrap(); // 裁剪 n1
        let r = s.recent(10);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|e| e.id != "n1"));
    }

    // ── 快照可复现 ──────────────────────────────────────────────────

    #[test]
    fn snapshot_is_deterministic() {
        // BTreeMap 排序 → 快照不依赖插入顺序。
        let build = |ids: &[&str]| -> NotifyStore {
            let mut s = NotifyStore::new(10).unwrap();
            for id in ids {
                // 用固定 ts，避免插入顺序影响时间戳。
                s.push(entry_with_ts(id, "p.a", 1000)).unwrap();
            }
            s
        };
        let a = build(&["n1", "n2", "n3"]);
        let b = build(&["n3", "n1", "n2"]);
        // 分组计数与条目集合相同（顺序不同但集合相同）。
        assert_eq!(a.group_counts(), b.group_counts());
        assert_eq!(a.len(), b.len());
        // by_group 按 id 排序（BTreeMap 序），故同集合必同序。
        let ca = serde_json::to_value(a.by_group("plugin:p.a")).unwrap();
        let cb = serde_json::to_value(b.by_group("plugin:p.a")).unwrap();
        assert_eq!(ca, cb);
    }

    // ── NotifyKind 序列化 ──────────────────────────────────────────

    #[test]
    fn notify_kind_serde_roundtrip() {
        for k in [NotifyKind::Info, NotifyKind::Warning, NotifyKind::Error] {
            let v = serde_json::to_value(k).unwrap();
            assert_eq!(v, serde_json::json!(k.as_str()));
            assert_eq!(serde_json::from_value::<NotifyKind>(v).unwrap(), k);
        }
    }

    #[test]
    fn notify_kind_rejects_unknown() {
        assert!(serde_json::from_value::<NotifyKind>(serde_json::json!("danger")).is_err());
        assert!(NotifyKind::parse("danger").is_none());
    }

    // ── 端到端 ────────────────────────────────────────────────────

    /// 计划 §4.15 测试项：容量边界 + 降级矩阵 + 卸载后历史清理。
    #[test]
    fn end_to_end_notify_flow() {
        let mut s = NotifyStore::new(5).unwrap();

        // 1. 写入超过容量 → 环形裁剪。
        for i in 1..=6 {
            dispatch(
                &mut s,
                &MockSink::new(i % 2 == 0, false),
                entry_with_ts(&format!("n{i}"), "p.audio", i),
            );
        }
        assert_eq!(s.len(), 5, "容量 5，写入 6 条后剩 5 条");
        assert!(s.get("n1").is_none(), "最旧的 n1 被裁剪");
        assert!(s.get("n6").is_some());

        // 2. 降级矩阵：偶数条走系统，奇数条降级。
        let degraded =
            s.dispatch_log().iter().filter(|r| r.outcome == DispatchOutcome::Degraded).count();
        assert!(degraded >= 1, "应至少有降级记录");

        // 3. 卸载清理：删除 p.audio 的所有通知。
        let removed = s.cleanup_plugin("p.audio");
        assert_eq!(removed, 5);
        assert!(s.is_empty());
        assert!(s.group_counts().is_empty());
        assert_eq!(s.unread_count(), 0);

        // 4. 清理后同插件可继续写入。
        dispatch(&mut s, &MockSink::new(true, false), entry("n7", "p.audio", NotifyKind::Info));
        assert_eq!(s.len(), 1);
    }
}
