//! 调用投递抽象（0.4-A1，解 P0-1）。
//!
//! 把「登记 pending」与「把请求送到真正能执行它的一端」拆成两件事：
//! [`Registry::call_begin`]/[`Registry::call_begin_cross`] 只做簿记与 TTL，
//! 投递由 [`CallDelivery`] 按插件形态选实现。
//!
//! 不变量（计划 §7 A1）：
//! - pending 表继续只做簿记；投递实现**不得**写 pending 表（签名只收 `&PendingCall`）。
//! - 投递失败 = 明确的 `DeliveryReceipt { delivered: false, .. }` 或结构化错误码，
//!   **绝不**静默丢弃。
//! - [`CallDelivery::deliver`] 只接收 `&PendingCall`；结算走 [`Registry::settle_call`]，
//!   由执行方（插件/sidecar）回发结果后调用，不在投递实现里改 pending 语义。

use crate::error::{ErrorCode, HostError, HostResult};
use crate::eventbus::EventBus;
use crate::registry::{PendingCall, Registry};
use parking_lot::Mutex;
use serde_json::Value;
use std::sync::Arc;

/// 目标插件形态；宿主按插件类型选投递实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeliveryKind {
    /// JS / webview 插件：复用事件总线 request 通道。
    Js,
    /// 进程（sidecar）插件：stdin/stdout JSON-RPC 帧回路（A3）。
    Process,
    /// 进程内 WASM 运行时（A4）。
    Wasm,
    /// 原生（Rust）插件：进程内直接派发（A4）。
    Native,
    /// 未装配任何投递实现：宿主据此返回有类型的 `Unsupported`，不静默成功。
    Unwired,
}

/// 投递回执。
///
/// `delivered == false` 表示「无可用通路」（`reason` 必非空）；宿主据此返回
/// `UnsupportedBody` 而非假装成功。投递过程中的传输错误（队列满等）走 `HostResult::Err`。
#[derive(Debug, Clone)]
pub struct DeliveryReceipt {
    pub delivered: bool,
    pub reason: Option<String>,
}

/// 执行方回填的结果（与 pending 表对账，超时/取消共用既有 TTL 机制）。
#[derive(Debug, Clone)]
pub struct CallOutcome {
    pub ok: bool,
    pub result: Option<Value>,
    pub error_code: Option<String>,
}

/// 调用投递：把已登记的 pending call 送到真正能执行它的一端。
pub trait CallDelivery: Send + Sync {
    /// 目标形态；宿主按插件类型选实现。
    fn target_kind(&self) -> DeliveryKind;

    /// 投递。返回 `delivered: false` 表示无可用通路（宿主转 `Unsupported`）；
    /// 传输错误（队列满等）以 `Err` 返回。
    fn deliver(&self, call: &PendingCall) -> HostResult<DeliveryReceipt>;

    /// 结果回执：把执行方回填的结果写入 pending 表（`Registry::settle_call`）。
    fn settle(&self, call_id: &str, outcome: CallOutcome) -> HostResult<PendingCall>;
}

/// 保留入站 topic 前缀/后缀：`plugin:<id>:__call`。
pub const CALL_TOPIC_PREFIX: &str = "plugin:";
pub const CALL_TOPIC_SUFFIX: &str = ":__call";

/// 构造某插件「入站调用」topic（前端取件泵据此识别调用帧）。
pub fn call_topic(target: &str) -> String {
    format!("{CALL_TOPIC_PREFIX}{target}{CALL_TOPIC_SUFFIX}")
}

/// **Js 型投递**：复用事件总线 request 通道，宿主 publish 到 `plugin:<id>:__call`，
/// 插件侧取件泵（[`crate::eventbus::EventBus::deliver_inbound`]`host_events_drain`）
/// 取件、执行、回发结果。**零新增命令、零新增传输**。
pub struct JsCallDelivery {
    bus: Arc<Mutex<EventBus>>,
    registry: Arc<Registry>,
}

