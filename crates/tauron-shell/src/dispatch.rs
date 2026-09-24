//! 宿主命令核心（设计文档 §2.1/§2.3）
//!
//! ⚠️ 其中的 `HostState` 与 `PluginRegistry` 是**legacy / 非 canonical**（R2-a）：
//! canonical 的宿主态是 `tauron_host` 的 `SubstrateState`（底座）+
//! `PluginRuntimeState`（插件运行时，R1b 拆分），canonical 的注册表/总线见
//! `docs/architecture/canonical-owners.md`。本模块保留其**协议门面**角色
//! （`plugin_invoke` 信封 ↔ 分发的翻译），引擎按实测决策冻结，不得再添加功能。
//!
//! 前端只调用三条固定命令：`plugin_invoke` / `plugin_cancel` / `plugin_emit`。
//! 本模块承载它们的**平台无关逻辑**：
//!
//! ```text
//! plugin_invoke → 信封校验 → 注册表状态检查 → PluginDispatcher 分发
//! plugin_cancel → 幂等取消（未命中返回 false，不报错）
//! plugin_emit   → topic 解析 → 事件总线发布（背压隔离）
//! ```
//!
//! Tauri 的 `#[tauri::command]` 包装层（`commands` 模块，`tauri` feature）只做
//! 参数转交与 `State` 提取，因此**核心链路可以在不引入 `tauri` 依赖的前提下
//! 被完整测试**（设计文档 §1.1、§8）。

use std::sync::Arc;

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use serde_json::Value;

use crate::envelope::{PluginCancelRequest, PluginErrorBody, PluginInvokeRequest, PluginInvokeResponse};
use crate::error::PluginErrorCode;
use crate::eventbus::{Event, EventBus, EventSubscriber};
use crate::registry::{PluginRegistry, PluginState};

/// 事件 topic 前缀（与 TS `eventNamespace()` 一致）。
pub const EVENT_TOPIC_PREFIX: &str = "plugin:";

/// 插件方法分发器：由具体插件形态（JS / WASM / 进程 / Rust）实现。
///
/// 框架只负责把**已校验**的请求交给它；返回的错误必须是结构化的
/// [`PluginInvokeResponse`]，不得 panic 空透到 IPC 边界（ADR-04）。
pub trait PluginDispatcher: Send + Sync {
    /// 执行一次插件方法调用。
    fn dispatch(&self, request: &PluginInvokeRequest) -> PluginInvokeResponse;

    /// 取消进行中的调用；返回是否命中。默认形态不支持取消。
    fn cancel(&self, _call_id: &str) -> bool {
        false
    }
}

/// 出站事件出口：把事件总线上的事件投递给**前端**。
///
/// 平台无关：Tauri 层注入把事件广播到 Tauri 事件系统（`emit`）的实现，
/// 测试与其他嵌入方注入自己的收集器。默认实现是空操作——但这只意味着
/// 「无人订阅前端事件」，不代表事件链路断裂：内部总线订阅者仍会收到。
pub trait EventSink: Send + Sync {
    /// 投递一条事件到前端。`topic` 为完整 topic（`plugin:<id>:<event>`）。
    fn deliver(&self, topic: &str, event: &Event);
}

/// 空事件出口（默认）。
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn deliver(&self, _topic: &str, _event: &Event) {}
}

/// 宿主命令状态（Tauri 侧通过 `app.manage()` 注入）。
pub struct HostState {
    registry: RwLock<PluginRegistry>,
    events: RwLock<EventBus>,
    dispatcher: RwLock<Option<Arc<dyn PluginDispatcher>>>,
    /// 出站事件出口（前端投递）。
    sink: RwLock<Arc<dyn EventSink>>,
}

impl HostState {
    /// 创建空状态。
    pub fn new() -> Self {
        Self {
            registry: RwLock::new(PluginRegistry::new()),
            events: RwLock::new(EventBus::new()),
            dispatcher: RwLock::new(None),
            sink: RwLock::new(Arc::new(NullEventSink)),
        }
    }

    /// 用既有注册表创建（便于宿主预先装载插件）。
    pub fn with_registry(registry: PluginRegistry) -> Self {
        Self {
            registry: RwLock::new(registry),
            events: RwLock::new(EventBus::new()),
            dispatcher: RwLock::new(None),
            sink: RwLock::new(Arc::new(NullEventSink)),
        }
    }

    /// 装载出站事件出口（Tauri 层注入 Tauri 事件系统广播器）。
    pub fn set_event_sink(&self, sink: Arc<dyn EventSink>) {
        *self.sink.write() = sink;
    }

