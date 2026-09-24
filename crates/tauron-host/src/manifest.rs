//! §4.2 manifest 与权限词表。
//!
//! 职责：解析、schema 校验、字段规范化、`framework` range 兼容判定；
//! 发版时导出 `permissions.index.json`（含 `risk` 元数据）。
//!
//! 关键约束（计划 §4.2）：
//! - **未知字段拒绝**（`deny_unknown_fields`，防拼写漂移）；
//! - `id` 反域名 + 全局唯一（唯一性由注册表强制）；
//! - `permissions` 只能取自 `permissions.index.json`，表外即安装失败；
//! - `scopes` 与 `permissions` **分离声明**；
//! - `framework` range **缺失视为不兼容**（非 Option，缺失即解析失败）。
//!
//! 前序审查修复对照：
//! - R3：manifest 示例中的权限字符串全部不是 Tauri 真实标识 → 词表必须来自
//!   机器生成的 `permissions.index.json`；
//! - T13：词表每项带 `risk: low|elevated|high`；
//! - T17：`framework` 为 semver range，A/D 类另加 `abi` 指纹；
//! - D13：`abi` 字段的消费落点（加载期校验）在 §4.7/§4.8，本模块只负责声明与解析。

use crate::error::{ErrorCode, HostError, HostResult};
use serde::{Deserialize, Serialize};
use semver::VersionReq;
use std::fmt;
use std::path::Path;

/// 支持的平台标识白名单（架构 §8.3）。
pub const SUPPORTED_PLATFORMS: &[&str] = &["win", "mac", "linux", "ios", "android"];

// ──────────────────────────────────────────────────────────────────────────
// PluginId
// ──────────────────────────────────────────────────────────────────────────

const PLUGIN_ID_MAX_LEN: usize = 200;

/// 插件标识：反域名格式（`com.example.formatter`）。
///
/// 唯一性不在本模块判定——由 [`crate::registry::Registry`] 强制。
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PluginId(String);

impl PluginId {
    /// 校验并构造。失败返回 `E_INVALID_MANIFEST`。
    pub fn new(raw: &str) -> HostResult<Self> {
        let s = raw.trim();
        if s.is_empty() {
            return Err(bad_manifest("plugin id 为空"));
        }
        if s.len() > PLUGIN_ID_MAX_LEN {
            return Err(bad_manifest(format!(
                "plugin id 长度 {} 超过上限 {}",
                s.len(),
                PLUGIN_ID_MAX_LEN
            )));
        }
        if s.starts_with('.') || s.ends_with('.') {
            return Err(bad_manifest(format!("plugin id `{s}` 不能以点开头或结尾")));
        }
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() < 2 {
            return Err(bad_manifest(format!(
                "plugin id `{s}` 不是反域名格式（至少需要两个点分隔的段，如 com.example.pkg）"
            )));
        }
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() {
                return Err(bad_manifest(format!("plugin id `{s}` 第 {i} 段为空（连续点）")));
            }
            let is_first = i == 0;
            if p.chars().any(|c| !c.is_ascii_alphanumeric() && c != '-') {
                return Err(bad_manifest(format!(
                    "plugin id `{s}` 第 {i} 段 `{p}` 含非法字符（仅允许 a-z 0-9 -）"
                )));
            }
            if p.chars().next().is_some_and(|c| c == '-') {
                return Err(bad_manifest(format!("plugin id `{s}` 第 {i} 段以连字符开头")));
            }
            if is_first && p.chars().any(|c| c.is_ascii_uppercase()) {
                return Err(bad_manifest(format!("plugin id `{s}` 首段必须小写")));
            }
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 便捷引用包装。
    pub fn get<'a>(&'a self) -> &'a str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for PluginId {
    type Err = HostError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Debug for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PluginId({})", self.0)
    }
}

fn bad_manifest(msg: impl Into<String>) -> HostError {
    HostError::new(ErrorCode::E_INVALID_MANIFEST, msg.into())
}

/// 解析 `framework` 版本范围。
///
/// 计划文档的规范写法用**空格**分隔比较子句（`">=2.0 <3.0"`），而 `semver`
/// crate 只接受逗号/分号分隔。此处做归一化，两种写法都可用；
/// `*` 仍会被 [`PluginManifest::validate`] 拒绝（过宽）。
pub fn parse_version_range(s: &str) -> HostResult<VersionReq> {
    let parts: Vec<&str> = s
        .split_whitespace()
        .map(|t| t.trim_end_matches(|c| c == ',' || c == ';'))
        .filter(|t| !t.is_empty())
        .collect();
    if parts.is_empty() {
        return Err(bad_manifest("framework range 为空"));
    }
    VersionReq::parse(&parts.join(", ")).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("framework range `{s}` 解析失败：{e}"),
        )
    })
}

/// `framework` 字段的反序列化：先按字符串读入再做归一化解析。
fn deserialize_version_range<'de, D>(d: D) -> Result<VersionReq, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    parse_version_range(&s).map_err(serde::de::Error::custom)
}

// ──────────────────────────────────────────────────────────────────────────
// Risk / Permission
// ──────────────────────────────────────────────────────────────────────────

/// 权限风险档位（T13；Tauri 官方未提供，框架自补）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Elevated,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Elevated => "elevated",
            Self::High => "high",
        }
    }
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 权限标识。只能取自 [`PermissionIndex`]（R3）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Permission(pub String);

impl Permission {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 所属插件名（`store:allow-get` → `store`）。
    pub fn plugin(&self) -> &str {
        self.0.split(':').next().unwrap_or("")
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 权限词表
// ──────────────────────────────────────────────────────────────────────────

/// `permissions.index.json` 单条记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEntry {
    /// Tauri 真实权限标识。
    pub identifier: String,
    pub risk: Risk,
    /// 面向人的说明（审批 UI 文案来源，计划 §4.5："文案与代码共享同一常量来源"）。
    pub description: String,
    /// 该权限是否接受 scope。
    #[serde(default)]
    pub scoped: bool,
}

/// 权限词表（机器生成，随框架发版锁版）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionIndex {
    /// 词表版本号。
    pub version: u32,
    /// 生成时间戳。
    #[serde(default)]
    pub generated_at: Option<String>,
    pub entries: Vec<PermissionEntry>,
}

