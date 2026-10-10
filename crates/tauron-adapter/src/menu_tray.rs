// 菜单 / 托盘宿主能力的命令实现（T-7 拆分：自 `lib.rs` **纯搬移**）。
//
// 这里收纳 `host_menu_*`（set/popup/reset）与 `host_tray_*`（create/set_menu/remove）
// 六个基础命令 + 其带身份判定（`*_as`）版本，以及本域私有的 `validate_menu_spec` /
// `menu_unavailable` / `tray_unavailable` / `validate_tray_spec` 四个 helper。之所以
// menu 与 tray 合并到一个模块而非拆两文件：`validate_tray_spec` 直接复用
// `validate_menu_spec`，同模块才能保持**零语义的纯 move**（不改可见性、不加跨模块
// `use`）。函数体、文档注释、可见性与拆前逐字节一致；crate 根通过 `mod menu_tray;` +
// `pub use menu_tray::{…}` 原样再导出，故 `crate::cmd_menu_set_as`（tauri.rs 接线）
// 与测试里的裸名调用都不受影响。
//
// 依赖（`SubstrateState`/`MenuSpec`/`MenuOutcome`/`TraySpec`/`TrayOutcome`/
// `ProviderResult`/`unsupported_body`/`guard`/`HostResult`/`HostError`/`ErrorCode`/
// `Caller`/`require_main_window`）全部来自 crate 根，故此处只需 `use super::*;`——
// 子模块可见父模块的私有项，无需逐个 re-import。

use super::*;
/// 菜单规格的闭集校验（非空、id 唯一、id/label 非空）。
fn validate_menu_spec(spec: &MenuSpec) -> HostResult<()> {
    if spec.items.is_empty() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "菜单规格不能为空（至少一个菜单项）".to_string(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for item in &spec.items {
        if item.id.trim().is_empty() {
            return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "菜单项 id 不能为空"));
        }
        if item.label.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("菜单项 `{}` 的 label 不能为空", item.id),
            ));
        }
        if !seen.insert(item.id.clone()) {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("菜单项 id `{}` 重复", item.id),
            ));
        }
    }
    Ok(())
}

fn menu_unavailable() -> ProviderResult<MenuOutcome> {
    ProviderResult::Unsupported(unsupported_body(
        "native menu provider is not configured",
        Some("注入 MenuSink 实现（生产：tauri.rs 的 TauriMenuSink）"),
    ))
}

/// `host_menu_set`：设置应用菜单（主窗专属）。
pub fn cmd_menu_set(
    state: &SubstrateState,
    spec: &MenuSpec,
) -> HostResult<ProviderResult<MenuOutcome>> {
    guard("menu_set", || {
        validate_menu_spec(spec)?;
        if !state.menu_sink.native_supported() {
            return Ok(menu_unavailable());
        }
        let applied = state.menu_sink.set_menu(spec)?;
        Ok(ProviderResult::Value(MenuOutcome {
            applied,
            item_count: spec.items.len(),
            reason: if applied {
                None
            } else {
                Some("菜单 sink 未应用该菜单（降级）".into())
            },
        }))
    })?
}

/// `host_menu_set` 的**带身份判定**版本（仅主窗）。
pub fn cmd_menu_set_as(
    caller: &Caller,
    state: &SubstrateState,
    spec: &MenuSpec,
) -> HostResult<ProviderResult<MenuOutcome>> {
    require_main_window(caller, "host_menu_set")?;
    cmd_menu_set(state, spec)
}

/// `host_menu_popup`：弹出上下文菜单（主窗专属）。
pub fn cmd_menu_popup(
    state: &SubstrateState,
    spec: &MenuSpec,
) -> HostResult<ProviderResult<MenuOutcome>> {
    guard("menu_popup", || {
        validate_menu_spec(spec)?;
        if !state.menu_sink.native_supported() {
            return Ok(menu_unavailable());
        }
        let applied = state.menu_sink.popup(spec)?;
        Ok(ProviderResult::Value(MenuOutcome {
            applied,
            item_count: spec.items.len(),
            reason: if applied {
                None
            } else {
                Some("菜单 sink 未能弹出（缺少目标窗口句柄，宿主需在窗口级接线）".into())
            },
        }))
    })?
}

/// `host_menu_popup` 的**带身份判定**版本（仅主窗）。
pub fn cmd_menu_popup_as(
    caller: &Caller,
    state: &SubstrateState,
    spec: &MenuSpec,
) -> HostResult<ProviderResult<MenuOutcome>> {
    require_main_window(caller, "host_menu_popup")?;
    cmd_menu_popup(state, spec)
}

