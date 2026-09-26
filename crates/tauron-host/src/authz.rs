//! §4.21 宿主命令三档授权（ADR-17）。
//!
//! 职责：命令 → 档位映射表；`self` 档的身份绑定与入参 pluginId 忽略；
//! `scoped-read` 的结果过滤；`privileged` 的授予检查。
//!
//! 关键约束（计划 §4.21）：
//! - **映射表为数组常量**（CI 遍历校验）；
//! - **新增命令未登记档位 → 编译期/CI 失败**（门禁 §8-17）；
//! - **禁止授予清单在此单点硬拒**（§2.1）。
//!
//! 第四轮修订：
//! - D15：`RegistryAdminOp` 枚举定稿，且 `host_registry_admin` **不是插件命令面**
//!   ——调用方是宿主 UI（主窗），故单列为 [`ADMIN_COMMANDS`]；
//! - D16：`host_grant_request` 在 v1 删除（无运行期消费场景）；
//! - D2：`host_plugin_call` 取代信封式 `host_call_begin`，本表登记为 `self` 档。
//!
//! 第七轮补充（R7 收口）：`host_events_drain` / `host_stream_open` /
//! `host_stream_write` / `host_stream_close` 这 4 条**插件面可触达**的命令此前
//! 漏登记（`HostClient` 早就在调它们），现按实现补齐为 `self` 档。
//!
//! **本表的范围**（刻意不扩大）：插件可触达命令面 + 主窗管理命令。其余 34 条
//! 底座/主窗命令（窗口、i18n、brand、notify、settings、recovery、market、
//! dialog、clipboard 等）由 Tauri ACL / `origin_gate` 管辖，不进本表——
//! 本表是「插件能碰到什么」的授权口径，不是全部命令的清单。
//!
//! 测试点（计划 §4.21）：低权限插件尝试 `host_registry_list` → 仅拿到可见集；
//! 跨插件 `host_lifecycle_report` 冒充被拒；未登记命令被 CI 拦。

use crate::error::{ErrorCode, HostError, HostResult};
use crate::manifest::{Permission, PermissionIndex, Risk};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use std::collections::HashMap;

/// 授权档位（ADR-17）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthTier {
    /// 免授予，但只能作用于调用者**自己**；pluginId 从身份取，忽略入参。
    Self_,
    /// 免授予，但结果按可见性过滤。
    ScopedRead,
    /// 需显式授予（主窗/宿主 UI）。
    Privileged,
}

impl AuthTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Self_ => "self",
            Self::ScopedRead => "scoped-read",
            Self::Privileged => "privileged",
        }
    }
}

impl std::fmt::Display for AuthTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 命令登记项（门禁 §8-17 的数据载体）。
///
/// `consumer` 字段是第四轮"命令面消费方登记"的要求：每条命令必须写清谁调用，
/// 否则就是孤儿命令面（前三轮 `host_call_begin` 就栽在这里）。
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CommandAuth {
    /// Tauri 命令名（不含 `plugin:tauron|` 前缀）。
    pub command: &'static str,
    pub tier: AuthTier,
    /// 已登记的调用方（门禁 §8-17）。
    pub consumer: &'static str,
    pub description: &'static str,
}