impl PermissionIndex {
    /// 从 JSON 文件加载词表。
    pub fn load(path: impl AsRef<Path>) -> HostResult<Self> {
        let p = path.as_ref();
        let bytes = std::fs::read(p).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("无法读取权限词表 {}: {e}", p.display()),
            )
        })?;
        let idx = serde_json::from_slice(&bytes).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("权限词表 JSON 解析失败 {}: {e}", p.display()),
            )
        })?;
        Ok(idx)
    }

    pub fn contains(&self, identifier: &str) -> bool {
        self.entries.iter().any(|e| e.identifier == identifier)
    }

    pub fn risk_of(&self, identifier: &str) -> Option<Risk> {
        self.entries
            .iter()
            .find(|e| e.identifier == identifier)
            .map(|e| e.risk)
    }

    pub fn entry_of(&self, identifier: &str) -> Option<&PermissionEntry> {
        self.entries.iter().find(|e| e.identifier == identifier)
    }

    /// 表外权限 → 安装期硬失败（计划 §4.2 关键约束）。
    pub fn check_permitted(&self, p: &Permission) -> HostResult<()> {
        if self.contains(p.as_str()) {
            Ok(())
        } else {
            Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "权限 `{}` 不在权限词表 v{} 中；表外字符串一律拒绝安装（R3）",
                    p.0,
                    self.version
                ),
            ))
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// manifest 字段
// ──────────────────────────────────────────────────────────────────────────

/// 插件形态四类（架构 §3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginType {
    /// A 类：Rust crate，直接编译进宿主。
    Rust,
    /// B 类：JS，运行在身份 Webview 内。
    Js,
    /// C 类：进程插件，sidecar 二进制。
    Process,
    /// D 类：WASM，Extism/wasmtime 执行。
    Wasm,
}

impl PluginType {
    pub fn is_a_or_d(self) -> bool {
        matches!(self, Self::Rust | Self::Wasm)
    }
}

impl fmt::Display for PluginType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rust => f.write_str("rust"),
            Self::Js => f.write_str("js"),
            Self::Process => f.write_str("process"),
            Self::Wasm => f.write_str("wasm"),
        }
    }
}

/// 入口声明。字段与 `type` 相关，按类型校验。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EntrySpec {
    /// B 类：JS 入口文件。
    pub js: Option<String>,
    /// C 类：sidecar 二进制名（随包分发，§9.5）。
    pub sidecar: Option<String>,
    /// D 类：WASM 模块文件。
    pub wasm: Option<String>,
    /// 主面板 UI 文件（可选）。
    pub ui: Option<String>,
}

impl EntrySpec {
    fn validate(&self, plugin_type: PluginType, id: &PluginId) -> HostResult<()> {
        match plugin_type {
            PluginType::Rust => {
                if self.js.is_some() || self.sidecar.is_some() || self.wasm.is_some() {
                    Err(bad_manifest(format!(
                        "插件 `{id}` 为 rust 类型，不应声明 js/sidecar/wasm 入口"
                    )))
                } else {
                    Ok(())
                }
            }
            PluginType::Js => {
                match &self.js {
                    Some(j) if !j.trim().is_empty() => Ok(()),
                    _ => Err(bad_manifest(format!(
                        "插件 `{id}` 为 js 类型，必须声明非空 `entry.js`"
                    ))),
                }
            }
            PluginType::Process => {
                match &self.sidecar {
                    Some(s) if !s.trim().is_empty() => Ok(()),
                    _ => Err(bad_manifest(format!(
                        "插件 `{id}` 为 process 类型，必须声明非空 `entry.sidecar`"
                    ))),
                }
            }
            PluginType::Wasm => {
                match &self.wasm {
                    Some(w) if !w.trim().is_empty() => Ok(()),
                    _ => Err(bad_manifest(format!(
                        "插件 `{id}` 为 wasm 类型，必须声明非空 `entry.wasm`"
                    ))),
                }
            }
        }
    }
}

/// ABI 指纹。A 类为 crate 版本指纹，D 类为 wasm 接口 hash（T17）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AbiFingerprint {
    pub rust: Option<String>,
    pub wasm: Option<String>,
}

impl AbiFingerprint {
    fn validate(&self, plugin_type: PluginType, id: &PluginId) -> HostResult<()> {
        if !plugin_type.is_a_or_d() {
            if self.rust.is_some() || self.wasm.is_some() {
                return Err(bad_manifest(format!(
                    "插件 `{id}` 类型为 {}，非 A/D 类，不应声明 `abi`",
                    plugin_type
                )));
            }
            return Ok(());
        }
        match plugin_type {
            PluginType::Rust => match &self.rust {
                Some(r) if !r.trim().is_empty() => Ok(()),
                _ => Err(bad_manifest(format!(
                    "插件 `{id}` 为 A 类（rust），必须声明 `abi.rust`（D13：加载期硬校验）"
                ))),
            },
            PluginType::Wasm => match &self.wasm {
                Some(w) if !w.trim().is_empty() => Ok(()),
                _ => Err(bad_manifest(format!(
                    "插件 `{id}` 为 D 类（wasm），必须声明 `abi.wasm`（D13：加载期硬校验）"
                ))),
            },
            _ => Ok(()),
        }
    }
}

/// 标题长度上限（安全审查 D3：防 UI 溢出 / XSS 载体）。
const MAX_TITLE_LEN: usize = 64;

/// 图标路径前缀白名单（安全审查 D3：仅允许 `assets/` 目录）。
const ALLOWED_ICON_PREFIX: &str = "assets/";

/// 命令贡献（安全审查 D3：类型化 + 校验）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandContribute {
    pub id: String,
    pub title: String,
}

/// 菜单贡献。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuContribute {
    pub id: String,
    /// 引用的命令 ID。
    pub command: String,
}

/// 面板贡献。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelContribute {
    pub id: String,
    pub title: String,
    pub icon: String,
}

/// 设置 Tab 贡献。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsTabContribute {
    pub id: String,
    pub title: String,
}

