// T-7 拆分：从 `lib.rs` 逐字节纯搬移的贡献（contributes）命令族。
//
// 三条命令都只经 `state.contributes.lock()` 读写贡献表、`cmd_contributes_reconcile`
// 另读 `state.registry.find` 拿 manifest 做「声明 vs 注册」对账；它们不点名任何随族
// 搬迁的结构体，对账返回体 `ContributesReconcileReport` 与本族共用。`declared_contribute_keys`
// 是只对账命令调用的同族私有助手（对 A 口径而言只被同文件消费），随族一起搬。
//
// `use super::*;` 令搬来的命令体原样调用 crate-root 私有助手（guard）与顶层 use 导入；
// 本族全为 pub（含 `ContributesReconcileReport` 经 tauri.rs 的 `crate::` 路径引用），
// 按原名再导出，保 `crate::cmd_contributes_*` / `crate::ContributesReconcileReport`
// / `crate::declared_contribute_keys`（tauri.rs 接线与内联测试裸名）解析不变。

use super::*;

/// `host_contributes_register`：注册贡献（self 档）。
pub fn cmd_contributes_register(
    state: &PluginRuntimeState,
    _plugin_id: &str,
    entry: ContributeEntry,
) -> HostResult<()> {
    guard("contributes_register", || {
        let mut contributes = state.contributes.lock();
        contributes.register(entry)
    })?
}

/// `host_contributes_list`：列出贡献（scoped-read 档）。
pub fn cmd_contributes_list(
    state: &PluginRuntimeState,
    kind: Option<&str>,
) -> HostResult<Vec<ContributeEntry>> {
    guard("contributes_list", || {
        let contributes = state.contributes.lock();
        let entries = match kind {
            Some(k) => contributes.list_by_kind(k).into_iter().cloned().collect(),
            None => contributes.list_all().to_vec(),
        };
        Ok(entries)
    })?
}

/// 贡献对账结果（`host_contributes_reconcile` 成功时的返回体）。
///
/// 只在**无分叉**时返回（有分叉走 `E_CONTRIBUTES_DRIFT`），因此 `missing` / `extra`
/// 按构造恒为空——保留字段是为了让线格式与错误消息里的诊断信息同形，
/// 调用方不必按分支读两种形状。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributesReconcileReport {
    /// 被对账的插件 id。
    pub plugin_id: String,
    /// manifest 声明的贡献数（`kind:id` 去重后）。
    pub declared: usize,
    /// activate 期实际注册的贡献数（`kind:id` 去重后）。
    pub registered: usize,
    /// 声明了但没注册的 `kind:id`（成功时恒为空）。
    pub missing: Vec<String>,
    /// 注册了但没声明的 `kind:id`（成功时恒为空）。
    pub extra: Vec<String>,
}

/// 把 manifest 的 `Contributes` 摊平成 `kind:id` 键集。
///
/// **kind 词汇与 `@tauron/app-plugin-sdk` 的 `createPlugin` 必须一致**
/// （`command` / `menu` / `panel` / `settings` / `shortcut`），否则对账会把
/// 每一对都报成分叉。两处漂移由 wire-gate 的 `contributes` 门禁钉住。
///
/// **shortcut 的 id 用 `command`**：快捷键没有天然 id（与 `createPlugin` 同口径），
/// 因此「同一条命令绑两个加速键」在声明侧只算一条——这不是缺陷，是两侧共同的有损
/// 映射；用 `BTreeSet` 去重后两侧口径一致。
pub fn declared_contribute_keys(
    contributes: &tauron_host::manifest::Contributes,
) -> std::collections::BTreeSet<String> {
    let mut keys = std::collections::BTreeSet::new();
    for c in &contributes.commands {
        keys.insert(format!("command:{}", c.id));
    }
    for m in &contributes.menus {
        keys.insert(format!("menu:{}", m.id));
    }
    for p in &contributes.panels {
        keys.insert(format!("panel:{}", p.id));
    }
    for t in &contributes.settings_tabs {
        keys.insert(format!("settings:{}", t.id));
    }
    for s in &contributes.shortcuts {
        keys.insert(format!("shortcut:{}", s.command));
    }
    keys
}

/// `host_contributes_reconcile`：对账「manifest 声明」与「activate 期注册」。
///
/// 这是 P0-6 的收口：`host_contributes_register` 过去只是往 `Vec` 里增删，
/// **没有任何东西比对声明与事实**——声明了却漏注册（入口点了没反应）与注册了
/// 却没声明（来源不明的入口）都无人发现。本函数把这份落差变成可检出的错误。
///
/// 纯只读（不改任何状态），因此可在安装完成 / 启用 / 列表三个触发点安全调用。
pub fn cmd_contributes_reconcile(
    state: &PluginRuntimeState,
    plugin_id: &str,
) -> HostResult<ContributesReconcileReport> {
    guard("contributes_reconcile", || {
        let id = tauron_host::manifest::PluginId::new(plugin_id).map_err(|_| {
            HostError::new(
                ErrorCode::E_UNKNOWN_PLUGIN,
                format!("插件 id `{plugin_id}` 不合法，无法对账贡献"),
            )
        })?;
        // 声明面来自注册表里的 manifest。不在注册表 = 无从对账（不是"零分叉"）：
        // 静默返回空报告会让调用方以为"对过账了，没问题"。
        let entry = state.registry.find(&id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_UNKNOWN_PLUGIN,
                format!("插件 `{plugin_id}` 不在注册表中，无法对账贡献声明"),
            )
        })?;
        let declared = declared_contribute_keys(&entry.manifest.contributes);
        // 事实面来自贡献表（只看该插件自己的条目）。
        let registered: std::collections::BTreeSet<String> = state
            .contributes
            .lock()
            .list_all()
            .iter()
            .filter(|e| e.plugin_id == plugin_id)
            .map(|e| format!("{}:{}", e.kind, e.id))
            .collect();

        let missing: Vec<String> = declared.difference(&registered).cloned().collect();
        let extra: Vec<String> = registered.difference(&declared).cloned().collect();
        if !missing.is_empty() || !extra.is_empty() {
            return Err(HostError::new(
                ErrorCode::E_CONTRIBUTES_DRIFT,
                format!(
                    "插件 `{plugin_id}` 的贡献声明与注册不一致：声明 {} 条 / 注册 {} 条；\
                     声明了但没注册 {:?}；注册了但没声明 {:?}",
                    declared.len(),
                    registered.len(),
                    missing,
                    extra
                ),
            ));
        }
        Ok(ContributesReconcileReport {
            plugin_id: plugin_id.to_string(),
            declared: declared.len(),
            registered: registered.len(),
            missing,
            extra,
        })
    })?
}