/// 插件侧宿主命令面（§2.1 定稿 9 条 + R7 收口补登记 4 条 + 0.4 审计补登记 1 条
/// + 0.4-A1 调用投递 3 条 = 17 条）。
pub static COMMANDS: &[CommandAuth] = &[
    CommandAuth {
        command: "host_plugin_call",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk createPlugin()",
        description: "C/D 类插件的 JS↔宿主调用往返（取代信封式 host_call_begin）",
    },
    CommandAuth {
        command: "host_call_end",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（流式终帧确认）",
        description: "流式调用终帧确认",
    },
    CommandAuth {
        command: "host_cancel",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk",
        description: "取消调用，取消传播到 sidecar/supervisor",
    },
    CommandAuth {
        command: "host_lifecycle_report",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk 生命周期钩子",
        description: "上报生命周期事件（state/reason）",
    },
    CommandAuth {
        command: "host_contributes_register",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（attach 期）",
        description: "注册 contributes（commands/menus/panels/…）",
    },
    CommandAuth {
        command: "host_events_publish",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk",
        description: "事件发布唯一入口（越界丢弃+计数）",
    },
    CommandAuth {
        command: "host_events_subscribe",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk",
        description: "事件订阅（跨插件订阅需对方 public:true）",
    },
    CommandAuth {
        command: "host_events_unsubscribe",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（dispose 期）",
        description: "事件退订（窗口销毁时由宿主 on_window_event → cleanup_closed_window 回收）",
    },
    CommandAuth {
        command: "host_registry_list",
        tier: AuthTier::ScopedRead,
        consumer: "plugin-sdk / <oc-plugin-manager>",
        description: "列出可见插件（结果按可见性过滤）",
    },
    // ── R7 收口：以下是**插件面可触达**但此前未登记档位的命令 ──────────
    //
    // `HostClient`（`packages/tauron-host/src/host.ts`）一直在调用这 4 条，
    // 但它们既不在本表、也不在 `ADMIN_COMMANDS`——「未登记命令被 CI 拦」
    // 是模块文档里的承诺，而那条门禁此前并不存在，于是缺口一直没被拦。
    // 后果是真实的：`packages/tauron-host/src/capabilities.ts` 的
    // `CAPABILITIES` 被脚手架（`tauron-app-cli` 的 `validateCapabilities`）
    // 当作能力白名单，未登记的命令在脚手架应用里根本无法启用。
    //
    // 档位一律 `Self_`，与各自 wire 入口的实现一致（都从 `WebviewWindow`
    // 的 label 解析身份，忽略入参里的身份字段）。
    CommandAuth {
        command: "host_events_drain",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（事件取件泵）",
        description: "拉取本插件待投递帧（只取自己订阅的可见集）",
    },
    CommandAuth {
        command: "host_stream_open",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（流式开流）",
        description: "为一次已挂帧载体的调用开流（self 档）",
    },
    CommandAuth {
        command: "host_stream_write",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（流式写帧）",
        description: "写一帧（self 档，seq 由宿主铸）",
    },
    CommandAuth {
        command: "host_stream_close",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（流式收尾）",
        description: "发终帧并使句柄失效（self 档）",
    },
    // ── 0.4 审计补登记：与 `host_registry_list` 同族的 scoped-read 只读命令 ──
    //
    // `ShellClient.contributesList()` 一直在调它，且它注册在 handler 宏里任何
    // webview 都可触达，却不在本表也不在 `ADMIN_COMMANDS`——「未登记命令被 CI
    // 拦」的缺口（R7 收口同类）。实现是纯只读（列贡献表，无身份写入面），
    // 档位与 `host_registry_list` 同为 `ScopedRead`。置于 R7 块之后、0.4-A1
    // 块之前：R7 的四连块与 A1 的表末三连块各自保持连续（镜像表按序比对）。
    CommandAuth {
        command: "host_contributes_list",
        tier: AuthTier::ScopedRead,
        consumer: "ShellClient.contributesList / 应用设置中心",
        description: "列出贡献表（commands/menus/panels/settings，纯只读）",
    },
    // ── 0.4-A1 调用投递闭环 ──────────────────────────────────────
    CommandAuth {
        command: "host_call_plugin",
        tier: AuthTier::Self_,
        consumer: "宿主主窗 / plugin-sdk（插件→插件）",
        description: "跨主体调用：宿主调插件或插件调插件（caller/target 显式）",
    },
    CommandAuth {
        command: "host_call_result",
        tier: AuthTier::Self_,
        consumer: "plugin-sdk（执行方回填）",
        description: "执行方回填一次调用的结果（仅 target 可回填）",
    },
    CommandAuth {
        command: "host_call_take",
        tier: AuthTier::Self_,
        consumer: "宿主主窗 / plugin-sdk（发起方取件）",
        description: "发起方取走一次已结算的结果（仅 caller 可取）",
    },
];