/// 快捷键贡献。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortcutContribute {
    /// 加速键表达式（如 `Ctrl+Shift+P`）。
    pub accelerator: String,
    /// 引用的命令 ID。
    pub command: String,
}

/// 贡献声明（contributes 注册，§4.6）。
///
/// 安全审查 D3：`Contributes` 从 `Vec<Value>` 改为类型化结构体，
/// 防止未校验字段注入、重复 ID、超长标题、图标路径穿越等攻击。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Contributes {
    pub commands: Vec<CommandContribute>,
    pub menus: Vec<MenuContribute>,
    pub panels: Vec<PanelContribute>,
    pub settings_tabs: Vec<SettingsTabContribute>,
    pub shortcuts: Vec<ShortcutContribute>,
}

impl Contributes {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
            && self.menus.is_empty()
            && self.panels.is_empty()
            && self.settings_tabs.is_empty()
            && self.shortcuts.is_empty()
    }

    /// 校验所有贡献声明（安全审查 D3/D5）。
    ///
    /// 校验项：
    /// - ID 全局唯一性（命令/菜单/面板/设置Tab）
    /// - 菜单/快捷键引用的命令 ID 必须存在于 `commands` 中
    /// - 快捷键加速器冲突检测
    /// - 标题长度 ≤64 字符
    /// - 标题 HTML 转义（禁止 `<` 和 `>`）
    /// - 图标路径仅允许 `assets/` 前缀
    /// - 空数组不报错（允许无贡献的插件）
    pub fn validate(&self, id: &PluginId) -> HostResult<()> {
        let errs = &mut Vec::new();

        // ── 标题校验（所有含 title 字段的贡献）──────────────────────
        fn check_title(title: &str, field: &str, plugin_id: &PluginId, errs: &mut Vec<String>) {
            if title.len() > MAX_TITLE_LEN {
                errs.push(format!(
                    "插件 `{plugin_id}` {field} 标题长度 {} 超过 {MAX_TITLE_LEN}",
                    title.len()
                ));
            }
            if title.contains('<') || title.contains('>') {
                errs.push(format!(
                    "插件 `{plugin_id}` {field} 标题含 HTML 标签字符（`<`/`>`），拒绝（D3/XSS）"
                ));
            }
        }

        // ── ID 唯一性检查 ──────────────────────────────────────────
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // ── 命令校验 ───────────────────────────────────────────────
        let mut command_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, cmd) in self.commands.iter().enumerate() {
            let label = format!("commands[{i}]");
            if cmd.id.trim().is_empty() {
                errs.push(format!("插件 `{id}` {label} id 为空"));
            }
            if cmd.id.chars().any(|c| c.is_whitespace()) {
                errs.push(format!("插件 `{id}` {label} id `{}` 含空白字符", cmd.id));
            }
            if !seen_ids.insert(cmd.id.clone()) {
                errs.push(format!(
                    "插件 `{id}` {label} id `{}` 与其他贡献重复（全局 ID 唯一性）",
                    cmd.id
                ));
            }
            command_ids.insert(cmd.id.clone());
            check_title(&cmd.title, &format!("{label}.title"), id, errs);
        }

        // ── 菜单校验 ───────────────────────────────────────────────
        for (i, menu) in self.menus.iter().enumerate() {
            let label = format!("menus[{i}]");
            if menu.id.trim().is_empty() {
                errs.push(format!("插件 `{id}` {label} id 为空"));
            }
            if !seen_ids.insert(menu.id.clone()) {
                errs.push(format!(
                    "插件 `{id}` {label} id `{}` 与其他贡献重复（全局 ID 唯一性）",
                    menu.id
                ));
            }
            if !command_ids.contains(&menu.command) {
                errs.push(format!(
                    "插件 `{id}` {label} 引用的命令 `{}` 未在任何 commands 中声明（D3/悬空引用）",
                    menu.command
                ));
            }
        }

        // ── 面板校验 ───────────────────────────────────────────────
        for (i, panel) in self.panels.iter().enumerate() {
            let label = format!("panels[{i}]");
            if panel.id.trim().is_empty() {
                errs.push(format!("插件 `{id}` {label} id 为空"));
            }
            if !seen_ids.insert(panel.id.clone()) {
                errs.push(format!(
                    "插件 `{id}` {label} id `{}` 与其他贡献重复（全局 ID 唯一性）",
                    panel.id
                ));
            }
            check_title(&panel.title, &format!("{label}.title"), id, errs);
            // 图标路径校验
            if panel.icon.is_empty() {
                errs.push(format!("插件 `{id}` {label} icon 为空"));
            } else if !panel.icon.starts_with(ALLOWED_ICON_PREFIX) {
                errs.push(format!(
                    "插件 `{id}` {label} icon `{}` 不在允许路径 `{ALLOWED_ICON_PREFIX}*`（D3/路径穿越）",
                    panel.icon
                ));
            }
        }

        // ── 设置 Tab 校验 ──────────────────────────────────────────
        for (i, tab) in self.settings_tabs.iter().enumerate() {
            let label = format!("settings_tabs[{i}]");
            if tab.id.trim().is_empty() {
                errs.push(format!("插件 `{id}` {label} id 为空"));
            }
            if !seen_ids.insert(tab.id.clone()) {
                errs.push(format!(
                    "插件 `{id}` {label} id `{}` 与其他贡献重复（全局 ID 唯一性）",
                    tab.id
                ));
            }
            check_title(&tab.title, &format!("{label}.title"), id, errs);
        }

        // ── 快捷键校验 ─────────────────────────────────────────────
        let mut seen_accelerators: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, sc) in self.shortcuts.iter().enumerate() {
            let label = format!("shortcuts[{i}]");
            if sc.accelerator.trim().is_empty() {
                errs.push(format!("插件 `{id}` {label} accelerator 为空"));
            }
            let normalized = sc.accelerator.to_uppercase();
            if !seen_accelerators.insert(normalized.clone()) {
                errs.push(format!(
                    "插件 `{id}` {label} accelerator `{}` 与同插件其他快捷键冲突（D3/快捷键冲突）",
                    sc.accelerator
                ));
            }
            if !command_ids.contains(&sc.command) {
                errs.push(format!(
                    "插件 `{id}` {label} 引用的命令 `{}` 未在任何 commands 中声明（D3/悬空引用）",
                    sc.command
                ));
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(bad_manifest(format!(
                "插件 `{id}` contributes 校验失败（{} 项）：{}",
                errs.len(),
                errs.join("; ")
            )))
        }
    }
}

