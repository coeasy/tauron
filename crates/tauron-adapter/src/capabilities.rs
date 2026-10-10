// T-7 拆分：从 `lib.rs` 逐字节纯搬移的能力协商命令族（十九片）。
//
// `use super::*;` 让子模块看到 crate 根的全部项——含私有助手（guard、
// brand_configured）与公共 const（SUBSTRATE_COMMANDS / PLUGIN_RUNTIME_COMMANDS /
// PLUGIN_INSTALL_COMMANDS），因此搬来的命令体可原样调用，零 `pub(crate)` 放宽。
// 广域线协议基础设施 UnsupportedBody / ProviderResult / DegradedValue /
// unsupported_body 与三条命令名 const、brand_configured 刻意留在 `lib.rs`。

use super::*;

/// 可选能力未装配时的明确说明。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnsupportedDomain {
    pub domain: String,
    pub reason: String,
}

/// Runtime enforcement strength for a capability domain.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityEnforcement {
    pub domain: String,
    /// Closed vocabulary: hard / partial / unsupported.
    pub level: String,
    pub detail: String,
}

/// 宿主当前实际装配的命令面与未实现能力。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilitiesBody {
    pub families: Vec<String>,
    pub commands: Vec<String>,
    pub unsupported: Vec<UnsupportedDomain>,
    /// V4: machine-readable strength, not just available/unavailable.
    pub enforcement: Vec<CapabilityEnforcement>,
    pub plugin_runtime: bool,
    /// V4 compile-time target fact used by ArtifactVariantResolver before any OS loader call.
    pub target: tauron_host::TargetSpec,
}

