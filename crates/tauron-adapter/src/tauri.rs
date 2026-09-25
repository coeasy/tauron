//! Tauri v2 命令桥接层（§4.23）。
//!
//! 本模块**仅在 `tauri` feature 下编译**，为核心逻辑保持平台无关。
//!
//! 设计参考 Tauri 官方插件（`tauri-plugin-*`）的标准模式：
//! - 每个 `#[tauri::command]` 函数是对 `cmd_*` 平台无关函数的薄包装；
//! - `self` 档命令的 webview label 从 `WebviewWindow` 自动解析（不信任前端入参）；
//! - 错误返回 `HostError`（已实现 `Serialize`，Tauri 会序列化为 JS 侧可消费的 `{ code, message }`）；
//! - `State<CommandState>` 由 Tauri DI 注入，无需全局状态。
//!
//! 集成示例（第三方 Tauri 应用，零配置 root 注册）：
//! ```rust,no_run
//! # fn main() {
//! let _builder = tauri::Builder::default()
//!     .plugin(tauron_adapter::tauri::state_init())
//!     .invoke_handler(tauron_adapter::tauron_generate_handler![]);
//! // 应用侧继续 .run(tauri::generate_context!())——需要宿主 tauri.conf.json，
//! // doctest 环境不提供，故此处止步于 Builder 装配。
//! # }
//! ```
//! 生产形态（需为 `tauron` 插件配置 capability/ACL）：
//! ```rust,ignore
//! tauri::Builder::default()
//!     .plugin(tauron_adapter::tauri::init())
//!     .run(tauri::generate_context!())
//!     .expect("error");
//! ```

#![cfg(feature = "tauri")]

use tauron_host::authz::RegistryAdminOp;
use tauron_host::lifecycle::Event;
use tauron_host::registry::{PluginSummary, PendingCall};
use tauron_host::eventbus::{Frame, PublishResult};
use tauron_host::lifecycle::TransitionOutcome;
use tauron_host::runtime::RuntimeHandle;
// `pub use`（而不是 `use`）：R8 §2 的装配器宏 `tauri_plugin_as_host_command!` 由
// **第三方 crate** 展开，展开结果里必须能命名这两个类型
// （`$crate::tauri::TauriError` / `$crate::tauri::HostResult`）。私有导入在跨 crate
// 展开时会解析失败。
pub use tauri::ipc::InvokeError as TauriError;
pub use tauron_host::{ErrorCode, HostError, HostResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::{ContributeEntry, RuntimeHealth, RuntimeSpawnProfile};
use tauri::{Emitter, Manager, State, WebviewWindow};

use crate::{AdapterConfig, CommandState, PluginRuntimeState, StreamOpened, SubstrateState};
use tauron_host::stream::{StreamFrame, StreamKind, StreamSink};

// ── 错误转换：HostError → TauriError ──────────────────────────────

/// 将 `HostError` 转换为 `TauriError`（Tauri 命令返回值要求）。
///
/// 序列化为 **JSON 对象**而非 `{:?}` 字符串转储：`InvokeError` 底层是
/// `serde_json::Value`，`From<T: Serialize>` 会原样保留字段。
/// 前端 `normalizeError` 的分支 1（`code` 以 `E_` 开头的对象）据此还原
/// `message` 与 `retryable`；字符串转储会丢掉 `message`，前端只能靠正则
/// 从 `HostError { code: E_XXX, … }` 里反抠错误码。
fn to_tauri_err(e: HostError) -> TauriError {
    TauriError::from(e)
}

// ──────────────────────────────────────────────────────────────────────────
// 应用层线格式（与 `@tauron/host` 客户端调用点逐字段对齐）
//
// 这些结构体是 **TS ↔ Rust 的唯一契约**：字段名（camelCase）必须与
// `packages/tauron-host/src/host.ts` 的调用点字面一致，否则 Tauri 反序列化会以
// "missing required key" 失败——而只比对命令名的一致性门禁（wire-gate）看不见
// 这类断链。跨语言形状由 `@tauron/contract-tests` 的形状门禁锁定。
// ──────────────────────────────────────────────────────────────────────────

/// `host_plugin_call` 的 `req` 载荷（`HostClient.pluginCall`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPluginCallReq {
    /// 调用方生成的关联 id。权威 `callId` 由宿主铸造（`PendingCall.callId`），
    /// 本字段仅用于调用方关联。
    pub call_id: String,
    /// 目标方法名（→ 核心 `cmd`）。
    pub method: String,
    /// 调用形态：`unary` / `stream`。
    ///
    /// 闭集词表，与 TS `PluginCallRequest['kind']: 'unary' | 'stream'` 同构；
    /// 宿主不据此改变行为（stream 帧由前端 Channel 承载，见 [`wire_plugin_call`]），
    /// 但由 wire 层**校验**——静默接受任意串会让 `streaming` 这类拼写错误无声通过。
    #[serde(default)]
    pub kind: Option<String>,
    /// JSON 载荷（→ 核心 `args`）。
    #[serde(default)]
    pub args_json: Option<serde_json::Value>,
}

/// `host_call_end` 的 `req` 载荷。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCallEndReq {
    pub call_id: String,
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub seq: Option<u64>,
}

/// `host_lifecycle_report` 的 `evt` 载荷。
///
/// `event` 为 Rust [`Event`] 的 SCREAMING_SNAKE_CASE 线名（`ATTACH` / `ENABLE` …）。
/// 未知线名在反序列化阶段被拒绝（确定性失败，不静默忽略）——插件上报的是
/// **事件**而非状态：状态由宿主单一写入（§4.3）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostLifecycleEvt {
    pub event: Event,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `host_events_publish` 的 `evt` 载荷。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEventPublish {
    pub topic: String,
    pub payload: serde_json::Value,
}

/// `host_events_subscribe` 的选择器（与 TS `EventSelector` 同形）。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostEventSelector {
    pub topic: String,
}

/// `host_events_subscribe` 的返回（与 TS `Subscription` 同形）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSubscription {
    pub token: String,
    pub selectors: Vec<HostEventSelector>,
}

/// `host_registry_admin` 的 `op` 载荷（D15：操作 + 目标插件 id）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAdminOp {
    /// 操作枚举（kebab-case 线名：`disable` / `enable` / `uninstall` / `purge`）。
    pub op: RegistryAdminOp,
    /// 目标插件 id。
    pub id: String,
}

/// 对话框文件过滤器（与 TS `FileFilter` 同形）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// `host_registry_list` 的 `scope` 合法取值。
///
/// 两种取值都**不能**突破调用方真实订阅：scoped-read 档不允许枚举全量 public
/// 插件（否则等于绕过可见性过滤）。`public` 保留为语义提示。
fn scope_is_valid(scope: Option<&str>) -> bool {
    matches!(scope, None | Some("visible") | Some("public"))
}

// ──────────────────────────────────────────────────────────────────────────
// 线格式 → 核心：可单测的映射层（不依赖 Tauri 类型，影子命令直接测这里）
// ──────────────────────────────────────────────────────────────────────────

/// 从 webview label 解析插件身份 id（`plugin-<id>` → `<id>`）。
fn plugin_id_of(label: &str) -> &str {
    label.strip_prefix("plugin-").unwrap_or(label)
}

/// 窗口销毁时的资源回收钩子——由宿主在 **App Builder** 的 `on_window_event`
/// 中调用（`tauri::plugin::Builder` 没有窗口事件钩子，只有 App Builder 有）。
///
/// 回收范围：
/// - 该插件的事件订阅、反向索引与队列（以插件 id 为键，窗口关闭后再无人
///   拉取，留着即泄漏）；
/// - 该插件的多选择器订阅分组登记（`prune_subscription_groups`，否则死条目
///   随开/关窗循环累积）；
/// - 该插件尚未结束的 pending 调用（`call_end_all`，ADR-04：不清会让前端
///   `invoke()` 永久挂起）。
///
/// **不做「最后窗口」判定**：本仓的身份模型是 **一插件一 webview**，
/// label 恰为 `plugin-<插件id>`（`tauron-acl` 生成的 capability 也是
/// `"webviews": ["plugin-<id>"]`）。label 与插件 id 一一对应，因此窗口销毁
/// 即可无条件回收；带后缀的子窗口 label 本就不在身份模型内
/// （`resolve_self_identity` 会拒绝它，它无法建立订阅）。若将来支持
/// 一插件多窗口，需要给本函数加 `AppHandle` 形参以枚举存活窗口后再回收。
///
/// **不回收 topic 声明**：窗口关闭不是卸载；声明留到 `host_registry_admin`
/// 的卸载/清除才回收——否则窗口重开时发布者会因 topic 未声明而被丢弃。
pub fn cleanup_closed_window(state: &CommandState, closed_label: &str) {
    let plugin_id = plugin_id_of(closed_label);
    state.bus.lock().dispose_subscriber(plugin_id);
    // 核心订阅已清，该订阅者的多选择器分组登记必须跟着回收——否则
    // 「开窗 → 订阅 → 关窗」循环会让登记表无限累积死条目（§8-3）。
    crate::prune_subscription_groups(state, plugin_id);

    if let Ok(id) = tauron_host::manifest::PluginId::new(plugin_id) {
        state.registry.call_end_all(&id);
    }
}

/// `host_plugin_call` 线格式 → 核心（返回宿主铸造的 pending call）。
///
/// `channel` 为前端流式载荷：应用层核心按 pending 簿记，帧由前端
/// Channel 承载（本 crate 不做派发，派发属框架层 `tauron-shell`）。
///
/// `kind` 校验后即弃：宿主不需要它来分派（`call_end` 才是 stream 终帧确认），
/// 但闭集词表必须校验——否则 `unary` 拼成 `streaming` 会在线上无声通过，
/// 调用方以为拿到了流式调用，实际与 unary 无异。
pub fn wire_plugin_call(
    state: &CommandState,
    webview_label: &str,
    claimed_id: Option<&str>,
    req: &HostPluginCallReq,
    sink: Option<Arc<dyn StreamSink>>,
) -> HostResult<PendingCall> {
    if let Some(kind) = req.kind.as_deref() {
        if kind != "unary" && kind != "stream" {
            return Err(HostError::new(
                tauron_host::ErrorCode::E_AUTH_DENIED,
                format!("未知调用形态 `{kind}`（合法值：unary / stream）"),
            ));
        }
    }
    let call = crate::cmd_plugin_call(
        state,
        webview_label,
        claimed_id,
        &req.method,
        req.args_json.clone().unwrap_or(serde_json::Value::Null),
    )?;
    if let Some(sink) = sink {
        // 帧载体在这里**真正接上**（R5/P0-1）：R5 之前 `channel` 只被当成不透明
        // JSON 收下、写完即丢，于是 `kind: 'stream'` 的调用在前端表现为「回调
        // 永不触发、也不报错」——静默、无日志、无异常，最难查的那类断链。
        //
        // 订阅者取**调用条目上的插件 id**（`call_begin` 已用 label 校验过的身份），
        // 不用前端传值、也不另做一次解析——两处派生若不一致，开流与写帧会互相
        // 判成跨插件（或被绕过），这类不一致必须从结构上排除。
        if let Err(e) = state
            .registry
            .stream_bind(&call.call_id, &call.plugin_id, sink)
        {
            // 带 channel 的调用必须挂上载体：挂不上就把条目撤掉，
            // 不留一条「有调用、没接收方」的悬挂。
            let _ = state.registry.call_cancel(&call.call_id);
            return Err(e);
        }
    }
    Ok(call)
}

/// 帧载体实现：Tauri `Channel<StreamFrame>`（R5）。
///
/// `Channel` 是**平台细节**，因此只出现在这一层；`tauron_host` 只认
/// [`StreamSink`] trait，非 Tauri 宿主（移动 gateway、测试 mock）各自实现即可。
struct ChannelSink {
    channel: tauri::ipc::Channel<StreamFrame>,
}

impl StreamSink for ChannelSink {
    fn send(&self, frame: &StreamFrame) -> HostResult<()> {
        self.channel.send(frame.clone()).map_err(|e| {
            HostError::new(
                tauron_host::ErrorCode::E_HOST_PANIC,
                format!("流帧派发失败（channel 已关闭？）：{e}"),
            )
        })
    }
}

/// 把前端传来的 channel 标识转成载体。
///
/// 为什么是 `String` 而不是 `tauri::ipc::Channel<StreamFrame>` 参数：Tauri 只为
/// **裸** `Channel<T>` 实现了 `CommandArg`，`Option<Channel<T>>` 会落到
/// `T: Deserialize` 的兜底实现上而编译失败（实测）。而 `channel` 必须可缺省——
/// `unary` 调用没有帧可收。因此这里收线值（`Channel.toJSON()` 产出的
/// `__CHANNEL__:<id>` 串）自己构造，顺带给非法串一个明确错误，而不是静默当成
/// 「没有载体」。
fn channel_sink(
    raw: &str,
    webview: tauri::Webview,
) -> HostResult<Arc<dyn StreamSink>> {
    let id: tauri::ipc::JavaScriptChannelId = raw.parse().map_err(|e| {
        HostError::new(
            tauron_host::ErrorCode::E_INVALID_MANIFEST,
            format!("非法 channel 标识（期望 `__CHANNEL__:<id>`）：{e}"),
        )
    })?;
    Ok(Arc::new(ChannelSink {
        channel: id.channel_on::<tauri::Wry, StreamFrame>(webview),
    }))
}

/// `host_stream_open` 线格式：`{ req: { callId } }` + 顶层 `channel?`。
///
/// `channel` 可选：不带 = 复用调用发起时登记的载体（同一接收方）；带 = **换**载体
/// （后订阅者接管后续帧，例如前端重连后新建 Channel）。换载体同样要过身份校验
/// （`stream_bind` 会确认调用归属）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStreamOpenReq {
    pub call_id: String,
}

/// `host_stream_write` 线格式：`{ req: { streamId, argsJson | argsRaw } }`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStreamWriteReq {
    pub stream_id: String,
    #[serde(default)]
    pub args_json: Option<serde_json::Value>,
    /// 二进制载荷出口（不经 base64 夹带 JSON，§4.8 R6）。
    #[serde(default)]
    pub args_raw: Option<Vec<u8>>,
}

/// `host_stream_close` 线格式：`{ req: { streamId, kind } }`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStreamCloseReq {
    pub stream_id: String,
    pub kind: String,
}

/// `host_stream_open` 线格式 → 核心。
pub fn wire_stream_open(
    state: &CommandState,
    subscriber: &str,
    req: &HostStreamOpenReq,
) -> HostResult<StreamOpened> {
    crate::cmd_stream_open(state, subscriber, &req.call_id)
}

/// `host_stream_write` 线格式 → 核心。
pub fn wire_stream_write(
    state: &CommandState,
    subscriber: &str,
    req: &HostStreamWriteReq,
) -> HostResult<StreamFrame> {
    crate::cmd_stream_write(
        state,
        subscriber,
        &req.stream_id,
        req.args_json.clone(),
        req.args_raw.clone(),
    )
}

/// `host_stream_close` 线格式 → 核心。
pub fn wire_stream_close(
    state: &CommandState,
    subscriber: &str,
    req: &HostStreamCloseReq,
) -> HostResult<StreamFrame> {
    crate::cmd_stream_close(state, subscriber, &req.stream_id, &req.kind)
}

/// `host_stream_open` 的帧种类词表（供 TS 侧 gate 与文档对齐；运行时校验在
/// [`crate::cmd_stream_close`]）。
pub const STREAM_KINDS: [&str; 3] = ["data", "end", "error"];

/// 帧种类解析（闭集，大小写敏感）。宿主自己不猜：未知值一律拒绝。
pub fn parse_stream_kind(kind: &str) -> Option<StreamKind> {
    StreamKind::parse(kind)
}

/// `host_call_end` 线格式 → 核心。
pub fn wire_call_end(state: &CommandState, req: &HostCallEndReq) -> HostResult<PendingCall> {
    crate::cmd_call_end(state, &req.call_id)
}

/// `host_lifecycle_report` 线格式 → 核心。
pub fn wire_lifecycle_report(
    state: &CommandState,
    webview_label: &str,
    claimed_id: Option<&str>,
    evt: &HostLifecycleEvt,
) -> HostResult<TransitionOutcome> {
    crate::cmd_lifecycle_report(state, webview_label, claimed_id, evt.event)
}