/// 发布事件声明（架构 §4.4：publish/subscribe 授权判定）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDecl {
    /// 路由键。前缀仅作路由键，不作授权依据（R8）。
    pub topic: String,
    /// `true` 时其他插件可订阅（仍需显式订阅动作）。
    #[serde(default)]
    pub public: bool,
}

/// 事件声明集合。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EventsDecl {
    pub publish: Vec<EventDecl>,
    pub subscribe: Vec<String>,
}

impl EventsDecl {
    fn validate(&self, id: &PluginId) -> HostResult<()> {
        for (i, e) in self.publish.iter().enumerate() {
            let t = e.topic.trim();
            if t.is_empty() {
                return Err(bad_manifest(format!(
                    "插件 `{id}` events.publish[{i}] topic 为空"
                )));
            }
            if t.chars().any(|c| c.is_whitespace() || c == ':') {
                return Err(bad_manifest(format!(
                    "插件 `{id}` events.publish[{i}] topic `{t}` 含空白或冒号"
                )));
            }
        }
        for (i, s) in self.subscribe.iter().enumerate() {
            if s.trim().is_empty() {
                return Err(bad_manifest(format!(
                    "插件 `{id}` events.subscribe[{i}] 为空"
                )));
            }
        }
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// PluginManifest
// ──────────────────────────────────────────────────────────────────────────

/// 插件清单（架构 §4.2 示例 + T17 framework range）。
///
/// `deny_unknown_fields`：未知字段一律拒绝，防拼写漂移（计划 §4.2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: semver::Version,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    #[serde(default)]
    pub entry: EntrySpec,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    /// 与 `permissions` 分离声明（计划 §4.2）。
    #[serde(default)]
    pub scopes: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub platforms: Vec<String>,
    /// `>=2.0 <3.0`。缺失即视为不兼容——故为非 Option（计划 §4.2）。
    #[serde(deserialize_with = "deserialize_version_range")]
    pub framework: VersionReq,
    #[serde(default)]
    pub abi: Option<AbiFingerprint>,
    #[serde(default)]
    pub contributes: Contributes,
    #[serde(default)]
    pub settings_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub events: EventsDecl,
    #[serde(default)]
    pub host_functions: Vec<String>,
    #[serde(default)]
    pub min_allowed_version: Option<semver::Version>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
}

impl PluginManifest {
    /// 对词表做完整校验（安装期调用）。
    pub fn validate(&self, index: &PermissionIndex) -> HostResult<()> {
        let errs = &mut Vec::new();

        if self.name.trim().is_empty() {
            errs.push("name 为空".to_string());
        }
        if self.name.len() > 128 {
            errs.push(format!("name 长度 {} 超过 128", self.name.len()));
        }

        if let Err(e) = self.entry.validate(self.plugin_type, &self.id) {
            errs.push(e.message);
        }

        if let Some(abi) = &self.abi {
            if let Err(e) = abi.validate(self.plugin_type, &self.id) {
                errs.push(e.message);
            }
        } else if self.plugin_type.is_a_or_d() {
            let field = match self.plugin_type {
                PluginType::Rust => "`abi.rust`",
                PluginType::Wasm => "`abi.wasm`",
                _ => "`abi`",
            };
            errs.push(format!(
                "插件 `{}` 为 {} 类型，缺少 {field}（A/D 类必填，D13）",
                self.id,
                self.plugin_type
            ));
        }

        if self.framework == VersionReq::STAR {
            errs.push(format!(
                "插件 `{}` 的 framework range 为 `*`（过宽，视为不兼容）",
                self.id
            ));
        }

        for p in &self.permissions {
            if let Err(e) = index.check_permitted(p) {
                errs.push(e.message);
            }
            if index.entry_of(p.as_str()).is_some_and(|e| e.scoped) {
                if !self.scopes.contains_key(p.as_str()) {
                    errs.push(format!(
                        "权限 `{p}` 需要 scope，但 manifest 未声明对应 scopes 项"
                    ));
                }
            }
        }

        // 权限重复检测（安全审查 D3 延伸）
        {
            let mut seen = std::collections::HashSet::new();
            for (i, p) in self.permissions.iter().enumerate() {
                if !seen.insert(p.as_str().to_string()) {
                    errs.push(format!(
                        "权限 `{p}` 在 permissions[{i}] 处重复声明"
                    ));
                }
            }
        }

        // scope 键必须在 permissions 中声明（分离声明的对称约束）。
        for k in self.scopes.keys() {
            if !self.permissions.iter().any(|p| p.as_str() == k) {
                errs.push(format!(
                    "scopes 声明了 `{k}`，但 permissions 未包含该权限（分离声明不对称）"
                ));
            }
        }

        for p in &self.platforms {
            if !SUPPORTED_PLATFORMS.contains(&p.as_str()) {
                errs.push(format!(
                    "平台 `{p}` 不在支持矩阵 {SUPPORTED_PLATFORMS:?} 中"
                ));
            }
        }

        if let Err(e) = self.events.validate(&self.id) {
            errs.push(e.message);
        }

        if let Err(e) = self.contributes.validate(&self.id) {
            errs.push(e.message);
        }

        for h in &self.host_functions {
            if h.trim().is_empty() {
                errs.push("host_functions 含空项".to_string());
            }
        }

        // host_functions 重复检测
        {
            let mut seen = std::collections::HashSet::new();
            for (i, h) in self.host_functions.iter().enumerate() {
                if !seen.insert(h.trim().to_string()) {
                    errs.push(format!(
                        "host_functions[{i}] `{h}` 重复声明"
                    ));
                }
            }
        }

        // 可选字段非空校验
        if let Some(ref pub_) = self.publisher {
            if pub_.trim().is_empty() {
                errs.push("publisher 声明为空字符串".to_string());
            }
        }
        if let Some(ref sig) = self.signature {
            if sig.trim().is_empty() {
                errs.push("signature 声明为空字符串".to_string());
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(bad_manifest(format!(
                "插件 `{}` manifest 校验失败（{} 项）：{}",
                self.id,
                errs.len(),
                errs.join("; ")
            )))
        }
    }

    /// framework range 兼容性判定（T17）。
    pub fn is_compatible(&self, current: &semver::Version) -> bool {
        self.framework.matches(current)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 身份单元
// ──────────────────────────────────────────────────────────────────────────

/// 身份单元（§4.6）：插件在宿主内的唯一身份绑定。
///
/// `webview_label` 由宿主生成（`plugin-<id>`），**插件不可指定**——
/// 这是 `self` 档"忽略入参 pluginId、仅认身份"的载体（§2.1）。
/// `token` 单调递增，用于绑定期防重放。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginIdentity {
    pub id: PluginId,
    pub webview_label: String,
    pub token: u64,
}

impl PluginIdentity {
    /// 构造身份（宿主内部路径）。
    pub fn new(id: PluginId, token: u64) -> Self {
        Self {
            id: id.clone(),
            webview_label: format!("plugin-{id}"),
            token,
        }
    }
}

impl std::fmt::Display for PluginIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.webview_label, self.token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> PermissionIndex {
        PermissionIndex {
            version: 1,
            generated_at: Some("2026-09-21".into()),
            entries: vec![
                PermissionEntry {
                    identifier: "store:allow-get".into(),
                    risk: Risk::Low,
                    description: "读取 store".into(),
                    scoped: false,
                },
                PermissionEntry {
                    identifier: "store:allow-set".into(),
                    risk: Risk::Elevated,
                    description: "写入 store".into(),
                    scoped: true,
                },
                PermissionEntry {
                    identifier: "fs:allow-app-read".into(),
                    risk: Risk::Low,
                    description: "读取应用目录".into(),
                    scoped: true,
                },
                PermissionEntry {
                    identifier: "http:allow-fetch".into(),
                    risk: Risk::Elevated,
                    description: "HTTP 请求".into(),
                    scoped: true,
                },
            ],
        }
    }

    fn js_manifest() -> PluginManifest {
        PluginManifest {
            id: PluginId::new("com.example.formatter").unwrap(),
            name: "Formatter".into(),
            version: semver::Version::new(1, 0, 0),
            plugin_type: PluginType::Js,
            entry: EntrySpec {
                js: Some("dist/index.js".into()),
                ..Default::default()
            },
            permissions: vec![Permission::new("store:allow-get")],
            scopes: serde_json::Map::new(),
            platforms: vec![],
            framework: parse_version_range(">=2.0 <3.0").unwrap(),
            abi: None,
            contributes: Contributes::default(),
            settings_schema: None,
            events: EventsDecl::default(),
            host_functions: vec![],
            min_allowed_version: None,
            signature: None,
            publisher: None,
        }
    }

    #[test]
    fn plugin_id_accepts_reverse_domain() {
        for s in [
            "com.example.formatter",
            "io.github.a.b.c",
            "org-foo.bar.baz",
            "a.b",
        ] {
            assert!(PluginId::new(s).is_ok(), "应接受 {s}");
        }
    }

    #[test]
    fn plugin_id_rejects_malformed() {
        for s in [
            "",
            "nodots",
            ".leading",
            "trailing.",
            "a..b",
            "A.lead.b",
            "com.bad space.x",
            "com._under.b",
            "-lead.b",
        ] {
            assert!(
                PluginId::new(s).is_err(),
                "应拒绝 {s:?}：{}",
                PluginId::new(s).unwrap_err().message
            );
        }
    }

    #[test]
    fn plugin_id_rejects_oversize() {
        let big = "a.".repeat(110);
        assert!(PluginId::new(&big).is_err());
    }

    #[test]
    fn plugin_id_fromstr() {
        let id: PluginId = "com.example.x".parse().unwrap();
        assert_eq!(id.as_str(), "com.example.x");
    }

    #[test]
    fn risk_roundtrips_lowercase() {
        assert_eq!(serde_json::to_string(&Risk::High).unwrap(), "\"high\"");
        assert_eq!(
            serde_json::from_str::<Risk>("\"elevated\"").unwrap(),
            Risk::Elevated
        );
    }

    #[test]
    fn permission_plugin_prefix() {
        assert_eq!(Permission::new("store:allow-get").plugin(), "store");
        assert_eq!(Permission::new("core:default").plugin(), "core");
    }

    #[test]
    fn index_membership_and_risk() {
        let i = idx();
        assert!(i.contains("store:allow-get"));
        assert!(!i.contains("store:allow-delete"));
        assert_eq!(i.risk_of("store:allow-set"), Some(Risk::Elevated));
        assert_eq!(i.risk_of("nope:allow-x"), None);
        assert!(i.check_permitted(&Permission::new("store:allow-get")).is_ok());
        let e = i
            .check_permitted(&Permission::new("store:allow-delete"))
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("词表"));
    }

