// tauron-adapter · 注册表域命令族（T-7 逐字节纯搬移 · 十六片）。
//
// 本模块由 crate 根 lib.rs 的「命令处理函数（平台无关，可直接测试）」段整体纯搬移而来（旧 lib
// 4830–5731 行，一段连续 902 行），未改动任何一行代码——函数体、注释、doc、cfg 全部逐字节保真，
// 仅把归属文件从 lib.rs 换到 registry.rs。内容分两层：
//  · 非门控 7 项：cmd_registry_list / cmd_registry_list_all / cmd_registry_list_all_as /
//    cmd_registry_admin / cmd_registry_admin_as / cmd_registry_admin_reviewed_as /
//    install_plugin_from_json（装配期安装入口，宿主 setup() 调用、examples 使用，恒在）。
//  · plugin-install 门控 3 命令 + 其私有助手：cmd_registry_install_as /
//    cmd_registry_install_reviewed_as / cmd_registry_install_preview_as，连同只服务它们的
//    registry_install_inner / registry_install_preview_inner / read_verified_package /
//    unix_time_seconds / InstallCleanupStage / stage_install_cleanup 一起搬入——它们彼此同模块可见，
//    故无需任何 `pub(crate)` 放宽；`cmd_registry_admin` 对 stage_install_cleanup 的调用本就包在
//    `#[cfg(feature = "plugin-install")]` 内，特性关闭时两侧同时消失，编译如常。
//
// 命名区分：本文件是 tauron-**adapter** 的注册表**命令族**（宿主命令面），与 tauron-**host** crate 里的
// `registry.rs`（`Registry` 注册表数据结构本体）是两回事，切勿混淆。
//
// 刻意留在 lib.rs 的是被其它域共用的助手与类型：边界/闸门 `run_registry_boundary` /
// `reconcile_registry_boundary` / `run_review_boundary` / `reconcile_review_boundary`、审计咽喉
// `admin_gate` / `require_main_window`、审批令牌助手 `mint_install_review` / `review_unexpired` /
// `review_matches_verified` / `mint_admin_review` / `validate_admin_review` / `admin_review_required` /
// `is_destructive_admin_op`、落盘摘要 `digest_file_and_rewind` / `permission_digest` /
// `write_plugin_ui_activation`、旁路回收 `prune_subscription_groups`、恢复对账 `reconcile_recovery_phase`，
// 以及类型 `RegistryAdminResponse` / `InstallReviewToken` / `AdminReviewToken` / `VerifiedPluginPackage` /
// `PluginInstallResult` / `PluginInstallPreview` 等。本族经 `use super::*;` 原样可见 crate 根的上述私助手
// 与 `guard` / `PluginRuntimeState` / `Caller` / `HostResult` / `ErrorCode` 等类型，零可见性放宽、纯归属搬移。

use super::*;

// ──────────────────────────────────────────────────────────────────────────
// 命令处理函数（平台无关，可直接测试）
// ──────────────────────────────────────────────────────────────────────────

/// `host_registry_list`：列出可见插件（scoped-read 档）。
pub fn cmd_registry_list(
    state: &PluginRuntimeState,
    caller: Option<&str>,
    subscribed_topics: &[String],
) -> HostResult<Vec<PluginSummary>> {
    let caller_id = caller.map(PluginId::new).transpose()?;
    run_registry_boundary(state, "registry_list", || {
        Ok(state.registry.list_visible(caller_id.as_ref(), subscribed_topics))
    })
}

/// `host_registry_list_all`：全量列表（privileged 档 / 主窗 UI）。
pub fn cmd_registry_list_all(state: &PluginRuntimeState) -> HostResult<Vec<PluginSummary>> {
    run_registry_boundary(state, "registry_list_all", || Ok(state.registry.list_all()))
}

/// `host_registry_list_all` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 判定在转调**之前**：全量列表暴露所有插件（含未安装/已禁用），插件主体看到
/// 别人存在本身就是 scoped-read 想遮住的信息。
pub fn cmd_registry_list_all_as(
    caller: &Caller,
    state: &PluginRuntimeState,
) -> HostResult<Vec<PluginSummary>> {
    require_main_window(caller, "host_registry_list_all")?;
    cmd_registry_list_all(state)
}