impl JsCallDelivery {
    pub fn new(bus: Arc<Mutex<EventBus>>, registry: Arc<Registry>) -> Self {
        Self { bus, registry }
    }
}

impl CallDelivery for JsCallDelivery {
    fn target_kind(&self) -> DeliveryKind {
        DeliveryKind::Js
    }

    fn deliver(&self, call: &PendingCall) -> HostResult<DeliveryReceipt> {
        let topic = call_topic(&call.target);
        // 取件泵要从帧上还原调用，因此把权威字段一次性带上（不依赖 topic 解析）。
        let payload = serde_json::json!({
            "callId": call.call_id,
            "caller": call.caller,
            "target": call.target,
            "cmd": call.cmd,
            "args": call.args,
            "seq": call.seq,
        });
        let delivered = {
            let bus = self.bus.lock();
            bus.deliver_inbound(&call.target, &topic, payload)?
        };
        if delivered > 0 {
            Ok(DeliveryReceipt {
                delivered: true,
                reason: None,
            })
        } else {
            Ok(DeliveryReceipt {
                delivered: false,
                reason: Some(format!("无法投递到 `{topic}`")),
            })
        }
    }

    fn settle(&self, call_id: &str, outcome: CallOutcome) -> HostResult<PendingCall> {
        self.registry.settle_call(call_id, outcome)
    }
}

/// **未装配投递**：任何插件类型若没有对应实现，落到这里返回 `Unsupported`，
/// 不得返回 `PendingCall` 假装成功（计划 §3-7 诚实约定）。
pub struct UnwiredDelivery;

impl CallDelivery for UnwiredDelivery {
    fn target_kind(&self) -> DeliveryKind {
        DeliveryKind::Unwired
    }

    fn deliver(&self, _call: &PendingCall) -> HostResult<DeliveryReceipt> {
        Ok(DeliveryReceipt {
            delivered: false,
            reason: Some("未装配任何调用投递实现（unwired）：该插件类型没有可执行通路".into()),
        })
    }

    fn settle(&self, _call_id: &str, _outcome: CallOutcome) -> HostResult<PendingCall> {
        Err(HostError::new(
            ErrorCode::E_CALL_NOT_FOUND,
            "unwired 投递实现无法结算调用",
        ))
    }
}

/// 按 `DeliveryKind` 选投递实现；未知/未装配 → [`UnwiredDelivery`]。
pub fn select_delivery(
    kind: DeliveryKind,
    deliveries: &std::collections::HashMap<DeliveryKind, Box<dyn CallDelivery>>,
) -> &dyn CallDelivery {
    deliveries
        .get(&kind)
        .map(|b| b.as_ref())
        .unwrap_or(&UNWIRED)
}