    #[test]
    fn valid_js_manifest_passes() {
        let i = idx();
        let m = js_manifest();
        assert!(
            m.validate(&i).is_ok(),
            "{}",
            m.validate(&i).unwrap_err().message
        );
    }

    #[test]
    fn unknown_field_is_rejected() {
        let json = r#"{
            "id":"com.example.x","name":"X","version":"1.0.0","type":"js",
            "entry":{"js":"dist/index.js"},"framework":">=2.0 <3.0",
            "permissiosn":["store:allow-get"]
        }"#;
        let r: Result<PluginManifest, _> = serde_json::from_str(json);
        assert!(r.is_err(), "拼写错误的 permissiosn 必须被 deny_unknown_fields 拒绝");
    }

    #[test]
    fn missing_framework_is_rejected_at_parse() {
        let json = r#"{
            "id":"com.example.x","name":"X","version":"1.0.0","type":"js",
            "entry":{"js":"dist/index.js"}
        }"#;
        let r: Result<PluginManifest, _> = serde_json::from_str(json);
        assert!(r.is_err(), "framework 缺失即视为不兼容，解析期应失败");
    }

    #[test]
    fn table_outside_permission_is_install_failure() {
        let i = idx();
        let mut m = js_manifest();
        m.permissions = vec![Permission::new("store:allow-get"), Permission::new("fs:allow-delete")];
        let e = m.validate(&i).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("fs:allow-delete"));
        assert!(e.message.contains("词表"));
    }

    #[test]
    fn js_plugin_requires_entry_js() {
        let mut m = js_manifest();
        m.entry = EntrySpec::default();
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("entry.js"));
    }

    #[test]
    fn wasm_plugin_requires_abi_wasm() {
        let mut m = js_manifest();
        m.plugin_type = PluginType::Wasm;
        m.entry = EntrySpec {
            wasm: Some("dist/p.a.wasm".into()),
            ..Default::default()
        };
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("abi.wasm"));

        m.abi = Some(AbiFingerprint {
            wasm: Some("sha256:deadbeef".into()),
            ..Default::default()
        });
        assert!(m.validate(&idx()).is_ok());
    }

    #[test]
    fn b_class_must_not_declare_abi() {
        let mut m = js_manifest();
        m.abi = Some(AbiFingerprint {
            wasm: Some("x".into()),
            ..Default::default()
        });
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("非 A/D 类"));
    }

    #[test]
    fn scope_separation_is_symmetric() {
        // scopes 声明了未在 permissions 中出现的键 → 拒绝。
        let mut m = js_manifest();
        let mut scopes = serde_json::Map::new();
        scopes.insert(
            "store:allow-set".to_string(),
            serde_json::Value::Array(vec![]),
        );
        m.scopes = scopes;
        m.permissions = vec![Permission::new("store:allow-get")];
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("分离声明不对称"));
    }

    #[test]
    fn scoped_permission_requires_scope() {
        // store:allow-set 是 scoped 权限，必须声明对应 scope。
        let mut m = js_manifest();
        m.permissions = vec![Permission::new("store:allow-set")];
        m.scopes = serde_json::Map::new();
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("需要 scope"));

        let mut scopes = serde_json::Map::new();
        scopes.insert(
            "store:allow-set".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("user/*".into())]),
        );
        m.scopes = scopes;
        assert!(m.validate(&idx()).is_ok());
    }

    #[test]
    fn star_framework_is_rejected() {
        let mut m = js_manifest();
        m.framework = VersionReq::STAR;
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("过宽"));
    }

    #[test]
    fn unsupported_platform_is_rejected() {
        let mut m = js_manifest();
        m.platforms = vec!["beos".into()];
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("beos"));
    }

    #[test]
    fn framework_range_boundaries() {
        // T17 测试点：>=2.0 <3.0 对 1.9 / 2.0 / 3.0 的边界行为。
        let mut m = js_manifest();
        m.framework = parse_version_range(">=2.0 <3.0").unwrap();
        assert!(!m.is_compatible(&semver::Version::parse("1.9.0").unwrap()));
        assert!(m.is_compatible(&semver::Version::parse("2.0.0").unwrap()));
        assert!(m.is_compatible(&semver::Version::parse("2.99.0").unwrap()));
        assert!(!m.is_compatible(&semver::Version::parse("3.0.0").unwrap()));
    }

    #[test]
    fn multiple_violations_are_collected() {
        let mut m = js_manifest();
        m.name = String::new();
        m.entry = EntrySpec::default();
        m.platforms = vec!["beos".into()];
        m.permissions = vec![Permission::new("nope:allow")];
        let e = m.validate(&idx()).unwrap_err();
        // 应同时报出 name、entry.js、platforms、权限 四项。
        assert!(e.message.contains("name 为空"));
        assert!(e.message.contains("entry.js"));
        assert!(e.message.contains("beos"));
        assert!(e.message.contains("nope:allow"));
        assert!(e.message.contains("4 项"));
    }

    #[test]
    fn manifest_roundtrips_through_json() {
        let m = js_manifest();
        let s = serde_json::to_string_pretty(&m).unwrap();
        let back: PluginManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, m.id);
        assert_eq!(back.plugin_type, m.plugin_type);
    }

    #[test]
    fn events_decl_validation() {
        let mut m = js_manifest();
        m.events = EventsDecl {
            publish: vec![
                EventDecl {
                    topic: "ok".into(),
                    public: true,
                },
                EventDecl {
                    topic: "has space".into(),
                    public: false,
                },
            ],
            subscribe: vec![],
        };
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("空白"));
    }

    #[test]
    fn manifest_loads_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.json");
        std::fs::write(&path, serde_json::to_vec(&idx()).unwrap()).unwrap();
        let loaded = PermissionIndex::load(&path).unwrap();
        assert_eq!(loaded.version, 1);
        assert!(loaded.contains("store:allow-get"));
    }

    #[test]
    fn manifest_load_fails_for_missing_file() {
        let e = PermissionIndex::load("no/such/file.json").unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn plugin_type_is_a_or_d() {
        assert!(PluginType::Rust.is_a_or_d());
        assert!(PluginType::Wasm.is_a_or_d());
        assert!(!PluginType::Js.is_a_or_d());
        assert!(!PluginType::Process.is_a_or_d());
    }

    #[test]
    fn process_plugin_requires_sidecar() {
        let mut m = js_manifest();
        m.plugin_type = PluginType::Process;
        m.entry = EntrySpec::default();
        assert!(m.validate(&idx()).is_err());
        m.entry.sidecar = Some("formatter.exe".into());
        m.abi = Some(AbiFingerprint {
            rust: Some("tauron@2.3.1".into()),
            ..Default::default()
        });
        let e = m.validate(&idx()).unwrap_err();
        assert!(e.message.contains("非 A/D 类"), "process 是 C 类，不应要求 abi");
        m.abi = None;
        assert!(m.validate(&idx()).is_ok());
    }

    // ── 真实词表文件 schema/permissions.index.json（计划 §4.2 / R3）──

    const SCHEMA_PATH: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../schema/permissions.index.json");

    #[test]
    fn real_permissions_index_loads_and_is_well_formed() {
        // 门禁：随框架发版的机器生成词表必须可解析、字段完备、无重复。
        let idx = PermissionIndex::load(SCHEMA_PATH)
            .expect("schema/permissions.index.json 必须存在且可解析");
        assert_eq!(idx.version, 1);
        assert!(
            idx.entries.len() >= 40,
            "词表条目过少（{}），覆盖不足",
            idx.entries.len()
        );

        let mut seen = std::collections::HashSet::new();
        for e in &idx.entries {
            assert!(!e.identifier.is_empty(), "identifier 不能为空");
            assert!(
                e.identifier.contains(':'),
                "identifier `{}` 必须是 `plugin:allow-x` 形式（Tauri 真实标识，R3）",
                e.identifier
            );
            assert!(
                !e.description.trim().is_empty(),
                "identifier `{}` 缺 description（审批 UI 文案来源，§4.5 要求同一来源）",
                e.identifier
            );
            assert!(
                seen.insert(e.identifier.clone()),
                "identifier `{}` 在词表中重复",
                e.identifier
            );
        }
    }

    #[test]
    fn forbidden_plains_must_exist_in_index() {
        // 禁止授予清单里的权限必须同时出现在词表内：
        // 否则安装期（§4.2 表外即失败）与授予期（§2.1 禁止清单）判定会不一致。
        let idx = PermissionIndex::load(SCHEMA_PATH).unwrap();
        for ident in crate::authz::FORBIDDEN_PLAIN {
            assert!(idx.contains(ident), "`{ident}` 必须在词表内");
            assert!(
                idx.risk_of(ident) == Some(Risk::High),
                "`{ident}` 在禁止授予清单内，词表中必须标为 high"
            );
        }
    }

    #[test]
    fn real_index_rejects_table_outside_permission() {
        // 表外字符串 → 安装期硬失败（计划 §4.2 关键约束）。
        let idx = PermissionIndex::load(SCHEMA_PATH).unwrap();
        let e = idx
            .check_permitted(&Permission::new("fs:allow-delete-everything"))
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("fs:allow-delete-everything"));
    }

    #[test]
    fn real_index_covers_every_manifest_example() {
        // 回归 R3：架构方案 §4.2 示例 manifest 里出现的权限必须全部在词表内。
        let idx = PermissionIndex::load(SCHEMA_PATH).unwrap();
        for ident in [
            "store:allow-get",
            "store:allow-set",
            "fs:allow-app-read",
            "http:allow-fetch",
            "event:allow-emit",
            "dialog:allow-open",
        ] {
            assert!(idx.contains(ident), "示例权限 `{ident}` 必须已登记");
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // Contributes 类型化校验测试（安全审查 D3/D5）
    // ────────────────────────────────────────────────────────────────────────

    fn contributes_plugin_id() -> PluginId {
        PluginId::new("com.example.test").unwrap()
    }

    #[test]
    fn contributes_empty_is_valid() {
        let id = contributes_plugin_id();
        let c = Contributes::default();
        assert!(c.is_empty());
        assert!(c.validate(&id).is_ok());
    }

    #[test]
    fn contributes_valid_full_is_accepted() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute {
                id: "cmd.format".into(),
                title: "Format Code".into(),
            }],
            menus: vec![MenuContribute {
                id: "menu.format".into(),
                command: "cmd.format".into(),
            }],
            panels: vec![PanelContribute {
                id: "panel.output".into(),
                title: "Output".into(),
                icon: "assets/output.svg".into(),
            }],
            settings_tabs: vec![SettingsTabContribute {
                id: "tab.general".into(),
                title: "General".into(),
            }],
            shortcuts: vec![ShortcutContribute {
                accelerator: "Ctrl+Shift+F".into(),
                command: "cmd.format".into(),
            }],
        };
        assert!(!c.is_empty());
        assert!(c.validate(&id).is_ok());
    }

    #[test]
    fn contributes_duplicate_command_id_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![
                CommandContribute { id: "cmd.dup".into(), title: "A".into() },
                CommandContribute { id: "cmd.dup".into(), title: "B".into() },
            ],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("重复"));
    }

    #[test]
    fn contributes_global_id_uniqueness_across_types() {
        let id = contributes_plugin_id();
        // 菜单 id 与命令 id 重复
        let c = Contributes {
            commands: vec![CommandContribute { id: "cmd.format".into(), title: "Format".into() }],
            menus: vec![MenuContribute {
                id: "cmd.format".into(),
                command: "cmd.format".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("重复"));
    }

    #[test]
    fn contributes_menu_dangling_command_ref_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute { id: "cmd.a".into(), title: "A".into() }],
            menus: vec![MenuContribute {
                id: "menu.a".into(),
                command: "cmd.nonexistent".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("未在任何 commands 中声明"));
    }

    #[test]
    fn contributes_shortcut_dangling_command_ref_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute { id: "cmd.a".into(), title: "A".into() }],
            shortcuts: vec![ShortcutContribute {
                accelerator: "Ctrl+Z".into(),
                command: "cmd.nonexistent".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("未在任何 commands 中声明"));
    }

    #[test]
    fn contributes_duplicate_shortcut_accelerator_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute { id: "cmd.a".into(), title: "A".into() }],
            shortcuts: vec![
                ShortcutContribute { accelerator: "Ctrl+Z".into(), command: "cmd.a".into() },
                ShortcutContribute { accelerator: "ctrl+z".into(), command: "cmd.a".into() },
            ],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("快捷键冲突"));
    }

    #[test]
    fn contributes_title_too_long_is_rejected() {
        let id = contributes_plugin_id();
        let long_title = "x".repeat(65);
        let c = Contributes {
            commands: vec![CommandContribute {
                id: "cmd.a".into(),
                title: long_title,
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("超过"));
    }

    #[test]
    fn contributes_title_max_length_is_accepted() {
        let id = contributes_plugin_id();
        let max_title = "x".repeat(64);
        let c = Contributes {
            commands: vec![CommandContribute {
                id: "cmd.a".into(),
                title: max_title,
            }],
            ..Default::default()
        };
        assert!(c.validate(&id).is_ok());
    }

    #[test]
    fn contributes_title_with_html_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute {
                id: "cmd.a".into(),
                title: "<script>alert(1)</script>".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("HTML"));
    }

    #[test]
    fn contributes_panel_icon_outside_assets_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            panels: vec![PanelContribute {
                id: "panel.a".into(),
                title: "Panel".into(),
                icon: "../etc/passwd".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("不在允许路径"));
    }

    #[test]
    fn contributes_panel_icon_absolute_path_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            panels: vec![PanelContribute {
                id: "panel.a".into(),
                title: "Panel".into(),
                icon: "/etc/passwd".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("不在允许路径"));
    }

    #[test]
    fn contributes_panel_icon_valid_assets_path_is_accepted() {
        let id = contributes_plugin_id();
        let c = Contributes {
            panels: vec![PanelContribute {
                id: "panel.a".into(),
                title: "Panel".into(),
                icon: "assets/icons/output.svg".into(),
            }],
            ..Default::default()
        };
        assert!(c.validate(&id).is_ok());
    }

    #[test]
    fn contributes_empty_command_id_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute { id: "".into(), title: "A".into() }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("id 为空"));
    }

    #[test]
    fn contributes_command_id_with_whitespace_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute { id: "cmd a".into(), title: "A".into() }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("空白"));
    }

    #[test]
    fn contributes_empty_accelerator_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![CommandContribute { id: "cmd.a".into(), title: "A".into() }],
            shortcuts: vec![ShortcutContribute {
                accelerator: "".into(),
                command: "cmd.a".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("accelerator 为空"));
    }

    #[test]
    fn contributes_multiple_errors_are_collected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            commands: vec![
                CommandContribute { id: "cmd.dup".into(), title: "A".into() },
                CommandContribute { id: "cmd.dup".into(), title: "<bad>".into() },
            ],
            shortcuts: vec![ShortcutContribute {
                accelerator: "Ctrl+Z".into(),
                command: "cmd.nonexistent".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("重复"));
        assert!(err.message.contains("HTML"));
        assert!(err.message.contains("未在任何 commands 中声明"));
    }

    #[test]
    fn contributes_panel_empty_icon_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            panels: vec![PanelContribute {
                id: "panel.a".into(),
                title: "Panel".into(),
                icon: "".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("icon 为空"));
    }

    #[test]
    fn contributes_settings_tab_title_with_html_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            settings_tabs: vec![SettingsTabContribute {
                id: "tab.a".into(),
                title: "<img src=x onerror=alert(1)>".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("HTML"));
    }

    #[test]
    fn contributes_panel_title_too_long_is_rejected() {
        let id = contributes_plugin_id();
        let c = Contributes {
            panels: vec![PanelContribute {
                id: "panel.a".into(),
                title: "x".repeat(100),
                icon: "assets/a.svg".into(),
            }],
            ..Default::default()
        };
        let err = c.validate(&id).unwrap_err();
        assert!(err.message.contains("超过"));
    }

    // ── PluginManifest::validate 集成测试 ──────────────────────────────

    #[test]
    fn manifest_validate_rejects_invalid_contributes() {
        let id = contributes_plugin_id();
        let manifest = PluginManifest {
            id: id.clone(),
            name: "Test".into(),
            version: semver::Version::new(1, 0, 0),
            plugin_type: PluginType::Js,
            entry: EntrySpec::default(),
            permissions: vec![],
            scopes: Default::default(),
            platforms: vec![],
            framework: VersionReq::STAR, // 会被拒绝，但主要测试 contributes
            abi: None,
            contributes: Contributes {
                commands: vec![CommandContribute {
                    id: "cmd.a".into(),
                    title: "<script>alert(1)</script>".into(),
                }],
                ..Default::default()
            },
            settings_schema: None,
            events: EventsDecl::default(),
            host_functions: vec![],
            min_allowed_version: None,
            signature: None,
            publisher: None,
        };
        let index = idx();
        let result = manifest.validate(&index);
        assert!(result.is_err(), "含 HTML 标题的 contributes 必须被拒绝");
        let err = result.unwrap_err();
        assert!(err.message.contains("HTML"), "错误消息应包含 HTML 相关说明");
    }

    // ── 权限与 host_functions 重复检测 ────────────────────────────

    #[test]
    fn duplicate_permission_is_rejected() {
        let mut m = js_manifest();
        m.permissions = vec![
            Permission::new("store:allow-get"),
            Permission::new("store:allow-get"), // 重复
        ];
        let err = m.validate(&idx()).unwrap_err();
        assert!(err.message.contains("重复声明"));
    }

    #[test]
    fn duplicate_host_function_is_rejected() {
        let mut m = js_manifest();
        m.host_functions = vec!["customCommand".into(), "customCommand".into()];
        let err = m.validate(&idx()).unwrap_err();
        assert!(err.message.contains("重复声明"));
    }

    #[test]
    fn unique_permissions_are_accepted() {
        let mut m = js_manifest();
        m.permissions = vec![
            Permission::new("store:allow-get"),
            Permission::new("store:allow-set"),
        ];
        let mut scopes = serde_json::Map::new();
        scopes.insert(
            "store:allow-set".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("user/*".into())]),
        );
        m.scopes = scopes;
        assert!(m.validate(&idx()).is_ok());
    }

    #[test]
    fn empty_publisher_is_rejected() {
        let mut m = js_manifest();
        m.publisher = Some("   ".into());
        let err = m.validate(&idx()).unwrap_err();
        assert!(err.message.contains("publisher"));
    }

    #[test]
    fn empty_signature_is_rejected() {
        let mut m = js_manifest();
        m.signature = Some("  ".into());
        let err = m.validate(&idx()).unwrap_err();
        assert!(err.message.contains("signature"));
    }

    #[test]
    fn valid_publisher_and_signature_are_accepted() {
        let mut m = js_manifest();
        m.publisher = Some("com.example".into());
        m.signature = Some("abc123def456".into());
        assert!(m.validate(&idx()).is_ok());
    }
}