/// `host_events_publish` 线格式 → 核心。
pub fn wire_events_publish(
    state: &SubstrateState,
    publisher: &str,
    evt: &HostEventPublish,
) -> HostResult<PublishResult> {
    crate::cmd_events_publish(state, publisher, &evt.topic, evt.payload.clone())
}

/// `host_events_subscribe` 线格式 → 核心（多选择器 = 分组订阅）。
///
/// 单选择器直接透传核心 token（与核心行为逐一一致）；多选择器为每个 topic
/// 订阅并记入分组表，返回分组 token——[`wire_events_unsubscribe`] 收到分组
/// token 时整体退订，避免"订阅 N 个只退掉 1 个"的悬挂订阅泄漏（§8-3）。
pub fn wire_events_subscribe(
    state: &SubstrateState,
    subscriber: &str,
    window: &str,
    sub: &[HostEventSelector],
) -> HostResult<HostSubscription> {
    if sub.is_empty() {
        return Err(HostError::new(
            tauron_host::ErrorCode::E_AUTH_DENIED,
            "sub 选择器列表不得为空".to_string(),
        ));
    }
    if sub.len() == 1 {
        let outcome = crate::cmd_events_subscribe(state, subscriber, window, &sub[0].topic)?;
        return Ok(HostSubscription {
            token: outcome.token,
            selectors: sub.to_vec(),
        });
    }

    let mut tokens = Vec::with_capacity(sub.len());
    for sel in sub {
        match crate::cmd_events_subscribe(state, subscriber, window, &sel.topic) {
            Ok(outcome) => tokens.push(outcome.token),
            Err(e) => {
                // 部分失败必须回滚：已建成的订阅若留着，调用方拿不到它们的
                // token，就永远无法退订（§8-3 零悬挂订阅——本函数的成组语义
                // 必须是全成或全败）。
                for t in &tokens {
                    let _ = crate::cmd_events_unsubscribe(state, t);
                }
                return Err(e);
            }
        }
    }
    let group_token = format!("grp:{}", tokens[0]);
    state
        .subscription_groups
        .lock()
        .insert(
            group_token.clone(),
            crate::GroupSubscription {
                subscriber: subscriber.to_string(),
                tokens,
            },
        );
    Ok(HostSubscription {
        token: group_token,
        selectors: sub.to_vec(),
    })
}

/// `host_events_unsubscribe` 线格式 → 核心（分组 token 整体退订）。
pub fn wire_events_unsubscribe(state: &SubstrateState, token: &str) -> HostResult<()> {
    let group = state.subscription_groups.lock().remove(token).map(|g| g.tokens);
    match group {
        Some(inner) => {
            // 分组退订：成员可能已被单独退订，尽力退净即可（分组登记必清）。
            for t in inner {
                let _ = crate::cmd_events_unsubscribe(state, &t);
            }
            Ok(())
        }
        None => crate::cmd_events_unsubscribe(state, token),
    }
}

/// `host_registry_list` 线格式 → 核心。
///
/// `subscribed_topics` 由宿主按 `(subscriber, window)` 从事件总线推导，
/// **不采信前端入参**——否则 scoped-read 档自报他人 public topic 即可越权枚举。
pub fn wire_registry_list(
    state: &CommandState,
    caller: Option<&str>,
    window: &str,
    scope: Option<&str>,
) -> HostResult<Vec<PluginSummary>> {
    if !scope_is_valid(scope) {
        return Err(HostError::new(
            tauron_host::ErrorCode::E_AUTH_DENIED,
            format!(
                "未知 scope `{}`（合法值：visible / public）",
                scope.unwrap_or("")
            ),
        ));
    }
    let topics = match caller {
        Some(id) => state.bus.lock().subscribed_topics_of(id, window),
        None => Vec::new(),
    };
    crate::cmd_registry_list(state, caller, &topics)
}

/// `host_registry_admin` 线格式 → 核心。
///
/// R7 收口：多了主体判定（[`crate::require_main_window`]，落点在
/// [`crate::cmd_registry_admin_as`]），故必须由调用方把**解析好的主体**传进来——
/// 本层拿不到 `WebviewWindow`，也就无从"忘了判"。
pub fn wire_registry_admin(
    caller: &crate::Caller,
    state: &CommandState,
    op: &HostAdminOp,
) -> HostResult<TransitionOutcome> {
    crate::cmd_registry_admin_as(caller, state, &op.id, op.op)
}

// ──────────────────────────────────────────────────────────────────────────
// self 档命令（webview label 从 WebviewWindow 解析，不信任前端入参 pluginId）
// ──────────────────────────────────────────────────────────────────────────

/// `host_lifecycle_report`：上报生命周期事件。
#[tauri::command]
pub fn host_lifecycle_report(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    claimed_id: Option<String>,
    evt: HostLifecycleEvt,
) -> Result<TransitionOutcome, TauriError> {
    let label = window.label();
    wire_lifecycle_report(&state, label, claimed_id.as_deref(), &evt)
        .map_err(to_tauri_err)
}

/// `host_plugin_call`：插件调用 C/D 后端（线格式：`{ req, channel? }`）。
#[tauri::command]
pub fn host_plugin_call(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    claimed_id: Option<String>,
    req: HostPluginCallReq,
    channel: Option<String>,
) -> Result<PendingCall, TauriError> {
    // 平台细节（`Channel`）只在这一层出现：wire 层只认平台无关的 `StreamSink`。
    let sink = match channel.as_deref() {
        Some(raw) => Some(channel_sink(raw, window.as_ref().clone()).map_err(to_tauri_err)?),
        None => None,
    };
    let label = window.label();
    wire_plugin_call(&state, label, claimed_id.as_deref(), &req, sink).map_err(to_tauri_err)
}

/// `host_call_end`：stream 终帧确认（线格式：`{ req: { callId, … } }`）。
#[tauri::command]
pub fn host_call_end(
    state: State<'_, PluginRuntimeState>,
    req: HostCallEndReq,
) -> Result<PendingCall, TauriError> {
    wire_call_end(&state, &req).map_err(to_tauri_err)
}

/// `host_cancel`：取消 pending call。
#[tauri::command]
pub fn host_cancel(
    state: State<'_, PluginRuntimeState>,
    call_id: String,
) -> Result<(), TauriError> {
    crate::cmd_cancel(&state, &call_id)
        .map_err(to_tauri_err)
}

/// self 档订阅者：从 webview label 解析**经校验的**身份（不用原始 label）。
///
/// 与 `host_events_publish` 的 publisher 同源；流句柄的归属校验用的就是这个字符串，
/// 因此开流与写帧必须走同一个派生函数——两处不一致会让合法调用被判成跨插件。
fn subscriber_of(label: &str) -> HostResult<String> {
    tauron_host::authz::resolve_self_identity(label, None).map(|id| id.as_str().to_string())
}

/// `host_stream_open`：为一次已挂帧载体的调用开流（self 档）。
#[tauri::command]
pub fn host_stream_open(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    req: HostStreamOpenReq,
    channel: Option<String>,
) -> Result<StreamOpened, TauriError> {
    let subscriber = subscriber_of(window.label()).map_err(to_tauri_err)?;
    // 带 channel = 换载体：后订阅者接管后续帧（旧 Channel 不再收到帧）。
    if let Some(raw) = channel.as_deref() {
        let sink = channel_sink(raw, window.as_ref().clone()).map_err(to_tauri_err)?;
        state
            .registry
            .stream_bind(&req.call_id, &subscriber, sink)
            .map_err(to_tauri_err)?;
    }
    wire_stream_open(&state, &subscriber, &req).map_err(to_tauri_err)
}

/// `host_stream_write`：写一帧（self 档，`seq` 由宿主铸）。
#[tauri::command]
pub fn host_stream_write(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    req: HostStreamWriteReq,
) -> Result<StreamFrame, TauriError> {
    let subscriber = subscriber_of(window.label()).map_err(to_tauri_err)?;
    wire_stream_write(&state, &subscriber, &req).map_err(to_tauri_err)
}

/// `host_stream_close`：发终帧并使句柄失效（self 档）。
#[tauri::command]
pub fn host_stream_close(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    req: HostStreamCloseReq,
) -> Result<StreamFrame, TauriError> {
    let subscriber = subscriber_of(window.label()).map_err(to_tauri_err)?;
    wire_stream_close(&state, &subscriber, &req).map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// 进程插件运行时命令（P0-2）
//
// 两条命令都属**插件运行时域**（依赖注册表 / 插件状态），只注册在
// `tauron_plugin_handler!` 里——底座-only 宿主未注册即不可达，这正是期望行为。
// ──────────────────────────────────────────────────────────────────────────

/// `host_runtime_spawn`：为 process 型插件启动 sidecar（线格式 `{ pluginId, profile }`）。
///
/// `profile` 里的验签材料是**必填**（§4.7：spawn 前校验签名与哈希）：宿主不会
/// 凭空替调用方造一个 hash——那等于把"未经验证的二进制"伪装成已验证。
/// **代码层判定（R7 收口）**：仅主窗。这条命令会按调用方给的 `pluginId` 启动一个
/// 可执行文件，插件 webview 若能调它，任何插件都能启动别的插件的 sidecar
/// （越权执行原语）。`window` 由 Tauri 注入，**线形不变**（入参仍是
/// `{ pluginId, profile }`）。
#[tauri::command]
pub fn host_runtime_spawn(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    plugin_id: String,
    profile: RuntimeSpawnProfile,
) -> Result<RuntimeHandle, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_runtime_spawn_as(&caller, &state, &plugin_id, &profile).map_err(to_tauri_err)
}

/// `host_runtime_health`：按 lease 查询进程健康（线格式 `{ lease }`）。
///
/// 未知 / 失效租约 → `E_LEASE_EXPIRED`（租约语义，不是 `E_CALL_NOT_FOUND`）。
///
/// **代码层判定（R7 收口）**：仅主窗——返回里带 `pid`（暴露宿主进程标识），
/// 且会驱动崩溃检测（`record_crash` + 崩溃投递）。
#[tauri::command]
pub fn host_runtime_health(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    lease: String,
) -> Result<RuntimeHealth, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_runtime_health_as(&caller, &state, &lease).map_err(to_tauri_err)
}

/// `host_events_publish`：发布事件（唯一通道；线格式 `{ evt: { topic, payload } }`）。
#[tauri::command]
pub fn host_events_publish(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    evt: HostEventPublish,
) -> Result<PublishResult, TauriError> {
    // publisher 从 webview label 解析（self 档身份绑定）
    let publisher = plugin_id_of(window.label());
    wire_events_publish(&state, publisher, &evt).map_err(to_tauri_err)
}

/// `host_events_subscribe`：订阅事件（线格式 `{ sub: [selector, …] }`）。
#[tauri::command]
pub fn host_events_subscribe(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    sub: Vec<HostEventSelector>,
) -> Result<HostSubscription, TauriError> {
    let label = window.label();
    wire_events_subscribe(&state, plugin_id_of(label), label, &sub).map_err(to_tauri_err)
}

/// `host_events_unsubscribe`：退订（支持多选择器订阅返回的分组 token）。
#[tauri::command]
pub fn host_events_unsubscribe(
    state: State<'_, SubstrateState>,
    token: String,
) -> Result<(), TauriError> {
    wire_events_unsubscribe(&state, &token).map_err(to_tauri_err)
}

/// `host_events_drain`：拉取本插件待投递帧（self 档）。
///
/// 订阅链路的取件步骤：`host_events_subscribe` 把帧排进每订阅者队列，
/// 前端通过本命令取走帧。缺了它队列只进不出，前端永远收不到事件。
///
/// R4：身份一律经 [`tauron_host::authz::resolve_principal`] 解析。畸形
/// `plugin-` label 直接拒绝，**不再**像此前那样把原始 label 当订阅者透传
/// （那会让伪造畸形 label 的窗口以未知身份拿到投递通道）。
#[tauri::command]
pub fn host_events_drain(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    kind: String,
) -> Result<Vec<Frame>, TauriError> {
    let label = window.label();
    let subscriber = match tauron_host::authz::resolve_principal(label) {
        tauron_host::authz::Principal::Plugin(id) => id.as_str().to_string(),
        tauron_host::authz::Principal::MainWindow => label.to_string(),
        tauron_host::authz::Principal::Invalid(raw) => {
            return Err(to_tauri_err(tauron_host::HostError::new(
                tauron_host::ErrorCode::E_AUTH_DENIED,
                format!("webview label `{raw}` 不是合法身份（`plugin-` 前缀但 id 非法）"),
            )));
        }
    };
    crate::cmd_events_drain(&state, &subscriber, &kind)
        .map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// scoped-read 档命令（结果按可见性过滤）
// ──────────────────────────────────────────────────────────────────────────

/// `host_registry_list`：列出可见插件（线格式 `{ scope }`）。
///
/// 调用方订阅由宿主从事件总线推导（见 [`wire_registry_list`]）。
///
/// R4：主体经 [`tauron_host::authz::resolve_principal`] 解析。畸形
/// `plugin-` label 拒绝——**这一点是安全关键**：若把它当作「无 caller」，
/// 它就会被当成主窗做 scoped-read，等于伪造畸形 label 即提权。
#[tauri::command]
pub fn host_registry_list(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    scope: Option<String>,
) -> Result<Vec<PluginSummary>, TauriError> {
    let label = window.label();
    let principal = tauron_host::authz::resolve_principal(label);
    if let tauron_host::authz::Principal::Invalid(raw) = &principal {
        return Err(to_tauri_err(tauron_host::HostError::new(
            tauron_host::ErrorCode::E_AUTH_DENIED,
            format!("webview label `{raw}` 不是合法身份（`plugin-` 前缀但 id 非法）"),
        )));
    }
    let caller = principal.plugin_id().map(|id| id.as_str());
    wire_registry_list(&state, caller, label, scope.as_deref()).map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// privileged 档命令（仅主窗）
// ──────────────────────────────────────────────────────────────────────────

/// `host_registry_list_all`：全量列表（仅主窗）。
///
/// **代码层判定**：主体从 `window.label()` 解析后交给
/// [`crate::cmd_registry_list_all_as`]，插件主体被硬拒——「仅主窗」不再只是
/// 部署配置（origin 白名单 / Tauri ACL）里的一句话。`window` 由 Tauri 注入，
/// **线形不变**（入参仍然没有，返回仍然是 `PluginSummary[]`）。
#[tauri::command]
pub fn host_registry_list_all(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
) -> Result<Vec<PluginSummary>, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_registry_list_all_as(&caller, &state).map_err(to_tauri_err)
}

/// `host_registry_admin`：启用/禁用/卸载/清除（仅主窗；线格式 `{ op: { op, id } }`）。
///
/// **代码层判定**：同上，拒绝路径不产生任何注册表副作用。
#[tauri::command]
pub fn host_registry_admin(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    op: HostAdminOp,
) -> Result<TransitionOutcome, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    wire_registry_admin(&caller, &state, &op).map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// 委托命令（settings / notify / recovery / market / brand / i18n）
// ──────────────────────────────────────────────────────────────────────────

/// `host_settings_get`：读取设置。
///
/// **线形不变**（前端契约）：入参 `key: string`，返回任意 JSON；未写过的键
/// 返回 `Null`（不是报错）。
///
/// **代码层身份判定（R7 收口）**：主窗可读任意键；插件只能读自己命名空间
/// （`plugin:<自己 id>` 或 `plugin:<自己 id>.…`）内的键，越界拒绝
/// [`tauron_host::ErrorCode::E_AUTH_DENIED`]。`window` 由 Tauri 注入，故**入参
/// 形状一个字节都没变**。
#[tauri::command]
pub fn host_settings_get(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    key: String,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_settings_get_as(&caller, &state, &key).map_err(to_tauri_err)
}

/// `host_settings_set`：写入设置。
///
/// **线形不变**（前端契约）：入参 `key: string, value: any`，返回 `()`。
/// 写入**经 [`SettingsStore`]**（不再是裸 `HashMap`）：键会被编码成合法点路径，
/// 值要过命名空间 schema。
///
/// **代码层身份判定（R7 收口）**：主窗可写任意键；插件只能写自己命名空间内的键。
/// 越界在**写之前**拒绝，拒绝路径不产生任何写入副作用。
#[tauri::command]
pub fn host_settings_set(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    key: String,
    value: serde_json::Value,
) -> Result<(), TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_settings_set_as(&caller, &state, &key, value).map_err(to_tauri_err)
}