/// 主窗特权命令（D15：不是插件命令面）。
///
/// **档位的执行点（R7 收口后）**：本表的 `privileged` 档**不再只靠部署配置**
/// （origin 白名单 / Tauri ACL）成立——`tauron-adapter` 的 wire 包装器会从
/// `WebviewWindow` 的 label 解析主体，并按本表的档位语义做**代码层判定**
/// （特权 = 仅主窗；畸形 `plugin-` label 一律拒绝，绝不降级成主窗）。
/// 判定与表同源：有测试逐条遍历本表，断言每条 `privileged` 命令对插件主体都被拒
/// （见 `tauron-adapter` 的 `every_privileged_command_in_the_authz_table_denies_plugins`）。
/// 改档位时两侧一起改，否则那条测试会红。
///
/// # 本表**不**收录、但同样有代码层身份判定的命令（轮 11）
///
/// 收录范围是「命令面 + 档位」的登记问题，**不是**判定存在与否的问题。下面这些
/// 命令刻意**不进本表**（它们既不是插件可触达的插件命令面，也不需要授予权限，
/// 塞进来会污染 `COMMANDS` 的插件面语义与 TS 侧的 1:1 镜像表），但它们的判定
/// 已经落在 `tauron-adapter` 的核心函数里，与上面的 `privileged` 档**同一套写法、
/// 同一个拒绝码**（`E_AUTH_DENIED`）。改这些命令时不要只看本表：
///
/// - **仅主窗**（`require_main_window`）：`host_recover_trial_enable`、
///   `host_market_check`、`host_market_download`、`host_market_install`
///   （分别是"改别人的状态 + 烧别人的试验预算"、以及宿主级更新通道）；
///   轮 11 第三批又补了两条**全局/应用级**状态：`host_i18n_set_locale`（切的是
///   宿主 UI + 所有插件共用的语言）、`host_deep_link_register`（注册的是应用级
///   OS 协议，且会注销上一个协议——R8 曾把它当 self-service，是错的）；
/// - **身份绑定参数**（`require_self_plugin_scope`，主窗任意 / 插件只能是自己，
///   `None` 这类宿主级取值对插件一律拒绝）：`host_notify`（署名）、
///   `host_i18n_load`（命名空间归属）、`host_i18n_cleanup_plugin`（销毁目标）、
///   `host_recover_report`（失败预算与故障归因的归属）、`host_notifications_read`
///   （改的是**已读状态**：插件只能标记自己的通知，`None` = 全部已读属全局档，
///   未知 id 也拒绝——否则 `marked` 会变成"这个 id 存不存在"的预言机）；
/// - **按身份过滤而非拒绝**（`visible_notifications`，scoped-read 口径，与
///   `host_registry_list` 同族）：`host_notifications_list`。插件读**自己的**通知
///   是正当功能，所以不能一刀切拒绝；但 `items` / `total` / `unread` / `dispatchLog`
///   必须**同源**于可见集合——只裁数组、留着全局未读数仍然是泄露。
///
/// 实现位置：`tauron-adapter` 的 `*_as` 核心函数（每个都写清了"为什么这条要绑
/// 身份 / 为什么这条主窗专属"），wire 包装器一律 `Caller::from_label(window.label())`
/// 取主体，**不硬编码** `Caller::MainWindow`。
pub static ADMIN_COMMANDS: &[CommandAuth] = &[
    CommandAuth {
        command: "host_registry_admin",
        tier: AuthTier::Privileged,
        consumer: "宿主 UI 主窗（<oc-plugin-manager>）",
        description: "管理操作：disable/enable/uninstall/purge",
    },
    // P0-2：进程插件运行时。**必须是特权档**——`host_runtime_spawn` 会按调用方
    // 给出的 `plugin_id` 启动一个可执行文件，若插件 webview 可调用，任何插件都能
    // 启动别的插件的 sidecar（越权执行原语）。故与注册表管理操作同级：仅主窗/宿主 UI。
    CommandAuth {
        command: "host_runtime_spawn",
        tier: AuthTier::Privileged,
        consumer: "宿主 UI 主窗（插件生命周期监管）",
        description: "启动进程插件 sidecar（幂等：已有租约则返回既有 pid/lease）",
    },
    CommandAuth {
        command: "host_runtime_health",
        tier: AuthTier::Privileged,
        consumer: "宿主 UI 主窗（插件生命周期监管）",
        description: "按租约查询 sidecar 健康（pid/崩溃窗口计数；暴露 PID 故同属特权）",
    },
    CommandAuth {
        command: "host_resource_stats",
        tier: AuthTier::Privileged,
        consumer: "宿主 UI 主窗（资源配额诊断）",
        description: "读取全局与逐插件资源配额占用",
    },
];

/// 管理操作枚举（D15 定稿）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegistryAdminOp {
    Disable,
    Enable,
    Uninstall,
    Purge,
}

impl RegistryAdminOp {
    pub fn all() -> &'static [RegistryAdminOp] {
        &[Self::Disable, Self::Enable, Self::Uninstall, Self::Purge]
    }
}

/// 按命令名解析档位。未登记返回 `None`。
pub fn resolve(command: &str) -> Option<&'static CommandAuth> {
    COMMANDS.iter().chain(ADMIN_COMMANDS.iter()).find(|c| c.command == command)
}

/// 未登记命令硬失败（§8-17 自查入口）。
///
/// 适用对象是**插件侧命令面**（[`COMMANDS`]）；运行期分发并不经过统一
/// dispatcher——每条命令的档位语义在各自 wire 入口落地（self 档绑 label、
/// privileged 档限主窗），登记表的强制兑底是门禁测试
/// （`validate_command_registry` + TS 镜像），故本函数在仓内由测试调用，
/// 供集成方在自建分发时自查。
pub fn require_registered(command: &str) -> HostResult<&'static CommandAuth> {
    resolve(command).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "宿主命令 `{command}` 未在授权档位表登记（§8-17：新增命令必须先登记 self/scoped-read/privileged）"
            ),
        )
    })
}

/// 校验命令登记表的唯一性与字段完备性（CI 门禁 §8-17 的可测内核）。
///
/// 接受任意登记项迭代器，便于单元测试构造反例（空字段、重复命令）。
pub fn validate_commands<'a>(
    commands: impl IntoIterator<Item = &'a CommandAuth>,
) -> HostResult<()> {
    let mut seen: HashMap<&str, &CommandAuth> = HashMap::new();
    for c in commands {
        if c.command.is_empty() || c.description.is_empty() || c.consumer.is_empty() {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("命令 `{}` 字段缺失（command/description/consumer 均必填）", c.command),
            ));
        }
        if let Some(prev) = seen.insert(c.command, c) {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "命令 `{}` 重复登记（{} vs {}）",
                    c.command, prev.description, c.description
                ),
            ));
        }
    }
    Ok(())
}