/// 返回当前装配形态能力快照。能力列表与 handler 宏由 wire-gate 锁定一致。
///
/// # `families` 与 `unsupported` 由运行期事实**推导**，且两列**互斥**
///
/// - `families` = 命令面已装配**且**运行期提供者可用（或该域无 provider 依赖）的域；
/// - `unsupported` = 命令面已装配但提供者**未接线**的域。
///
/// 判定依据是各域 sink 的 `native_supported()` / 等价运行期事实（`fs` 看允许根是否
/// 非空，`brand` 看来源环境变量是否设置），**不是静态清单**（这条对应已知问题 P2-4：
/// 此前两列都是硬编码，且同一域会同时出现在两侧——既声称已装配又声称未实现，是
/// 自相矛盾的过度声明）。
///
/// **诚实语义**：缺省装配（无 `tauri` feature）下 menu / tray / http / updater 确实
/// 不可用，故落在 `unsupported`；注入对应 sink 后自动移入 `families`。同一进程内结果
/// 会随装配与环境变化——这是**快照**，不是常量。
pub fn cmd_host_capabilities(state: &SubstrateState) -> HostResult<CapabilitiesBody> {
    guard("host_capabilities", || {
        let plugin_runtime = state.plugin_flags.get().is_some();
        let mut commands = SUBSTRATE_COMMANDS.to_vec();
        if plugin_runtime {
            commands.extend_from_slice(PLUGIN_RUNTIME_COMMANDS);
            #[cfg(feature = "plugin-install")]
            commands.extend_from_slice(PLUGIN_INSTALL_COMMANDS);
        }

        // 无 provider 依赖的域：命令面装配即可用。
        let mut families: Vec<String> =
            ["shell", "ipc", "settings", "i18n", "notify", "recovery", "theme"]
                .into_iter()
                .map(str::to_string)
                .collect();

        // 有 provider 语义的域：可用 → families，不可用 → unsupported（**二选一**）。
        // (域名, 是否可用, 不可用原因)
        let probes: [(&str, bool, &str); 8] = [
            (
                "menu",
                state.menu_sink.native_supported(),
                "菜单提供者未注入：缺省为进程内留痕实现，不建任何菜单",
            ),
            (
                "tray",
                state.tray_sink.native_supported(),
                "托盘提供者未注入：缺省为进程内留痕实现，不建任何托盘图标",
            ),
            (
                "fs",
                !state.fs_allowed_roots.is_empty(),
                "fs 提供者未配置：允许根目录为空（经 AdapterConfig::with_fs_roots 配置）",
            ),
            (
                "http",
                state.http_sink.native_supported() && !state.http_policy.domains.is_empty(),
                "HTTP provider 或 V4 network scope 未配置",
            ),
            (
                "updater",
                state.updater_sink.native_supported(),
                "更新端点未注入：缺省无 EndpointClient（用 DistributeUpdaterSink::with_endpoint 注入）",
            ),
            (
                "brand",
                brand_configured(),
                "品牌来源未配置：设置 TAURON_BRAND_CONFIG_JSON 或 TAURON_BRAND_CONFIG",
            ),
            (
                "dialog",
                state.dialog_sink.native_supported(),
                "对话框提供者未接入：缺省为不显示任何 UI 的降级实现",
            ),
            (
                "deep-link-os",
                state.deep_link_sink.native_supported(),
                "无 OS 级深链接注册：tauri-plugin-deep-link 不在依赖闭包内",
            ),
        ];

        let mut unsupported: Vec<UnsupportedDomain> = Vec::new();
        for (domain, available, reason) in probes {
            if available {
                families.push(domain.to_string());
            } else {
                unsupported.push(UnsupportedDomain {
                    domain: domain.to_string(),
                    reason: reason.to_string(),
                });
            }
        }

        // 无 provider 抽象、当前明确未实现的两个域：恒列 unsupported。
        for (domain, reason) in [
            ("clipboard", "无 OS 级剪贴板：当前为进程内缓冲区（真实但非系统剪贴板）"),
            (
                "market-update",
                "缺省装配无更新通道：host_market_check 是桩（不做探测）；host_market_download \
                 / host_market_install 在宿主装配 UpgradeInstaller 后为真（缺省如实模拟，\
                 不落任何字节）。可用性检查请走 updater 域（host_updater_check，注入端点即为真）",
            ),
        ] {
            unsupported
                .push(UnsupportedDomain { domain: domain.to_string(), reason: reason.to_string() });
        }

        let process_sandbox = state.process_sandbox.get().cloned();
        let (process_sandbox_level, process_sandbox_detail) = if !plugin_runtime {
            ("unsupported", "plugin runtime is not installed".to_string())
        } else {
            let descriptor = process_sandbox.unwrap_or_else(|| {
                tauron_proc::ProcessSandboxDescriptor::unsupported(
                    "plugin runtime did not publish a process sandbox descriptor",
                )
            });
            (descriptor.enforcement.as_str(), descriptor.detail)
        };
        if plugin_runtime && process_sandbox_level != "unsupported" {
            families.push("process-sandbox".to_string());
        } else {
            unsupported.push(UnsupportedDomain {
                domain: "process-sandbox".to_string(),
                reason: process_sandbox_detail.clone(),
            });
        }

        let fs_level = if state.fs_allowed_roots.is_empty() {
            "unsupported"
        } else {
            match tauron_host::scoped_fs_enforcement() {
                tauron_host::FsEnforcement::Hard => "hard",
                tauron_host::FsEnforcement::Partial => "partial",
                tauron_host::FsEnforcement::Unsupported => "unsupported",
            }
        };
        let http_level =
            if !state.http_sink.native_supported() || state.http_policy.domains.is_empty() {
                "unsupported"
            } else {
                match state.http_sink.network_enforcement() {
                    tauron_host::NetworkEnforcement::RedirectAndDns => "hard",
                    tauron_host::NetworkEnforcement::UrlOnly => "partial",
                }
            };
        let enforcement = vec![
            CapabilityEnforcement {
                domain: "fs".to_string(),
                level: fs_level.to_string(),
                detail: match fs_level {
                    "hard" => "root-handle relative I/O with no-follow enforcement".to_string(),
                    "partial" => {
                        "platform fallback does not yet provide native reparse/junction handle validation"
                            .to_string()
                    }
                    _ => "filesystem scope is not configured".to_string(),
                },
            },
            CapabilityEnforcement {
                domain: "http".to_string(),
                level: http_level.to_string(),
                detail: match http_level {
                    "hard" => {
                        "URL, redirect, DNS/private-network and credential-scope policy enforced"
                            .to_string()
                    }
                    "partial" => {
                        "provider does not prove redirect-and-DNS enforcement".to_string()
                    }
                    _ => "HTTP provider or network scope is not configured".to_string(),
                },
            },
            CapabilityEnforcement {
                domain: "process-sandbox".to_string(),
                level: process_sandbox_level.to_string(),
                detail: process_sandbox_detail,
            },
        ];

        Ok(CapabilitiesBody {
            families,
            commands: commands.into_iter().map(str::to_string).collect(),
            unsupported,
            enforcement,
            plugin_runtime,
            target: tauron_host::current_target_spec(),
        })
    })?
}
