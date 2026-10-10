// 对话框宿主能力（`host_dialog_*`）的命令实现（T-7 拆分：自 `lib.rs` 纯搬移）。
//
// 这里是文件对话框 / 消息框 / 确认框四类命令 + 其带身份判定（`*_as`）版本 +
// `kind` 闭集校验的唯一实现。之所以单独成模块而不是留在 `lib.rs`：V7 Batch 5'
// 的方向就是把各域逻辑按文件搬出去（`contracts/adapter-domain-ownership.json`
// 的 R5「god file 封顶」正是为此），而本域是**零语义变更的纯 move**——函数体、
// 文档注释、可见性（八个 `pub fn cmd_dialog_*` 对外、`validated_dialog_kind` 内部）
// 与拆前逐字节一致，crate 根通过 `mod dialog;` + `pub use dialog::{…}` 原样再导出，
// 因此 `crate::cmd_dialog_open_as`（tauri.rs 的接线）与测试里的裸名调用都不受影响。
//
// 依赖（`SubstrateState`/`DialogSink`/`ProviderResult`/`unsupported_body`/`guard`/
// `HostResult`/`HostError`/`ErrorCode`/`Caller`/`require_main_window`）全部来自 crate
// 根，故此处只需 `use super::*;`——子模块可见父模块的私有项，无需逐个 re-import。

use super::*;

/// `host_dialog_open`：打开文件/目录对话框。
///
/// 缺少原生 provider 时返回 `UnsupportedBody`，有 provider 时 `None` 才表示用户取消。
pub fn cmd_dialog_open(
    state: &SubstrateState,
    multiple: bool,
    directory: bool,
) -> HostResult<ProviderResult<Option<String>>> {
    guard("dialog_open", || {
        if !state.dialog_sink.native_supported() {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                "native dialog provider is not configured",
                None,
            )));
        }
        state.dialog_sink.open_file(multiple, directory).map(ProviderResult::Value)
    })?
}

/// `host_dialog_save`：保存文件对话框；缺少 provider 时返回 `UnsupportedBody`。
pub fn cmd_dialog_save(
    state: &SubstrateState,
    default_name: Option<&str>,
) -> HostResult<ProviderResult<Option<String>>> {
    guard("dialog_save", || {
        if !state.dialog_sink.native_supported() {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                "native dialog provider is not configured",
                None,
            )));
        }
        state.dialog_sink.save_file(default_name).map(ProviderResult::Value)
    })?
}

/// `host_dialog_message`：消息对话框。
///
/// `kind` 是**闭集**（`info` | `error` | `warning`，缺省 `info`）：R8 之前这个字段
/// 被整个忽略（写错了也没人知道），现在表外值直接拒绝
/// （[`ErrorCode::E_INVALID_MANIFEST`]，参数错用参数错码），不静默当成 `info`。
pub fn cmd_dialog_message(
    state: &SubstrateState,
    title: &str,
    message: &str,
    kind: Option<&str>,
) -> HostResult<ProviderResult<()>> {
    guard("dialog_message", || {
        let kind = validated_dialog_kind(kind)?;
        if !state.dialog_sink.native_supported() {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                "native dialog provider is not configured",
                None,
            )));
        }
        state.dialog_sink.message(kind, title, message).map(ProviderResult::Value)
    })?
}

/// `host_dialog_confirm`：确认对话框。
///
/// 缺少原生 provider 时返回 `UnsupportedBody`；只有真实 provider 的 `false`
/// 才表示用户明确拒绝。
pub fn cmd_dialog_confirm(
    state: &SubstrateState,
    title: &str,
    message: &str,
) -> HostResult<ProviderResult<bool>> {
    guard("dialog_confirm", || {
        if !state.dialog_sink.native_supported() {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                "native dialog provider is not configured",
                None,
            )));
        }
        state.dialog_sink.confirm(title, message).map(ProviderResult::Value)
    })?
}

/// 对话框族的**带身份判定**版本（轮 12）。
///
/// # 为什么桩也要判
///
/// 现状 `native_supported()` 为假 → 一律返回 `UnsupportedBody`，看着无害。但线形
/// 已经同形：`host_dialog_open` 返回**文件系统路径**、`host_dialog_save` 决定**写
/// 到哪里**、message/confirm 弹的是**应用级模态**（遮挡整个宿主界面）。原生 provider
/// 一接入，这些能力就同时下放给任何插件——插件可以用宿主的名义弹一个假确认框骗取
/// 同意，也可以借原生选择器把用户磁盘上的路径读走。轮 11 对 `host_market_*` /
/// `host_updater_*` 用的是同一条判据：**判定必须在接线之前就位**，否则越权面会在
/// 没人注意时从"无害桩"变成真实入口。
///
/// 主窗专属不影响现有消费者：这四条的前端落点只有 `@tauron/host` 的
/// `DialogClient`（主窗外壳），插件 SDK 不碰它们。
pub fn cmd_dialog_open_as(
    caller: &Caller,
    state: &SubstrateState,
    multiple: bool,
    directory: bool,
) -> HostResult<ProviderResult<Option<String>>> {
    require_main_window(caller, "host_dialog_open")?;
    cmd_dialog_open(state, multiple, directory)
}

/// `host_dialog_save` 的带身份判定版本，理由同 [`cmd_dialog_open_as`]。
pub fn cmd_dialog_save_as(
    caller: &Caller,
    state: &SubstrateState,
    default_name: Option<&str>,
) -> HostResult<ProviderResult<Option<String>>> {
    require_main_window(caller, "host_dialog_save")?;
    cmd_dialog_save(state, default_name)
}

/// `host_dialog_message` 的带身份判定版本，理由同 [`cmd_dialog_open_as`]。
pub fn cmd_dialog_message_as(
    caller: &Caller,
    state: &SubstrateState,
    title: &str,
    message: &str,
    kind: Option<&str>,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_dialog_message")?;
    cmd_dialog_message(state, title, message, kind)
}

/// `host_dialog_confirm` 的带身份判定版本，理由同 [`cmd_dialog_open_as`]。
pub fn cmd_dialog_confirm_as(
    caller: &Caller,
    state: &SubstrateState,
    title: &str,
    message: &str,
) -> HostResult<ProviderResult<bool>> {
    require_main_window(caller, "host_dialog_confirm")?;
    cmd_dialog_confirm(state, title, message)
}

/// 对话框 `kind` 的闭集校验（缺省 `info`）。
fn validated_dialog_kind(kind: Option<&str>) -> HostResult<&'static str> {
    match kind {
        None | Some("info") => Ok("info"),
        Some("error") => Ok("error"),
        Some("warning") => Ok("warning"),
        Some(other) => Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("对话框 kind `{other}` 非法：只接受 info / error / warning（缺省 info）"),
        )),
    }
}