/// 全局共享的未装配投递实现（无任何通路时返回 `Unsupported`）。
static UNWIRED: UnwiredDelivery = UnwiredDelivery;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventbus::ChannelKind;
    use crate::manifest::{
        Contributes, EntrySpec, EventDecl, EventsDecl, Permission, PermissionEntry, PermissionIndex,
        PluginId, PluginManifest, PluginType, Risk,
    };

    fn index() -> PermissionIndex {
        PermissionIndex {
            version: 1,
            generated_at: None,
            entries: vec![PermissionEntry {
                identifier: "store:allow-get".into(),
                risk: Risk::Low,
                description: "读取 store".into(),
                scoped: false,
            }],
        }
    }

    fn manifest(id: &str) -> PluginManifest {
        let mut ev = EventsDecl::default();
        ev.publish.push(EventDecl { topic: format!("{id}.ping"), public: true });
        PluginManifest {
            id: PluginId::new(id).unwrap(),
            name: format!("plugin {id}"),
            version: semver::Version::new(1, 0, 0),
            plugin_type: PluginType::Js,
            entry: EntrySpec { js: Some("dist/index.js".into()), ..Default::default() },
            permissions: vec![Permission::new("store:allow-get")],
            scopes: serde_json::Map::new(),
            platforms: Vec::new(),
            framework: crate::manifest::parse_version_range(">=2.0 <3.0").unwrap(),
            abi: None,
            contributes: Contributes::default(),
            settings_schema: None,
            events: ev,
            host_functions: Vec::new(),
            min_allowed_version: None,
            signature: None,
            publisher: None,
        }
    }

    /// 0.4-A1 Js 投递闭环（Rust 层）：登记 → 投递进 request 队列 → 取回帧字段齐全
    /// → `settle_call` 结算。这正是计划里「宿主 → 插件方法 → 返回值」的宿主侧两跳
    /// （中间的「插件执行」在前端泵里，由 SDK 测试覆盖）。
    #[test]
    fn js_delivery_enqueues_call_frame_and_settles() {
        let bus = Arc::new(Mutex::new(EventBus::with_capacity(16)));
        let registry = Arc::new(Registry::default());
        let id = registry.install(&index(), manifest("com.example.b")).unwrap();
        registry.admin_op(&id, crate::authz::RegistryAdminOp::Enable).unwrap();

        let delivery = JsCallDelivery::new(bus.clone(), registry.clone());
        assert_eq!(delivery.target_kind(), DeliveryKind::Js);

        // 宿主（caller="main"）→ Js 插件：登记 + 投递。
        let call = registry
            .call_begin_cross("main", "com.example.b", "main", "doThing", serde_json::json!({"x":1}))
            .expect("目标可用，登记应成功");
        let receipt = delivery.deliver(&call).expect("投递不应出错");
        assert!(receipt.delivered, "Js 投递应有通路：{:?}", receipt.reason);

        // 执行方取件：request 队列里应有一帧，权威字段齐全。
        let frames = bus.lock().drain("com.example.b", ChannelKind::Request).expect("取件应成功");
        assert_eq!(frames.len(), 1, "应恰好投出一帧");
        let payload = &frames[0].payload;
        assert_eq!(payload["callId"], call.call_id.as_str());
        assert_eq!(payload["caller"], "main");
        assert_eq!(payload["target"], "com.example.b");
        assert_eq!(payload["cmd"], "doThing");
        assert_eq!(payload["args"], serde_json::json!({"x":1}));
        assert_eq!(frames[0].topic, call_topic("com.example.b"));

        // 执行方回填 → 结算；重复回填被拒（E_CALL_ALREADY_SETTLED）。
        let settled = delivery
            .settle(&call.call_id, CallOutcome {
                ok: true,
                result: Some(serde_json::json!({"done": true})),
                error_code: None,
            })
            .expect("结算应成功");
        assert_eq!(settled.state, crate::registry::CallState::Settled);
        let err = delivery.settle(&call.call_id, CallOutcome {
            ok: true,
            result: None,
            error_code: None,
        });
        assert_eq!(err.unwrap_err().code, crate::error::ErrorCode::E_CALL_ALREADY_SETTLED);
    }

    /// 未装配投递：`deliver` 必须返回 `delivered: false`（**不是** Err、更不是假成功），
    /// `settle` 必须显式拒绝——「无通路」是结构化事实，不是异常（计划 §3-7）。
    #[test]
    fn unwired_delivery_honestly_reports_no_path() {
        let delivery = UnwiredDelivery;
        assert_eq!(delivery.target_kind(), DeliveryKind::Unwired);
        let call = PendingCall {
            call_id: "c1".into(),
            plugin_id: "main".into(),
            cmd: "x".into(),
            args: serde_json::Value::Null,
            caller: "main".into(),
            target: "com.example.b".into(),
            state: crate::registry::CallState::Pending,
            result: None,
            error_code: None,
            seq: 1,
            created_at: std::time::Instant::now(),
            expires_at: std::time::Instant::now(),
        };
        let receipt = delivery.deliver(&call).unwrap();
        assert!(!receipt.delivered);
        assert!(receipt.reason.is_some(), "未投递时 reason 必非空");
        assert!(delivery.settle("c1", CallOutcome { ok: true, result: None, error_code: None }).is_err());
    }
}