/// `host_registry_admin`：主窗特权命令（启用/禁用/卸载/清除）。
/// **装配期插件安装**（宿主接入方在 `setup()` 阶段调用）。
///
/// 这是 `Registry::install` 的**生产入口**。在此之前它只在测试里被调用过：
/// 注册表在生产上永远是空的，于是 `host_plugin_call` / 流式 / 生命周期 /
/// `host_runtime_spawn` 这一整条插件链在真机上**没有起点**——「有类型、有接口、
/// 有测试、没有入口」的典型形态。
///
/// 三个动作：
/// 1. 解析 manifest JSON（`deny_unknown_fields`，拼写漂移即失败）；
/// 2. 解析失败但 `id` 可辨认 → [`Registry::record_install_failure`] 落一条
///    `INSTALL_FAILED`，让 UI **看得见**这次失败（否则插件凭空消失）；
/// 3. 解析成功 → 用**内嵌权限词表**（[`tauron_host::manifest::embedded_permission_index`]）
///    做 `validate` 并落库。
///
/// 为什么不做成命令：安装发生在宿主**装配期**，输入是接入方自己的插件清单，
/// 不是前端可控的运行期动作。运行期的管理面是 `host_registry_admin`
/// （enable/disable/uninstall/purge）——两者分工明确。
///
/// 用法见 `examples/minimal-app/src-tauri/src/main.rs`。
pub fn install_plugin_from_json(
    state: &PluginRuntimeState,
    manifest_json: &str,
) -> HostResult<PluginId> {
    let index = tauron_host::manifest::embedded_permission_index();

    match serde_json::from_str::<PluginManifest>(manifest_json) {
        Ok(manifest) => state.registry.install(&index, manifest),
        Err(parse_err) => {
            // 兜底：manifest 都解析不出来时，尽力从原始 JSON 里抠出 id，
            // 把它记成 INSTALL_FAILED，而不是让这个插件"从没出现过"。
            let reason = format!("manifest JSON 解析失败：{parse_err}");
            let recovered_id = serde_json::from_str::<serde_json::Value>(manifest_json)
                .ok()
                .and_then(|v| v.get("id").and_then(|id| id.as_str()).map(String::from))
                .and_then(|raw| PluginId::new(&raw).ok());

            if let Some(id) = recovered_id {
                // 已经装过就不覆盖（`record_install_failure` 会返回 E_PLUGIN_EXISTS）。
                // 结果要进文案：留痕失败时不能宣称"已记入 INSTALL_FAILED"。
                let recorded =
                    state.registry.record_install_failure(id.clone(), reason.clone()).is_ok();
                let tail = if recorded {
                    format!("插件 `{id}` 已记入 INSTALL_FAILED，可卸载")
                } else {
                    format!(
                        "插件 `{id}` **未**记入 INSTALL_FAILED（同 id 已在注册表，或注册表已满）"
                    )
                };
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("{reason}（{tail}）"),
                ));
            }

            Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("{reason}；且无法从 JSON 中辨认插件 id，未落库"),
            ))
        }
    }
}

/// Install a signed local `.tpkg`; every trust input comes from host configuration or the
/// explicit main-window approval. Unsupported runtime types fail before any side effect.
#[cfg(feature = "plugin-install")]
pub fn cmd_registry_install_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    package_path: &str,
    approved_permissions: &[String],
) -> HostResult<PluginInstallResult> {
    admin_gate(&state.substrate, caller, "host_registry_install")?;
    if state.deployment_mode == tauron_host::DeploymentMode::Production {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "production install requires InstallReviewToken from host_registry_install_preview",
        ));
    }
    run_registry_boundary(state, "registry_install_legacy", || {
        registry_install_inner(state, package_path, approved_permissions, None)
    })
}

/// V4 reviewed install path. This is the only path wired to production Tauri IPC.
#[cfg(feature = "plugin-install")]
pub fn cmd_registry_install_reviewed_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    package_path: &str,
    approved_permissions: &[String],
    review_token: &InstallReviewToken,
) -> HostResult<PluginInstallResult> {
    admin_gate(&state.substrate, caller, "host_registry_install")?;
    run_registry_boundary(state, "registry_install_reviewed", || {
        registry_install_inner(state, package_path, approved_permissions, Some(review_token))
    })
}

#[cfg(feature = "plugin-install")]
pub fn cmd_registry_install_preview_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    package_path: &str,
) -> HostResult<PluginInstallPreview> {
    admin_gate(&state.substrate, caller, "host_registry_install_preview")?;
    if package_path.trim().is_empty() {
        return Err(HostError::new(ErrorCode::E_INSTALL_FAILED, "安装包路径不能为空"));
    }
    // A91（轮 47）：预览是审批域的**修复入口**——审批闸门故障时，重新预览先做
    // 确定性清空（旧令牌全失效）再铸发新令牌；闸门正常时这一步是幂等空操作。
    reconcile_review_boundary(state)?;
    guard("registry_install_preview", || registry_install_preview_inner(state, package_path))?
}