    /// 装载插件分发器。未装载时 `plugin_invoke` 返回 `SC-9001`。
    pub fn set_dispatcher(&self, dispatcher: Arc<dyn PluginDispatcher>) {
        *self.dispatcher.write() = Some(dispatcher);
    }

    /// 卸载分发器（测试与热重载用）。
    pub fn clear_dispatcher(&self) {
        *self.dispatcher.write() = None;
    }

    /// 读注册表。
    pub fn registry(&self) -> RwLockReadGuard<'_, PluginRegistry> {
        self.registry.read()
    }

    /// 写注册表。
    pub fn registry_mut(&self) -> RwLockWriteGuard<'_, PluginRegistry> {
        self.registry.write()
    }

    /// 订阅某 topic 的事件（topic 形如 `plugin:<id>:<event>`）。
    ///
    /// topic 不合法时返回 `false`，不 panic。
    pub fn subscribe(&self, topic: &str, subscriber: Box<dyn EventSubscriber>) -> bool {
        match parse_topic(topic) {
            Some((plugin_id, event_name)) => {
                self.events
                    .write()
                    .subscribe(&plugin_id, &event_name, subscriber);
                true
            }
            None => false,
        }
    }

    /// 处理 `plugin_invoke`。
    ///
    /// 永不 panic、永不返回 `Err`：所有失败都编码进 [`PluginInvokeResponse`]，
    /// 保证前端拿到的一定是结构化结果。
    pub fn handle_invoke(&self, request: PluginInvokeRequest) -> PluginInvokeResponse {
        let call_id = request.call_id.clone();

        // 1. 信封完整性
        if request.plugin_id.trim().is_empty() {
            return PluginInvokeResponse::error(
                call_id,
                PluginErrorCode::InvalidPayload,
                "pluginId is required".to_string(),
            );
        }
        if request.method.trim().is_empty() {
            return PluginInvokeResponse::error(
                call_id,
                PluginErrorCode::InvalidPayload,
                "method is required".to_string(),
            );
        }

        // 2. 注册表状态（只有 ENABLED 可被调用）
        match self.registry.read().get_state(&request.plugin_id) {
            None => {
                return PluginInvokeResponse::error(
                    call_id,
                    PluginErrorCode::PluginNotFound,
                    format!("plugin not registered: {}", request.plugin_id),
                );
            }
            Some(PluginState::Enabled) => {}
            Some(PluginState::Errored) => {
                return PluginInvokeResponse::error(
                    call_id,
                    PluginErrorCode::PluginErrored,
                    format!("plugin is in ERRORED state: {}", request.plugin_id),
                );
            }
            Some(other) => {
                return PluginInvokeResponse::error(
                    call_id,
                    PluginErrorCode::PluginDisabled,
                    format!("plugin is not enabled ({other:?}): {}", request.plugin_id),
                );
            }
        }

        // 3. 分发
        let dispatcher = self.dispatcher.read().clone();
        match dispatcher {
            Some(d) => d.dispatch(&request),
            None => PluginInvokeResponse::error(
                call_id,
                PluginErrorCode::Internal,
                "no plugin dispatcher installed".to_string(),
            ),
        }
    }

    /// 处理 `plugin_cancel`。
    ///
    /// **幂等**：目标调用不存在时返回 `false`，不产生错误——重复取消是合法的。
    pub fn handle_cancel(&self, request: PluginCancelRequest) -> bool {
        let dispatcher = self.dispatcher.read().clone();
        match dispatcher {
            Some(d) => d.cancel(&request.call_id),
            None => false,
        }
    }

    /// 处理 `plugin_emit`。
    ///
    /// `topic` 必须形如 `plugin:<pluginId>:<eventName>`。
    pub fn handle_emit(&self, topic: &str, payload: Value) -> Result<(), PluginErrorBody> {
        let (source_plugin, event_name) = parse_topic(topic).ok_or_else(|| PluginErrorBody {
            code: format!("{}", PluginErrorCode::InvalidPayload),
            message: format!("invalid event topic (expected 'plugin:<id>:<event>'): {topic}"),
            retryable: false,
        })?;

        let event = Event {
            source_plugin,
            event_name,
            payload,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        // 1. 内部总线（Rust 侧订阅者，背压隔离）
        self.events
            .write()
            .emit(event.clone())
            .map_err(|err| PluginErrorBody {
                code: format!("{}", PluginErrorCode::Internal),
                message: format!("event bus rejected event: {err:?}"),
                retryable: false,
            })?;

        // 2. 出站投递（前端监听 `plugin:<id>:<event>`）
        //
        // 投递失败不回滚内部总线：内部订阅者已收到是既成事实，
        // 前端投递失败属于传输层问题，由 sink 自行记录/重试。
        self.sink.read().deliver(topic, &event);

        Ok(())
    }
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}

