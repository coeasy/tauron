// T-7 拆分：本文件是 `crates/tauron-adapter/src/lib.rs` 的 i18n 命令族纯搬移落点。
// 逐字节原样搬迁、零语义改动；`mod i18n;` + 按原名 `pub use` 追加在 lib.rs 文件末尾
// （不在顶部插行，把行号漂移面压到最小）。
// 子模块经 `use super::*;` 可见父 crate-root 的私有助手（guard/unsupported_body/
// require_main_window/require_self_plugin_scope 等）与再导出，故搬来的命令照常编译。
// 说明：i18n 命令族含 10 个 pub fn（t/t_params/set_locale(_as)/load(_as)/stats(_as)/
// cleanup_plugin(_as)）与 3 个私有助手（validated_locale/validated_key/i18n_state_payload，
// 仅本族内部使用，故随本模块私有不外泄）；它们操作的 I18nEngine/ResourceBundle 经
// `SubstrateState::i18n` 字段访问，不在此模块内定义。

use super::*;

/// 校验语言代码。
///
/// [`I18nEngine::set_locale`] 与 `ResourceBundle::new` 都不做校验（内部走
/// `Locale::new`，永不失败），所以类型化校验必须放在这一层：空串或超过 35 字符
/// 的语言代码会污染回退链。
fn validated_locale(raw: &str) -> HostResult<tauron_i18n::Locale> {
    raw.parse::<tauron_i18n::Locale>().map_err(|e| {
        HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("非法语言代码 `{raw}`：{e}"))
    })
}

fn validated_key(raw: &str) -> HostResult<()> {
    if raw.trim().is_empty() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "i18n key 不能为空（调用方可能传了 undefined/null）".to_string(),
        ));
    }
    Ok(())
}

/// i18n 引擎当前状态的线形态（多个命令共用）。
fn i18n_state_payload(engine: &I18nEngine) -> serde_json::Value {
    let locales = engine.registered_locales();
    let mut bundle_keys = serde_json::Map::new();
    for locale in &locales {
        bundle_keys.insert(
            locale.clone(),
            serde_json::json!(engine.get_bundle(locale).map_or(0, ResourceBundle::len)),
        );
    }
    serde_json::json!({
        "locale": engine.locale().as_str(),
        "rtl": engine.is_rtl(),
        "fallbackChain": engine.fallback_chain(),
        "registeredLocales": locales,
        "bundleKeys": bundle_keys,
        "missingTotal": engine.missing_total(),
    })
}

/// `host_i18n_t`：翻译一个 key。
///
/// 回退链由引擎负责：当前语言 → 短形式（`zh-CN` → `zh`）→ 默认语言
/// （`en-US`）。**全部缺失时返回 key 本身**并计入缺失键计数——这是刻意设计：
/// 让用户看到 `oc.settings.title` 比看到空按钮更能暴露缺失文案，而空串会让
/// 问题彻底隐形。缺失可观测性见 `host_i18n_stats` 的 `missingTotal`。
pub fn cmd_i18n_t(state: &SubstrateState, key: &str) -> HostResult<String> {
    guard("i18n_t", || {
        validated_key(key)?;
        Ok(state.i18n.lock().t(key))
    })?
}

/// `host_i18n_t_params`：带 `{{param}}` 占位替换的翻译。
pub fn cmd_i18n_t_params(
    state: &SubstrateState,
    key: &str,
    params: serde_json::Map<String, serde_json::Value>,
) -> HostResult<String> {
    guard("i18n_t_params", || {
        validated_key(key)?;
        let mut owned = Vec::with_capacity(params.len());
        for (name, value) in &params {
            // 参数值必须是字符串：模板替换不接受 JSON 对象/数组。
            let Some(text) = value.as_str() else {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("i18n 参数 `{name}` 的值必须是字符串，收到 `{value}`"),
                ));
            };
            owned.push((name.clone(), text.to_string()));
        }
        let pairs: Vec<(&str, &str)> =
            owned.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        Ok(state.i18n.lock().t_params(key, &pairs))
    })?
}