#[cfg(feature = "plugin-install")]
fn registry_install_preview_inner(
    state: &PluginRuntimeState,
    package_path: &str,
) -> HostResult<PluginInstallPreview> {
    use tauron_host::manifest::{embedded_permission_index, PluginType};
    let verified = read_verified_package(state, package_path)?;
    let manifest = &verified.manifest;
    if manifest.plugin_type != PluginType::Js {
        return Err(HostError::new(ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME, "当前仅支持 JS 插件安装"));
    }
    let index = embedded_permission_index();
    let draft = tauron_acl::draft_grant_set(
        manifest.id.as_str(),
        &manifest.version.to_string(),
        &manifest.framework.to_string(),
        &manifest.permissions,
        &manifest.scopes,
        &index,
        "preview",
        unix_time_seconds(),
        1,
    )?;
    let review_token = mint_install_review(state, &verified)?;
    {
        // 清理读数取令牌自身的 issued_at（同一次 `review_now` 判定，不二次取样时钟）。
        let now = review_token.issued_at;
        let mut reviews = state.install_reviews.lock();
        reviews.retain(|_, token| token.expires_at > now);
        if reviews.len() >= MAX_INSTALL_REVIEWS {
            if let Some(oldest) = reviews
                .values()
                .min_by_key(|token| token.issued_at)
                .map(|token| token.nonce.clone())
            {
                reviews.remove(&oldest);
            }
        }
        reviews.insert(review_token.nonce.clone(), review_token.clone());
    }
    Ok(PluginInstallPreview {
        plugin_id: manifest.id.to_string(),
        plugin_name: manifest.name.clone(),
        version: manifest.version.to_string(),
        permissions: tauron_acl::build_approval_rows(&draft, &index)
            .into_iter()
            .map(|row| {
                // 先取借用形态的确认词，再按字段移动 `row`（`confirmation_hint()` 借 `&self`）。
                let confirmation_hint = row.confirmation_hint().map(str::to_string);
                PluginPermissionReview {
                    permission: row.permission,
                    risk: row.risk.as_str().to_string(),
                    description: row.human_text,
                    default_checked: row.default_checked,
                    scope: row.scope,
                    confirmation_hint,
                }
            })
            .collect(),
        review_token,
    })
}