/// 校验内置命令面（CI 门禁 §8-17）。
pub fn validate_command_registry() -> HostResult<()> {
    validate_commands(COMMANDS.iter().chain(ADMIN_COMMANDS.iter()))
}

/// 身份 Webview 的 label 前缀。宿主生成，插件不可指定（§4.6）。
pub const IDENTITY_LABEL_PREFIX: &str = "plugin-";

/// 从 label 解析插件 id。
pub fn label_to_plugin_id(label: &str) -> HostResult<crate::manifest::PluginId> {
    let id = label.strip_prefix(IDENTITY_LABEL_PREFIX).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "webview label `{label}` 不以 `{IDENTITY_LABEL_PREFIX}` 开头，不是插件身份单元"
            ),
        )
    })?;
    crate::manifest::PluginId::new(id).map_err(|_| {
        HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!("webview label `{label}` 中的插件 id 不合法"),
        )
    })
}

/// `self` 档身份解析：**身份从 label 取，忽略入参 pluginId**。
///
/// 这是 §4.1 的关键约束与 §4.21 的测试点"跨插件冒充被拒"的落地点：
/// 插件在入参里伪造 pluginId 会被硬拒，因为身份只来自宿主生成的 label。
pub fn resolve_self_identity(
    webview_label: &str,
    claimed_id: Option<&str>,
) -> HostResult<crate::manifest::PluginId> {
    let actual = label_to_plugin_id(webview_label)?;
    if let Some(claimed) = claimed_id {
        if claimed != actual.as_str() {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "入参 pluginId `{claimed}` 与身份 `{actual}` 不一致；self 档忽略入参、仅认身份（§2.1）"
                ),
            ));
        }
    }
    Ok(actual)
}

// ──────────────────────────────────────────────────────────────────────────
// 身份主体模型（R4 / D1）
// ──────────────────────────────────────────────────────────────────────────

/// webview 的身份主体。
///
/// 此前身份模型只区分「是插件」与「`pluginId` 为空」，**主窗因此是二等公民**：
/// 它的特权来自「没有身份」而不是来自一个显式主体，既无法给主窗分级，也无法
/// 表达「主窗绑定了哪个 origin / lease」。
///
/// 安全要点：[`Principal::Invalid`] 是**显式**归宿——`plugin-` 前缀但 id 非法的
/// label **绝不**降级为 [`Principal::MainWindow`]，否则伪造一个畸形 label 就能
/// 拿到主窗档权力（提权）。调用方遇到 `Invalid` 必须拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// 插件 webview（label = `plugin-<合法 id>`）。
    Plugin(crate::manifest::PluginId),
    /// 主窗 / 应用自身的非插件 webview（特权主体）。
    MainWindow,
    /// label 以 `plugin-` 开头但 id 非法：不在身份模型内，调用方必须拒绝。
    Invalid(String),
}

impl Principal {
    /// 插件 id；非插件主体返回 `None`。
    pub fn plugin_id(&self) -> Option<&crate::manifest::PluginId> {
        match self {
            Self::Plugin(id) => Some(id),
            _ => None,
        }
    }

    /// 是否为插件主体。
    pub fn is_plugin(&self) -> bool {
        matches!(self, Self::Plugin(_))
    }

    /// 是否为非法身份（调用方应拒绝，**不得**按主窗放行）。
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }

    /// 主体种类名（诊断 / 线格式用）。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Plugin(_) => "plugin",
            Self::MainWindow => "main-window",
            Self::Invalid(_) => "invalid",
        }
    }
}

/// 从 webview label 解析身份主体（R4）。
///
/// 与 [`resolve_self_identity`] 的分工：后者用于 **self 档**（非插件 label 直接
/// 报错），本函数用于需要区分主体的场景（主窗 / 插件 / 非法），**不返回错误**。
pub fn resolve_principal(webview_label: &str) -> Principal {
    match webview_label.strip_prefix(IDENTITY_LABEL_PREFIX) {
        None => Principal::MainWindow,
        Some(raw) => match crate::manifest::PluginId::new(raw) {
            Ok(id) => Principal::Plugin(id),
            Err(_) => Principal::Invalid(webview_label.to_string()),
        },
    }
}

#[cfg(test)]
mod principal_tests {
    use super::*;

    #[test]
    fn plugin_label_resolves_to_plugin_principal() {
        let p = resolve_principal("plugin-com.example.a");
        assert_eq!(p.kind(), "plugin");
        assert!(p.is_plugin());
        assert_eq!(p.plugin_id().map(|i| i.as_str()), Some("com.example.a"));
    }

