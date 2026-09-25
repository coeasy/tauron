//! 事件总线（**legacy / 非 canonical**，R2-a）。
//!
//! ⚠️ canonical 的事件总线是 `tauron_host::eventbus::EventBus`（19 个公开方法：
//! 主题审批、可靠 drain 队列、订阅组、悬挂检测）。本模块是框架协议层
//! `plugin_invoke` 仍在用的**旧一代实现**（9 个公开方法），按实测决策冻结：
//! **不得再添加功能**（wire-gate 会比对方法集指纹）。
//! 决策依据与迁移路线见 `docs/architecture/canonical-owners.md`。
//!
//! **已知边界（不在此修复，属冻结范围）**：`emit()` 没有重入闸——订阅者的
//! `deliver()` 里若反过来调 `emit()`，会因为外层持有 `&mut self` 而在 `std::sync::
//! RwLock`（`dispatch.rs` 的 `events`）上**死锁**而不是无限递归（Rust 的 `RwLock`
//! 不可重入）。修复它需要给 `emit` 加重入标志，那会改变公开方法的语义，已超出
//! "冻结"允许的范围。canonical 侧的 `tauron_host::eventbus` 不受此限制。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 事件（设计文档 §2.3）
///
/// 线上字段为 camelCase，与 TS `PluginEvent`（`sourcePlugin` / `eventName`）
/// 保持一致；否则前端读到的会是 `undefined`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Event {
    /// 发出者插件 ID
    pub source_plugin: String,
    /// 事件名（不含前缀）
    pub event_name: String,
    /// 事件负载
    pub payload: serde_json::Value,
    /// 时间戳（ISO 8601）
    pub timestamp: String,
}

impl Event {
    /// 生成事件命名空间（plugin:<id>:<event>）
    pub fn namespace(&self) -> String {
        format!("plugin:{}:{}", self.source_plugin, self.event_name)
    }
}

/// 事件订阅者
pub trait EventSubscriber: Send + Sync {
    fn deliver(&self, event: &Event);
}

/// 事件总线错误
#[derive(Debug)]
pub enum EventBusError {
    QueueOverflow,
}

/// 单个主题上的订阅者上限。
///
/// 没有它，反复调 `subscribe` 的插件可以让 `subscribers` 里那一项的 `Vec`
/// 无界增长（订阅者持有 `Box<dyn EventSubscriber>`，往往又闭包捕获了别的东西，
/// 实际是内存泄漏）。达到上限时新的订阅**被丢弃并计入 `dropped_count`**——
/// 可通过既有的 `queue_stats()` 观察到，不是静默丢失。
///
/// 与 canonical 侧 `tauron_host::eventbus::MAX_SUBSCRIPTIONS`（4096，作用于
/// **全局订阅数**）口径不同：本模块按主题计，且数值更小，因为 legacy 面不该
/// 被当成容量主力来用。
pub const MAX_SUBSCRIBERS_PER_TOPIC: usize = 256;

/// 事件总线（设计文档 §4.5）
pub struct EventBus {
    /// 每插件独立队列（背压隔离）
    queues: HashMap<String, Vec<Event>>,
    /// 订阅者
    subscribers: HashMap<String, Vec<Box<dyn EventSubscriber>>>,
    /// 最大队列长度
    max_queue_size: usize,
    /// 丢弃计数
    dropped_count: usize,
}

impl EventBus {
    /// 创建事件总线（默认最大队列 1000）
    pub fn new() -> Self {
        Self {
            queues: HashMap::new(),
            subscribers: HashMap::new(),
            max_queue_size: 1000,
            dropped_count: 0,
        }
    }

    /// 创建带自定义队列大小的事件总线
    pub fn with_queue_size(max_queue_size: usize) -> Self {
        Self {
            queues: HashMap::new(),
            subscribers: HashMap::new(),
            max_queue_size,
            dropped_count: 0,
        }
    }

    /// 发布事件
    pub fn emit(&mut self, event: Event) -> Result<(), EventBusError> {
        let key = event.namespace();

        // 背压检查
        let queue = self.queues.entry(event.source_plugin.clone()).or_default();
        if queue.len() >= self.max_queue_size {
            queue.remove(0);
            self.dropped_count += 1;
        }
        queue.push(event.clone());

        // 分发到订阅者
        if let Some(subs) = self.subscribers.get(&key) {
            for sub in subs {
                sub.deliver(&event);
            }
        }

        Ok(())
    }

    /// 订阅事件
    pub fn subscribe(
        &mut self,
        plugin_id: &str,
        event_name: &str,
        subscriber: Box<dyn EventSubscriber>,
    ) {
        let key = format!("plugin:{plugin_id}:{event_name}");
        let subs = self.subscribers.entry(key).or_default();
        if subs.len() >= MAX_SUBSCRIBERS_PER_TOPIC {
            // 超上限：丢弃新订阅并计数（不静默——`queue_stats()` 看得到）。
            self.dropped_count += 1;
            return;
        }
        subs.push(subscriber);
    }

    /// 取消订阅
    pub fn unsubscribe(&mut self, plugin_id: &str, event_name: &str) {
        let key = format!("plugin:{plugin_id}:{event_name}");
        self.subscribers.remove(&key);
    }

    /// 清理插件的所有队列和订阅
    pub fn clear_plugin(&mut self, plugin_id: &str) {
        self.queues.remove(plugin_id);
        let prefix = format!("plugin:{plugin_id}:");
        self.subscribers.retain(|k, _| !k.starts_with(&prefix));
    }