/// `host_settings_adopt_legacy`：接手一份旧版（v1）宿主设置文档。
///
/// **谁能调（R7 收口后）**：**仅主窗**。这里此前写的是「与 `host_settings_get` /
/// `host_settings_set` 完全同一口径，没有额外身份判定」——那是缺口的原文：
/// 「特权 = 仅主窗」当时只在部署配置（origin 白名单 / Tauri ACL）里成立，代码里
/// 没有判定。现在主体由 `window.label()` 解析并经
/// [`crate::require_main_window`] 硬判：插件主体一律拒绝
/// （[`tauron_host::ErrorCode::E_AUTH_DENIED`]）。
///
/// 为什么不做成「按插件键空间限定」：本命令接手的是**整份**文档（跨所有插件与
/// 宿主键），还会把用户层整体改写、把数据版本重标为 v1。按键的命名空间规则在这
/// 里没有对应的键可以施加，所以只能整体拒绝。
///
/// **`doc` 的期望形状**：对象（键 = 设置键，值 = 设置值），即 R7 之前裸
/// `HashMap` 落盘的那份文档；数组 / 标量 / null 返回 `E_INVALID_MANIFEST`，
/// 不静默退化成空文档。文档的键按 **v1 裸键**语义解释（可以含 `.`），写入后
/// 数据版本标为 v1，随后由 `host_settings_migrate` 转成当前版本。
///
/// **返回**：`()`（与 `host_settings_set` 的返回口径一致）。**线形未变**——
/// `window` 由 Tauri 注入，前端入参仍是 `{ doc }`。
///
/// **写失败不影响启动**：本命令只改内存里的用户层，不做任何与启动/恢复链路的
/// 交互；失败只是返回错误，标记文件、恢复引擎、启动流程都不受影响。
#[tauri::command]
pub fn host_settings_adopt_legacy(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    doc: serde_json::Value,
) -> Result<(), TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_settings_adopt_legacy_as(&caller, &state, doc).map_err(to_tauri_err)
}

/// `host_settings_migrate`：把设置迁到当前 schema 版本。
///
/// **入参**：无。**返回**：迁移**步数**（数字）。`0` = 已是最新（或本就无
/// 数据）；重复调用恒为 `0`（幂等）。
///
/// **谁能调（R7 收口后）**：**仅主窗**——理由同 `host_settings_adopt_legacy`：
/// 迁移会整体改写用户层（键编码契约换版），按键的命名空间规则无法表达
/// 「只准动自己的键」。插件主体一律拒绝。
///
/// **原子性**：全有或全无——迁移链缺失、或迁移结果过不了新 schema 校验时，
/// 用户层一个字节都不改，返回 `E_INVALID_MANIFEST`。**无数据版本标注却有数据**
/// 时同样拒绝（宁可报错，也不猜起点）。
///
/// **写失败的影响**：同 `host_settings_adopt_legacy`（不触碰启动/恢复链路）。
#[tauri::command]
pub fn host_settings_migrate(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
) -> Result<usize, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_settings_migrate_as(&caller, &state).map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// R7-3：DispatchSink 的 Tauri / OS 实现
// ──────────────────────────────────────────────────────────────────────────

/// 系统通知投递的 Tauri 事件主题（前端 in-app 通道）。
///
/// 这是**投递主题**，不是新的生命周期事件：`tauron-notify` 的约束是
/// 「只产出一条框架无关的事件，渲染由前端决定」，本常量就是那条事件在 Tauri
/// 侧的落点。
pub const NOTIFICATION_TOPIC: &str = "tauron://notification";

/// `DispatchSink` 的 Tauri 实现（R7-3 门禁「DispatchSink 必须有 Tauri 实现」）。
///
/// # 它到底做了什么（逐条如实，不美化）
///
/// 1. **真实投递**：把通知作为 Tauri 事件 [`NOTIFICATION_TOPIC`] `emit` 给前端
///    （前端据此刷新通知中心）。这一步每次都会执行；**事件只带信号与归属，
///    不带正文**（`emit` = 广播给所有 webview，正文见 [`notification_payload`]
///    的文档），正文由 `host_notifications_list` 按身份过滤后提供；
/// 2. **真实 OS 信号**：对第一个 webview 窗口调 `request_user_attention`，
///    让 OS 层闪烁任务栏 / 跳动程序坞。只有 `Warning` / `Error` 才请求——
///    每条 Info 都闪一次任务栏是骚扰用户，不是通知；
/// 3. **系统通知气泡：没有**。Tauri v2 的通知中心 API 在
///    `tauri-plugin-notification` 里，而它**不在本仓的依赖闭包**
///    （`Cargo.lock` 里没有任何 `tauri-plugin-*` 条目，`tauri` 自身的
///    features/source 里也没有 notification 模块），本轮硬约束又不允许新增
///    外部依赖版本 —— 因此这里**没有任何可调用的系统通知 API**。
///
/// 因为第 3 条，`send` 返回 `Ok(false)`（= `Degraded`）：`Ok(false)` 的既有
/// 语义正是「不支持/未授权 → 静默降级为应用内」，而第 1 条做的恰好就是应用内
/// 投递。这**不是 no-op**（真实的 Tauri/OS 调用确实发生了），但也**不能**谎称
/// 系统通知已送达：那会让上层的降级矩阵失去意义。
///
/// 接入 `tauri-plugin-notification` 时只需把它的发送结果放进返回值，
/// 降级矩阵、`dispatchLog`、环形缓冲都不用动。
pub struct TauriDispatchSink {
    app: tauri::AppHandle<tauri::Wry>,
}

impl TauriDispatchSink {
    pub fn new(app: tauri::AppHandle<tauri::Wry>) -> Self {
        Self { app }
    }
}

/// 通知的 Tauri 事件载荷（camelCase）。
///
/// # 为什么这里**不带正文**（轮 11 审计修正）
///
/// `Manager::emit` 在 Tauri 2.x 的语义是「发给**所有** target」——不是"发给主窗"。
/// 而本仓的多插件宿主里，每个插件都有自己的 webview，于是**通知正文会进入每一个
/// 插件 webview 的事件回调**：宿主想说给用户的话、别的插件的标题/正文（可能含
/// 跳转 token）全都送达无关插件。这是**跨插件内容泄露**，不是"看得见别人存在"的
/// 元信息级。
///
/// 修法是把它降级成**信号**：事件只说明"有通知了"，正文只从
/// `host_notifications_list` 取——那条命令按身份过滤（插件只见自己的条目），
/// 是所有读端的唯一权威出口。
///
/// 残留（如实登记）：`pluginId` / `kind` / `ts` 仍在广播里，即"某插件在 T 时刻发了
/// 一条 X 类通知"这一层元信息对所有 webview 可见。要连这层也去掉，需要用
/// `emit_to("main")` + `emit_to("plugin-<id>")` 定向投递，但那要求宿主知道主窗
/// 的真实 label（本仓未在真实运行时验证过 label 取值，不能凭猜写死）。
fn notification_payload(entry: &tauron_notify::NotifyEntry) -> serde_json::Value {
    serde_json::json!({
        "id": entry.id,
        "pluginId": entry.plugin_id,
        "kind": entry.kind.as_str(),
        "ts": entry.ts,
    })
}

impl tauron_notify::DispatchSink for TauriDispatchSink {
    fn send(&self, entry: &tauron_notify::NotifyEntry) -> Result<bool, String> {
        // ① 应用内投递（真实 Tauri 事件）。
        let emit_error = self
            .app
            .emit(NOTIFICATION_TOPIC, notification_payload(entry))
            .err()
            .map(|e| e.to_string());
        // ② OS 层「请求用户注意」（真实 OS 信号）；仅警示级以上。
        let attention_error = if matches!(
            entry.kind,
            tauron_notify::NotifyKind::Warning | tauron_notify::NotifyKind::Error
        ) {
            match self.app.webview_windows().values().next() {
                Some(window) => window
                    .request_user_attention(Some(tauri::UserAttentionType::Informational))
                    .err()
                    .map(|e| e.to_string()),
                None => Some("没有可请求用户注意的窗口".to_string()),
            }
        } else {
            None
        };
        // ③ 系统通知气泡：能力缺失（见类型文档）→ 一律降级。
        //    通道本身出错时返回 `Err`（同样降级，但把原因留在日志里）；
        //    一切正常时返回 `Ok(false)` = 「不支持系统通知」。
        match (emit_error, attention_error) {
            (None, None) => Ok(false),
            (e, a) => {
                let mut parts = Vec::new();
                if let Some(e) = e {
                    parts.push(format!("应用内投递失败：{e}"));
                }
                if let Some(a) = a {
                    parts.push(format!("用户注意请求失败：{a}"));
                }
                Err(parts.join("；"))
            }
        }
    }
}

/// `host_notify`：发送通知。
///
/// **代码层身份判定（轮 11 第二批）**：入参 `pluginId` 是通知的**署名**，核心
/// [`crate::cmd_notify_as`] 会拒绝"以别人的名义发通知"（插件只能发自己的；主窗可发
/// 任意署名）。`window` 由 Tauri 注入，**线形不变**（前端参数没变）。
#[tauri::command]
pub fn host_notify(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    plugin_id: String,
    title: String,
    body: String,
) -> Result<(), TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_notify_as(&caller, &state, &plugin_id, &title, &body).map_err(to_tauri_err)
}

/// `host_notifications_list`：通知中心读取端（`host_notify` 的配对）。
///
/// **代码层身份过滤（轮 11 第三批）**：插件只看得见**署名是自己**的条目，
/// `total` / `unread` / `dispatchLog` 与 `items` 同源（都在可见集合上重新数）。
/// 见 [`crate::cmd_notifications_list_as`]。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_notifications_list(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    limit: Option<usize>,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_notifications_list_as(&caller, &state, limit).map_err(to_tauri_err)
}

/// `host_notifications_read`：标记通知已读（`id` 缺省 = 全部）。
///
/// **代码层身份判定（轮 11 第三批）**：插件只能标记**自己**的通知，`None`（全部
/// 已读 = 全局状态）一律拒绝，未知 id 也拒绝（否则 `marked` 变成存在性预言机）。
/// 见 [`crate::cmd_notifications_read_as`]。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_notifications_read(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    id: Option<String>,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_notifications_read_as(&caller, &state, id.as_deref()).map_err(to_tauri_err)
}

/// `host_recover_boot`：启动恢复检查。
#[tauri::command]
pub fn host_recover_boot(
    state: State<'_, SubstrateState>,
) -> Result<serde_json::Value, TauriError> {
    crate::cmd_recover_boot(&state)
        .map_err(to_tauri_err)
}

/// `host_recover_report`：应用上报启动结果（恢复引擎的**驱动信号**）。
///
/// `outcome` 是闭集（`success` / `failure`），表外值由核心层拒绝——不静默当成
/// failure 处理，否则一个拼写错误会悄悄把应用推进安全模式。
///
/// `pluginId` 仅在该插件处于 `TrialEnable` 时改变计数路径（按试验失败记，
/// 不累入全局计数）；否则只做诊断提示。
///
/// **代码层身份判定（轮 11 第二批）**：正因为上面这条"改变计数路径"，`pluginId`
/// 是**能伤到别人的参数**——核心 [`crate::cmd_recover_report_as`] 拒绝插件替别的
/// 插件（或应用级 `None`）上报。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_recover_report(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    outcome: String,
    plugin_id: Option<String>,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_recover_report_as(&caller, &state, &outcome, plugin_id.as_deref())
        .map_err(to_tauri_err)
}

/// `host_recover_trial_enable`：安全模式内试验性启用一个插件。
///
/// **代码层身份判定（轮 11 第二批）**：仅主窗——见
/// [`crate::cmd_recover_trial_enable_as`] 的危害说明（改别人的状态、烧别人的试验
/// 预算）。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_recover_trial_enable(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    plugin_id: String,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_recover_trial_enable_as(&caller, &state, &plugin_id).map_err(to_tauri_err)
}

/// `host_market_check`：检查更新（**模拟**：不做任何真实可用性探测）。
///
/// **返回**（R8 §4）：`{ available: boolean, simulated: boolean, version: string | null,
/// reason: string | null }`——`simulated: true` 与 `reason` 让前端**据字段**判断
/// "这是本地桩结果"，而不是靠读注释。
///
/// **代码层身份判定（轮 11 第二批）**：仅主窗——商城操作的是宿主级产物（更新源 /
/// 安装包），见 [`crate::cmd_market_check_as`]。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_market_check(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    // 与 host_market_download 保持同一线格式：更新源配置由前端持有并逐次下发。
    // 当前 check 为本地桩实现，源参数暂不使用（接入 tauri-plugin-updater 后生效）。
    #[allow(unused_variables)] endpoints: Option<Vec<String>>,
    #[allow(unused_variables)] pubkey: Option<String>,
) -> Result<crate::MarketCheckResult, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_market_check_as(&caller, &state).map_err(to_tauri_err)
}

/// `host_brand_info`：品牌信息。
#[tauri::command]
pub fn host_brand_info(
    state: State<'_, SubstrateState>,
) -> Result<serde_json::Value, TauriError> {
    crate::cmd_brand_info(&state)
        .map_err(to_tauri_err)
}

/// `host_i18n_t`：翻译。
///
/// 全部缺失时返回 key 本身（并计入缺失计数），不是空串——空串会让缺失文案
/// 彻底隐形。缺失可观测性见 `host_i18n_stats`。
#[tauri::command]
pub fn host_i18n_t(
    state: State<'_, SubstrateState>,
    key: String,
) -> Result<String, TauriError> {
    crate::cmd_i18n_t(&state, &key)
        .map_err(to_tauri_err)
}

/// `host_i18n_t_params`：带 `{{param}}` 占位替换的翻译。
#[tauri::command]
pub fn host_i18n_t_params(
    state: State<'_, SubstrateState>,
    key: String,
    params: serde_json::Map<String, serde_json::Value>,
) -> Result<String, TauriError> {
    crate::cmd_i18n_t_params(&state, &key, params)
        .map_err(to_tauri_err)
}

/// `host_i18n_set_locale`：切换语言（§4.20：语言状态的单一来源）。
///
/// **代码层身份判定（轮 11 第三批）**：仅主窗——切的是**全局**语言（宿主 UI +
/// 所有插件界面），插件改它 = 跨插件全局状态篡改。见
/// [`crate::cmd_i18n_set_locale_as`]。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_i18n_set_locale(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    locale: String,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_i18n_set_locale_as(&caller, &state, &locale).map_err(to_tauri_err)
}

/// `host_i18n_load`：装载一个语言包（同语言按 key 合并，不整体替换）。
///
/// 传 `pluginId` 时每个 key 自动加 `plugin:<id>.oc.` 前缀。
///
/// **代码层身份判定（轮 11 第二批）**：`pluginId` 就是命名空间归属，核心
/// [`crate::cmd_i18n_load_as`] 拒绝插件往别人的命名空间（或宿主文案）里装东西。
/// `window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_i18n_load(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    locale: String,
    entries: serde_json::Map<String, serde_json::Value>,
    plugin_id: Option<String>,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_i18n_load_as(&caller, &state, &locale, entries, plugin_id.as_deref())
        .map_err(to_tauri_err)
}

/// `host_i18n_stats`：i18n 状态与缺失键可观测性。
#[tauri::command]
pub fn host_i18n_stats(
    state: State<'_, SubstrateState>,
) -> Result<serde_json::Value, TauriError> {
    crate::cmd_i18n_stats(&state).map_err(to_tauri_err)
}

/// `host_i18n_cleanup_plugin`：清除一个插件的全部文案。
///
/// **代码层身份判定（轮 11 第二批）**：这条会**销毁**目标插件的全部文案，核心
/// [`crate::cmd_i18n_cleanup_plugin_as`] 拒绝插件清别人的。`window` 由 Tauri 注入，
/// **线形不变**。
#[tauri::command]
pub fn host_i18n_cleanup_plugin(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    plugin_id: String,
) -> Result<serde_json::Value, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_i18n_cleanup_plugin_as(&caller, &state, &plugin_id).map_err(to_tauri_err)
}

/// `host_contributes_register` 线格式：`{ entry: { kind, id, label } }`。
///
/// **刻意不含 `pluginId`**：self 档身份只从 webview label 取（§4.1）。旧线形
/// 多带的 `pluginId` 会被 serde 忽略——它只是声称，绑定发生在
/// [`wire_contributes_register`]。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributeEntryInput {
    pub kind: String,
    pub id: String,
    pub label: String,
}