    #[test]
    fn non_plugin_label_is_main_window() {
        for label in ["main", "settings", "w1"] {
            let p = resolve_principal(label);
            assert_eq!(p, Principal::MainWindow, "label `{label}` 应为主窗主体");
            assert!(p.plugin_id().is_none());
            assert!(!p.is_invalid());
        }
    }

    #[test]
    fn malformed_plugin_label_is_invalid_not_main_window() {
        // 关键安全断言：畸形 `plugin-` label **不得**降级为主窗（提权防线）。
        for label in ["plugin-", "plugin-com.example.a:settings", "plugin-bad id"] {
            let p = resolve_principal(label);
            assert!(p.is_invalid(), "label `{label}` 应为 Invalid");
            assert!(!p.is_plugin());
            assert_ne!(p, Principal::MainWindow, "畸形 label 不得当作主窗");
            assert!(p.plugin_id().is_none());
        }
    }

    #[test]
    fn resolve_principal_agrees_with_resolve_self_identity() {
        // 两条路径对合法插件 label 必须给出一致的 id。
        let p = resolve_principal("plugin-com.example.a");
        let s = resolve_self_identity("plugin-com.example.a", None).unwrap();
        assert_eq!(p.plugin_id(), Some(&s));
    }
}

// ──────────────────────────────────────────────────────────────────────────
// origin 允许清单（R4-D2）
// ──────────────────────────────────────────────────────────────────────────

/// 取不到调用方 origin 时使用的哨兵。
///
/// 该值**永远不匹配**任何清单项（即使运维把哨兵本身写进清单）——否则「清单里恰好
/// 有哨兵」就等于放行所有取不到 origin 的调用方，把 fail-closed 变成 fail-open。
pub const ORIGIN_UNKNOWN: &str = "<unknown>";

/// 规范化 origin：去首尾空白与尾随 `/`。
///
/// `Url::origin().ascii_serialization()` 已把 scheme/host 规范化为小写，因此逐字
/// 比较是充分的；此处只容忍运维手写清单时多打的尾随斜杠与空白。
fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_string()
}

/// **origin 允许清单判定（R4-D2 的唯一策略点）**。
///
/// - **空清单 = 不启用** → 一律放行（兼容既有装配；方案 §R4 不变量）。
/// - 非空清单 = **fail-closed**：逐字命中才放行；[`ORIGIN_UNKNOWN`] 与空串一律
///   拒绝，且**不接受**把哨兵写进清单来「放行未知来源」。
pub fn origin_allowed(allowlist: &[String], origin: &str) -> bool {
    if allowlist.is_empty() {
        return true;
    }
    if origin == ORIGIN_UNKNOWN || origin.trim().is_empty() {
        return false;
    }
    let target = normalize_origin(origin);
    allowlist.iter().any(|entry| normalize_origin(entry) == target)
}