/// `host_i18n_set_locale` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// 语言**不是"某个插件的设置"**，而是整个应用界面的单一状态源：`state.i18n` 的 locale
/// 同时决定宿主 UI 与**所有**插件界面的呈现（`host_i18n_t` 的回退链第一环就是它）。
/// 因此插件调它 = **跨插件的全局状态篡改**：
///
/// - 把语言切到别的语种，别的插件（和宿主）的界面**当场变成另一种语言**——用户
///   明明在中文环境里看到的却是日文菜单，而界面文案正是安全提示（"即将删除"、
///   "允许访问文件"）的载体，篡改它可以骗过用户；
/// - 它不是拒绝服务那么轻：切换是静默成功的（返回 200 语义的正常状态体），
///   用户只会觉得"这个应用坏了"，而根因在另一个插件的一次调用里。
///
/// 主窗是唯一合法主体（用户自己的偏好设置）。拒绝码复用既有的
/// [`ErrorCode::E_AUTH_DENIED`]，判定在**写入引擎之前**：拒绝时 locale 与
/// `registeredLocales` 一律不动（有测试断言引擎状态逐字不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_i18n_set_locale_as(
    caller: &Caller,
    state: &SubstrateState,
    locale: &str,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_i18n_set_locale")?;
    cmd_i18n_set_locale(state, locale)
}

/// `host_i18n_set_locale`：切换语言（§4.20：语言状态的单一来源）。
///
/// **入口**：wire 层转调 [`cmd_i18n_set_locale_as`]（仅主窗：切的是全局语言），
/// 本函数是**不过身份**的核心。
pub fn cmd_i18n_set_locale(state: &SubstrateState, locale: &str) -> HostResult<serde_json::Value> {
    guard("i18n_set_locale", || {
        let validated = validated_locale(locale)?;
        let mut engine = state.i18n.lock();
        engine.set_locale(validated.as_str()).map_err(|e| {
            HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("切换语言失败：{e}"))
        })?;
        Ok(i18n_state_payload(&engine))
    })?
}

/// `host_i18n_load`：装载一个语言包。
///
/// **合并语义**：同一语言的包已存在时按 key 合并，而不是整体替换——否则先装
/// 应用文案、再装插件文案会把应用自己的文案覆盖掉。
///
/// 传入 `pluginId` 时，每个 key 自动加 `plugin:<id>.oc.` 前缀（§4.20 命名空间）；
/// 已经带该前缀的 key 原样保留（幂等）。插件卸载时由
/// [`cmd_i18n_cleanup_plugin`] / `cmd_registry_admin` 的 Uninstall/Purge 回收。
///
/// **入口**：wire 层转调 [`cmd_i18n_load_as`]（`pluginId` = 命名空间归属，插件
/// 只能装自己的；`None` = 宿主文案，插件不得主张），本函数是**不过身份**的核心。
pub fn cmd_i18n_load(
    state: &SubstrateState,
    locale: &str,
    entries: serde_json::Map<String, serde_json::Value>,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    guard("i18n_load", || {
        let validated = validated_locale(locale)?;
        let plugin_prefix = match plugin_id {
            Some(pid) => Some(PluginId::new(pid)?.as_str().to_string()),
            None => None,
        };

        // 读-改-写必须在**同一把锁**内完成：拆成「锁内读 bundle → 锁外拼 →
        // 再锁内写回」会让并发装载丢更新——两个调用各读到同一旧快照，后写者
        // 整体覆盖先写者刚装入的文案。
        let mut engine = state.i18n.lock();
        let mut bundle = engine
            .get_bundle(validated.as_str())
            .cloned()
            .unwrap_or_else(|| ResourceBundle::new(validated.as_str()));

        let mut new_keys = 0usize;
        for (key, value) in &entries {
            // 文案值必须是字符串：非字符串的条目说明 bundle 文件格式错了。
            let Some(text) = value.as_str() else {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("i18n 条目 `{key}` 的值必须是字符串，收到 `{value}`"),
                ));
            };
            let full = match &plugin_prefix {
                Some(prefix) if !key.starts_with(&format!("plugin:{prefix}.")) => {
                    I18nEngine::plugin_key(prefix, key)
                }
                _ => key.clone(),
            };
            if bundle.get(&full).is_none() {
                new_keys += 1;
            }
            bundle.insert(&full, text);
        }
        engine.add_resource_bundle(bundle);

        let mut payload = i18n_state_payload(&engine);
        payload["loadedKeys"] = serde_json::json!(entries.len());
        payload["newKeys"] = serde_json::json!(new_keys);
        payload["pluginId"] = serde_json::json!(plugin_prefix);
        Ok(payload)
    })?
}