/// `host_contributes_register` 线格式 → 核心（身份从 label 绑定）。
///
/// 不绑定就等于把「贡献署名」交给入参：任何插件都能把菜单/面板入口署名到
/// 别的插件（`ContributesRegistry` 以 `entry.plugin_id` 为键），并在目标插件
/// 卸载时被 `clear_plugin` 误删、平时顶着别人的名义展示。非插件 label
///（主窗等）直接拒绝——贡献注册是插件 attach 期行为（authz 登记的
/// consumer），主窗没有替他人署名的入口。
pub fn wire_contributes_register(
    state: &CommandState,
    label: &str,
    entry: &ContributeEntryInput,
) -> HostResult<()> {
    let id = tauron_host::authz::resolve_self_identity(label, None)?;
    crate::cmd_contributes_register(
        state,
        id.as_str(),
        ContributeEntry {
            plugin_id: id.to_string(),
            kind: entry.kind.clone(),
            id: entry.id.clone(),
            label: entry.label.clone(),
        },
    )
}

/// `host_contributes_register`：注册贡献（self 档）。
#[tauri::command]
pub fn host_contributes_register(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    entry: ContributeEntryInput,
) -> Result<(), TauriError> {
    wire_contributes_register(&state, window.label(), &entry).map_err(to_tauri_err)
}

/// `host_contributes_list`：列出贡献（scoped-read 档）。
#[tauri::command]
pub fn host_contributes_list(
    state: State<'_, PluginRuntimeState>,
    kind: Option<String>,
) -> Result<Vec<ContributeEntry>, TauriError> {
    crate::cmd_contributes_list(&state, kind.as_deref())
        .map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// R8 §1：三类宿主能力 Sink 的 **Tauri 实现**
//
// 这里是"平台部分"唯一的落地处：包装器不再自己操作窗口，底座也不再留桩。
// 三个实现的**真实度差异必须一眼看清**：
//   · [`TauriWindowSink`] —— **真的**操作窗口（minimize/maximize/unmaximize/close/
//     set_position/set_size/exit/restart/create 全是真实 Tauri 调用）；
//   · [`TauriDialogSink`]  —— **降级**：没有原生对话框（无 `tauri-plugin-dialog`），
//     只发一条降级信令并以"取消"返回；
//   · [`TauriDeepLinkSink`]—— **降级**：没有 OS 级协议注册（无
//     `tauri-plugin-deep-link`），`native_supported()` 恒 `false`。
//
// 为什么不做成"尽力而为的假实现"：谎报用户意图（选了某个文件 / 点了确定）或谎报
// OS 已注册，会让上层按伪造前提行事——那是安全缺陷。**如实降级**才是正确答案，
// 硬约束（不许新增依赖）把这两条路封死之后，这是唯一诚实的写法。
// ──────────────────────────────────────────────────────────────────────────

/// 窗口能力的 **Tauri 实现**：R8 之前写在包装器里的平台操作，逐条搬到这里。
///
/// `label` 由调用方（核心命令）给出，来自 wire 层的 `window.label()`——本类型
/// 不接受任何前端自报的目标，也不自己猜"当前窗口"。
///
/// # 错误口径（复用既有码，不新增）
///
/// | 情形 | 错误码 |
/// |---|---|
/// | 目标窗口不存在（未装配 / 已关闭） | [`ErrorCode::E_STATE_INVALID_TRANSITION`] |
/// | 平台拒绝该操作（Tauri 返回 `Err`） | [`ErrorCode::E_STATE_INVALID_TRANSITION`] |
/// | `create` 目标 label 已存在 | [`ErrorCode::E_PLUGIN_EXISTS`] |
///
/// 19 码封闭词表里没有"平台操作失败"这一类；`E_STATE_INVALID_TRANSITION` 的语义
/// （该操作在当前状态下不成立）是其中最贴近的，且 `deep_link_delivered` 已有同样
/// 用法。**静默 no-op 是更坏的选择**：前端会以为窗口已经动过了。
pub struct TauriWindowSink {
    app: tauri::AppHandle<tauri::Wry>,
}

impl TauriWindowSink {
    /// 绑定宿主 `AppHandle`。
    pub fn new(app: tauri::AppHandle<tauri::Wry>) -> Self {
        Self { app }
    }

    /// 取目标窗口；不存在时**如实报错**（不静默 no-op）。
    pub fn window(&self, label: &str) -> HostResult<WebviewWindow> {
        self.app.get_webview_window(label).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!(
                    "webview 窗口 `{label}` 不存在（未装配 / 已被关闭）：窗口操作未执行，\
                     这不是「操作成功」"
                ),
            )
        })
    }

    /// 统一的"取窗口 → 执行 → 转错误"口径。
    fn op<T>(
        &self,
        label: &str,
        op: &str,
        f: impl FnOnce(WebviewWindow) -> tauri::Result<T>,
    ) -> HostResult<T> {
        let window = self.window(label)?;
        f(window).map_err(|e| {
            HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                format!("窗口操作 `{op}` 在 `{label}` 上失败：{e}"),
            )
        })
    }
}

impl crate::WindowSink for TauriWindowSink {
    fn minimize(&self, label: &str) -> HostResult<()> {
        self.op(label, "minimize", |w| w.minimize())
    }

    fn maximize(&self, label: &str) -> HostResult<()> {
        self.op(label, "maximize", |w| w.maximize())
    }

    /// `restore` → Tauri 的 `unmaximize`（与 R8 之前的包装器逐字一致。
    /// Tauri v2 没有"unminimize"：还原最小化窗口的平台语义由 `show`/`set_focus`
    /// 承担，本实现**不做**额外动作，避免引入与旧行为不一致的副作用）。
    fn restore(&self, label: &str) -> HostResult<()> {
        self.op(label, "restore(unmaximize)", |w| w.unmaximize())
    }

    fn close(&self, label: &str) -> HostResult<()> {
        self.op(label, "close", |w| w.close())
    }

    /// 逻辑像素（DIP）——与 TS 侧 `screenX/screenY` 的口径一致，避免 DPI ≠ 1 时
    /// 恢复位置偏移。
    fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()> {
        self.op(label, "set_position", |w| {
            w.set_position(tauri::LogicalPosition::new(x as f64, y as f64))
        })
    }

    /// 逻辑像素（DIP）——与 TS 侧 `outerWidth/outerHeight` 一致。
    fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()> {
        self.op(label, "set_size", |w| {
            w.set_size(tauri::LogicalSize::new(width as f64, height as f64))
        })
    }

    fn quit(&self) -> HostResult<()> {
        self.app.exit(0);
        Ok(())
    }

    /// 重启：`AppHandle::restart()` 是**发散**函数（`-> !`），它**不返回**——
    /// 进程会重新执行自己。因此：
    ///
    /// - 调用方（`host_window_relaunch`）在真实重启路径上**永远读不到**返回值；
    ///   `relaunchRequested: true` 这个字段只在**测试**与**降级**路径上有意义；
    /// - `Ok(true)` 的 `true` 因此是"已请求"而非"已完成"的断言。
    ///
    /// `restart()` 之前**不做**任何清理：对账已经由命令核心在调用本函数之前完成
    /// （顺序不变量在 `cmd_window_relaunch_as` 里，不在这里——放这里就没法断言了）。
    fn relaunch(&self) -> HostResult<bool> {
        self.app.restart()
    }

    /// 创建插件窗口。
    ///
    /// # 真的做了什么
    ///
    /// 用 `WebviewWindowBuilder` 以 `spec.label`（= `plugin-<id>`）与
    /// `WebviewUrl::App(entry.ui)` 建一个 webview 窗口，标题/内尺寸取自 spec。
    /// 创建后窗口销毁的回收**不需要新钩子**：宿主已有的
    /// `on_window_event(Destroyed) → cleanup_closed_window` 就是按这个 label 回收的。
    ///
    /// # 未做 / 未验证（如实登记）
    ///
    /// - **身份令牌没有注入**：真实宿主把插件身份的 token 放在 URL fragment
    ///   （`#tauron-token=<...>`，见 `@tauron/plugin-sdk` 的 `plugin-context`），
    ///   而本仓库里 `PluginIdentity::token` 是 `u64` 计数器、前端期望的是 uuid 形态，
    ///   两者**对不上**，且无法在无运行时环境里验证 Tauri 对 fragment 的处理 →
    ///   本轮**不注入**，也不编一个假 token。创建出的窗口因此拿到的是
    ///   "只有 label 身份"的 webview；
    /// - **origin 白名单交互未验证**：窗口加载 `WebviewUrl::App`（asset 协议），
    ///   其 origin 与 dev server（`http://localhost:5173`）不同。宿主若启用了
    ///   `origin_allowlist`，需要把 asset origin 一并加进去，否则新窗口调宿主命令
    ///   会被 `origin_gate` 拒绝。这一条**没有运行期证据**（本轮无法起真窗口）。
    fn create(&self, spec: &crate::WindowCreateSpec) -> HostResult<bool> {
        if self.app.get_webview_window(&spec.label).is_some() {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_EXISTS,
                format!(
                    "窗口 `{}` 已存在：本命令不创建第二个同 label 窗口（请先关闭它）",
                    spec.label
                ),
            ));
        }
        let url = tauri::WebviewUrl::App(std::path::PathBuf::from(&spec.url));
        tauri::WebviewWindowBuilder::new(&self.app, &spec.label, url)
            .title(&spec.title)
            .inner_size(spec.width as f64, spec.height as f64)
            .build()
            .map_err(|e| {
                HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("创建窗口 `{}` 失败：{e}", spec.label),
                )
            })?;
        Ok(true)
    }
}

/// 对话框降级信令主题（**不是**对话框，是"请你改用 web 方案"的通知）。
pub const DIALOG_DEGRADED_TOPIC: &str = "tauron://dialog-degraded";

/// 降级原因的单一文案（信令里照原样给出，避免前端自己编解释）。
pub const DIALOG_DEGRADED_REASON: &str =
    "未接入 tauri-plugin-dialog（不在依赖闭包内，且本轮禁止新增依赖）：宿主没有原生对话框，\
     返回的\"取消\"不是用户操作的结果";

/// 对话框能力的 **Tauri 实现**——**降级路径**。
///
/// # 逐条如实
///
/// 1. **原生对话框：没有**。Tauri v2 的对话框 API 在 `tauri-plugin-dialog` 里，它
///    **不在本仓依赖闭包**（`Cargo.lock` 里没有任何 `tauri-plugin-*`），本轮的硬约束
///    又禁止新增依赖 → 这里**没有任何可调用的原生对话框 API**；
/// 2. **真实做的事**：向前端 `emit` 一条降级信令 [`DIALOG_DEGRADED_TOPIC`]，载荷
///    `{ op, degraded: true, nativeDialog: false, fallback, reason }`——它是**信令**，
///    作用是让前端知道"该用 web 文件选择器 / 自备确认 UI 了"，**不是**对话框本身；
/// 3. **返回值**与 R8 之前的桩逐字一致（行为不退化）：
///    `open_file` / `save_file` → `Ok(None)`（取消）、`confirm` → `Ok(false)`、
///    `message` → `Ok(())`。
///
/// 因此：**本实现没有弹出任何原生对话框**，也永远不会返回"用户选了 X / 点了确定"。
/// 谎报用户意图会让上层按伪造前提执行破坏性操作——那是安全缺陷，不是功能缺失。
pub struct TauriDialogSink {
    app: tauri::AppHandle<tauri::Wry>,
}

impl TauriDialogSink {
    /// 绑定宿主 `AppHandle`。
    pub fn new(app: tauri::AppHandle<tauri::Wry>) -> Self {
        Self { app }
    }

    /// 发一条降级信令（**best-effort**）。
    ///
    /// 为什么允许 best-effort：本命令的**权威结果**是"取消"，信令只是解释原因；
    /// 信令失败若升级成错误，会把一次降级误报成故障。信令失败不改变任何语义。
    fn signal(&self, op: &str, fallback: &str) {
        let _ = self.app.emit(
            DIALOG_DEGRADED_TOPIC,
            serde_json::json!({
                "op": op,
                "degraded": true,
                "nativeDialog": false,
                "fallback": fallback,
                "reason": DIALOG_DEGRADED_REASON,
            }),
        );
    }
}

impl crate::DialogSink for TauriDialogSink {
    fn open_file(&self, _multiple: bool, directory: bool) -> HostResult<Option<String>> {
        self.signal(
            "open",
            if directory { "web 目录选择（webkitdirectory）" } else { "web 文件选择器（<input type=file>）" },
        );
        Ok(None)
    }

    fn save_file(&self, _default_name: Option<&str>) -> HostResult<Option<String>> {
        self.signal("save", "web 下载 / File System Access API（可用性视 webview 而定）");
        Ok(None)
    }

    fn message(&self, kind: &str, _title: &str, _body: &str) -> HostResult<()> {
        self.signal(kind, "web 应用内提示（toast / 对话框组件）");
        Ok(())
    }

    fn confirm(&self, _title: &str, _body: &str) -> HostResult<bool> {
        // ⚠️ `Ok(false)` = "没有弹过对话框"。它既不是"用户点了否"也不是"用户点了是"。
        self.signal("confirm", "web 应用内确认组件（破坏性操作必须自备确认 UI）");
        Ok(false)
    }
}

/// 深链接 OS 级注册的信令主题（**不是**注册成功通知）。
pub const DEEP_LINK_NATIVE_TOPIC: &str = "tauron://deep-link-registration";

/// 深链接降级原因（信令里照原样给出）。
pub const DEEP_LINK_DEGRADED_REASON: &str =
    "未接入 tauri-plugin-deep-link（不在依赖闭包内，且本轮禁止新增依赖）：这个宿主\
     **没有**向 OS 注册协议，OS 不会把 tauri://… 之类的链接交给本应用";

/// 深链接能力的 **Tauri 实现**——同样**只有应用层那一半是真的**。
///
/// [`tauron_adapter::cmd_deep_link_register`]（核心）已经完成了**真实**的应用层动作：
/// 记录协议 + 声明 `deep-link` 公共 topic（前端能收到帧的前提）。本类型只负责
/// OS 那一半，而：
///
/// - **OS 级注册/注销：没有做**（`tauri-plugin-deep-link` 不在依赖闭包内，无法调用
///   任何 OS 注册 API）；
/// - `register` / `unregister` 返回 `Ok(())`：**语义是"应用层那一半已完成，OS 那半
///   是能力缺失"**，不是"OS 已注册"。这个区分由 [`Self::native_supported`]（恒
///   `false`）与信令载荷里的 `native: false` 承载——两者都能被机器读，不靠注释；
/// - 信令 best-effort：注册结果的权威来源是"OS 是否真的会拉起应用"，而那是 `false`；
///   信令失败不改变这个事实。
pub struct TauriDeepLinkSink {
    app: tauri::AppHandle<tauri::Wry>,
}

impl TauriDeepLinkSink {
    /// 绑定宿主 `AppHandle`。
    pub fn new(app: tauri::AppHandle<tauri::Wry>) -> Self {
        Self { app }
    }

    fn signal(&self, action: &str, protocol: &str) {
        let _ = self.app.emit(
            DEEP_LINK_NATIVE_TOPIC,
            serde_json::json!({
                "action": action,
                "protocol": protocol,
                "native": false,
                "reason": DEEP_LINK_DEGRADED_REASON,
            }),
        );
    }
}

impl crate::DeepLinkSink for TauriDeepLinkSink {
    fn register(&self, protocol: &str) -> HostResult<()> {
        self.signal("register", protocol);
        Ok(())
    }

    fn unregister(&self, protocol: &str) -> HostResult<()> {
        self.signal("unregister", protocol);
        Ok(())
    }