#[cfg(test)]
mod origin_acl_tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_allowlist_disables_the_gate() {
        // 缺省不启用：既有装配（未配置清单）行为不变。
        assert!(origin_allowed(&[], "http://127.0.0.1:63896"));
        assert!(origin_allowed(&[], ORIGIN_UNKNOWN));
        assert!(origin_allowed(&[], ""));
    }

    #[test]
    fn enabled_allowlist_admits_exact_match_only() {
        let allow = list(&["http://127.0.0.1:63896", "tauri://localhost"]);
        assert!(origin_allowed(&allow, "http://127.0.0.1:63896"));
        assert!(origin_allowed(&allow, "tauri://localhost"));
        // 端口/主机不同即拒绝（fail-closed）。
        assert!(!origin_allowed(&allow, "http://127.0.0.1:63897"));
        assert!(!origin_allowed(&allow, "http://localhost:63896"));
        assert!(!origin_allowed(&allow, "https://evil.example"));
    }

    #[test]
    fn trailing_slash_and_whitespace_are_tolerated_on_both_sides() {
        let allow = list(&[" http://127.0.0.1:63896/ "]);
        assert!(origin_allowed(&allow, "http://127.0.0.1:63896"));
        assert!(origin_allowed(&allow, "http://127.0.0.1:63896/"));
    }

    #[test]
    fn unknown_origin_is_rejected_even_if_listed() {
        // 关键：哨兵不可被「写进清单」绕过——那是 fail-open。
        let allow = list(&[ORIGIN_UNKNOWN]);
        assert!(!origin_allowed(&allow, ORIGIN_UNKNOWN));
        assert!(!origin_allowed(&list(&["http://ok.example"]), ORIGIN_UNKNOWN));
        // 空 origin 同理。
        assert!(!origin_allowed(&list(&[""]), ""));
        assert!(!origin_allowed(&list(&["http://ok.example"]), "   "));
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 禁止授予清单（§2.1）——单点硬拒
// ──────────────────────────────────────────────────────────────────────────

/// 无条件禁止的权限（审批 UI 不提供勾选项，需要则走 A 类）。
pub const FORBIDDEN_PLAIN: &[&str] = &["shell:allow-execute"];

/// 审批提示（文案与代码共享同一常量来源，计划 §4.5）。
#[derive(Debug, Clone, Serialize)]
pub struct ApprovalHint {
    pub permission: String,
    pub risk: Risk,
    pub description: String,
}

impl ApprovalHint {
    /// 从词表取提示。表外权限返回 `None`（安装期已拒绝，此处兜底）。
    pub fn of(index: &PermissionIndex, p: &Permission) -> Option<Self> {
        let e = index.entry_of(p.as_str())?;
        Some(Self {
            permission: p.as_str().to_string(),
            risk: e.risk,
            description: e.description.clone(),
        })
    }
}

/// 高危权限逐条展开成人话并默认不勾（计划 §4.5）。
pub fn approval_hints(index: &PermissionIndex, perms: &[Permission]) -> Vec<ApprovalHint> {
    perms.iter().filter_map(|p| ApprovalHint::of(index, p)).collect()
}

fn scope_strings(scopes: &Map<String, serde_json::Value>, key: &str) -> Vec<String> {
    match scopes.get(key) {
        Some(serde_json::Value::Array(items)) => {
            items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        }
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// 校验授予集不违反禁止授予清单（§2.1）。
///
/// 四条硬规则：
/// 1. `shell:allow-execute` —— 无条件拒绝；
/// 2. `fs:allow-app-write-recursive` 配 `scope: ["$APPCONFIG/**"]` —— 拒绝；
/// 3. `fs` 指向 `app_data_dir` 之外的通配根 —— 拒绝；
/// 4. `http:allow-fetch` 配 `scope: ["*"]` —— 拒绝。
pub fn check_grants(
    scopes: &Map<String, serde_json::Value>,
    perms: &[Permission],
) -> HostResult<()> {
    for p in perms {
        let s = p.as_str();

        // 规则 1
        if FORBIDDEN_PLAIN.contains(&s) {
            return Err(HostError::new(
                ErrorCode::E_FORBIDDEN_PERMISSION,
                format!("权限 `{s}` 在禁止授予清单内（§2.1）：审批 UI 不提供勾选项，需要该能力请走 A 类原生插件"),
            ));
        }

        // 规则 2
        if s == "fs:allow-app-write-recursive" {
            for e in scope_strings(scopes, s) {
                if e == "$APPCONFIG/**" {
                    return Err(HostError::new(
                        ErrorCode::E_FORBIDDEN_PERMISSION,
                        format!(
                            "`{s}` 配 `$APPCONFIG/**` 在禁止授予清单内（§2.1）：可写配置根等价于任意持久化写入，请走 A 类"
                        ),
                    ));
                }
            }
        }

        // 规则 3：fs 系列 scope 只能指向 $ 引用的应用目录或 ./ 相对路径
        if s.starts_with("fs:") {
            for e in scope_strings(scopes, s) {
                let is_dir_ref = e.starts_with('$') && e.ends_with("/**");
                let is_rel = e.starts_with("./");
                if !(is_dir_ref || is_rel) {
                    return Err(HostError::new(
                        ErrorCode::E_FORBIDDEN_PERMISSION,
                        format!(
                            "fs 权限 `{s}` 的 scope `{e}` 不是应用目录引用（$APPCONFIG/** 等）或 ./ 相对路径；指向 app_data_dir 之外的通配根一律拒绝（§2.1）"
                        ),
                    ));
                }
            }
        }

        // 规则 4
        if s == "http:allow-fetch" {
            for e in scope_strings(scopes, s) {
                if e == "*" {
                    return Err(HostError::new(
                        ErrorCode::E_FORBIDDEN_PERMISSION,
                        format!("`{s}` 配 `scope: [\"*\"]` 在禁止授予清单内（§2.1）：全量通配网络访问请走 A 类"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{PermissionEntry, Risk};

    fn index() -> PermissionIndex {
        PermissionIndex {
            version: 1,
            generated_at: None,
            entries: vec![
                PermissionEntry {
                    identifier: "store:allow-get".into(),
                    risk: Risk::Low,
                    description: "读取 store 键值".into(),
                    scoped: false,
                },
                PermissionEntry {
                    identifier: "http:allow-fetch".into(),
                    risk: Risk::Elevated,
                    description: "发起 HTTP 请求".into(),
                    scoped: true,
                },
            ],
        }
    }

    fn scopes(pairs: &[(&str, &str)]) -> Map<String, serde_json::Value> {
        let mut m = Map::new();
        for (k, v) in pairs {
            m.insert(
                k.to_string(),
                serde_json::Value::Array(vec![serde_json::Value::String(v.to_string())]),
            );
        }
        m
    }

    #[test]
    fn tier_display_is_kebab() {
        assert_eq!(AuthTier::Self_.as_str(), "self");
        assert_eq!(AuthTier::ScopedRead.as_str(), "scoped-read");
        assert_eq!(AuthTier::Privileged.as_str(), "privileged");
        assert_eq!(serde_json::to_string(&AuthTier::ScopedRead).unwrap(), "\"scoped-read\"");
    }

    #[test]
    fn registry_is_well_formed() {
        // 门禁 §8-17 的 CI 校验入口。
        assert!(validate_command_registry().is_ok());
    }

    #[test]
    fn all_registered_commands_have_consumers() {
        for c in COMMANDS.iter().chain(ADMIN_COMMANDS.iter()) {
            assert!(!c.consumer.is_empty(), "{} 缺 consumer 登记", c.command);
        }
        assert_eq!(COMMANDS.len(), 17, "§2.1 定稿 9 条 + R7 收口补登记 4 条 + 0.4-A1 跨主体调用 3 条 + 0.4 审计补登记 host_contributes_list 1 条");
        // 主窗面包含注册表管理、sidecar 管理与 M8 配额诊断四条特权命令。
        assert_eq!(
            ADMIN_COMMANDS.len(),
            4,
            "核心注册的主窗特权命令 4 条；adapter 可按 feature 扩展"
        );
    }

    /// R7 收口：这 4 条是 `HostClient` 实际调用、且此前**完全未登记**的命令。
    /// 逐条断言档位与命令名，防止有人把它们塞进 `ADMIN_COMMANDS`（那会让插件面
    /// 失去自档语义）或调换顺序（TS 侧镜像表按顺序逐条比对）。
    #[test]
    fn r7_backfilled_plugin_commands_are_self_tier_and_in_order() {
        let backfilled =
            ["host_events_drain", "host_stream_open", "host_stream_write", "host_stream_close"];
        // 紧接 `host_registry_list` 之后，顺序固定。
        let start = COMMANDS
            .iter()
            .position(|c| c.command == "host_registry_list")
            .expect("host_registry_list 必须在表内");
        let actual: Vec<&str> =
            COMMANDS[start + 1..start + 1 + backfilled.len()].iter().map(|c| c.command).collect();
        assert_eq!(actual, backfilled, "补登记的命令必须紧接 host_registry_list 且按序");
        // 0.4-A1：跨主体调用三命令**追加在表末尾**（追加语义；TS 侧能力镜像
        // 按序逐条比对，中间插入会整体错位）。
        let a1: Vec<&str> = COMMANDS[COMMANDS.len() - 3..].iter().map(|c| c.command).collect();
        assert_eq!(
            a1,
            ["host_call_plugin", "host_call_result", "host_call_take"],
            "0.4-A1 三命令必须按序追加在表末尾"
        );
        for name in backfilled {
            let c = resolve(name).unwrap_or_else(|| panic!("{name} 未登记档位"));
            assert_eq!(c.tier, AuthTier::Self_, "{name} 必须是 self 档");
            assert!(
                !ADMIN_COMMANDS.iter().any(|a| a.command == name),
                "{name} 是插件面命令，不得进 ADMIN_COMMANDS"
            );
            assert!(!c.consumer.is_empty() && !c.description.is_empty());
        }
    }

    #[test]
    fn resolve_known_commands() {
        assert_eq!(resolve("host_plugin_call").unwrap().tier, AuthTier::Self_);
        assert_eq!(resolve("host_registry_list").unwrap().tier, AuthTier::ScopedRead);
        assert_eq!(resolve("host_registry_admin").unwrap().tier, AuthTier::Privileged);
        // P0-2：运行时命令**必须是特权档**——`host_runtime_spawn` 会按入参
        // `pluginId` 启动一个可执行文件；若登记成 self 档，任何插件 webview 都能
        // 启动别的插件的 sidecar（越权执行原语）。
        assert_eq!(resolve("host_runtime_spawn").unwrap().tier, AuthTier::Privileged);
        assert_eq!(resolve("host_runtime_health").unwrap().tier, AuthTier::Privileged);
        assert_eq!(resolve("host_resource_stats").unwrap().tier, AuthTier::Privileged);
    }

    #[test]
    fn unregistered_command_hard_fails() {
        let e = require_registered("host_grant_request").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("未在授权档位表登记"));
        assert!(e.message.contains("host_grant_request"));
        // D16：该命令在 v1 已删除。
        assert!(resolve("host_grant_request").is_none());
        // D2：信封式命令已废弃。
        assert!(resolve("host_call_begin").is_none());
    }

    #[test]
    fn label_to_plugin_id_works() {
        let id = label_to_plugin_id("plugin-com.example.formatter").unwrap();
        assert_eq!(id.as_str(), "com.example.formatter");
    }

    #[test]
    fn label_without_prefix_is_rejected() {
        let e = label_to_plugin_id("main").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn self_identity_ignores_claimed_and_accepts_match() {
        // 声明一致 → 通过。
        let id = resolve_self_identity("plugin-com.example.x", Some("com.example.x")).unwrap();
        assert_eq!(id.as_str(), "com.example.x");
        // 未声明 → 通过（身份来自 label）。
        let id = resolve_self_identity("plugin-com.example.x", None).unwrap();
        assert_eq!(id.as_str(), "com.example.x");
    }

    #[test]
    fn spoofed_plugin_id_is_rejected() {
        // §4.1 测试点："伪造 pluginId 的上报被拒"。
        let e = resolve_self_identity("plugin-com.example.x", Some("com.attacker.y")).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("com.attacker.y"));
        assert!(e.message.contains("仅认身份"));
    }

    #[test]
    fn shell_execute_is_forbidden() {
        let e = check_grants(&Map::new(), &[Permission::new("shell:allow-execute")]).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("A 类"));
    }

    #[test]
    fn fs_appconfig_recursive_write_is_forbidden() {
        let s = scopes(&[("fs:allow-app-write-recursive", "$APPCONFIG/**")]);
        let e = check_grants(&s, &[Permission::new("fs:allow-app-write-recursive")]).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
    }

    #[test]
    fn fs_wildcard_root_is_forbidden() {
        // 绝对路径或裸通配 → 拒绝。
        for bad in ["C:\\Users\\*", "*", "/tmp/**", "~/"] {
            let s = scopes(&[("fs:allow-app-read", bad)]);
            let e = check_grants(&s, &[Permission::new("fs:allow-app-read")]).unwrap_err();
            assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION, "scope `{bad}`");
            assert!(e.message.contains(bad));
        }
    }

    #[test]
    fn fs_dir_ref_and_relative_are_allowed() {
        let s = scopes(&[
            ("fs:allow-app-read", "$APPCONFIG/**"),
            ("fs:allow-app-write-recursive", "./data/**"),
        ]);
        assert!(check_grants(
            &s,
            &[
                Permission::new("fs:allow-app-read"),
                Permission::new("fs:allow-app-write-recursive"),
            ]
        )
        .is_ok());
    }

    #[test]
    fn http_fetch_wildcard_is_forbidden() {
        let s = scopes(&[("http:allow-fetch", "*")]);
        let e = check_grants(&s, &[Permission::new("http:allow-fetch")]).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(e.message.contains("\"*\""));
    }

    #[test]
    fn http_fetch_specific_origin_is_allowed() {
        let s = scopes(&[("http:allow-fetch", "https://api.example.com/**")]);
        assert!(check_grants(&s, &[Permission::new("http:allow-fetch")]).is_ok());
    }

    #[test]
    fn approval_hints_share_source_with_index() {
        let i = index();
        let hints = approval_hints(
            &i,
            &[Permission::new("store:allow-get"), Permission::new("http:allow-fetch")],
        );
        assert_eq!(hints.len(), 2);
        assert_eq!(hints[0].permission, "store:allow-get");
        assert_eq!(hints[0].risk, Risk::Low);
        assert_eq!(hints[0].description, "读取 store 键值");
        assert_eq!(hints[1].risk, Risk::Elevated);
        // 表外权限不出现在提示里（安装期已拒）。
        assert!(ApprovalHint::of(&i, &Permission::new("nope:allow-x")).is_none());
    }

    #[test]
    fn admin_ops_are_complete() {
        assert_eq!(RegistryAdminOp::all().len(), 4);
        let s = serde_json::to_string(RegistryAdminOp::all()).unwrap();
        assert!(s.contains("\"disable\"") && s.contains("\"purge\""));
    }

    #[test]
    fn validate_catches_empty_fields() {
        // 直接调用真实校验内核，验证空字段被拦截（而非复制一份逻辑自测）。
        let bad =
            CommandAuth { command: "x", tier: AuthTier::Self_, consumer: "", description: "d" };
        let e = validate_commands(std::iter::once(&bad)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("字段缺失"));
        assert!(e.message.contains("x"));

        for field in [
            &CommandAuth { command: "", tier: AuthTier::Self_, consumer: "c", description: "d" },
            &CommandAuth { command: "x", tier: AuthTier::Self_, consumer: "c", description: "" },
        ] {
            let e = validate_commands(std::iter::once(field)).unwrap_err();
            assert!(e.message.contains("字段缺失"));
        }
    }

    #[test]
    fn validate_catches_duplicate_commands() {
        let a = CommandAuth {
            command: "dup",
            tier: AuthTier::Self_,
            consumer: "c",
            description: "first",
        };
        let b = CommandAuth {
            command: "dup",
            tier: AuthTier::ScopedRead,
            consumer: "c",
            description: "second",
        };
        let e = validate_commands([&a, &b]).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("重复"));
        assert!(e.message.contains("first") && e.message.contains("second"));
    }

    #[test]
    fn validate_accepts_well_formed_table() {
        let a =
            CommandAuth { command: "a", tier: AuthTier::Self_, consumer: "c", description: "d" };
        let b = CommandAuth {
            command: "b",
            tier: AuthTier::Privileged,
            consumer: "c",
            description: "d",
        };
        assert!(validate_commands([&a, &b]).is_ok());
    }
}
