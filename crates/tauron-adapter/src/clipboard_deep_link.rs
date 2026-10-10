// T-7 拆分：从 `lib.rs` 逐字节纯搬移的剪贴板 + 深链接命令族。
//
// 这两族共享 `SubstrateState::shell_ext`（进程内状态槽），但 `ShellExtState` 结构体
// 本体与窗口几何持久化（`cmd_window_set_position/set_size`）仍留在 `lib.rs`——剪贴板
// 命令只经 `state.shell_ext` 字段访问、不直接点名 `ShellExtState`。
//
// `use super::*;` 令搬来的命令体原样调用 crate-root 私有助手（guard、require_main_window、
// unsupported_body 等）与顶层 use 导入；两条 `pub const DEEP_LINK_*` 与 `deep_link_delivered`
// 一并搬来并按原名再导出，保 `crate::DEEP_LINK_TOPIC`（tauri.rs 投递腿）解析不变。

use super::*;

/// 写入进程内缓冲区并明示系统剪贴板不支持。
pub fn cmd_clipboard_write(state: &SubstrateState, text: String) -> HostResult<UnsupportedBody> {
    guard("clipboard_write", || {
        state.shell_ext.lock().clipboard = text;
        Ok(unsupported_body(
            "system clipboard provider is not configured",
            Some("in-process-buffer"),
        ))
    })?
}

/// 读取进程内缓冲区并明示系统剪贴板不支持。
pub fn cmd_clipboard_read(state: &SubstrateState) -> HostResult<DegradedValue<String>> {
    guard("clipboard_read", || {
        Ok(DegradedValue {
            supported: false,
            reason: "system clipboard provider is not configured".to_string(),
            fallback: "in-process-buffer".to_string(),
            value: state.shell_ext.lock().clipboard.clone(),
        })
    })?
}

/// 剪贴板族（读写）的**带身份判定**版本（轮 12）。
///
/// # 为什么这两条是主窗专属
///
/// 现状的"进程内缓冲区"是 `shell_ext.clipboard` 这**一个**全局槽位，不分主体：
/// 任何插件 `host_clipboard_write` 就能覆盖别人刚写的内容（下一位读者拿到的是它的
/// 值），`host_clipboard_read` 就能读到别的插件刚放进去的内容。也就是说它虽然
/// **降级**（`supported: false`），却仍是**跨主体的共享可变状态**——和
/// `host_notifications_*` 在轮 11 的处理理由同型。
///
/// 判定选择"仅主窗"而不是"按插件分槽"：剪贴板的语义是**用户级**的跨应用粘贴面
/// （`@tauron/host` 的 `ClipboardClient` 由主窗外壳使用），插件侧没有"我的剪贴板"
/// 这种合法用法；真要给插件用，正确形态是每主体独立槽 + 显式共享，那是新功能，
/// 不是把越权面留在原地冒充降级实现。
///
/// 接原生 provider 后判定同样够用：那时它变成"读/写用户系统剪贴板"，插件能碰等于
/// 任何插件都能窃听与改写用户剪贴板，比现在更严重。
pub fn cmd_clipboard_read_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<DegradedValue<String>> {
    require_main_window(caller, "host_clipboard_read")?;
    cmd_clipboard_read(state)
}

/// `host_clipboard_write` 的带身份判定版本，理由同 [`cmd_clipboard_read_as`]。
pub fn cmd_clipboard_write_as(
    caller: &Caller,
    state: &SubstrateState,
    text: String,
) -> HostResult<UnsupportedBody> {
    require_main_window(caller, "host_clipboard_write")?;
    cmd_clipboard_write(state, text)
}

/// `host_deep_link_register` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// # R8 曾把它当 self-service——那是错的，这里是修正
///
/// 它写的不是"某个插件自己的东西"，而是**应用级**的两处共享状态：
///
/// 1. `shell_ext.deep_link_protocol`（整个应用当前认哪个协议）——插件换值就是
///    **把整个应用的深链接入口改到自己名下**；
/// 2. `deep-link` 公共 topic 的声明（EventBus 的授权前提）——换值会连带
///    [`cmd_deep_link_register`] 的"先注销旧值"路径，把**上一个协议**从 OS 关联里
///    摘掉（`unregister(old)`）。
///
/// 组合起来的真实攻击面：插件 A 调一次 `register("evil")`，应用的深链接入口就被
/// 改成 `evil://`，用户点原本的 `tauron://` 链接**不再拉起本应用**（旧关联已被注销），
/// 而 A 可以用自己的协议接管后续 URL 投递（含带 token 的跳转链接）。这是跨插件/
/// 应用级的越权面，不是"插件配置自己的协议"。
///
/// 主窗是唯一合法主体。拒绝码复用既有 [`ErrorCode::E_AUTH_DENIED`]，判定在
/// **任何副作用之前**：拒绝时 `shell_ext.deep_link_protocol` 与 topic 声明、
/// 以及 sink 的 `unregister` 一律不动（有测试断言协议值逐字不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_deep_link_register_as(
    caller: &Caller,
    state: &SubstrateState,
    protocol: String,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_deep_link_register")?;
    cmd_deep_link_register(state, protocol)
}