    fn native_supported(&self) -> bool {
        false
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 窗口管理命令（P1-1；R8 §1：包装器只做「取真实 label + 转调 sink」）
//
// **R8 之前**：`crate::cmd_window_*` 是 `Ok(())` 桩，真实窗口操作（`window.minimize()`
// / `app.exit(0)`）写在**这里**。于是"命令做了什么"横跨两层，而平台那一半在单测里
// 完全不可达（构造不出 `WebviewWindow`）。
//
// **现在**：平台操作全部搬进 [`TauriWindowSink`]，本层只剩两件事——
// ① 从宿主侧取**真实** label（`window.label()`，绝不信任前端入参）；
// ② 转调核心命令（核心再转调 sink）。行为不退化：`minimize` / `maximize` /
// `unmaximize`（= `restore`）/ `close` / `set_position(LogicalPosition)` /
// `set_size(LogicalSize)` / `app.exit(0)` 逐条与 R8 之前一致，含"逻辑像素"这一
// DPI 口径（TS 侧用 screenX/outerWidth 等逻辑像素）。
// ──────────────────────────────────────────────────────────────────────────

/// `host_window_minimize`：最小化**调用方自己**的窗口。
#[tauri::command]
pub fn host_window_minimize(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
) -> Result<(), TauriError> {
    crate::cmd_window_minimize(&state, window.label()).map_err(to_tauri_err)
}

/// `host_window_maximize`：最大化调用方的窗口。
#[tauri::command]
pub fn host_window_maximize(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
) -> Result<(), TauriError> {
    crate::cmd_window_maximize(&state, window.label()).map_err(to_tauri_err)
}

/// `host_window_restore`：还原调用方的窗口（Tauri 侧为 `unmaximize`）。
#[tauri::command]
pub fn host_window_restore(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
) -> Result<(), TauriError> {
    crate::cmd_window_restore(&state, window.label()).map_err(to_tauri_err)
}

/// `host_window_close`：关闭调用方的窗口。
#[tauri::command]
pub fn host_window_close(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
) -> Result<(), TauriError> {
    crate::cmd_window_close(&state, window.label()).map_err(to_tauri_err)
}

/// `host_window_quit`：退出应用（`app.exit(0)` 在 [`TauriWindowSink`] 里）。
#[tauri::command]
pub fn host_window_quit(state: State<'_, SubstrateState>) -> Result<(), TauriError> {
    crate::cmd_window_quit(&state).map_err(to_tauri_err)
}

/// `host_window_relaunch`：重启应用（**仅主窗**；先对账恢复阶段、再重启）。
///
/// **线形**：入参无（`window` 由 Tauri 注入），返回
/// `{ reconcile: { scanned, entered, exited, ignored }, relaunchRequested: bool,
/// reason: string | null }`。
///
/// **顺序**（R8 §3 的核心不变量）在 `crate::cmd_window_relaunch_as` 里是两行按序
/// 代码：先 `reconcile_recovery_phase`，再 `window_sink.relaunch()`。反序会把
/// 「注册表标志未与引擎判定对齐」带进下一次启动（阶段计数跨进程持久化）→ 重启循环。
/// 对账结果随返回值给出去，所以这个顺序在运行时可观测、有单测钉住。
#[tauri::command]
pub fn host_window_relaunch(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
) -> Result<crate::WindowRelaunchOutcome, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_window_relaunch_as(&caller, &state).map_err(to_tauri_err)
}

/// `host_window_create`：为**已注册**插件创建主面板窗口（**仅主窗**）。
///
/// **线形**：`{ pluginId: string, title?: string, width?: number, height?: number }`
/// → `{ label: string, pluginId: string, created: boolean, reason: string | null }`。
/// **没有 URL 参数**（刻意的）：URL 只能来自 manifest 的 `entry.ui`——让调用方指定
/// URL 等于让主窗把"带插件身份的 webview"指向任意地址，而 label 决定身份。
///
/// 身份判定与注册表检查都在核心
/// （[`crate::cmd_window_create_as`]），本层只解析主体并转调。
#[tauri::command]
pub fn host_window_create(
    state: State<'_, PluginRuntimeState>,
    window: WebviewWindow,
    plugin_id: String,
    title: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<crate::WindowCreateOutcome, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    let req = crate::WindowCreateRequest {
        plugin_id,
        title,
        width,
        height,
    };
    crate::cmd_window_create_as(&caller, &state, &req).map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// 壳扩展命令（P2 断链修复：dialog/clipboard/deep-link/market/window-geometry）
// ──────────────────────────────────────────────────────────────────────────

/// `host_window_set_position`：移动窗口（真实 Tauri 操作在 sink 里）。
#[tauri::command]
pub fn host_window_set_position(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    x: i32,
    y: i32,
) -> Result<(), TauriError> {
    crate::cmd_window_set_position(&state, window.label(), x, y).map_err(to_tauri_err)
}

/// `host_window_set_size`：调整窗口大小（真实 Tauri 操作在 sink 里）。
#[tauri::command]
pub fn host_window_set_size(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    width: u32,
    height: u32,
) -> Result<(), TauriError> {
    crate::cmd_window_set_size(&state, window.label(), width, height).map_err(to_tauri_err)
}

/// `host_clipboard_write`：写入剪贴板（进程内）。
#[tauri::command]
pub fn host_clipboard_write(
    state: State<'_, SubstrateState>,
    text: String,
) -> Result<(), TauriError> {
    crate::cmd_clipboard_write(&state, text).map_err(to_tauri_err)
}

/// `host_clipboard_read`：读取剪贴板（进程内）。
#[tauri::command]
pub fn host_clipboard_read(
    state: State<'_, SubstrateState>,
) -> Result<String, TauriError> {
    crate::cmd_clipboard_read(&state).map_err(to_tauri_err)
}

/// `host_deep_link_register`：注册深链接协议。
///
/// **代码层身份判定（轮 11 第三批）**：仅主窗——注册的是**应用级**协议（写
/// `shell_ext` + 声明 topic，并会注销上一个协议），插件改它等于把整个应用的深链接
/// 入口改到自己名下。R8 曾把它当 self-service，这里是修正。见
/// [`crate::cmd_deep_link_register_as`]。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_deep_link_register(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    protocol: String,
) -> Result<(), TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_deep_link_register_as(&caller, &state, protocol).map_err(to_tauri_err)
}

/// `host_market_download`：下载更新（**模拟**，待接 `tauri-plugin-updater`）。
///
/// `endpoints`/`pubkey` 为协议保留参数（TS 侧已发送，真实 updater 接线后启用）。
///
/// **返回**（R8 §4：`simulated` 是**线字段**，不再只写在注释里）：
/// `{ ok: boolean, simulated: boolean, version: string | null, reason: string | null }`。
/// `ok: true` 只表示"命令执行成功"，**不是**"更新已下载"——判断后者看 `simulated`。
///
/// **代码层身份判定（轮 11 第二批）**：仅主窗（同 [`host_market_check`] 的理由：
/// 会推进宿主的 `updateState`）。`window` 由 Tauri 注入，**线形不变**。
#[tauri::command]
pub fn host_market_download(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    version: Option<String>,
    #[allow(unused_variables)] endpoints: Option<Vec<String>>,
    #[allow(unused_variables)] pubkey: Option<String>,
) -> Result<crate::MarketUpdateResult, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_market_download_as(&caller, &state, version.as_deref()).map_err(to_tauri_err)
}

/// `host_market_install`：安装更新（**模拟**，待接 `tauri-plugin-updater`）。
///
/// 返回线与 [`host_market_download`] 同形（`MarketUpdateResult`）。
///
/// **代码层身份判定（轮 11 第二批）**：仅主窗（接线后就是"替换应用自身二进制"）。
#[tauri::command]
pub fn host_market_install(
    state: State<'_, SubstrateState>,
    window: WebviewWindow,
    version: Option<String>,
) -> Result<crate::MarketUpdateResult, TauriError> {
    let caller = crate::Caller::from_label(window.label()).map_err(to_tauri_err)?;
    crate::cmd_market_install_as(&caller, &state, version.as_deref()).map_err(to_tauri_err)
}

/// `host_dialog_open`：文件选择对话框（当前返回 None = 取消）。
#[tauri::command]
pub fn host_dialog_open(
    state: State<'_, SubstrateState>,
    multiple: Option<bool>,
    directory: Option<bool>,
    // `filters` / `defaultPath` 属线格式一部分（TS DialogClient 逐次下发）；
    // 当前对话框为桩实现，原生 UI 接入后生效。
    #[allow(unused_variables)] filters: Option<Vec<HostFileFilter>>,
    #[allow(unused_variables)] default_path: Option<String>,
) -> Result<Option<String>, TauriError> {
    crate::cmd_dialog_open(&state, multiple.unwrap_or(false), directory.unwrap_or(false))
        .map_err(to_tauri_err)
}

/// `host_dialog_save`：保存对话框（当前返回 None = 取消）。
#[tauri::command]
pub fn host_dialog_save(
    state: State<'_, SubstrateState>,
    default_name: Option<String>,
    #[allow(unused_variables)] filters: Option<Vec<HostFileFilter>>,
    #[allow(unused_variables)] default_path: Option<String>,
) -> Result<Option<String>, TauriError> {
    crate::cmd_dialog_save(&state, default_name.as_deref()).map_err(to_tauri_err)
}

/// `host_dialog_message`：消息对话框。
#[tauri::command]
pub fn host_dialog_message(
    state: State<'_, SubstrateState>,
    title: String,
    message: String,
    kind: Option<String>,
) -> Result<(), TauriError> {
    crate::cmd_dialog_message(&state, &title, &message, kind.as_deref()).map_err(to_tauri_err)
}

/// `host_dialog_confirm`：确认对话框（当前返回 false = 取消）。
#[tauri::command]
pub fn host_dialog_confirm(
    state: State<'_, SubstrateState>,
    title: String,
    message: String,
    // 按钮文案属线格式一部分（TS DialogClient 下发，缺省 OK/Cancel）；
    // 当前确认为桩实现，原生 UI 接入后生效。
    #[allow(unused_variables)] confirm_label: Option<String>,
    #[allow(unused_variables)] cancel_label: Option<String>,
) -> Result<bool, TauriError> {
    crate::cmd_dialog_confirm(&state, &title, &message).map_err(to_tauri_err)
}

// ──────────────────────────────────────────────────────────────────────────
// R8 §2：第三方插件命令的**宿主形态装配器**
// ──────────────────────────────────────────────────────────────────────────

/// 宿主形态命令的**唯一执行口径**（panic → `E_HOST_PANIC`，`HostError` → 结构化
/// `InvokeError`）。
///
/// 为什么需要它：`to_tauri_err` 是私有的，而"错误怎么过桥"是这个仓库的**契约**
/// （前端 `normalizeError` 靠 `{ code, message, retryable }` 对象分流；字符串转储
/// 会丢掉 `message`，前端只能正则反抠）。第三方插件的命令若自己
/// `InvokeError::from(e.to_string())`，线上行为就与宿主命令不一致了——而且这种
/// 不一致在类型上完全看不出来。装配器宏（[`tauri_plugin_as_host_command!`]）走本函数，
/// 于是"宿主形态"这件事只有一个实现。
///
/// `command` 只用于 panic 诊断（会被拼进 `E_HOST_PANIC` 的 message）。
pub fn host_command<T, F>(command: &str, body: F) -> Result<T, TauriError>
where
    F: FnOnce() -> HostResult<T>,
{
    tauron_host::guard(command, body)?.map_err(to_tauri_err)
}

/// 把一个**宿主形态**的处理器装配成 Tauri 命令（R8 §2）。
///
/// # 它解决什么问题
///
/// 第三方 Tauri 插件要把自己的命令接到宿主的能力面上时，必须自己拼出这套东西：
/// ① `#[tauri::command]` 包装；② `HostError` → `{ code, message, retryable }` 的
/// 结构化错误过桥（私有函数，抄不到）；③ panic 守卫（漏了就让 JS `invoke()` 永久
/// 挂起——Tauri 不捕获 panic，官方 issue #10327）；④ 参数由 Tauri 注入（`State` /
/// `WebviewWindow`）。**②③④ 任何一步漏掉都不报错**，只会在运行期表现为"前端卡死"
/// 或"错误信息丢失"。本宏把这四步固化成一条展开式：插件作者只写业务体。
///
/// # 用法（真实消费者：`examples/minimal-app/src-tauri/src/main.rs`）
///
/// ```rust,ignore
/// use tauri::{State, WebviewWindow};
/// use tauron_adapter::{Caller, HostResult, SubstrateState};
///
/// tauron_adapter::tauri_plugin_as_host_command! {
///     plugin = "my-plugin",
///     cmd = pub my_stats(state: State<'_, SubstrateState>, window: WebviewWindow, note: Option<String>)
///         -> serde_json::Value,
///     handler = my_stats_body,
/// }
///
/// /// 业务体：只写这一半，返回 `HostResult<T>`。
/// fn my_stats_body(
///     state: State<'_, SubstrateState>,
///     window: WebviewWindow,
///     note: Option<String>,
/// ) -> HostResult<serde_json::Value> {
///     let caller = Caller::from_label(window.label())?;      // 身份从 label 解析
///     let stats = tauron_adapter::cmd_i18n_stats(&state)?;   // 宿主能力照用
///     Ok(serde_json::json!({ "note": note, "i18n": stats }))
/// }
///
/// // 注册（插件自己的 invoke_handler；命令路由为 `plugin:my-plugin|my_stats`）：
/// tauri::plugin::Builder::new("my-plugin")
///     .invoke_handler(tauron_adapter::tauri::origin_gated_handler(
///         tauri::generate_handler![my_stats],
///     ))
///     .build()
/// ```
///
/// # 为什么形态是「声明式」而不是 `(plugin, cmd, handler)` 三元组
///
/// Tauri 的 `tauri::generate_handler!` 是 proc macro，只吃**字面 path 列表**，
/// 且一个应用只能有一个 `invoke_handler`——**没有任何办法**把一条命令从宏展开里
/// 注入到宿主的 handler 列表里（实测：往 `generate_handler!` 里传 `family!()` 会得到
/// `error: expected ','`）。因此装配器的落点是"帮插件**产出一条宿主形态的命令**"，
/// 插件把它注册进**自己的** handler（`plugin:<name>|<cmd>` 路由），宿主
/// `.plugin(...)` 一行接入。`plugin = "…"` 不是装饰：它进 panic 诊断名，
/// 让 `E_HOST_PANIC` 的 message 能指出是哪个插件的哪条命令。
///
/// # 展开结果（逐条不隐藏）
///
/// 1. 生成 `#[tauri::command] pub fn <cmd>(<参数原样>)`——**参数列表由调用方给出**
///    （宏无法反射被调用函数的签名），因此必须把形参名与类型写全；
/// 2. 返回类型是 `Result<_, TauriError>`（`TauriError = tauri::ipc::InvokeError`），
///    错误对象照宿主的形状过桥；
/// 3. 函数体 = [`host_command`]（panic 守卫 + 错误过桥），业务逻辑转发给
///    `handler`（同名的普通函数，返回 `HostResult<T>`）；
/// 4. **不做**的事：不解身份、不查 ACL、不加 origin 门。身份判定属于业务体
///    （用 `crate::Caller::from_label(window.label())`），origin 门属于 handler 装配
///    （用 [`origin_gated_handler`] 包一层，见上面的用法）；本宏不替调用方决定安全策略。
#[macro_export]
macro_rules! tauri_plugin_as_host_command {
    (
        plugin = $plugin:literal,
        cmd = $vis:vis $name:ident (
            $($arg:ident : $ty:ty),* $(,)?
        ) -> $ok:ty,
        handler = $handler:path,
    ) => {
        #[tauri::command]
        $vis fn $name($($arg: $ty),*) -> ::core::result::Result<$ok, $crate::tauri::TauriError> {
            // `move`：把参数所有权交给闭包，闭包被 `guard` 立即调用一次。
            $crate::tauri::host_command(
                ::core::concat!($plugin, "|", ::core::stringify!($name)),
                move || -> $crate::tauri::HostResult<$ok> {
                    $handler($($arg),*)
                },
            )
        }
    };
}

// ──────────────────────────────────────────────────────────────────────────
// 注册宏 + 初始化辅助
// ──────────────────────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────────────────────
// R4-D2：origin ACL —— 命令分发的**单一咽喉点**
// ──────────────────────────────────────────────────────────────────────────

