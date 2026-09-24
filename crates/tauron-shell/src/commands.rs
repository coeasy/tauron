//! Tauri 命令层（设计文档 §2.1）——**薄适配器**
//!
//! 仅在 `tauri` feature 下编译。这里只做两件事：
//! 1. 从 Tauri `State` 取出 [`HostState`]；
//! 2. 把参数转交给 [`crate::dispatch`] 的纯逻辑函数。
//!
//! 所有业务判断（信封校验、注册表状态、分发）都在 `dispatch` 模块，
//! 因此核心链路可在不启用本 feature 时被完整测试（设计文档 §1.1/§8）。
//!
//! 命令名与 TS `@tauron/core` 的 `TAURON_COMMANDS` 一一对应，
//! 由跨语言契约门禁锁定：
//!
//! | Tauri 命令 | TS 常量 | 语义 |
//! |---|---|---|
//! | `plugin_invoke` | `TAURON_COMMANDS.invoke` | 信封调用 |
//! | `plugin_cancel` | `TAURON_COMMANDS.cancel` | 取消（幂等） |
//! | `plugin_emit` | `TAURON_COMMANDS.emit` | 发布事件 |

use tauri::{Emitter, Manager, Runtime, State};

use crate::dispatch::{EventSink, HostState};
use crate::envelope::{
    PluginCancelRequest, PluginErrorBody, PluginInvokeRequest, PluginInvokeResponse, ProgressEvent,
};
use crate::eventbus::Event;

/// 把事件投递到 **Tauri 事件系统**。
///
/// 前端通过 `@tauri-apps/api` 的 `listen(topic, ...)`（即 `@tauron/core`
/// `createTauriBackend().listen()`）接收。没有这一步，`plugin_emit`
/// 只会进入 Rust 内部总线，前端永远收不到任何插件事件。
///
/// 句柄在 setup 阶段捕获并存入自身（`AppHandle<R>` 是 `Send + Sync`），
/// 因此命令可运行在任意工作线程。投递失败（未初始化/无窗口）时静默
/// 丢弃——事件是可丢的（设计文档 §2.3）。
struct TauriEventSink<R: Runtime> {
    app: std::sync::OnceLock<tauri::AppHandle<R>>,
}

impl<R: Runtime> TauriEventSink<R> {
    fn new() -> Self {
        Self { app: std::sync::OnceLock::new() }
    }

    fn attach(&self, app: &tauri::AppHandle<R>) {
        let _ = self.app.set(app.clone());
    }
}

impl<R: Runtime> EventSink for TauriEventSink<R> {
    fn deliver(&self, topic: &str, event: &Event) {
        if let Some(app) = self.app.get() {
            let _ = app.emit(topic, event);
        }
    }
}

/// 信封调用：前端唯一的主调用入口。
///
/// `channel` 为可选的进度通道（设计文档 §2.1）。分发器
/// [`crate::dispatch::PluginDispatcher::dispatch`] 是**同步请求/响应**，
/// 不产出细粒度步骤，因此这里投递的是**调用级粗粒度帧**：
/// 开始（`0/1`、0%、`started`）与终态（`1/1`、100%、`completed`/`failed`）。
///
/// 通道类型是 [`tauri::ipc::JavaScriptChannelId`] 而非
/// `Channel<ProgressEvent>`：`Channel<T>` 只实现了 `CommandArg`（未实现
/// `Deserialize`），**`Option<Channel<T>>` 无法编译**；`JavaScriptChannelId`
/// 可反序列化，取到后再用调用方 webview 转成真实通道。这样通道缺省时
/// 前端连 key 都不发，宿主零开销（若改成必填裸通道，每次调用都要白投两帧）。
///
/// 通道已关闭时静默丢弃——进度是可丢的（设计文档 §2.3）。
///
/// 没有这一层，前端 `createTauriBackend().invoke(…, onProgress)` 建立的
/// 通道永远收不到消息：TS 侧 `channel.onmessage` 是**有产者才成立**的接线。
#[tauri::command]
pub fn plugin_invoke<R: Runtime>(
    state: State<'_, HostState>,
    webview: tauri::Webview<R>,
    request: PluginInvokeRequest,
    channel: Option<tauri::ipc::JavaScriptChannelId>,
) -> PluginInvokeResponse {
    let channel: Option<tauri::ipc::Channel<ProgressEvent>> =
        channel.map(|id| id.channel_on::<R, ProgressEvent>(webview));

    invoke_with_progress(request, channel.as_ref(), |req| state.handle_invoke(req))
}

