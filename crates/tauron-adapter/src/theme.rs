// T-7 拆分：本文件是 `crates/tauron-adapter/src/lib.rs` 的 theme 命令族纯搬移落点。
// 逐字节原样搬迁、零语义改动；`mod theme;` + 按原名 `pub use` 追加在 lib.rs 文件末尾
// （不在顶部插行，把行号漂移面压到最小）。
// 子模块经 `use super::*;` 可见父 crate-root 的私有助手（guard/unsupported_body/
// require_main_window 等）与再导出，故搬来的命令照常编译。
// 说明：theme 命令族只有 6 个 pub fn（list/get/set 各带 _as 版本），无私有助手、无 pub const；
// 它们操作的 ThemeRegistry 经 `SubstrateState::themes` 字段访问，不在此模块内定义。

use super::*;

/// `host_theme_list`：列出可用主题（主窗专属；接孤儿 crate `tauron-theme`）。
pub fn cmd_theme_list(state: &SubstrateState) -> HostResult<Vec<tauron_theme::ThemeContribute>> {
    guard("theme_list", || Ok(state.themes.lock().to_contributes()))?
}

/// `host_theme_list` 的**带身份判定**版本（仅主窗）。
pub fn cmd_theme_list_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<Vec<tauron_theme::ThemeContribute>> {
    require_main_window(caller, "host_theme_list")?;
    cmd_theme_list(state)
}

/// `host_theme_get`：读取单个主题（主窗专属）。
pub fn cmd_theme_get(
    state: &SubstrateState,
    id: &str,
) -> HostResult<ProviderResult<serde_json::Value>> {
    guard("theme_get", || {
        let reg = state.themes.lock();
        match reg.get(id) {
            Some(theme) => Ok(ProviderResult::Value(serde_json::json!({
                "id": theme.id,
                "name": theme.name,
                "isDark": theme.is_dark,
                "variables": theme.variables,
            }))),
            None => Ok(ProviderResult::Unsupported(unsupported_body(
                &format!("主题 `{id}` 不存在"),
                Some("先调用 host_theme_list 查看可用主题"),
            ))),
        }
    })?
}

/// `host_theme_get` 的**带身份判定**版本（仅主窗）。
pub fn cmd_theme_get_as(
    caller: &Caller,
    state: &SubstrateState,
    id: &str,
) -> HostResult<ProviderResult<serde_json::Value>> {
    require_main_window(caller, "host_theme_get")?;
    cmd_theme_get(state, id)
}

/// `host_theme_set`：切换激活主题（主窗专属）。
pub fn cmd_theme_set(
    state: &SubstrateState,
    id: &str,
) -> HostResult<ProviderResult<serde_json::Value>> {
    guard("theme_set", || {
        let mut reg = state.themes.lock();
        if reg.get(id).is_none() {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                &format!("主题 `{id}` 不存在，无法激活"),
                Some("先调用 host_theme_list 查看可用主题"),
            )));
        }
        reg.set_active(id).map_err(|e| {
            HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("切换主题 `{id}` 失败：{e}"))
        })?;
        Ok(ProviderResult::Value(serde_json::json!({
            "activeId": reg.active_id(),
            "dataTheme": reg.data_theme_attribute(),
        })))
    })?
}

/// `host_theme_set` 的**带身份判定**版本（仅主窗）。
pub fn cmd_theme_set_as(
    caller: &Caller,
    state: &SubstrateState,
    id: &str,
) -> HostResult<ProviderResult<serde_json::Value>> {
    require_main_window(caller, "host_theme_set")?;
    cmd_theme_set(state, id)
}