/// origin 门：已配置 [`crate::AdapterConfig::origin_allowlist`] 时拒绝越界调用方。
///
/// 判定所需三要素**全部取自宿主侧**（`Invoke` 由 Tauri 在分发时构造）：
/// `message.command()` 真实命令名、`message.webview().url()` 真实 webview 的
/// origin、`message.state()` 真实装配状态。前端自报的任何 origin 都不参与判定。
///
/// 未装配本适配器状态时返回 `Ok`：此时命令自身会以「state not managed」失败，
/// 不在此伪造鉴权结论（也避免让未用本适配器状态的宿主被误拦）。
///
/// **读的是 `SubstrateState`**（R1b）：origin 允许清单属底座，底座-only 宿主也装配
/// 它；若读 `CommandState`（插件运行时态），底座-only 宿主就会被判成「未装配」而
/// **静默跳过鉴权**——origin ACL 在正是最需要它的那类宿主上失效。
fn origin_gate<R: tauri::Runtime>(invoke: &tauri::ipc::Invoke<R>) -> Result<(), HostError> {
    let state_manager = invoke.message.state();
    let Some(state) = state_manager.try_get::<SubstrateState>() else {
        return Ok(());
    };
    let allow = state.shell_ext.lock().origin_allowlist.clone();
    if allow.is_empty() {
        // 未启用（缺省）：兼容既有装配。
        return Ok(());
    }
    let origin = invoke
        .message
        .webview()
        .url()
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| tauron_host::authz::ORIGIN_UNKNOWN.to_string());
    if tauron_host::authz::origin_allowed(&allow, &origin) {
        return Ok(());
    }
    Err(HostError::new(
        tauron_host::ErrorCode::E_AUTH_DENIED,
        format!(
            "origin `{origin}` 不在允许清单内，拒绝执行 `{}`（R4-D2 origin ACL）",
            invoke.message.command()
        ),
    ))
}

/// 把 [`origin_gate`] 套在 `tauri::generate_handler!` 产出的处理器外面。
///
/// **为什么是咽喉点而不是逐命令补丁**：54 条命令逐条加校验必然漏，而漏掉的那条
/// 不会带来任何编译期或门禁期提示——只会静默裸奔。这里在唯一的分发入口判定，
/// 新命令只要进 [`tauron_generate_handler!`] 就自动受管（`host_*` 命令族清单本身
/// 也被 wire-gate 锁死，见 `@tauron/contract-tests`）。
///
/// **代价（诚实标注）**：`tauri::ipc::Invoke` 的官方文档自述「used internally by
/// macros and is explicitly **NOT** stable」。因此本函数是本仓库**唯一**依赖该
/// 结构的位置——升级 Tauri 时只需改这一处，且门禁「命令分发必须经开 origin 门」
/// 会锁死它仍在链路上。
pub fn origin_gated_handler<R, F>(inner: F) -> impl Fn(tauri::ipc::Invoke<R>) -> bool
where
    R: tauri::Runtime,
    F: Fn(tauri::ipc::Invoke<R>) -> bool,
{
    move |invoke: tauri::ipc::Invoke<R>| match origin_gate(&invoke) {
        Ok(()) => inner(invoke),
        Err(err) => {
            invoke.resolver.invoke_error(to_tauri_err(err));
            true
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 命令族的**编译期可选集合**（R1a：分模块组合，替代单宏）
//
// **为什么不是「10 个可嵌套的 token 宏」**：`tauri::generate_handler!` 是 proc
// macro，输入按**字面 path 列表**解析——实测传入 `family!()` 会得到
// `error: expected ','`，族的展开结果无法拼进同一个 handler。因此族以**两组编译期
// 可选集合**表达：宿主在**编译期**二选一，而不是运行时过滤。
//   · [`tauron_substrate_handler!`] 底座-only（38 条）
//   · [`tauron_plugin_handler!`] 全量（54 条 = 底座 38 + 插件运行时 16）
//
// 两组集合的一致性**不靠人眼**：wire-gate 断言
//   ① 全量集合 == tauri.rs 中全部 `#[tauri::command] pub fn host_*` 定义；
//   ② 底座集合 ⊂ 全量集合，且**不含任何插件域命令**。
// 漏减（底座宿主白拿插件面）或误加都会被门禁挡住，而不是静默放行。
// ──────────────────────────────────────────────────────────────────────────

/// **底座-only** 命令集：不暴露插件运行时命令面（`registry_*` / `contributes_*`
/// / `plugin_call` / `call_end` / `cancel` / `lifecycle_report`）。
///
/// 适用「不跑插件运行时」的宿主（harness 壳、单窗口工具壳）：未注册即不可达，
/// 前端调用插件命令得到 `command not found`——这正是期望行为。
#[macro_export]
macro_rules! tauron_substrate_handler {
    () => {
        // 与全量集合同用**一个** origin 门（R4-D2）：收窄命令面不影响鉴权链。
        $crate::tauri::origin_gated_handler(tauri::generate_handler![
            // shell 域：窗口 / 剪贴板 / 对话框 / 深链接 / 市场（更新链）
            $crate::tauri::host_window_minimize,
            $crate::tauri::host_window_maximize,
            $crate::tauri::host_window_restore,
            $crate::tauri::host_window_close,
            $crate::tauri::host_window_quit,
            // R8 §3：重启属于**底座**（只用恢复对账 + WindowSink，不碰注册表）。
            $crate::tauri::host_window_relaunch,
            $crate::tauri::host_window_set_position,
            $crate::tauri::host_window_set_size,
            $crate::tauri::host_clipboard_write,
            $crate::tauri::host_clipboard_read,
            $crate::tauri::host_deep_link_register,
            $crate::tauri::host_dialog_open,
            $crate::tauri::host_dialog_save,
            $crate::tauri::host_dialog_message,
            $crate::tauri::host_dialog_confirm,
            $crate::tauri::host_market_check,
            $crate::tauri::host_market_download,
            $crate::tauri::host_market_install,
            // ipc 域：事件总线
            $crate::tauri::host_events_publish,
            $crate::tauri::host_events_subscribe,
            $crate::tauri::host_events_unsubscribe,
            $crate::tauri::host_events_drain,
            // i18n 域
            $crate::tauri::host_i18n_t,
            $crate::tauri::host_i18n_t_params,
            $crate::tauri::host_i18n_set_locale,
            $crate::tauri::host_i18n_load,
            $crate::tauri::host_i18n_stats,
            $crate::tauri::host_i18n_cleanup_plugin,
            // 通知域
            $crate::tauri::host_notify,
            $crate::tauri::host_notifications_list,
            $crate::tauri::host_notifications_read,
            // 设置域
            $crate::tauri::host_settings_get,
            $crate::tauri::host_settings_set,
            // R7-2 迁移入口：没有这两条命令，迁移机制就是孤儿逻辑
            // （只有 Rust 测试碰得到，宿主没有渠道把旧配置递进来）。
            $crate::tauri::host_settings_adopt_legacy,
            $crate::tauri::host_settings_migrate,
            // 恢复域
            $crate::tauri::host_recover_boot,
            $crate::tauri::host_recover_report,
            // 注意：`host_recover_trial_enable` **不在底座集合**——它要读注册表里
            // 插件的当前状态并补发 `TrialEnable`（R1b 实测发现），属插件运行时域。
            // 品牌 / 诊断域
            $crate::tauri::host_brand_info,
        ])
    };
}

/// **全量**命令集：底座 38 条 + 插件运行时 16 条（多插件宿主）。
#[macro_export]
macro_rules! tauron_plugin_handler {
    () => {
        $crate::tauri::origin_gated_handler(tauri::generate_handler![
            // ── 底座（与 [`tauron_substrate_handler!`] 的 38 条逐条一致）──
            $crate::tauri::host_window_minimize,
            $crate::tauri::host_window_maximize,
            $crate::tauri::host_window_restore,
            $crate::tauri::host_window_close,
            $crate::tauri::host_window_quit,
            // R8 §3：底座命令（与底座集合同条，逐条一致由门禁守住）。
            $crate::tauri::host_window_relaunch,
            $crate::tauri::host_window_set_position,
            $crate::tauri::host_window_set_size,
            $crate::tauri::host_clipboard_write,
            $crate::tauri::host_clipboard_read,
            $crate::tauri::host_deep_link_register,
            $crate::tauri::host_dialog_open,
            $crate::tauri::host_dialog_save,
            $crate::tauri::host_dialog_message,
            $crate::tauri::host_dialog_confirm,
            $crate::tauri::host_market_check,
            $crate::tauri::host_market_download,
            $crate::tauri::host_market_install,
            $crate::tauri::host_events_publish,
            $crate::tauri::host_events_subscribe,
            $crate::tauri::host_events_unsubscribe,
            $crate::tauri::host_events_drain,
            $crate::tauri::host_i18n_t,
            $crate::tauri::host_i18n_t_params,
            $crate::tauri::host_i18n_set_locale,
            $crate::tauri::host_i18n_load,
            $crate::tauri::host_i18n_stats,
            $crate::tauri::host_i18n_cleanup_plugin,
            $crate::tauri::host_notify,
            $crate::tauri::host_notifications_list,
            $crate::tauri::host_notifications_read,
            $crate::tauri::host_settings_get,
            $crate::tauri::host_settings_set,
            $crate::tauri::host_settings_adopt_legacy,
            $crate::tauri::host_settings_migrate,
            $crate::tauri::host_recover_boot,
            $crate::tauri::host_recover_report,
            $crate::tauri::host_brand_info,
            // ── 插件运行时（16 条；底座-only 宿主不得注册）──
            // 其中**插件面可触达**的那些（`host_lifecycle_report` / `host_plugin_call` /
            // `host_call_end` / `host_cancel` / `host_registry_list` /
            // `host_contributes_register` / 流式三命令 / `host_recover_trial_enable`…）
            // 必须在 `tauron_host::authz` 的档位表里有登记——「注册了但没登记档位」
            // 正是 R7 收口时抓到的缺口（`host_events_drain` / `host_stream_*`）。
            // 主窗命令（`host_registry_list_all` / `host_registry_admin` /
            // `host_runtime_*` 等）不走档位表，由 Tauri ACL + `origin_gate` 管辖。
            $crate::tauri::host_lifecycle_report,
            $crate::tauri::host_plugin_call,
            $crate::tauri::host_call_end,
            $crate::tauri::host_cancel,
            $crate::tauri::host_registry_list,
            $crate::tauri::host_registry_list_all,
            $crate::tauri::host_registry_admin,
            $crate::tauri::host_contributes_register,
            $crate::tauri::host_contributes_list,
            $crate::tauri::host_recover_trial_enable,
            // 流式三命令（R5/P0-1）**成组**注册：只注册其中一两条会让流无法终结
            // （缺 close）或无法开流（缺 open），因此由门禁锁死「三缺一即失败」。
            $crate::tauri::host_stream_open,
            $crate::tauri::host_stream_write,
            $crate::tauri::host_stream_close,
            // 进程插件运行时（P0-2）：**成对**注册——只有 spawn 没有 health 就
            // 无法发现 sidecar 崩溃（崩溃检测是轮询式的），只有 health 没有
            // spawn 则永远拿不到 lease。二者同属插件运行时域。
            $crate::tauri::host_runtime_spawn,
            $crate::tauri::host_runtime_health,
            // R8 §3：`host_window_create` 属**插件运行时域**——它要在注册表里确认
            // 目标插件存在、并从 manifest 取 `entry.ui` 作为窗口 URL；底座态
            // （`SubstrateState`）在编译期就拿不到注册表。只注册底座集合的宿主
            // 因此拿不到「为插件开窗」这条命令（语义正确：它没有插件可开窗）。
            $crate::tauri::host_window_create,
        ])
    };
}

/// `host_*` 命令族全量注册（历史名称，等价 [`tauron_plugin_handler!`]）。
///
/// 两种注册方式（Tauri v2 ACL 语义决定）：
/// 1. **应用层 root 注册（零配置，推荐起步）**：
///    `tauri::Builder::default().invoke_handler(tauron_adapter::tauron_generate_handler![])`
///    ——前端以裸命令名 `invoke('host_window_minimize')` 调用；本地 origin 且无
///    app ACL manifest 时不经能力检查直达 handler。此时 `TauriBackend` 需传
///    `commandPrefix: ''`。
/// 2. **插件注册**（[`init`]，即本适配器内置方式）：命令路由为
///    `plugin:tauron|<name>`——**必须**为 `tauron` 插件配置 capability/permission
///    授予所需命令（Tauri v2 对 `plugin:` 命令强制 ACL），生产客户端应采用。
///
/// 不跑插件运行时的宿主改用 [`tauron_substrate_handler!`]（少 16 条插件命令）。
#[macro_export]
macro_rules! tauron_generate_handler {
    () => {
        $crate::tauron_plugin_handler![]
    };
}

/// 内部别名（保持本 crate 内 `generate_handler!()` 调用点不变）。
macro_rules! generate_handler {
    () => {
        $crate::tauron_generate_handler![]
    };
}

/// 编译期证据（方案 R1「验证」项）：命令族两组集合都能独立成立。
///
/// 这里只做**类型检查**——若 [`tauron_substrate_handler!`] 里任一命令不存在、或
/// origin 门被摘掉、或两组集合类型不再兼容 `invoke_handler`，本模块即编译失败。
/// 运行期鉴权行为由 `origin_gate` 的单测与门禁覆盖。
#[cfg(test)]
mod handler_families {
    /// 底座-only：不注册任何插件运行时命令。
    #[allow(dead_code)]
    fn substrate_only_handler_compiles() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool {
        tauron_substrate_handler![]
    }

    /// 全量：底座 + 插件运行时。
    #[allow(dead_code)]
    fn full_handler_compiles() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool {
        tauron_plugin_handler![]
    }

    /// R7-3 门禁「DispatchSink 必须有 Tauri 实现」的**编译期证据**。
    ///
    /// 真实 `AppHandle` 在单测里构造不出来（需要 Tauri 运行时与窗口），所以这里
    /// 用类型证据替代行为证据：`TauriDispatchSink` 一旦不再实现 `DispatchSink`，
    /// 本模块即编译失败。行为路径（emit 是否送达、系统通知是否可用）**未在单测
    /// 覆盖**，属未验证项。
    #[allow(dead_code)]
    fn tauri_dispatch_sink_implements_the_trait() {
        fn assert_sink<T: tauron_notify::DispatchSink>() {}
        assert_sink::<super::TauriDispatchSink>();
    }

    /// R7-2 迁移入口「已接线」的**编译期证据**。
    ///
    /// 两个包装器带 `State<'_, SubstrateState>` 与注入的 `WebviewWindow`，单测里
    /// 构造不出来（要 Tauri 运行时），所以只能做类型证据：函数项一旦改名 / 改签名
    /// （例如去掉 `window`、把 `doc` 改成 `String`、把返回改成 `()`），这里立刻
    /// 编译失败。行为路径由 `lib.rs` 的
    /// `settings_migration_commands_round_trip_through_the_command_layer` 驱动
    /// `cmd_*_as` 入口覆盖。
    #[allow(dead_code)]
    fn settings_migration_commands_are_wired() {
        let _: fn(
            tauri::State<'_, super::SubstrateState>,
            tauri::WebviewWindow,
            serde_json::Value,
        ) -> Result<(), super::TauriError> = super::host_settings_adopt_legacy;
        let _: fn(
            tauri::State<'_, super::SubstrateState>,
            tauri::WebviewWindow,
        ) -> Result<usize, super::TauriError> = super::host_settings_migrate;
    }
}

/// 按宿主配置装配 [`CommandState`]：恢复持久化在这里打开（[`init`] /
/// [`state_init`] 共用的唯一装配点）。
///
/// 不接这一层就等于 §4.14 的崩溃检测只活在单进程内——进程一退计数器即失，
/// 安全模式永不触发。因此两个入口都必须走这里，而不是 [`CommandState::new`]
/// （后者刻意关闭持久化，仅供单元测试）。
///
/// **发现于轮 7 收口**：此前两个入口各自硬编码 `command_state_with_dir(...)`，
/// 其内部固定 `AdapterConfig::default()`——于是 `AdapterConfig` 的**每一个字段
/// 对宿主都不可达**（origin 允许清单、注册表配置、必需插件集合全在内），配置面
/// 形同虚设。现在两个入口都提供 `*_with_config` 变体，缺省变体等价于原行为。
///
/// `recovery_data_dir` 的优先级：宿主显式配置 > Tauri `app_config_dir()`。
/// 兜底仍走 app 目录，避免调用方忘了配就悄悄丢掉跨进程崩溃检测；`dir = None`
/// （路径解析失败）时退回纯内存态而非 panic，`host_recover_boot` 的
/// `persistence.enabled` 会如实报告 `false`。
///
/// R7-3：`app` 只用来注入系统通知通道（[`TauriDispatchSink`]）。底座本身
/// 不依赖 tauri，`OnceLock` 是它与 wire 层之间唯一的接缝。
fn command_state_with_dir_and_config(
    dir: Option<std::path::PathBuf>,
    mut cfg: AdapterConfig,
    app: &tauri::AppHandle<tauri::Wry>,
) -> CommandState {
    if cfg.recovery_data_dir.is_none() {
        cfg.recovery_data_dir = dir;
    }
    // R1b：先建底座（唯一一份），再在同一底座上装插件运行时。
    let mut substrate = SubstrateState::with_adapter_config(&cfg);
    // R7-3：注入 Tauri 系统通知通道。注入点只有这里（`OnceLock` 只设一次）——
    // 与 `plugin_flags` 同一模式，底座保持平台无关。
    let _ = substrate
        .notify_sink
        .set(std::sync::Arc::new(TauriDispatchSink::new(app.clone())));
    // R8 §1：注入三类平台能力。**不注入 = 全部走 `lib.rs` 的进程内降级实现**
    // （窗口操作不生效、对话框恒取消、深链接不注册）——那对真实宿主是"哑掉"，所以
    // 生产装配点必须在这里补齐。真伪差异见各类型文档：只有窗口是真的。
    substrate.window_sink = std::sync::Arc::new(TauriWindowSink::new(app.clone()));
    substrate.dialog_sink = std::sync::Arc::new(TauriDialogSink::new(app.clone()));
    substrate.deep_link_sink = std::sync::Arc::new(TauriDeepLinkSink::new(app.clone()));
    PluginRuntimeState::with_substrate(std::sync::Arc::new(substrate), cfg)
}

/// 注册**两个** managed 状态：底座 `SubstrateState` + 插件运行时 `PluginRuntimeState`。
///
/// 底座 Arc 在这里创建一次，两个 managed 值共享同一份——不是两份状态。分开注册是
/// 为了让命令的 `State` bound 能表达归属：底座命令绑 `SubstrateState`（编译期拿不到
/// 注册表），插件命令绑 `PluginRuntimeState`。
///
/// 底座-only 宿主**不用**本函数：只 `manage(SubstrateState)` 并注册
/// `tauron_substrate_handler![]`，一分插件状态都不建。
fn manage_states<R: tauri::Runtime, M: tauri::Manager<R>>(manager: &M, state: CommandState) {
    // 先克隆底座（浅克隆：全部字段是 Arc/Mutex，共享同一份内部状态），再移动插件态。
    manager.manage((*state.substrate).clone());
    manager.manage(state);
}

/// 初始化辅助：在 Tauri Builder 中注册 CommandState + handler。
///
/// 参考 Tauri 官方插件的 `init()` 模式：
/// ```rust,ignore
/// tauri::Builder::default()
///     .plugin(tauron_adapter::tauri::init())
///     .run(tauri::generate_context!())
/// ```
pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    init_with_adapter_config(AdapterConfig::default())
}

/// [`init`] 的可配置变体：装配完整宿主配置（origin 允许清单 / 注册表 / 必需插件 …）。
///
/// 缺省变体 [`init`] 只是本函数传入 `AdapterConfig::default()` 的语法糖。
/// 历史入口 [`init_with_config`]（只接注册表配置）也委托到这里。
pub fn init_with_adapter_config(cfg: AdapterConfig) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("tauron")
        .setup(move |app, _api| {
            manage_states(
                app,
                command_state_with_dir_and_config(app.path().app_config_dir().ok(), cfg, app),
            );
            Ok(())
        })
        .invoke_handler(generate_handler!())
        .build()
}

/// 仅 manage [`CommandState`] 的轻量初始化（配合
/// [`tauron_generate_handler!`] 的 root 注册使用）。
///
/// 为什么需要它：[`init`] 把命令注册在插件 invoke_handler 上，前端必须经
/// `plugin:tauron|<name>` 路由，而 Tauri v2 对 `plugin:` 命令**强制 capability/ACL**；
/// 零配置的起步集成改用 root 注册（裸命令名）+ 本函数补状态，即无需任何
/// capability 文件。二者**二选一**——同时使用会重复 `manage::<CommandState>` 而 panic。
pub fn state_init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    state_init_with_adapter_config(AdapterConfig::default())
}