    /// 获取队列统计
    pub fn queue_stats(&self) -> (usize, usize) {
        let total: usize = self.queues.values().map(|q| q.len()).sum();
        (total, self.dropped_count)
    }

    /// 清空所有队列
    pub fn clear(&mut self) {
        self.queues.clear();
        self.dropped_count = 0;
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct MockSubscriber {
        delivered: Arc<Mutex<Vec<Event>>>,
    }

    impl EventSubscriber for MockSubscriber {
        fn deliver(&self, event: &Event) {
            self.delivered.lock().unwrap().push(event.clone());
        }
    }

    fn make_event(plugin_id: &str, event_name: &str, payload: serde_json::Value) -> Event {
        Event {
            source_plugin: plugin_id.to_string(),
            event_name: event_name.to_string(),
            payload,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_emit_delivers_to_subscriber() {
        let mut bus = EventBus::new();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sub = Box::new(MockSubscriber { delivered: delivered.clone() });

        bus.subscribe("test", "done", sub);
        bus.emit(make_event("test", "done", serde_json::json!({"result": "ok"}))).unwrap();

        let events = delivered.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_name, "done");
    }

    #[test]
    fn test_namespace_format() {
        let event = make_event("com.example.test", "format-done", serde_json::json!({}));
        assert_eq!(event.namespace(), "plugin:com.example.test:format-done");
    }

    #[test]
    fn test_multiple_subscribers() {
        let mut bus = EventBus::new();
        let d1 = Arc::new(Mutex::new(Vec::new()));
        let d2 = Arc::new(Mutex::new(Vec::new()));

        bus.subscribe("test", "event", Box::new(MockSubscriber { delivered: d1.clone() }));
        bus.subscribe("test", "event", Box::new(MockSubscriber { delivered: d2.clone() }));

        bus.emit(make_event("test", "event", serde_json::json!({}))).unwrap();

        assert_eq!(d1.lock().unwrap().len(), 1);
        assert_eq!(d2.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_unsubscribe() {
        let mut bus = EventBus::new();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let sub = Box::new(MockSubscriber { delivered: delivered.clone() });

        bus.subscribe("test", "event", sub);
        bus.unsubscribe("test", "event");
        bus.emit(make_event("test", "event", serde_json::json!({}))).unwrap();

        assert!(delivered.lock().unwrap().is_empty());
    }

    #[test]
    fn test_backpressure_drop_oldest() {
        let mut bus = EventBus::with_queue_size(3);

        // Emit 5 events
        for i in 0..5 {
            bus.emit(make_event("test", "event", serde_json::json!({"seq": i}))).unwrap();
        }

        let (queue_size, dropped) = bus.queue_stats();
        assert_eq!(queue_size, 3);
        assert_eq!(dropped, 2);
    }

    #[test]
    fn test_clear_plugin() {
        let mut bus = EventBus::new();
        let delivered = Arc::new(Mutex::new(Vec::new()));

        bus.subscribe("test", "event", Box::new(MockSubscriber { delivered: delivered.clone() }));
        bus.emit(make_event("test", "event", serde_json::json!({"seq": 0}))).unwrap();

        bus.clear_plugin("test");
        bus.emit(make_event("test", "event", serde_json::json!({"seq": 1}))).unwrap();

        let events = delivered.lock().unwrap();
        assert_eq!(events.len(), 1); // Only the first one before clear
    }

    #[test]
    fn test_queue_stats() {
        let mut bus = EventBus::new();
        bus.emit(make_event("p1", "e1", serde_json::json!({}))).unwrap();
        bus.emit(make_event("p1", "e2", serde_json::json!({}))).unwrap();
        bus.emit(make_event("p2", "e1", serde_json::json!({}))).unwrap();

        let (queue_size, dropped) = bus.queue_stats();
        assert_eq!(queue_size, 3);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn test_clear() {
        let mut bus = EventBus::new();
        bus.emit(make_event("test", "event", serde_json::json!({}))).unwrap();
        bus.clear();

        let (queue_size, dropped) = bus.queue_stats();
        assert_eq!(queue_size, 0);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn test_event_serde() {
        let event = make_event("test", "done", serde_json::json!({"result": 42}));
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.source_plugin, "test");
        assert_eq!(deserialized.event_name, "done");
    }

    /// 线上字段必须是 camelCase，否则前端 TS `PluginEvent` 读到 undefined。
    #[test]
    fn test_event_wire_format_is_camel_case() {
        let event = make_event("com.example.p", "changed", serde_json::json!({"n": 1}));
        let value: serde_json::Value = serde_json::to_value(&event).unwrap();

        assert_eq!(value["sourcePlugin"], "com.example.p");
        assert_eq!(value["eventName"], "changed");
        assert!(value.get("source_plugin").is_none());
        assert!(value.get("event_name").is_none());
        // 允许反序列化 TS 侧产出的 camelCase 事件
        let back: Event = serde_json::from_value(value).unwrap();
        assert_eq!(back.source_plugin, "com.example.p");
    }

    /// snake_case 载荷必须被拒绝（deny_unknown_fields 生效）。
    #[test]
    fn test_event_rejects_snake_case_payload() {
        let snake = serde_json::json!({
            "source_plugin": "p",
            "event_name": "e",
            "payload": null,
            "timestamp": "2026-01-01T00:00:00Z",
        });
        assert!(serde_json::from_value::<Event>(snake).is_err());
    }
}