#[cfg(feature = "plugin-install")]
fn registry_install_inner(
    state: &PluginRuntimeState,
    package_path: &str,
    approved_permissions: &[String],
    review_token: Option<&InstallReviewToken>,
) -> HostResult<PluginInstallResult> {
    use std::collections::BTreeSet;
    use tauron_acl::{draft_grant_set, validate_grants, AclStore};
    use tauron_host::manifest::{embedded_permission_index, PluginType};

    let config = state.install_config.as_ref().ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "plugin-install 未配置安装目录、可信密钥与 ACL 密钥（拒绝安装）",
        )
    })?;
    if config.acl_signing_key.as_ref().is_none_or(|key| key.len() < 32) {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "ACL HMAC key 未配置或少于 32 字节",
        ));
    }
    let verified = read_verified_package(state, package_path)?;
    if let Some(review_token) = review_token {
        run_review_boundary(state, "install_review_consume", || {
            let stored =
                state.install_reviews.lock().remove(&review_token.nonce).ok_or_else(|| {
                    HostError::new(
                        ErrorCode::E_INSTALL_FAILED,
                        "install review token is unknown, expired, or already consumed",
                    )
                })?;
            if stored != *review_token {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    "install review token is stale or has been tampered with; preview again",
                ));
            }
            // 轮 43：TTL 判定走 A100（provider 可信 → `require_unexpired` 失败关闭；
            // 开发/测试档无源回落墙钟，与既有行为一致）。
            review_unexpired(state, review_token.expires_at)?;
            if !review_matches_verified(review_token, &verified) {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    "package changed after approval; preview and approval must be repeated",
                ));
            }
            Ok(())
        })?;
    } else if state.deployment_mode == tauron_host::DeploymentMode::Production {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "production install requires an unexpired one-time InstallReviewToken",
        ));
    }

    let VerifiedPluginPackage { manifest, archive, .. } = verified;
    if manifest.plugin_type != PluginType::Js {
        return Err(HostError::new(
            ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME,
            format!(
                "本宿主当前仅接受具备 WebView runtime 的 JS 插件，拒绝 {:?}",
                manifest.plugin_type
            ),
        ));
    }
    let declared: BTreeSet<String> =
        manifest.permissions.iter().map(|p| p.as_str().to_string()).collect();
    let approved: BTreeSet<String> = approved_permissions.iter().cloned().collect();
    if declared != approved {
        return Err(HostError::new(
            ErrorCode::E_FORBIDDEN_PERMISSION,
            "审批确认必须精确覆盖 manifest 权限集；扩权或缺项均拒绝",
        ));
    }
    let index = embedded_permission_index();
    let grants = draft_grant_set(
        manifest.id.as_str(),
        &manifest.version.to_string(),
        &manifest.framework.to_string(),
        &manifest.permissions,
        &manifest.scopes,
        &index,
        "main-window",
        unix_time_seconds(),
        1,
    )?;
    validate_grants(&grants, &index)?;

    let root = &config.root;
    std::fs::create_dir_all(root).map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("创建插件目录失败：{e}"))
    })?;
    let id = manifest.id.to_string();
    let final_dir = root.join(&id);
    let plugin_id = manifest.id.clone();
    if state.registry.find(&plugin_id).is_some() {
        return Err(HostError::new(
            ErrorCode::E_PLUGIN_EXISTS,
            format!("插件 `{id}` 已存在于注册表"),
        ));
    }
    if final_dir.exists() {
        return Err(HostError::new(
            ErrorCode::E_PLUGIN_EXISTS,
            format!("插件 `{id}` 已有安装目录"),
        ));
    }
    let temp_dir = root.join(format!(".{id}.install-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temp_dir).map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("创建临时安装目录失败：{e}"))
    })?;
    let unpack_result = (|| -> HostResult<()> {
        let mut zip = zip::ZipArchive::new(archive)
            .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, format!("打开包失败：{e}")))?;
        // 解包常量（条目数 / 单文件 / 解压总量 / 压缩比）与路径清洗**不在这里**
        // 重复实现：它们在 `read_verified_package` →
        // `package_signature::verify_tpkg_reader[_with_time]` 里已经强制过一遍，
        // 而且那边还额外做了「每个条目必须被签名」与逐文件哈希比对。这里再写一份
        // 只会变成两处各自演化的策略副本（本仓已经因为这种副本吃过亏）。
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, e.to_string()))?;
            let relative = PathBuf::from(entry.name());
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || relative.components().any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err(HostError::new(ErrorCode::E_INSTALL_FAILED, "解包路径非法"));
            }
            let output = temp_dir.join(&relative);
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, e.to_string()))?;
            }
            if output.exists() {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("ZIP 路径重复：{}", relative.display()),
                ));
            }
            let mut file = std::fs::File::create(&output)
                .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, e.to_string()))?;
            std::io::copy(&mut entry, &mut file)
                .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, e.to_string()))?;
        }
        let js_entry = manifest
            .entry
            .js
            .as_ref()
            .ok_or_else(|| HostError::new(ErrorCode::E_INVALID_MANIFEST, "JS entry 缺失"))?;
        if !temp_dir.join(js_entry).is_file() {
            return Err(HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("包缺少 manifest entry `{js_entry}`"),
            ));
        }
        Ok(())
    })();
    if let Err(error) = unpack_result {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(error);
    }
    if let Err(error) = write_plugin_ui_activation(
        &temp_dir,
        &manifest,
        config.acl_signing_key.as_deref().unwrap_or_default(),
    ) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(error);
    }
    std::fs::rename(&temp_dir, &final_dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&temp_dir);
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("安装目录原子提交失败：{e}"))
    })?;

    let acl_store =
        AclStore::new(root.join(".acl"), config.acl_signing_key.clone().unwrap_or_default());
    let mut acl_saved = false;
    let result = (|| -> HostResult<()> {
        state.registry.install(&index, manifest.clone())?;
        match acl_store.save(&grants) {
            Ok(_) => acl_saved = true,
            Err(error) => {
                let _ = state.registry.admin_op(&plugin_id, RegistryAdminOp::Purge);
                return Err(error);
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        if acl_saved {
            let _ = std::fs::remove_file(acl_store.path_for(&id));
        }
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err(error);
    }
    Ok(PluginInstallResult {
        plugin_id: id,
        version: manifest.version.to_string(),
        install_path: final_dir.to_string_lossy().into_owned(),
        approved_permissions: approved.into_iter().collect(),
    })
}

#[cfg(feature = "plugin-install")]
fn unix_time_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(feature = "plugin-install")]
fn read_verified_package(
    state: &PluginRuntimeState,
    package_path: &str,
) -> HostResult<VerifiedPluginPackage> {
    let config = state.install_config.as_ref().ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "plugin-install 未配置安装目录、可信密钥与 ACL 密钥（拒绝操作）",
        )
    })?;
    if config.acl_signing_key.as_ref().is_none_or(|key| key.len() < 32) {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "ACL HMAC key 未配置或少于 32 字节",
        ));
    }
    let source = PathBuf::from(package_path);
    if source.extension().and_then(|e| e.to_str()) != Some("tpkg") {
        return Err(HostError::new(ErrorCode::E_INSTALL_FAILED, "仅支持本地 .tpkg 安装"));
    }
    let sidecar_path = PathBuf::from(format!("{package_path}.sig"));
    let mut archive = std::fs::File::open(&source)
        .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, format!("打开安装包失败：{e}")))?;
    let package_digest = digest_file_and_rewind(&mut archive)?;
    let sidecar = std::fs::read_to_string(&sidecar_path).map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("读取签名 sidecar 失败：{e}"))
    })?;
    let envelope: tauron_market::package_signature::PackageSignature =
        serde_json::from_str(&sidecar).map_err(|e| {
            HostError::new(ErrorCode::E_INSTALL_FAILED, format!("sidecar 无效：{e}"))
        })?;
    let public_key = config.signing_keys.get(&envelope.kid).ok_or_else(|| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("未信任的签名 kid `{}`", envelope.kid))
    })?;
    let verification = if let Some(provider) = config.trusted_time_provider.as_deref() {
        tauron_market::package_signature::verify_tpkg_reader_with_time(
            &mut archive,
            &sidecar,
            public_key,
            provider,
        )
    } else if state.deployment_mode == tauron_host::DeploymentMode::Production {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "production plugin installation requires a currently trusted time provider",
        ));
    } else {
        // Development/Test compatibility: preserve the pre-A100 local-clock behavior when the
        // host has not opted into a trusted-time provider.
        tauron_market::package_signature::verify_tpkg_reader(&mut archive, &sidecar, public_key)
    };
    let (verified, manifest) =
        verification.map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, e.to_string()))?;
    {
        use std::io::{Seek, SeekFrom};
        archive.seek(SeekFrom::Start(0)).map_err(|e| {
            HostError::new(ErrorCode::E_INSTALL_FAILED, format!("重置已验签包位置失败：{e}"))
        })?;
    }
    if verified.kid != envelope.kid {
        return Err(HostError::new(ErrorCode::E_INSTALL_FAILED, "验签 kid 不一致"));
    }
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|e| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("manifest canonicalization failed: {e}"),
        )
    })?;
    Ok(VerifiedPluginPackage {
        package_digest,
        manifest_digest: digest_bytes(&manifest_bytes),
        permission_digest: permission_digest(&manifest),
        key_id: envelope.kid,
        publisher_id: manifest.publisher.clone(),
        manifest,
        archive,
    })
}