/// [`state_init`] 的可配置变体（见 [`init_with_config`]）。
pub fn state_init_with_adapter_config(cfg: AdapterConfig) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("tauron-state")
        .setup(move |app, _api| {
            manage_states(
                app,
                command_state_with_dir_and_config(app.path().app_config_dir().ok(), cfg, app),
            );
            Ok(())
        })
        .build()
}

/// 深链接到达统一入口（生产接 `tauri-plugin-deep-link` 回调）。
///
/// 一条 OS 事件，两条投递管道，覆盖前端全部订阅方式：
/// 1. **Tauri 原生事件** `deep-link`——`backend.listen('deep-link')` 消费者
///    （`@tauron/host` 的 `TauriBackend.listen` 即映射到原生事件，
///    `DeepLinkClient` 走此管道）；
/// 2. **应用层 EventBus**——`HostClient` 订阅 + `host_events_drain` 拉取消费者。
///
/// 接线示例（应用入口）：
/// ```rust,ignore
/// tauri::Builder::default()
///     .plugin(tauron_adapter::tauri::init())
///     .plugin(tauri_plugin_deep_link::init())
///     .build(tauri::generate_context!())
///     .expect("failed to build")
///     .run(|app, event| {
///         #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
///         if let tauri::RunEvent::NewDeepLinkRequest(urls) = event {
///             for url in urls {
///                 let _ = tauron_adapter::tauri::deliver_deep_link(app, &url);
///             }
///         }
///     });
/// ```
pub fn deliver_deep_link<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    url: &str,
) -> Result<(), String> {
    use tauri::Emitter;
    // 管道 2：EventBus（ACL 化订阅者可见）。
    let state = app.state::<SubstrateState>();
    crate::deep_link_delivered(&state, url).map_err(|e| e.message)?;
    // 管道 1：Tauri 原生广播（payload 与 core 层投递保持同形状）。
    let payload = serde_json::json!({
        "url": url,
        "protocol": state.shell_ext.lock().deep_link_protocol.clone(),
    });
    app.emit(crate::DEEP_LINK_TOPIC, payload)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 用指定**注册表**配置初始化（历史入口，保留以免破坏既有调用方）。
///
/// 等价于 `init_with_adapter_config(AdapterConfig { registry: Some(config), ..Default })`。
/// 新代码请直接用 [`init_with_adapter_config`]——它能配 origin 允许清单等。
///
/// **轮 7 收口修正**：本入口此前直接 `manage(CommandState::with_config(config))`，
/// **绕过了 [`command_state_with_dir_and_config`] 这个唯一装配点**，于是恢复持久化
/// 在该入口静默失活（`recovery_data_dir = None` → 崩溃检测只在进程内有效，安全
/// 模式永不触发），origin 允许清单也无从配置。改为委托后两个问题一并消失。
pub fn init_with_config(config: tauron_host::RegistryConfig) -> tauri::plugin::TauriPlugin<tauri::Wry> {
    init_with_adapter_config(AdapterConfig {
        registry: Some(config),
        ..AdapterConfig::default()
    })
}

// ──────────────────────────────────────────────────────────────────────────
// 线格式门禁测试
//
// `#[tauri::command]` 包装体本身依赖 Tauri 运行时（`WebviewWindow`/`State`）无法
// 单测，故线格式契约全部下沉到 `wire_*` 映射函数在此直测；JSON 字面量逐字复制
// `packages/tauron-host/src/host.ts` 的调用点，TS 侧形状一旦漂移这些用例即失败。
// ──────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod wire_tests {
    use super::*;
    use tauron_host::manifest::EventDecl;

    #[test]
    fn plugin_call_req_matches_ts_call_site() {
        // 镜像 TS：pluginCall 发送 { req: { callId, method, kind, argsJson } }
        let req: HostPluginCallReq = serde_json::from_value(serde_json::json!({
            "callId": "c-1",
            "method": "format",
            "kind": "unary",
            "argsJson": { "text": "hi" }
        }))
        .expect("TS pluginCall 的 req 形状必须可反序列化");
        assert_eq!(req.call_id, "c-1");
        assert_eq!(req.method, "format");
        assert_eq!(req.kind.as_deref(), Some("unary"));
        assert_eq!(req.args_json, Some(serde_json::json!({ "text": "hi" })));

        // argsJson 缺省必须合法（对应 TS 的 argsRaw/unary 无参分支）
        let bare: HostPluginCallReq =
            serde_json::from_value(serde_json::json!({ "callId": "c-2", "method": "m", "kind": "stream" }))
                .unwrap();
        assert!(bare.args_json.is_none());
    }

    /// 闭集词表：未知 `kind` 必须被拒（否则 `streaming` 这类拼写错误无声通过）。
    #[test]
    fn plugin_call_rejects_unknown_kind() {
        let state = CommandState::new();
        let req: HostPluginCallReq = serde_json::from_value(serde_json::json!({
            "callId": "c-1",
            "method": "format",
            "kind": "streaming"
        }))
        .unwrap();

        let err = wire_plugin_call(&state, "plugin-com.a", Some("com.a"), &req, None)
            .expect_err("未知 kind 必须被拒");
        assert_eq!(err.code, tauron_host::ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("未知调用形态"), "message={}", err.message);
    }

    /// 两个合法 `kind` 都必须通过词表校验（后续失败原因不得是 kind）。
    #[test]
    fn plugin_call_accepts_both_documented_kinds() {
        let state = CommandState::new();
        for kind in ["unary", "stream"] {
            let req: HostPluginCallReq = serde_json::from_value(serde_json::json!({
                "callId": "c-1",
                "method": "format",
                "kind": kind
            }))
            .unwrap();

            let err = wire_plugin_call(&state, "plugin-com.a", Some("com.a"), &req, None)
                .expect_err("未注册插件应失败");
            assert!(
                !err.message.contains("未知调用形态"),
                "kind={kind} 被误判为非法：{}",
                err.message
            );
        }
    }

    /// `kind` 缺省（老客户端）必须仍被接受。
    #[test]
    fn plugin_call_without_kind_is_accepted() {
        let state = CommandState::new();
        let req: HostPluginCallReq =
            serde_json::from_value(serde_json::json!({ "callId": "c-1", "method": "format" }))
                .unwrap();

        let err = wire_plugin_call(&state, "plugin-com.a", Some("com.a"), &req, None)
            .expect_err("未注册插件应失败");
        assert!(!err.message.contains("未知调用形态"), "message={}", err.message);
    }

    #[test]
    fn call_end_req_matches_ts_call_site() {
        let minimal: HostCallEndReq =
            serde_json::from_value(serde_json::json!({ "callId": "c-3", "ok": false })).unwrap();
        assert_eq!(minimal.call_id, "c-3");
        assert_eq!(minimal.ok, Some(false));
        assert!(minimal.error_code.is_none());

        let full: HostCallEndReq = serde_json::from_value(serde_json::json!({
            "callId": "c-4",
            "ok": true,
            "errorCode": "E_CALL_TIMEOUT",
            "seq": 7
        }))
        .unwrap();
        assert_eq!(full.error_code.as_deref(), Some("E_CALL_TIMEOUT"));
        assert_eq!(full.seq, Some(7));
    }

    #[test]
    fn lifecycle_evt_uses_event_wire_names_not_states() {
        let evt: HostLifecycleEvt = serde_json::from_value(serde_json::json!({
            "event": "ATTACH",
            "reason": "user attach"
        }))
        .unwrap();
        assert_eq!(evt.event, Event::Attach);
        assert_eq!(evt.reason.as_deref(), Some("user attach"));

        // reason 缺省合法
        let bare: HostLifecycleEvt =
            serde_json::from_value(serde_json::json!({ "event": "HEALTH_OK" })).unwrap();
        assert_eq!(bare.event, Event::HealthOk);
        assert!(bare.reason.is_none());

        // 生命周期"状态"线名不是"事件"线名：必须确定性拒绝
        // （这正是本轮修掉的应用层断链：插件上报事件，状态由宿主单一写入 §4.3）
        assert!(
            serde_json::from_value::<HostLifecycleEvt>(serde_json::json!({ "event": "RUNNING" }))
                .is_err(),
            "状态线名不得被当作生命周期事件接受"
        );
    }

    #[test]
    fn events_publish_evt_matches_ts_call_site() {
        let evt: HostEventPublish = serde_json::from_value(serde_json::json!({
            "topic": "com.example.x.ready",
            "payload": { "ok": true }
        }))
        .unwrap();
        assert_eq!(evt.topic, "com.example.x.ready");
        assert_eq!(evt.payload, serde_json::json!({ "ok": true }));
    }

    #[test]
    fn subscribe_selectors_match_ts_call_site() {
        let subs: Vec<HostEventSelector> = serde_json::from_value(serde_json::json!([
            { "topic": "com.example.x.ready" }
        ]))
        .unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].topic, "com.example.x.ready");
    }

    #[test]
    fn admin_op_matches_ts_call_site_and_d15_kebab_enum() {
        for (wire, expected) in [
            ("disable", RegistryAdminOp::Disable),
            ("enable", RegistryAdminOp::Enable),
            ("uninstall", RegistryAdminOp::Uninstall),
            ("purge", RegistryAdminOp::Purge),
        ] {
            let op: HostAdminOp = serde_json::from_value(serde_json::json!({
                "op": wire,
                "id": "com.example.x"
            }))
            .expect("TS registryAdmin 的 op 形状必须可反序列化");
            assert_eq!(op.op, expected);
            assert_eq!(op.id, "com.example.x");
        }
        // D15：枚举定稿，非法操作名必须拒绝
        assert!(
            serde_json::from_value::<HostAdminOp>(serde_json::json!({ "op": "delete", "id": "x" }))
                .is_err()
        );
    }

    #[test]
    fn scope_accepts_only_declared_values() {
        assert!(scope_is_valid(None), "缺省 = visible");
        assert!(scope_is_valid(Some("visible")));
        assert!(scope_is_valid(Some("public")));
        assert!(!scope_is_valid(Some("all")), "未知 scope 必须拒绝");
        assert!(!scope_is_valid(Some("")));
    }

    #[test]
    fn registry_list_rejects_unknown_scope_before_touching_registry() {
        let state = CommandState::new();
        let err = wire_registry_list(&state, Some("com.a"), "plugin-com.a", Some("all"))
            .expect_err("未知 scope 必须拒绝");
        assert_eq!(err.code, tauron_host::ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn registry_admin_maps_id_and_op_onto_core() {
        let state = CommandState::new();
        let op = HostAdminOp {
            op: RegistryAdminOp::Disable,
            id: "com.example.x".to_string(),
        };
        // 未注册插件 → 核心以"未知插件"拒绝，证明 id/op 已正确映射到核心参数
        // （R7 收口后须带主体：这里用主窗，绕开身份判定、只验映射）。
        let main = crate::Caller::MainWindow;
        let err = wire_registry_admin(&main, &state, &op).expect_err("未注册插件不可管理");
        assert_eq!(err.code, tauron_host::ErrorCode::E_UNKNOWN_PLUGIN);
    }

    #[test]
    fn empty_selector_list_is_rejected() {
        let state = CommandState::new();
        let err = wire_events_subscribe(&state, "com.a", "plugin-com.a", &[])
            .expect_err("空选择器列表必须拒绝");
        assert_eq!(err.code, tauron_host::ErrorCode::E_AUTH_DENIED);
    }

    fn declare_self_topics(state: &CommandState, publisher: &str, topics: &[&str]) {
        let decls: Vec<EventDecl> = topics
            .iter()
            .map(|t| EventDecl {
                topic: (*t).to_string(),
                public: false,
            })
            .collect();
        state
            .bus
            .lock()
            .declare_topics(publisher, &decls)
            .expect("topic 声明应成功");
    }

    #[test]
    fn single_selector_subscribe_passes_core_token_through() {
        let state = CommandState::new();
        declare_self_topics(&state, "com.a", &["plugin:com.a:x"]);
        let sub = vec![HostEventSelector {
            topic: "plugin:com.a:x".to_string(),
        }];
        let out = wire_events_subscribe(&state, "com.a", "plugin-com.a", &sub).unwrap();
        assert!(
            !out.token.starts_with("grp:"),
            "单选择器必须透传核心 token（与核心行为逐一一致）"
        );
        assert_eq!(out.selectors.len(), 1);
        assert!(state.bus.lock().has_hanging_subscriptions("com.a"));
        wire_events_unsubscribe(&state, &out.token).unwrap();
        assert!(!state.bus.lock().has_hanging_subscriptions("com.a"));
    }

    #[test]
    fn multi_selector_subscribe_groups_and_unsubscribes_all() {
        let state = CommandState::new();
        declare_self_topics(&state, "com.a", &["plugin:com.a:x", "plugin:com.a:y"]);
        let sub = vec![
            HostEventSelector {
                topic: "plugin:com.a:x".to_string(),
            },
            HostEventSelector {
                topic: "plugin:com.a:y".to_string(),
            },
        ];
        let out = wire_events_subscribe(&state, "com.a", "plugin-com.a", &sub).unwrap();
        assert!(out.token.starts_with("grp:"), "多选择器必须返回分组 token");
        assert_eq!(out.selectors.len(), 2);
        assert!(
            state.bus.lock().has_hanging_subscriptions("com.a"),
            "两个 topic 都应完成订阅"
        );

        wire_events_unsubscribe(&state, &out.token).unwrap();
        assert!(
            !state.bus.lock().has_hanging_subscriptions("com.a"),
            "分组退订必须退净全部成员（§8-3 零悬挂订阅）"
        );
        assert!(
            state.subscription_groups.lock().is_empty(),
            "分组登记必须清理，不留孤儿条目"
        );
    }

    #[test]
    fn multi_selector_partial_failure_rolls_back_created_tokens() {
        // 断链回归：多选择器是「全成或全败」语义。第 2 个选择器失败时，第 1 个
        // 的 token 已经发不出去（调用方只收到 Err）——不回滚就是永久悬挂订阅，
        // 没人能退订它。
        let state = CommandState::new();
        declare_self_topics(&state, "com.a", &["plugin:com.a:x"]);
        let sub = vec![
            HostEventSelector {
                topic: "plugin:com.a:x".to_string(),
            },
            // 未声明的 topic → 授权拒绝，触发第 2 个选择器失败。
            HostEventSelector {
                topic: "plugin:com.b:missing".to_string(),
            },
        ];
        wire_events_subscribe(&state, "com.a", "plugin-com.a", &sub)
            .expect_err("第 2 个选择器失败必须整体失败");
        assert!(
            !state.bus.lock().has_hanging_subscriptions("com.a"),
            "部分失败必须回滚已建成的订阅"
        );
        assert!(
            state.subscription_groups.lock().is_empty(),
            "失败不得留下分组登记"
        );
    }

    #[test]
    fn window_cleanup_prunes_subscription_groups() {
        // 断链回归：核心订阅被 dispose 掉之后，分组登记若不跟着回收就是指向
        // 已消亡 token 的死条目，随开/关窗循环无限累积（§8-3）。
        let state = CommandState::new();
        declare_self_topics(&state, "com.a", &["plugin:com.a:x", "plugin:com.a:y"]);
        let sub = vec![
            HostEventSelector {
                topic: "plugin:com.a:x".to_string(),
            },
            HostEventSelector {
                topic: "plugin:com.a:y".to_string(),
            },
        ];
        let out = wire_events_subscribe(&state, "com.a", "plugin-com.a", &sub).unwrap();
        assert!(out.token.starts_with("grp:"));
        // 另一订阅者的分组（直接登记）：不得被连坐回收。
        state.subscription_groups.lock().insert(
            "grp:other".to_string(),
            crate::GroupSubscription {
                subscriber: "com.b".to_string(),
                tokens: vec!["t.other".to_string()],
            },
        );

        cleanup_closed_window(&state, "plugin-com.a");

        assert!(
            !state.bus.lock().has_hanging_subscriptions("com.a"),
            "关窗必须清核心订阅"
        );
        let groups = state.subscription_groups.lock();
        assert!(
            !groups.contains_key(&out.token),
            "关窗必须回收本订阅者的分组登记"
        );
        assert!(
            groups.contains_key("grp:other"),
            "不得连坐回收其它订阅者的分组"
        );
    }

    #[test]
    fn contributes_register_binds_identity_to_label() {
        // 断链回归：此前命令收 `plugin_id: String` 入参、核心又盲信
        // `entry.plugin_id`——self 档没有绑 label，任何插件都能把贡献署名到
        // 别的插件（并在对方卸载时被 clear_plugin 误删）。线形现在不含
        // pluginId，身份只从 label 来。
        let state = CommandState::new();
        let entry = ContributeEntryInput {
            kind: "menu".to_string(),
            id: "com.a.menu".to_string(),
            label: "A".to_string(),
        };

        // 插件窗口：身份 = label，注册成功且署名不可伪造。
        wire_contributes_register(&state, "plugin-com.a", &entry).unwrap();
        let list = crate::cmd_contributes_list(&state, None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].plugin_id, "com.a", "署名必须来自 label 而非入参");

        // 主窗等非插件 label：贡献注册是插件 attach 期行为，硬拒。
        let err = wire_contributes_register(&state, "main", &entry)
            .expect_err("非插件 label 不得注册贡献");
        assert_eq!(err.code, tauron_host::ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn subscribed_topics_of_filters_by_window_and_dedups() {
        let state = CommandState::new();
        declare_self_topics(&state, "com.a", &["plugin:com.a:x", "plugin:com.a:y"]);
        let bus = state.bus.lock();
        // 同窗口两次订阅同一 topic（幂等命中），另一窗口订阅另一个
        bus.subscribe("com.a", "w1", "plugin:com.a:x").unwrap();
        bus.subscribe("com.a", "w1", "plugin:com.a:x").unwrap();
        bus.subscribe("com.a", "w2", "plugin:com.a:y").unwrap();
        assert_eq!(
            bus.subscribed_topics_of("com.a", "w1"),
            vec!["plugin:com.a:x".to_string()],
            "必须按窗口过滤并去重"
        );
        assert_eq!(
            bus.subscribed_topics_of("com.a", "w2"),
            vec!["plugin:com.a:y".to_string()]
        );
        assert!(
            bus.subscribed_topics_of("com.b", "w1").is_empty(),
            "他人订阅不得被计入"
        );
    }

    // ── 窗口 label ↔ 插件身份（一插件一 webview 约定）──────────────

    /// label 必须是 `plugin-<插件id>`：身份直接从 label 解析（§2.1）。
    #[test]
    fn window_label_is_plugin_identity() {
        assert_eq!(plugin_id_of("plugin-com.example.a"), "com.example.a");
        // 非插件窗口（主窗）原样透传；它不是合法插件身份（见下一个用例）。
        assert_eq!(plugin_id_of("main"), "main");
    }

    /// 窗口销毁回收用 label 直接推出插件 id，因此**不需要**「最后窗口」判定：
    /// 带后缀的子窗口 label 解析出的 id 非法（含 `:`），不在身份模型内，
    /// 本就无法建立订阅。
    #[test]
    fn suffixed_child_window_label_is_not_a_valid_identity() {
        let derived = plugin_id_of("plugin-com.example.a:settings");
        assert_eq!(derived, "com.example.a:settings");
        assert!(
            tauron_host::manifest::PluginId::new(derived).is_err(),
            "子窗口 label 不构成合法插件身份（一插件一 webview）"
        );
        // 裸 label（主窗）同样非法 → `cleanup_closed_window` 里 call_end_all 被跳过。
        assert!(tauron_host::manifest::PluginId::new(plugin_id_of("main")).is_err());
    }

    // ──────────────────────────────────────────────────────────────────────
    // R8 §2/§3/§4：装配器执行口径 + 新命令的**线形**（字段名逐字断言）
    //
    // 这些用例赌的是**跨语言契约**：Rust 侧字段名/形状一变，它们立刻失败。
    // TS 侧对应的读取点：
    //   · `packages/tauron-host/src/tauri-backend.ts`（命令名 + 返回值形状）
    //   · `packages/tauron-host/src/shell-client.ts`（market check/download）
    // ──────────────────────────────────────────────────────────────────────

    /// 线形字段名集合（**排序后**比较）。
    ///
    /// 为什么排序：JSON 对象的键序不是契约的一部分（`serde_json` 是否启用
    /// `preserve_order` 是编译期 feature，前端也不依赖键序）。**键集合**才是契约，
    /// 这里断言集合相等 —— 多一个字段、少一个字段都会失败。
    fn wire_keys(json: &serde_json::Value) -> Vec<String> {
        let mut keys: Vec<String> = json
            .as_object()
            .unwrap_or_else(|| panic!("线形必须是对象：{json}"))
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    }

    /// 装配器宏展开后的执行口径①：`Ok` 原样透传（不做任何包装/改名）。
    #[test]
    fn host_command_passes_ok_through_unchanged() {
        let out = host_command("my-plugin|my_stats", || Ok(serde_json::json!({ "n": 1 })))
            .expect("Ok 必须原样透传");
        assert_eq!(out, serde_json::json!({ "n": 1 }));
    }

    /// 执行口径②：`HostError` 必须过桥成**结构化** `InvokeError`（JSON 对象带
    /// `code`/`message`/`retryable`），而不是 `to_string()` 转储——前者的
    /// `normalizeError` 能还原 message，后者只能正则反抠。
    #[test]
    fn host_command_bridges_host_error_as_structured_invoke_error() {
        let err = host_command::<(), _>("my-plugin|my_stats", || {
            Err(HostError::new(ErrorCode::E_AUTH_DENIED, "插件不得开窗"))
        })
        .expect_err("Err 必须保持为错误");

        // `InvokeError` 是 `pub struct InvokeError(pub serde_json::Value)`（tauri 2.11
        // `src/ipc/mod.rs:224`）：结构化过桥的产物就是 `HostError` 的序列化对象本身。
        let json = err.0.clone();
        assert_eq!(
            json["code"],
            serde_json::json!("E_AUTH_DENIED"),
            "错误码必须是字段（前端按 `code.startsWith('E_')` 分流）：{json}"
        );
        assert_eq!(json["message"], serde_json::json!("插件不得开窗"));
        assert!(json.get("retryable").is_some(), "retryable 必须在线形里：{json}");
    }

    /// 执行口径③：panic 必须变成 `E_HOST_PANIC`（Tauri 不捕获 panic，漏了这一步
    /// 就是 JS 侧 `invoke()` 永久挂起），且诊断名里带插件与命令名。
    #[test]
    fn host_command_turns_a_panic_into_e_host_panic_naming_plugin_and_command() {
        let err = host_command::<(), _>("my-plugin|my_stats", || panic!("业务体炸了"))
            .expect_err("panic 不得逃逸成挂起的 invoke");

        let json = err.0.clone();
        assert_eq!(json["code"], serde_json::json!("E_HOST_PANIC"));
        let message = json["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("my-plugin|my_stats"),
            "诊断必须指出是哪个插件的哪条命令：{message}"
        );
        assert!(message.contains("业务体炸了"), "panic 载荷必须保留：{message}");
    }

    /// 通知事件载荷**不得带正文/附带数据**（轮 11 审计修正）。
    ///
    /// 这条不是格式偏好：`Manager::emit` 的语义是"发给**所有** target"，而多插件宿主里
    /// 每个插件都有自己的 webview —— 载荷里一旦有 `title`/`message`/`data`，宿主与别的
    /// 插件的通知内容（可能含跳转 token）就进入了每一个插件 webview 的回调。
    /// 正文的唯一出口是按身份过滤的 `host_notifications_list`。
    #[test]
    fn notification_event_payload_carries_no_body() {
        let entry = tauron_notify::NotifyEntry {
            id: "n1".to_string(),
            plugin_id: "p.a".to_string(),
            kind: tauron_notify::NotifyKind::Info,
            title: "标题不该出现在广播里".to_string(),
            message: "reset-token=secret".to_string(),
            ts: 7,
            read: false,
            data: serde_json::json!({ "url": "https://evil.example/steal" }),
        };
        let payload = notification_payload(&entry);
        let obj = payload.as_object().expect("载荷必须是 JSON 对象");
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["id", "kind", "pluginId", "ts"],
            "广播载荷的键集合变了：正文与附带数据一律不得进广播"
        );
        let text = payload.to_string();
        assert!(!text.contains("secret"), "正文泄露进广播载荷：{text}");
        assert!(
            !text.contains("evil.example"),
            "附带数据（跳转载荷）泄露进广播载荷：{text}"
        );
    }

    /// `host_market_check` 线形：字段名与数量**逐字**钉住（R8 §4）。
    #[test]
    fn market_check_wire_shape_is_exact() {
        let state = CommandState::new();
        let json = serde_json::to_value(crate::cmd_market_check(&state).unwrap()).unwrap();

        assert_eq!(
            wire_keys(&json),
            vec!["available", "reason", "simulated", "version"],
            "{json}"
        );
        assert_eq!(json["available"], serde_json::json!(false));
        assert_eq!(
            json["simulated"],
            serde_json::json!(true),
            "模拟必须是线字段，否则前端无法据字段判断这是桩结果"
        );
        assert!(json["reason"].as_str().is_some_and(|r| r.contains("未接入更新源")));

        // 同时必须能被反序列化（TS 侧形状漂移时这里会失败）。
        let back: crate::MarketCheckResult = serde_json::from_value(json).unwrap();
        assert!(back.simulated && !back.available && back.version.is_none());
    }

    /// `host_market_download` / `install` 线形：同一形状，`simulated` 恒 true。
    #[test]
    fn market_update_wire_shape_is_exact() {
        let state = CommandState::new();
        for json in [
            serde_json::to_value(crate::cmd_market_download(&state, Some("2.0.0")).unwrap()).unwrap(),
            serde_json::to_value(crate::cmd_market_install(&state, Some("2.0.0")).unwrap()).unwrap(),
        ] {
            assert_eq!(wire_keys(&json), vec!["ok", "reason", "simulated", "version"], "{json}");
            assert_eq!(json["ok"], serde_json::json!(true));
            assert_eq!(json["simulated"], serde_json::json!(true));
            assert_eq!(json["version"], serde_json::json!("2.0.0"));
            assert!(json["reason"].as_str().is_some_and(|r| r.contains("模拟")));
        }
    }

    /// `WindowCreateRequest` 是 **camelCase** 线形：`pluginId` 必须被接受，
    /// `plugin_id` 必须**不被**接受（名称写错 = 静默用默认值/缺参失败，门禁看不见）。
    #[test]
    fn window_create_request_wire_is_camel_case_only() {
        let ok: crate::WindowCreateRequest =
            serde_json::from_value(serde_json::json!({ "pluginId": "com.a" }))
                .expect("TS 侧发送 pluginId");
        assert_eq!(ok.plugin_id, "com.a");
        assert!(ok.title.is_none() && ok.width.is_none() && ok.height.is_none());

        let full: crate::WindowCreateRequest = serde_json::from_value(
            serde_json::json!({ "pluginId": "com.a", "title": "A", "width": 640, "height": 480 }),
        )
        .unwrap();
        assert_eq!(full.width, Some(640));

        assert!(
            serde_json::from_value::<crate::WindowCreateRequest>(
                serde_json::json!({ "plugin_id": "com.a" })
            )
            .is_err(),
            "snake_case 必须失败（线形只有 camelCase）"
        );
    }

    /// 两条新命令的**返回线形**逐字钉住。
    #[test]
    fn new_window_command_outcomes_have_exact_wire_shapes() {
        let create = serde_json::to_value(crate::WindowCreateOutcome {
            label: "plugin-com.a".to_string(),
            plugin_id: "com.a".to_string(),
            created: false,
            reason: Some("没有创建任何窗口".to_string()),
        })
        .unwrap();
        assert_eq!(wire_keys(&create), vec!["created", "label", "pluginId", "reason"], "{create}");
        assert_eq!(create["pluginId"], serde_json::json!("com.a"));

        let relaunch = serde_json::to_value(crate::WindowRelaunchOutcome {
            reconcile: crate::PhaseReconcileOutcome {
                scanned: 1,
                entered: 2,
                exited: 3,
                ignored: 4,
            },
            relaunch_requested: true,
            reason: None,
        })
        .unwrap();
        assert_eq!(
            wire_keys(&relaunch),
            vec!["reason", "reconcile", "relaunchRequested"],
            "{relaunch}"
        );
        assert_eq!(relaunch["relaunchRequested"], serde_json::json!(true));
        assert_eq!(
            relaunch["reconcile"],
            serde_json::json!({ "scanned": 1, "entered": 2, "exited": 3, "ignored": 4 }),
            "对账结果必须是对象（顺序证据随返回值可见）"
        );

        // 降级路径的形状与成功路径**同形**，只有取值不同——前端不必写两套解析。
        let degraded = serde_json::to_value(crate::WindowRelaunchOutcome {
            reconcile: crate::PhaseReconcileOutcome::default(),
            relaunch_requested: false,
            reason: Some("无重启原语".to_string()),
        })
        .unwrap();
        assert_eq!(wire_keys(&degraded), wire_keys(&relaunch));
    }
}