/// 解析 `plugin:<id>:<event>` → `(id, event)`。
///
/// 插件 ID 本身可以含 `.`、`-`、`_`，但**不含 `:`**，因此按 `:` 分割恰好 3 段。
fn parse_topic(topic: &str) -> Option<(String, String)> {
    let rest = topic.strip_prefix(EVENT_TOPIC_PREFIX)?;
    let mut parts = rest.splitn(2, ':');
    let plugin_id = parts.next()?;
    let event_name = parts.next()?;
    if plugin_id.is_empty() || event_name.is_empty() {
        return None;
    }
    Some((plugin_id.to_string(), event_name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::PluginType;
    use crate::envelope::generate_call_id;

    /// 记录调用的测试分发器。
    struct EchoDispatcher {
        cancelled: RwLock<Vec<String>>,
    }

    impl EchoDispatcher {
        fn new() -> Self {
            Self {
                cancelled: RwLock::new(Vec::new()),
            }
        }
    }

    impl PluginDispatcher for EchoDispatcher {
        fn dispatch(&self, request: &PluginInvokeRequest) -> PluginInvokeResponse {
            PluginInvokeResponse::ok(
                request.call_id.clone(),
                serde_json::json!({ "method": request.method }),
            )
        }
        fn cancel(&self, call_id: &str) -> bool {
            self.cancelled.write().push(call_id.to_string());
            true
        }
    }

    fn request(plugin_id: &str) -> PluginInvokeRequest {
        PluginInvokeRequest {
            plugin_id: plugin_id.to_string(),
            method: "format".to_string(),
            payload: None,
            call_id: generate_call_id(),
            timeout_ms: None,
        }
    }

    /// 把插件推进到 ENABLED。
    fn enable(state: &HostState, id: &str) {
        let mut reg = state.registry_mut();
        reg.register(id.to_string(), PluginType::Js, serde_json::json!({}));
        reg.transition(id, PluginState::Installing);
        reg.transition(id, PluginState::Installed);
        reg.transition(id, PluginState::Enabling);
        reg.transition(id, PluginState::Enabled);
    }

    #[test]
    fn unregistered_plugin_returns_not_found() {
        let state = HostState::new();
        let res = state.handle_invoke(request("com.example.missing"));
        assert!(!res.ok);
        assert_eq!(res.error.unwrap().code, format!("{}", PluginErrorCode::PluginNotFound));
    }

    #[test]
    fn disabled_plugin_is_rejected() {
        let state = HostState::new();
        state.registry_mut().register(
            "com.example.p".to_string(),
            PluginType::Js,
            serde_json::json!({}),
        );
        let res = state.handle_invoke(request("com.example.p"));
        assert!(!res.ok);
        assert_eq!(res.error.unwrap().code, format!("{}", PluginErrorCode::PluginDisabled));
    }

    #[test]
    fn errored_plugin_reports_errored_code() {
        let state = HostState::new();
        {
            let mut reg = state.registry_mut();
            reg.register("p".to_string(), PluginType::Js, serde_json::json!({}));
            reg.transition("p", PluginState::Installing);
            reg.transition("p", PluginState::Errored);
        }
        let res = state.handle_invoke(request("p"));
        assert_eq!(res.error.unwrap().code, format!("{}", PluginErrorCode::PluginErrored));
    }

    #[test]
    fn enabled_plugin_dispatches_to_dispatcher() {
        let state = HostState::new();
        enable(&state, "com.example.p");
        state.set_dispatcher(Arc::new(EchoDispatcher::new()));

        let res = state.handle_invoke(request("com.example.p"));
        assert!(res.ok, "expected ok, got {res:?}");
        assert_eq!(res.result.unwrap()["method"], "format");
    }

    #[test]
    fn enabled_without_dispatcher_reports_internal() {
        let state = HostState::new();
        enable(&state, "com.example.p");

        let res = state.handle_invoke(request("com.example.p"));
        assert!(!res.ok);
        let err = res.error.unwrap();
        assert_eq!(err.code, format!("{}", PluginErrorCode::Internal));
        // SC-9001 按错误码契约是可重试的（与 TS RETRYABLE_ERROR_CODES 一致）。
        assert!(err.retryable);
    }

    #[test]
    fn empty_plugin_id_or_method_is_invalid_payload() {
        let state = HostState::new();
        let mut req = request("p");
        req.plugin_id = "  ".to_string();
        assert_eq!(
            state.handle_invoke(req).error.unwrap().code,
            format!("{}", PluginErrorCode::InvalidPayload)
        );

        let mut req2 = request("p");
        req2.method = String::new();
        assert_eq!(
            state.handle_invoke(req2).error.unwrap().code,
            format!("{}", PluginErrorCode::InvalidPayload)
        );
    }

    #[test]
    fn response_call_id_always_matches_request() {
        let state = HostState::new();
        let req = request("com.example.missing");
        let expected = req.call_id.clone();
        assert_eq!(state.handle_invoke(req).call_id, expected);
    }

    #[test]
    fn cancel_is_idempotent_and_hits_dispatcher() {
        let state = HostState::new();
        let d = Arc::new(EchoDispatcher::new());
        state.set_dispatcher(d.clone());

        assert!(state.handle_cancel(PluginCancelRequest {
            call_id: "c1".to_string()
        }));
        // 未知调用同样不报错（幂等）
        assert!(state.handle_cancel(PluginCancelRequest {
            call_id: "unknown".to_string()
        }));
        assert_eq!(d.cancelled.read().len(), 2);
    }

    #[test]
    fn cancel_without_dispatcher_is_false_not_error() {
        let state = HostState::new();
        assert!(!state.handle_cancel(PluginCancelRequest {
            call_id: "c1".to_string()
        }));
    }

    #[test]
    fn emit_routes_to_subscribers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Counter(Arc<AtomicUsize>);
        impl EventSubscriber for Counter {
            fn deliver(&self, _event: &Event) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let state = HostState::new();
        let hits = Arc::new(AtomicUsize::new(0));
        state.subscribe("plugin:com.example.p:changed", Box::new(Counter(hits.clone())));

        state
            .handle_emit("plugin:com.example.p:changed", serde_json::json!({"n": 1}))
            .expect("emit should succeed");

        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn emit_rejects_malformed_topic() {
        let state = HostState::new();
        for bad in ["", "changed", "plugin:", "plugin:only-id", "other:a:b"] {
            let err = state
                .handle_emit(bad, serde_json::json!({}))
                .expect_err("should reject");
            assert_eq!(err.code, format!("{}", PluginErrorCode::InvalidPayload), "topic={bad}");
        }
    }

    #[test]
    fn parse_topic_handles_ids_with_dots_and_dashes() {
        assert_eq!(
            parse_topic("plugin:com.example.my-plugin:data-changed"),
            Some(("com.example.my-plugin".to_string(), "data-changed".to_string()))
        );
        // event 名里允许出现 ':'（只在第一个 ':' 处分割）
        assert_eq!(
            parse_topic("plugin:p:a:b"),
            Some(("p".to_string(), "a:b".to_string()))
        );
    }

    /// 记录出站投递的收集器（模拟前端监听）。
    struct RecordingSink(std::sync::Mutex<Vec<(String, String)>>);
    impl EventSink for RecordingSink {
        fn deliver(&self, topic: &str, event: &Event) {
            self.0
                .lock()
                .unwrap()
                .push((topic.to_string(), event.event_name.clone()));
        }
    }

    #[test]
    fn emit_forwards_to_outbound_sink_for_frontend() {
        // 断链回归测试：handle_emit 除内部总线外，必须把事件投递给出站
        // sink（前端经 Tauri 事件系统 listen）。缺这一步前端永远收不到事件。
        let state = HostState::new();
        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        state.set_event_sink(sink.clone());

        state
            .handle_emit("plugin:com.example.p:changed", serde_json::json!({"n": 1}))
            .expect("emit should succeed");

        let delivered = sink.0.lock().unwrap();
        assert_eq!(delivered.len(), 1, "sink must receive exactly one event");
        assert_eq!(delivered[0].0, "plugin:com.example.p:changed");
        assert_eq!(delivered[0].1, "changed");
    }

    #[test]
    fn emit_still_reaches_internal_bus_when_sink_present() {
        // 出站投递不得取代内部总线：Rust 侧订阅者仍须收到事件。
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Counter(Arc<AtomicUsize>);
        impl EventSubscriber for Counter {
            fn deliver(&self, _event: &Event) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let state = HostState::new();
        state.set_event_sink(Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new()))));
        let hits = Arc::new(AtomicUsize::new(0));
        state.subscribe("plugin:com.example.p:changed", Box::new(Counter(hits.clone())));

        state
            .handle_emit("plugin:com.example.p:changed", serde_json::json!({}))
            .expect("emit should succeed");

        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn emit_rejects_topic_before_touching_sink_or_bus() {
        // 非法 topic 不得产生任何投递（内部或出站）。
        let state = HostState::new();
        let sink = Arc::new(RecordingSink(std::sync::Mutex::new(Vec::new())));
        state.set_event_sink(sink.clone());

        let err = state
            .handle_emit("not-a-topic", serde_json::json!({}))
            .expect_err("should reject");

        assert_eq!(err.code, format!("{}", PluginErrorCode::InvalidPayload));
        assert!(sink.0.lock().unwrap().is_empty());
    }
}