/// `host_menu_reset`：移除应用菜单（主窗专属）。
pub fn cmd_menu_reset(state: &SubstrateState) -> HostResult<ProviderResult<MenuOutcome>> {
    guard("menu_reset", || {
        if !state.menu_sink.native_supported() {
            return Ok(menu_unavailable());
        }
        let applied = state.menu_sink.reset()?;
        Ok(ProviderResult::Value(MenuOutcome {
            applied,
            item_count: 0,
            reason: if applied {
                None
            } else {
                Some("菜单 sink 未移除菜单（降级）".into())
            },
        }))
    })?
}

/// `host_menu_reset` 的**带身份判定**版本（仅主窗）。
pub fn cmd_menu_reset_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<ProviderResult<MenuOutcome>> {
    require_main_window(caller, "host_menu_reset")?;
    cmd_menu_reset(state)
}

fn tray_unavailable() -> ProviderResult<TrayOutcome> {
    ProviderResult::Unsupported(unsupported_body(
        "native tray provider is not configured",
        Some("注入 TraySink 实现（生产：tauri.rs 的 TauriTraySink）"),
    ))
}

fn validate_tray_spec(spec: &TraySpec) -> HostResult<()> {
    if let Some(menu) = &spec.menu {
        validate_menu_spec(menu)?;
    }
    Ok(())
}

/// `host_tray_create`：创建/更新系统托盘（主窗专属）。
pub fn cmd_tray_create(
    state: &SubstrateState,
    spec: &TraySpec,
) -> HostResult<ProviderResult<TrayOutcome>> {
    guard("tray_create", || {
        validate_tray_spec(spec)?;
        if !state.tray_sink.native_supported() {
            return Ok(tray_unavailable());
        }
        let applied = state.tray_sink.create(spec)?;
        Ok(ProviderResult::Value(TrayOutcome {
            applied,
            reason: if applied {
                None
            } else {
                Some("托盘 sink 未创建托盘（降级）".into())
            },
        }))
    })?
}

/// `host_tray_create` 的**带身份判定**版本（仅主窗）。
pub fn cmd_tray_create_as(
    caller: &Caller,
    state: &SubstrateState,
    spec: &TraySpec,
) -> HostResult<ProviderResult<TrayOutcome>> {
    require_main_window(caller, "host_tray_create")?;
    cmd_tray_create(state, spec)
}

/// `host_tray_set_menu`：设置托盘菜单（主窗专属）。
pub fn cmd_tray_set_menu(
    state: &SubstrateState,
    spec: &MenuSpec,
) -> HostResult<ProviderResult<TrayOutcome>> {
    guard("tray_set_menu", || {
        validate_menu_spec(spec)?;
        if !state.tray_sink.native_supported() {
            return Ok(tray_unavailable());
        }
        let applied = state.tray_sink.set_menu(spec)?;
        Ok(ProviderResult::Value(TrayOutcome {
            applied,
            reason: if applied {
                None
            } else {
                Some("托盘 sink 未更新菜单（降级）".into())
            },
        }))
    })?
}

/// `host_tray_set_menu` 的**带身份判定**版本（仅主窗）。
pub fn cmd_tray_set_menu_as(
    caller: &Caller,
    state: &SubstrateState,
    spec: &MenuSpec,
) -> HostResult<ProviderResult<TrayOutcome>> {
    require_main_window(caller, "host_tray_set_menu")?;
    cmd_tray_set_menu(state, spec)
}

/// `host_tray_remove`：移除系统托盘（主窗专属）。
pub fn cmd_tray_remove(state: &SubstrateState) -> HostResult<ProviderResult<TrayOutcome>> {
    guard("tray_remove", || {
        if !state.tray_sink.native_supported() {
            return Ok(tray_unavailable());
        }
        let applied = state.tray_sink.remove()?;
        Ok(ProviderResult::Value(TrayOutcome {
            applied,
            reason: if applied {
                None
            } else {
                Some("托盘 sink 未移除托盘（降级）".into())
            },
        }))
    })?
}

/// `host_tray_remove` 的**带身份判定**版本（仅主窗）。
pub fn cmd_tray_remove_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<ProviderResult<TrayOutcome>> {
    require_main_window(caller, "host_tray_remove")?;
    cmd_tray_remove(state)
}