pub fn cmd_registry_admin(
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
) -> HostResult<TransitionOutcome> {
    let id = PluginId::new(plugin_id)?;
    // A91（轮 47）：注册表闸门包住**整段**（含特性门后的落盘预备）。判定必须发生在
    // 一切副作用之前——若先 stage 再拒绝，staged backup 会留在盘上；且 admin_op 的
    // panic 现在会如实登记到注册表边界（而不是只被 guard 吞成一条 E_HOST_PANIC）。
    run_registry_boundary(state, "registry_admin", || {
        #[cfg(feature = "plugin-install")]
        let staged_cleanup = stage_install_cleanup(state, id.as_str(), op)?;
        let out = match state.registry.admin_op(&id, op) {
            Ok(out) => out,
            Err(error) => {
                #[cfg(feature = "plugin-install")]
                if let Some(stage) = staged_cleanup {
                    stage.restore()?;
                }
                return Err(error);
            }
        };

        #[cfg(feature = "plugin-install")]
        let cleanup_commit = if let Some(stage) = staged_cleanup {
            if out.illegal || out.from == out.to {
                stage.restore()?;
                None
            } else {
                Some(stage)
            }
        } else {
            None
        };

        // 卸载/清除成功后，**所有按插件 id 为键的旁路状态**必须一起回收——它们不随
        // 注册表条目一起消失，不回收就是缓慢泄漏（卸载→重装循环会持续累积）：
        // - 事件总线的发布/订阅（§8-3 零悬挂订阅），反向索引与队列以插件 id 为键；
        // - 多选择器订阅的分组登记（`subscription_groups`，以订阅者为键——不回收
        //   就是指向已消亡 token 的死条目）；
        // - 贡献注册表（菜单/命令/面板入口，残留会让卸载的插件入口还显示）；
        // - i18n 文案（`plugin:<id>.oc.*`，跨所有语言包）；
        // - 恢复引擎的插件登记（状态表是**持久化**的，残留会让已卸载插件永远
        //   出现在 `host_recover_boot` 的 `disabledPlugins` 里）。
        // 仅在迁移真的发生（非非法且状态确实改变）时回收——非法迁移的条目仍在
        // 注册表，回收它的旁路状态反而会造成不一致。
        if !out.illegal
            && out.from != out.to
            && matches!(op, RegistryAdminOp::Uninstall | RegistryAdminOp::Purge)
        {
            // 锁纪律：bus 锁**只**覆盖两个 dispose 调用，随后立即释放——不持着
            // bus 锁去取 subscription_groups / contributes / i18n / notify / recovery
            // 五把锁（此前整块共享一个 bus guard，形成一组未声明的嵌套序；虽然
            // 全仓没有反向获取路径、不构成死锁环，但任何一方未来持这些锁取 bus
            // 即成 ABBA）。各旁路状态的回收彼此独立，无需同持。
            {
                let bus = state.bus.lock();
                bus.dispose_publisher(plugin_id);
                bus.dispose_subscriber(plugin_id);
            }
            prune_subscription_groups(state, plugin_id);
            state.contributes.lock().clear_plugin(plugin_id);
            state.i18n.lock().cleanup_plugin(plugin_id);
            // - 通知存储（`host_notify` 落进环形缓冲的通知，残留会让已卸载插件的
            //   未读计数永远膨胀、通知中心显示死条目）；
            state.notify_store.lock().cleanup_plugin(plugin_id);
            state.recovery.lock().remove_plugin(plugin_id);
            // - 进程侧崩溃窗口（`ProcRuntime` 的 `CrashTracker`，同样**以插件 id 为键**）。
            //   注册表条目在上一步已被删掉，重装同名插件是被允许的——残留会让"修好的
            //   新版本"一上来就撞上旧版本的崩溃预算（5 分钟窗口内直接拒绝
            //   `host_runtime_spawn`），表现为"刚装的插件永远起不来"。
            //   `Enable`（人工确认路径）会清它，但卸载是**另一条出口**，两条都要清。
            state.proc_runtime.reset_crashes(plugin_id);
        }

        // **人工确认的唯一出口**（P0-2 崩溃预算）：Enable 是宿主 UI 的显式动作，
        // 因此它必须同时清零**两道**预算——状态机侧的 `ResetCounters`（迁移表里
        // `ERRORED_USER_CONFIRM + ENABLE → ENABLED` 已带）与进程侧的崩溃窗口
        // （`ProcRuntime::reset_crashes`）。只清前者会出现死结：用户已确认、状态机说
        // ENABLED、`host_runtime_spawn` 仍被崩溃窗口拒绝，而窗口是 5 分钟——
        // 表现为"点了启用但插件永远起不来"。
        //
        // 条件只排除**非法迁移**（状态机明确拒绝了这个事件，说明用户想做的事与当前
        // 状态无关）；`from == to`（本来就 ENABLED）也算用户确认，一并清零。
        if !out.illegal && matches!(op, RegistryAdminOp::Enable) {
            state.proc_runtime.reset_crashes(plugin_id);
        }

        // 管理操作会改变插件状态，对账一次保持恢复判定与注册表一致。
        // 注意：在安全模式下手动 Enable 一个非必需插件会被 `SafemodeEnter` 重新
        // 禁用——返回的 `TransitionOutcome` 描述的是本次迁移，不是最终状态，
        // 最终状态以 `host_registry_list` 为准。这是刻意设计：允许手动绕过会
        // 让安全模式失去意义。
        reconcile_recovery_phase(state);

        #[cfg(feature = "plugin-install")]
        if let Some(stage) = cleanup_commit {
            stage.commit()?;
        }

        Ok(out)
    })
}