/// `host_i18n_load` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// `pluginId` 在这里就是**命名空间归属**：传 `Some(id)` 时每个 key 会被加上
/// `plugin:<id>.oc.` 前缀，于是"谁装的文案"完全由这个入参决定。不判定的话：
///
/// - 插件 A 能以 `pluginId = "com.b"` 装载文案，**写进 B 的命名空间**——B 界面上
///   出现的每一句文案（按钮标签、确认框正文、危险操作提示）都可被 A 改写，
///   这是**命名空间投毒**：不需要执行任何代码就能伪造 B 呈现给用户的内容；
/// - 传 `None` 则装进**宿主级命名空间**（无前缀的应用文案），插件能覆盖应用自己的
///   文案——同样是不执行代码就能改整个应用的说法。
///
/// 规则是"命名空间必须是自己"（[`require_self_plugin_scope`]）：主窗可装任意
/// `pluginId` 或 `None`（宿主本来就是应用文案、以及代插件装载的一方），插件只能装
/// 自己的。拒绝路径**零副作用**：`entries` 里的任何 key 都不会进入引擎
/// （有测试断言目标命名空间在引擎里**不存在**）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_i18n_load_as(
    caller: &Caller,
    state: &SubstrateState,
    locale: &str,
    entries: serde_json::Map<String, serde_json::Value>,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    require_self_plugin_scope(caller, "host_i18n_load", plugin_id)?;
    cmd_i18n_load(state, locale, entries, plugin_id)
}

/// `host_i18n_stats`：i18n 状态与缺失键可观测性。
pub fn cmd_i18n_stats(state: &SubstrateState) -> HostResult<serde_json::Value> {
    guard("i18n_stats", || Ok(i18n_state_payload(&state.i18n.lock())))?
}

/// `host_i18n_stats` 的**带身份判定**版本（轮 12）。
///
/// 它返回的是**全局**文案普查（当前 locale、各命名空间条数、缺失键清单）——里面
/// 带着别的插件装了哪些命名空间、哪些键缺失。判据与 `host_production_doctor` 同：
/// 可观测性/诊断面只给主窗。对照 `host_i18n_t` / `host_i18n_t_params`：那两条是
/// **渲染用的只读取值**，插件界面本来就要取文案，故不判（同一份 store，但一次只答
/// 一个键，不暴露拓扑）。
pub fn cmd_i18n_stats_as(caller: &Caller, state: &SubstrateState) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_i18n_stats")?;
    cmd_i18n_stats(state)
}

/// `host_i18n_cleanup_plugin`：清除一个插件的全部文案。
///
/// 返回清除的 key 数。插件卸载（`host_registry_admin` 的 Uninstall/Purge）
/// 会自动调用它；这里独立暴露是给「插件被禁用但仍在注册表里」的情形用。
///
/// **入口**：wire 层转调 [`cmd_i18n_cleanup_plugin_as`]（插件只能清自己的文案，
/// 清别人就是跨插件销毁），本函数是**不过身份**的核心。
pub fn cmd_i18n_cleanup_plugin(
    state: &SubstrateState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    guard("i18n_cleanup_plugin", || {
        let id = PluginId::new(plugin_id)?;
        let mut engine = state.i18n.lock();
        let removed = engine.cleanup_plugin(id.as_str());
        drop(engine);

        let mut payload = i18n_state_payload(&state.i18n.lock());
        payload["pluginId"] = serde_json::json!(id.as_str());
        payload["removed"] = serde_json::json!(removed);
        Ok(payload)
    })?
}

/// `host_i18n_cleanup_plugin` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// 这条命令**销毁**目标插件的全部文案（`plugin:<id>.oc.` 前缀的 key，跨所有语言包）。
/// 不判定的话，插件 A 调一次 `pluginId = "com.b"` 就能**清空 B 的全部界面文案**：
/// B 之后每个 `host_i18n_t` 都会落空——按设计（"缺失时返回 key 本身"）B 的界面会
/// 变成一屏 `plugin:com.b.oc.xxx` 原始 key。这是纯破坏性的跨插件操作，且不需要
/// 任何权限授予即可调用，所以判定必须落在参数上（[`require_self_plugin_scope`]）。
///
/// 主窗可清任意插件（卸载/禁用回收的正当路径：`cmd_registry_admin` 的
/// Uninstall/Purge 内部就会调核心 `cmd_i18n_cleanup_plugin`），插件只能清自己的。
/// 拒绝路径**零副作用**：别人的 bundle 一个 key 都不会掉（有测试断言它仍在）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_i18n_cleanup_plugin_as(
    caller: &Caller,
    state: &SubstrateState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    require_self_plugin_scope(caller, "host_i18n_cleanup_plugin", Some(plugin_id))?;
    cmd_i18n_cleanup_plugin(state, plugin_id)
}