/// [`plugin_invoke`] 的可测内核：把「投递进度帧」与「执行分发」解耦。
///
/// 抽出来是为了让帧投递能在**不构造 Tauri 运行时**的前提下被断言
/// （`State<'_, _>` 无法在单测里构造）——与 `tauron-adapter` 的
/// `wire_*` 薄包装同一模式。
pub fn invoke_with_progress<F>(
    request: PluginInvokeRequest,
    channel: Option<&tauri::ipc::Channel<ProgressEvent>>,
    run: F,
) -> PluginInvokeResponse
where
    F: FnOnce(PluginInvokeRequest) -> PluginInvokeResponse,
{
    let call_id = request.call_id.clone();
    send_progress(channel, &call_id, 0, "started", 0);

    let response = run(request);

    let message = if response.ok { "completed" } else { "failed" };
    send_progress(channel, &response.call_id, 1, message, 100);

    response
}

/// 投递一条粗粒度进度帧；通道缺省或已关闭时静默忽略。
fn send_progress(
    channel: Option<&tauri::ipc::Channel<ProgressEvent>>,
    call_id: &str,
    step: u64,
    message: &str,
    percentage: u8,
) {
    if let Some(ch) = channel {
        let _ = ch.send(ProgressEvent {
            call_id: call_id.to_string(),
            step,
            total: 1,
            message: message.to_string(),
            percentage,
        });
    }
}

/// 取消进行中的调用。幂等：未命中返回 `false` 而非错误。
#[tauri::command]
pub fn plugin_cancel(state: State<'_, HostState>, request: PluginCancelRequest) -> bool {
    state.handle_cancel(request)
}

/// 发布插件事件（topic 形如 `plugin:<id>:<event>`）。
#[tauri::command]
pub fn plugin_emit(
    state: State<'_, HostState>,
    topic: String,
    payload: serde_json::Value,
) -> Result<(), PluginErrorBody> {
    state.handle_emit(&topic, payload)
}

/// 生成命令处理表，供宿主 `invoke_handler` 使用。
///
/// ```rust,ignore
/// tauri::Builder::default()
///     .plugin(tauron_shell::commands::init())
///     .run(tauri::generate_context!())
/// ```
#[macro_export]
macro_rules! tauron_generate_handler {
    () => {
        tauri::generate_handler![
            $crate::commands::plugin_invoke,
            $crate::commands::plugin_cancel,
            $crate::commands::plugin_emit,
        ]
    };
}

/// 在 Tauri Builder 中注册 [`HostState`] 与三条命令。
///
/// 参考 Tauri 官方插件的 `init()` 模式；宿主可用
/// [`init_with_state`] 注入预装载的注册表与分发器。
///
/// 注意：插件注册形态的命令经 `plugin:tauron-shell|<name>` 路由，Tauri v2
/// 对 `plugin:` 命令强制 capability/ACL。零配置的起步集成请改用
/// [`tauron_generate_handler!`] root 注册 + [`state_init`]（裸命令名，与
/// `tauron-core::createTauriBackend()` 直接匹配）。
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    init_with_state(HostState::new())
}

/// 仅注册 [`HostState`] 与事件出口（不注册命令）——配合
/// `tauron_generate_handler!` 的 root 注册使用。与 [`init`] 二选一，
/// 同时调用会重复 `manage::<HostState>` 而 panic。
pub fn state_init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    state_init_with_state(HostState::new())
}