/// `host_registry_admin` 的**带身份判定**版本（非 wire 的兼容入口）。
///
/// 判定必须在所有副作用之前：本命令会改注册表状态、回收旁路状态、清崩溃预算。
/// 拒绝路径**一个副作用都不产生**（有测试断言拒绝后注册表与插件状态不变）。
///
/// 轮 43：与安装域的 `cmd_registry_admin` 旧入口同构——生产档的破坏性操作
/// （uninstall/purge）必须走 [`cmd_registry_admin_reviewed_as`] 的审批令牌，
/// 本入口在生产档直接拒绝，防止绕过令牌强制。
pub fn cmd_registry_admin_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
) -> HostResult<TransitionOutcome> {
    admin_gate(&state.substrate, caller, "host_registry_admin")?;
    if admin_review_required(state) && is_destructive_admin_op(op) {
        return Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            "production uninstall/purge requires an AdminReviewToken from the preview path (*_reviewed_as)",
        ));
    }
    cmd_registry_admin(state, plugin_id, op)
}

/// `host_registry_admin` 的**审批令牌**入口（wire 层转调的就是它，轮 43）。
///
/// 形态与安装域完全同构：
/// - `preview = true`：只铸发令牌并返回将被破坏的事实（不产生任何副作用）；
///   仅对 uninstall/purge 有效；
/// - `review_token = Some`：一次性消费 + 事实重核（操作/插件/版本），失败不产生副作用；
/// - 两者都缺：生产档（且装插件特性开启）的 uninstall/purge 被拒；其余照旧执行。
pub fn cmd_registry_admin_reviewed_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
    preview: bool,
    review_token: Option<&AdminReviewToken>,
) -> HostResult<RegistryAdminResponse> {
    admin_gate(&state.substrate, caller, "host_registry_admin")?;
    let destructive = is_destructive_admin_op(op);
    if preview {
        if !destructive {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                "仅 uninstall/purge 支持审批预览（disable/enable 可逆且无破坏面）",
            ));
        }
        if review_token.is_some() {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                "审批预览与令牌提交不能在同一次调用里发生：preview 只铸发令牌",
            ));
        }
        // A91（轮 47）：与安装域 preview 同构——预览是审批域的修复入口，闸门故障时
        // 先做确定性清空再铸发；正常时幂等空操作。
        reconcile_review_boundary(state)?;
        let id = PluginId::new(plugin_id)?;
        let entry = state.registry.find(&id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_UNKNOWN_PLUGIN,
                format!("插件 `{plugin_id}` 不存在，无法预览破坏性操作"),
            )
        })?;
        let version = entry.manifest.version.to_string();
        let token = mint_admin_review(state, plugin_id, op, &version)?;
        return Ok(RegistryAdminResponse::Review(RegistryAdminReview {
            op,
            plugin_id: plugin_id.to_string(),
            version,
            state: entry.state.state,
            review_token: token,
        }));
    }
    if let Some(token) = review_token {
        if !destructive {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                "该操作不接受审批令牌（仅 uninstall/purge 的破坏性路径消费令牌）",
            ));
        }
        validate_admin_review(state, plugin_id, op, token)?;
    } else if destructive && admin_review_required(state) {
        return Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            "生产档的 uninstall/purge 要求先经 preview 取得一次性审批令牌（A83）",
        ));
    }
    let outcome = cmd_registry_admin(state, plugin_id, op)?;
    Ok(RegistryAdminResponse::Executed(outcome))
}