/// `host_deep_link_register`：注册深链接协议。
///
/// # 这条命令做两件事，其中**只有一件是真的平台事**
///
/// 1. **应用层（真实）**：记录协议到 `shell_ext`，并声明 `deep-link` 公共 topic——
///    这是前端 `subscribe` 与 [`deep_link_delivered`] 投递的授权前提（EventBus 要求
///    先声明后订阅/发布）。这一步任何时候都会发生。
/// 2. **平台层（走 sink）**：转调 [`SubstrateState::deep_link_sink`]。
///    ⚠️ **本仓的 sink 实现全部是降级路径**（`tauri-plugin-deep-link` 不在依赖闭包
///    内）：`register` 只记账并（Tauri 实现下）向前端发一个"未做 OS 注册"的信号，
///    `native_supported()` 恒 `false`。也就是说：**OS 不会把 `tauron://` 交给本应用**，
///    本命令只是让"URL 到了宿主之后"的那一段可用（谁把 URL 送进来是宿主自己的事，
///    例如命令行/单实例转发）。
///
/// # 为什么换协议时要先注销（`unregister` 的真实消费者）
///
/// 协议名是 OS 关联的键：`tauron` → `other` 的换名如果不注销旧值，系统里会留下
/// 旧协议的关联（用户点旧链接仍会拉起本应用，而应用层已按新协议解析）。
/// 因此核心在写新值之前先对旧值调 `unregister`——这不是预留接口。
///
/// **入口**：wire 层转调 [`cmd_deep_link_register_as`]（仅主窗：注册的是**应用级**
/// 协议，插件改它 = 把整个应用的深链接入口改到自己名下），本函数是**不过身份**的核心。
pub fn cmd_deep_link_register(
    state: &SubstrateState,
    protocol: String,
) -> HostResult<ProviderResult<()>> {
    guard("deep_link_register", || {
        // 空协议不是一个"稍微不对"的协议名，而是一次无意义的 OS 注册请求。
        if protocol.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "深链接协议不能为空（调用方可能传了 undefined/null）".to_string(),
            ));
        }
        let previous = state.shell_ext.lock().deep_link_protocol.clone();
        if let Some(old) = previous.filter(|old| old != &protocol) {
            state.deep_link_sink.unregister(&old)?;
        }
        {
            let mut ext = state.shell_ext.lock();
            ext.deep_link_protocol = Some(protocol.clone());
        }
        // 声明 `deep-link` 公共 topic：这是前端 subscribe 与
        // deep_link_delivered 投递的授权前提（EventBus 要求先声明后订阅/发布）。
        {
            let bus = state.bus.lock();
            bus.declare_topics(
                DEEP_LINK_PUBLISHER,
                &[EventDecl { topic: DEEP_LINK_TOPIC.to_string(), public: true }],
            )?;
        }
        // 平台侧注册放在最后：应用层状态与 topic 都到位之后才谈"让 OS 认这个协议"。
        state.deep_link_sink.register(&protocol)?;
        if state.deep_link_sink.native_supported() {
            Ok(ProviderResult::Value(()))
        } else {
            Ok(ProviderResult::Unsupported(unsupported_body(
                "OS deep-link provider is not configured",
                Some("internal-event-routing"),
            )))
        }
    })?
}

/// 深链接事件的 topic 与伪发布者（框架内部源，非插件）。
pub const DEEP_LINK_TOPIC: &str = "deep-link";
pub const DEEP_LINK_PUBLISHER: &str = "core.deep-link";

/// 深链接到达入口（OS 协议回调侧 glue 调用点）。
///
/// 生产接线：`tauri-plugin-deep-link` 的回调里调用本函数，把收到的 URL
/// 发布到 EventBus 的 `deep-link` 主题；前端 `DeepLinkClient.subscribe()`
/// 即在该 URL 上收到（含 parseUrl 结构化解析）。
///
/// 这不是一条新 IPC 命令（JS 不可调用，防伪造）——只供 Rust 侧回调使用。
pub fn deep_link_delivered(state: &SubstrateState, url: &str) -> HostResult<()> {
    guard("deep_link_delivered", || {
        let payload = serde_json::json!({
            "url": url,
            "protocol": state.shell_ext.lock().deep_link_protocol.clone(),
        });
        let bus = state.bus.lock();
        // Event 通道：与前端 backend.listen('deep-link') 的 drain 通道一致。
        let res = bus.publish(DEEP_LINK_PUBLISHER, DEEP_LINK_TOPIC, payload, ChannelKind::Event);
        if res.dropped {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                "deep-link topic 未声明：须先调用 host_deep_link_register",
            ));
        }
        Ok(())
    })?
}