/// 用给定的 [`HostState`] 初始化（预装载插件/分发器时使用）。
pub fn init_with_state<R: Runtime>(state: HostState) -> tauri::plugin::TauriPlugin<R> {
    let sink = build_state_plugin::<R>(&state);
    tauri::plugin::Builder::<R>::new("tauron-shell")
        .setup(move |app, _api| {
            sink.attach(app.app_handle());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauron_generate_handler!())
        .build()
}

/// [`state_init`] 的带状态版本（同 [`init_with_state`]，但不注册命令）。
pub fn state_init_with_state<R: Runtime>(state: HostState) -> tauri::plugin::TauriPlugin<R> {
    let sink = build_state_plugin::<R>(&state);
    tauri::plugin::Builder::<R>::new("tauron-shell-state")
        .setup(move |app, _api| {
            sink.attach(app.app_handle());
            app.manage(state);
            Ok(())
        })
        .build()
}

/// 共用装配：安装出站事件出口（若 setup 阶段未安装，前端将收不到任何事件）。
fn build_state_plugin<R: Runtime>(state: &HostState) -> std::sync::Arc<TauriEventSink<R>> {
    let sink = std::sync::Arc::new(TauriEventSink::<R>::new());
    state.set_event_sink(sink.clone());
    sink
}

#[cfg(test)]
mod progress_tests {
    use super::*;
    use crate::PluginErrorCode;
    use std::sync::{Arc, Mutex};

    /// 收集 channel 投递的帧（`Channel::new` 无需 Tauri 运行时）。
    fn collector() -> (Arc<Mutex<Vec<ProgressEvent>>>, tauri::ipc::Channel<ProgressEvent>) {
        let seen: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let channel = tauri::ipc::Channel::<ProgressEvent>::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(raw) = body {
                // 无法解析即视为测试失败信号：不静默跳过。
                let evt: ProgressEvent = serde_json::from_str(&raw).expect("进度帧必须是合法 JSON");
                sink.lock().expect("frame lock").push(evt);
            }
            Ok(())
        });
        (seen, channel)
    }

    fn request(call_id: &str) -> PluginInvokeRequest {
        PluginInvokeRequest {
            plugin_id: "com.example.x".to_string(),
            method: "run".to_string(),
            payload: None,
            call_id: call_id.to_string(),
            timeout_ms: None,
        }
    }

    /// 成功调用：投递「开始 + completed」两帧，且字段形态正确。
    #[test]
    fn progress_frames_are_sent_on_success() {
        let (seen, channel) = collector();

        let res = invoke_with_progress(request("c-1"), Some(&channel), |req| {
            PluginInvokeResponse::ok(req.call_id, serde_json::json!({ "ok": 1 }))
        });

        assert!(res.ok);
        let frames = seen.lock().expect("frame lock").clone();
        assert_eq!(frames.len(), 2, "开始帧 + 终态帧");

        assert_eq!(frames[0].call_id, "c-1");
        assert_eq!((frames[0].step, frames[0].total, frames[0].percentage), (0, 1, 0));
        assert_eq!(frames[0].message, "started");

        assert_eq!(frames[1].call_id, "c-1");
        assert_eq!((frames[1].step, frames[1].total, frames[1].percentage), (1, 1, 100));
        assert_eq!(frames[1].message, "completed");
    }

    /// 失败调用：终态帧 message 为 `failed`，且沿用响应里的 callId。
    #[test]
    fn progress_terminal_frame_reports_failure() {
        let (seen, channel) = collector();

        let res = invoke_with_progress(request("c-2"), Some(&channel), |_req| {
            PluginInvokeResponse::error(
                "c-2".to_string(),
                PluginErrorCode::InvalidPayload,
                "bad".to_string(),
            )
        });

        assert!(!res.ok);
        let frames = seen.lock().expect("frame lock").clone();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].message, "failed");
        assert_eq!(frames[1].percentage, 100);
    }

    /// 不传通道：不投递任何帧，且分发仍被调用一次（零行为改变）。
    #[test]
    fn no_channel_means_no_frames_and_dispatch_still_runs() {
        let calls = Arc::new(Mutex::new(0usize));
        let counter = calls.clone();

        let res = invoke_with_progress(request("c-3"), None, move |req| {
            *counter.lock().expect("counter lock") += 1;
            PluginInvokeResponse::ok(req.call_id, serde_json::Value::Null)
        });

        assert!(res.ok);
        assert_eq!(*calls.lock().expect("counter lock"), 1);
    }

    /// 帧的线名必须是 camelCase（TS `ProgressEvent` 逐字段消费）。
    #[test]
    fn progress_frame_wire_names_are_camel_case() {
        let evt = ProgressEvent {
            call_id: "c-4".to_string(),
            step: 1,
            total: 2,
            message: "half".to_string(),
            percentage: 50,
        };
        let json = serde_json::to_value(&evt).expect("serialize");
        let obj = json.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["callId", "message", "percentage", "step", "total"]);
    }
}