#[cfg(feature = "plugin-install")]
struct InstallCleanupStage {
    plugin_path: PathBuf,
    plugin_backup: Option<PathBuf>,
    acl_path: PathBuf,
    acl_backup: Option<PathBuf>,
}

#[cfg(feature = "plugin-install")]
impl InstallCleanupStage {
    fn restore(self) -> HostResult<()> {
        if let Some(backup) = self.plugin_backup {
            std::fs::rename(&backup, &self.plugin_path).map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("恢复插件安装目录失败 `{}`：{error}", self.plugin_path.display()),
                )
            })?;
        }
        if let Some(backup) = self.acl_backup {
            std::fs::rename(&backup, &self.acl_path).map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("恢复插件 ACL 授予失败 `{}`：{error}", self.acl_path.display()),
                )
            })?;
        }
        Ok(())
    }

    fn commit(self) -> HostResult<()> {
        let mut first_error = None;
        if let Some(backup) = self.plugin_backup {
            if let Err(error) = std::fs::remove_dir_all(&backup) {
                first_error =
                    Some(format!("清理已卸载插件目录 `{}` 失败：{error}", backup.display()));
            }
        }
        if let Some(backup) = self.acl_backup {
            if let Err(error) = std::fs::remove_file(&backup) {
                first_error.get_or_insert_with(|| {
                    format!("清理已卸载插件 ACL `{}` 失败：{error}", backup.display())
                });
            }
        }
        if let Some(message) = first_error {
            return Err(HostError::new(ErrorCode::E_INSTALL_FAILED, message));
        }
        Ok(())
    }
}

/// Atomically hide installed files and grants before changing registry state. This keeps
/// failed/illegal lifecycle transitions from leaving a registry/filesystem split-brain.
#[cfg(feature = "plugin-install")]
fn stage_install_cleanup(
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
) -> HostResult<Option<InstallCleanupStage>> {
    if !matches!(op, RegistryAdminOp::Uninstall | RegistryAdminOp::Purge) {
        return Ok(None);
    }
    let Some(config) = state.install_config.as_ref() else {
        return Ok(None);
    };
    let Ok(root) = config.root.canonicalize() else {
        return Ok(None);
    };
    let plugin_path = root.join(plugin_id);
    let acl_dir = root.join(".acl");
    let acl_path = acl_dir.join(format!("{plugin_id}.acl.json"));
    let suffix = uuid::Uuid::new_v4();
    let plugin_backup = root.join(format!(".{plugin_id}.uninstall-{suffix}"));
    let acl_backup = acl_dir.join(format!(".{plugin_id}.uninstall-{suffix}.acl.json"));

    let mut plugin_staged = false;
    let mut acl_staged = false;
    match std::fs::symlink_metadata(&plugin_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    "插件安装路径不是安全的普通目录；拒绝卸载以避免越界删除",
                ));
            }
            let canonical = plugin_path.canonicalize().map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("解析插件安装路径失败：{error}"),
                )
            })?;
            if !canonical.starts_with(&root) || canonical == root {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    "插件安装路径越出安装根目录；拒绝卸载",
                ));
            }
            std::fs::rename(&plugin_path, &plugin_backup).map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("暂存插件安装目录失败：{error}"),
                )
            })?;
            plugin_staged = true;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("检查插件安装目录失败：{error}"),
            ))
        }
    }

    match std::fs::symlink_metadata(&acl_dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                if plugin_staged {
                    let _ = std::fs::rename(&plugin_backup, &plugin_path);
                }
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    "ACL 目录不是安全的普通目录；拒绝卸载",
                ));
            }
            let canonical_acl_dir = match acl_dir.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    if plugin_staged {
                        let _ = std::fs::rename(&plugin_backup, &plugin_path);
                    }
                    return Err(HostError::new(
                        ErrorCode::E_INSTALL_FAILED,
                        format!("解析 ACL 目录失败：{error}"),
                    ));
                }
            };
            if !canonical_acl_dir.starts_with(&root) || canonical_acl_dir == root {
                if plugin_staged {
                    let _ = std::fs::rename(&plugin_backup, &plugin_path);
                }
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    "ACL 目录越出安装根目录；拒绝卸载",
                ));
            }
            match std::fs::symlink_metadata(&acl_path) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() || !meta.is_file() {
                        if plugin_staged {
                            let _ = std::fs::rename(&plugin_backup, &plugin_path);
                        }
                        return Err(HostError::new(
                            ErrorCode::E_INSTALL_FAILED,
                            "ACL 授予路径不是普通文件；拒绝卸载",
                        ));
                    }
                    if let Err(error) = std::fs::rename(&acl_path, &acl_backup) {
                        if plugin_staged {
                            let _ = std::fs::rename(&plugin_backup, &plugin_path);
                        }
                        return Err(HostError::new(
                            ErrorCode::E_INSTALL_FAILED,
                            format!("暂存 ACL 授予失败：{error}"),
                        ));
                    }
                    acl_staged = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    if plugin_staged {
                        let _ = std::fs::rename(&plugin_backup, &plugin_path);
                    }
                    return Err(HostError::new(
                        ErrorCode::E_INSTALL_FAILED,
                        format!("检查 ACL 授予失败：{error}"),
                    ));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            if plugin_staged {
                let _ = std::fs::rename(&plugin_backup, &plugin_path);
            }
            return Err(HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("检查 ACL 目录失败：{error}"),
            ));
        }
    }
    Ok(Some(InstallCleanupStage {
        plugin_path,
        plugin_backup: plugin_staged.then_some(plugin_backup),
        acl_path,
        acl_backup: acl_staged.then_some(acl_backup),
    }))
}
