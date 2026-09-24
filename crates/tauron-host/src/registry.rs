//! §4.1 注册表 + pending call 表。
//! 职责：插件安装/查询/启停/卸载；活跃身份 LRU；pending call 的 TTL GC；
//! 调用序号（`seq`）分配；**生命周期状态的唯一写入者**（`plugin.state` 单点写入）。
//!
//! 关键约束（计划 §4.1）：
//! - 插件**注册表超上限硬失败**（缺省 8），拒绝而非挤占；
//! - 活跃身份上限 8（LRU），超限时淘汰最旧者；
//! - **pending call 必须有 TTL GC**（防僵尸条目）；
//! - **窗口关闭必须清理 pending**（否则 JS 侧 `invoke()` 永久挂起——ADR-04）；
//! - 调用序号由宿主分配、单调递增，**不接受前端传入**（防伪造与乱序）；
//! - 状态变更**只能经 `report_event` / `lifecycle_report`**（单写者原则，计划 §4.3）。
//!
//! 并发模型：`parking_lot`。`entries` 用 `RwLock`（读多写少），
//! `pending` / `runtime` / `active_order` 用 `Mutex`（高频短临界区）。
//! **锁不嵌套**：`entries` 与 `active_order` 永不同时持有——LRU 淘汰在锁外
//! 完成，避免 parking_lot 非重入导致的死锁。域内锁序见 [`Registry`] 的文档
//! （`pending → streams → runtime → active_order`）。

use crate::error::{ErrorCode, HostError, HostResult};
use crate::lifecycle::{Event, PluginState, State, TransitionOutcome, transition};
use crate::manifest::{PluginIdentity, PluginId, PluginManifest, PermissionIndex};
use crate::runtime::{RuntimeHandle, RuntimeLease, RuntimeTable};
use crate::stream::{StreamFrame, StreamKind, StreamRegistry, StreamSink};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// 缺省配置（计划 §4.1 的数值约束）。
pub fn default_config() -> RegistryConfig {
    RegistryConfig {
        max_plugins: 8,
        max_active_identities: 8,
        max_pending_calls: 1000,
        pending_ttl: Duration::from_secs(30),
        plugin_filter: None,
    }
}

/// 插件加载过滤器（配置化选择加载——不是每个客户端都需要全量加载插件）。
///
/// 优先级：`deny` > `allow` > `types` > `platforms` > `include_builtins`。
/// 空 `allow` = 允许所有；空 `types` = 所有类型；空 `platforms` = 所有平台。
///
/// 设计原则：
/// - 过滤器只影响安装（`install()`），不影响已安装插件的查询/卸载；
/// - 被过滤的插件返回 `E_PLUGIN_FILTERED`（可重试错误，用户可修改配置后重试）；
/// - 内置插件（`oc.` 前缀）默认加载，可通过 `include_builtins: false` 排除。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginFilter {
    /// 允许加载的插件 ID 列表（空 = 允许所有）。
    pub allow: Vec<String>,
    /// 禁止加载的插件 ID 列表（从允许列表中排除）。
    pub deny: Vec<String>,
    /// 仅加载指定类型的插件（空 = 所有类型；值如 `"js"`, `"rust"`, `"process"`, `"wasm"`）。
    pub types: Vec<String>,
    /// 仅加载指定平台的插件（空 = 所有平台；值如 `"win"`, `"mac"`, `"linux"`）。
    pub platforms: Vec<String>,
    /// 是否加载内置插件（默认 `true`；`false` 时排除 `oc:` 前缀插件）。
    pub include_builtins: bool,
}

impl PluginFilter {
    /// 判断插件是否通过过滤器。
    pub fn matches(&self, manifest: &PluginManifest) -> bool {
        let id_str = manifest.id.as_str();

        // 1. 内置插件检查（`oc.` 前缀 = 内置插件）
        if !self.include_builtins && id_str.starts_with("oc.") {
            return false;
        }

        // 2. 禁止列表（最高优先级）
        if self.deny.iter().any(|d| d == id_str) {
            return false;
        }

        // 3. 允许列表（空 = 允许所有）
        if !self.allow.is_empty() && !self.allow.iter().any(|a| a == id_str) {
            return false;
        }

        // 4. 类型过滤（空 = 所有类型）
        if !self.types.is_empty() {
            let type_str = manifest.plugin_type.to_string();
            if !self.types.iter().any(|t| t == &type_str) {
                return false;
            }
        }

        // 5. 平台过滤（空 = 所有平台）
        if !self.platforms.is_empty() {
            let manifest_platforms: Vec<&str> = manifest.platforms.iter().map(|s| s.as_str()).collect();
            if !manifest_platforms.iter().any(|p| self.platforms.iter().any(|f| f == p)) {
                return false;
            }
        }

        true
    }

    /// 检查过滤器配置是否合法（CI 门禁用）。
    pub fn validate(&self) -> HostResult<()> {
        // allow 与 deny 不能重复
        for a in &self.allow {
            if self.deny.iter().any(|d| d == a) {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("插件过滤器配置错误：`{a}` 同时出现在 allow 与 deny 中"),
                ));
            }
        }
        Ok(())
    }
}

/// 注册表配置。
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    /// 注册表插件上限（超过即硬失败）。
    pub max_plugins: usize,
    /// 活跃身份上限（LRU）。
    pub max_active_identities: usize,
    /// pending call 上限。
    pub max_pending_calls: usize,
    /// pending call TTL（超时即视为超时并回收）。
    pub pending_ttl: Duration,
    /// 插件加载过滤器（配置化选择加载；`None` = 全量加载）。
    pub plugin_filter: Option<PluginFilter>,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        default_config()
    }
}

/// 注册表条目（内部记录，不跨 IPC 序列化）。
#[derive(Debug, Clone)]
pub struct PluginEntry {
    pub id: PluginId,
    pub manifest: PluginManifest,
    pub state: PluginState,
    /// 身份令牌（webview 绑定期防重放）。
    pub identity_token: u64,
    pub installed_at: Instant,
    pub last_accessed: Instant,
}

impl PluginEntry {
    fn new(manifest: PluginManifest, token: u64) -> Self {
        let now = Instant::now();
        Self {
            id: manifest.id.clone(),
            manifest,
            state: PluginState::default(),
            identity_token: token,
            installed_at: now,
            last_accessed: now,
        }
    }

    /// 是否占用活跃身份槽位。
    pub fn is_active(&self) -> bool {
        matches!(self.state.state, State::Enabled | State::Running)
    }
}

/// `host_registry_list` 的返回单元（跨 IPC）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: State,
    pub plugin_type: String,
    /// 该插件声明的 public 事件 topic（供订阅方发现）。
    pub published_topics: Vec<String>,
    /// 是否被安全模式禁用（D25/D28；`<oc-plugin-manager>` 角标来源）。
    pub disabled_by_safemode: bool,
}

/// pending call 条目（内部路由数据）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCall {
    pub call_id: String,
    pub plugin_id: String,
    pub cmd: String,
    pub args: serde_json::Value,
    /// 宿主分配的单调序号（§2.1）。
    pub seq: u64,
    #[serde(serialize_with = "serialize_instant")]
    pub created_at: Instant,
    #[serde(serialize_with = "serialize_instant")]
    pub expires_at: Instant,
}

/// 将 `Instant` 序列化为 u64（毫秒时间戳，参考进程启动时间）。
fn serialize_instant<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let elapsed = instant.elapsed().as_millis() as u64;
    serializer.serialize_u64(elapsed)
}

/// 注册表。
///
/// **锁序**：`pending → streams → runtime → active_order`。`streams` 与 `pending` 同域
/// （方案 R5 不变量 1）：`call_end` / `call_end_all` 要先摘 pending 条目、再回收
/// 该条目上的流并补发终帧，两步必须在同一次加锁语义下完成，否则会出现
/// 「调用已结束、流还活着」的悬挂（接收方永远等不到终帧）。
///
/// `runtime`（进程插件租约表，P0-2）排在 `streams` 之后：运行时命令只在
/// `runtime` 锁内做「查既有租约 / 启动进程 / 登记 / 终止」，**不碰 pending 与
/// streams**，因此它既不构成新的一环、也不会被别人反序持有。租约与流不同域（一个是
/// 进程句柄、一个是帧句柄），没有"必须同一次加锁"的语义，故不合并。
///
/// **运行时租约不变量**（P0-2）
/// - `plugin_id ↔ lease ↔ pid` 一一绑定，一个插件至多一条租约；
/// - **不可用即无进程**：状态离开 `ENABLED`/`RUNNING` 后，仍在跑的 sidecar 一律被
///   终止（唯一执行点是 [`Registry::reclaim_if_unusable`]，在 `report_event` 的
///   公共尾部调用——放这里才能覆盖禁用/安全模式对账/卸载/错误迁移全部离场路径）；
/// - 已标崩溃的租约不在此列：进程已死，租约留着给 `host_runtime_health` 报账。
pub struct Registry {
    config: RegistryConfig,
    entries: RwLock<HashMap<PluginId, PluginEntry>>,
    pending: Mutex<HashMap<String, PendingCall>>,
    /// 流句柄表 + 调用帧载体表（R5）。
    streams: Mutex<StreamRegistry>,
    /// 进程插件运行时租约表（P0-2）：`plugin_id ↔ lease ↔ pid`。
    ///
    /// 放在**同一个域锁之内**（而不是另立一个 Registry）：租约的生命周期与插件
    /// 条目同源（卸载必须回收租约），而"插件有哪些运行期句柄"必须能被同一处
    /// 回答，否则卸载路径一定会漏。
    runtime: Mutex<RuntimeTable>,
    /// 活跃身份顺序表（末尾为最近使用）。
    active_order: Mutex<Vec<PluginId>>,
    next_seq: AtomicU64,
    next_token: AtomicU64,
}

impl Registry {
    pub fn new(config: RegistryConfig) -> Self {
        Self {
            config,
            entries: RwLock::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            streams: Mutex::new(StreamRegistry::new()),
            runtime: Mutex::new(RuntimeTable::new()),
            active_order: Mutex::new(Vec::new()),
            next_seq: AtomicU64::new(0),
            next_token: AtomicU64::new(1),
        }
    }

    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    // ────────────────────────────────────────────────────────────
    // 安装 / 查询
    // ────────────────────────────────────────────────────────────

    /// 安装插件：过滤器 → schema 校验 → 唯一性 → 容量 → 迁移至 `INSTALLED`。
    ///
    /// 校验失败也会落库为 `INSTALL_FAILED`（D3：安装期失败可见、可卸载，
    /// 但**不自动重试**）。id 无法解析时不入库——那是解析失败，不是安装失败。
    ///
    /// 过滤器（配置化选择加载）在 schema 校验之前执行：被过滤的插件返回
    /// `E_PLUGIN_FILTERED`，不入库、不占用容量。
    pub fn install(&self, index: &PermissionIndex, manifest: PluginManifest) -> HostResult<PluginId> {
        let id = manifest.id.clone();

        // 配置化选择加载：过滤器检查
        if let Some(ref filter) = self.config.plugin_filter {
            if !filter.matches(&manifest) {
                return Err(HostError::new(
                    ErrorCode::E_PLUGIN_FILTERED,
                    format!(
                        "插件 `{id}` 被配置过滤器排除（allow/deny/types/platforms/include_builtins）；修改配置后重试"
                    ),
                ));
            }
        }

        let entries = self.entries.read();
        if entries.contains_key(&id) {
            drop(entries);
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_EXISTS,
                format!("插件 `{id}` 已存在（唯一性约束，计划 §4.2）"),
            ));
        }
        if entries.len() >= self.config.max_plugins {
            drop(entries);
            return Err(HostError::new(
                ErrorCode::E_REGISTRY_FULL,
                format!(
                    "注册表已达上限 {}；插件 `{id}` 被拒绝（硬失败而非挤占，计划 §4.1）",
                    self.config.max_plugins
                ),
            ));
        }
        drop(entries);

        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let mut entry = PluginEntry::new(manifest.clone(), token);

        // DISCOVERED → INSTALLING（内部路径，不应非法）。
        let o = transition(&mut entry.state, Event::InstallStart);
        assert!(!o.illegal, "注册表内部迁移不应非法：{o:?}");

        match manifest.validate(index) {
            Ok(()) => {
                let o = transition(&mut entry.state, Event::InstallOk);
                assert!(!o.illegal);
                self.put_entry(entry, id.clone());
                Ok(id)
            }
            Err(e) => {
                let o = transition(&mut entry.state, Event::InstallFail);
                assert!(!o.illegal);
                entry.state.last_reason = Some(e.message.clone());
                self.put_entry(entry, id.clone());
                Err(HostError::new(
                    e.code,
                    format!(
                        "{}（插件 `{id}` 已记入 INSTALL_FAILED，可卸载；不自动重试）",
                        e.message
                    ),
                ))
            }
        }
    }

    /// 安装期失败登记（manifest 尚未完整解析时的兜底路径）。
    pub fn record_install_failure(&self, id: PluginId, reason: String) -> HostResult<()> {
        let entries = self.entries.read();
        if entries.contains_key(&id) {
            drop(entries);
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_EXISTS,
                format!("插件 `{id}` 已存在"),
            ));
        }
        if entries.len() >= self.config.max_plugins {
            drop(entries);
            return Err(HostError::new(
                ErrorCode::E_REGISTRY_FULL,
                format!("注册表已达上限 {}", self.config.max_plugins),
            ));
        }
        drop(entries);

        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        let mut entry = PluginEntry::new(minimal_manifest(id.clone()), token);
        let o1 = transition(&mut entry.state, Event::InstallStart);
        assert!(!o1.illegal);
        let o2 = transition(&mut entry.state, Event::InstallFail);
        assert!(!o2.illegal);
        entry.state.last_reason = Some(reason);
        self.put_entry(entry, id);
        Ok(())
    }

    /// 读取条目快照。
    pub fn find(&self, id: &PluginId) -> Option<PluginEntry> {
        self.entries.read().get(id).cloned()
    }

    /// 未知 plugin_id → 结构化错误（计划 §4.1：不 panic）。
    pub fn require(&self, id: &PluginId) -> HostResult<PluginEntry> {
        self.find(id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_UNKNOWN_PLUGIN,
                format!("plugin_id `{id}` 未在注册表中"),
            )
        })
    }

    /// 可见插件列表（`host_registry_list`，`scoped-read` 档）。
    ///
    /// 过滤规则：调用者只能看到**自己**、以及**自己订阅了对方 public topic 的插件**。
    /// 计划 §4.21 测试点"低权限插件尝试 list → 仅拿到可见集"的落地点。
    pub fn list_visible(
        &self,
        caller: Option<&PluginId>,
        subscribed_topics: &[String],
    ) -> Vec<PluginSummary> {
        let entries = self.entries.read();
        let mut out = Vec::with_capacity(entries.len());
        for e in entries.values() {
            let is_self = caller.is_some_and(|c| c == &e.id);
            let subscribes_public = !subscribed_topics.is_empty()
                && e.manifest.events.publish.iter().any(|p| {
                    p.public && subscribed_topics.iter().any(|t| t == &p.topic)
                });
            if is_self || subscribes_public {
                out.push(summary_of(e));
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 全量列表（`privileged` 档 / 主窗 UI）。
    pub fn list_all(&self) -> Vec<PluginSummary> {
        let entries = self.entries.read();
        let mut out: Vec<PluginSummary> = entries.values().map(summary_of).collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// 身份绑定：为插件分配 webview label 与令牌。
    pub fn bind_identity(&self, id: &PluginId) -> HostResult<PluginIdentity> {
        let entries = self.entries.read();
        let e = entries.get(id).ok_or_else(|| {
            HostError::new(ErrorCode::E_UNKNOWN_PLUGIN, format!("plugin_id `{id}` 未在注册表中"))
        })?;
        Ok(PluginIdentity::new(e.id.clone(), e.identity_token))
    }

    // ────────────────────────────────────────────────────────────
    // 生命周期（`plugin.state` 单写者入口）
    // ────────────────────────────────────────────────────────────

    /// **`plugin.state` 的唯一写入入口**（计划 §4.3）。
    ///
    /// 身份从 webview label 取，**忽略入参 plugin_id**（self 档，§2.1）；
    /// 跨插件冒充（伪造 plugin_id）在此硬拒。
    pub fn lifecycle_report(
        &self,
        webview_label: &str,
        claimed_id: Option<&str>,
        event: Event,
    ) -> HostResult<TransitionOutcome> {
        let id = crate::authz::resolve_self_identity(webview_label, claimed_id)?;
        self.report_event(&id, event)
    }

    /// 按 id 上报事件（宿主内部路径）。
    pub fn report_event(&self, id: &PluginId, event: Event) -> HostResult<TransitionOutcome> {
        let out = {
            let mut entries = self.entries.write();
            let e = entries
                .get_mut(id)
                .ok_or_else(|| HostError::new(ErrorCode::E_UNKNOWN_PLUGIN, format!("plugin_id `{id}` 未在注册表中")))?;
            let out = transition(&mut e.state, event);
            e.last_accessed = Instant::now();
            out
        };
        // 锁外维护 LRU（避免嵌套持锁）。
        // 自环迁移（如 `HealthOk`：ENABLED→ENABLED）也刷新最近访问时间。
        if !out.illegal {
            // **「不可用就不许有进程」单一规则**（P0-2）：状态离开
            // ENABLED/RUNNING 后，仍在跑的 sidecar 必须被终止（回收租约）。
            // 放在这里而不是各调用点，是因为它要覆盖**所有**离场路径：
            // 用户禁用（`Disable`）、安全模式对账（`SafemodeEnter`）、
            // 卸载/清除、以及任何错误迁移——逐一在各命令里补会被漏掉，
            // 而"注册表说禁用、进程却在跑"正是不该被容忍的不自洽。
            // 顺序也重要：先回收再维护 LRU（回收会取 `runtime` 锁，与
            // `entries` 不重叠；域内锁序仍是 `pending → streams → runtime`）。
            self.reclaim_if_unusable(id);
            self.maintain_active(id);
        }
        Ok(out)
    }

    /// 状态不再"可用"时回收**仍在跑**的租约（终止进程）。
    ///
    /// 不变量：注册表说插件不可用（`ENABLED`/`RUNNING` 之外），就不该有活着的
    /// sidecar——前端显示"已禁用"而进程还在跑，比"少一次自动启动"严重得多。
    ///
    /// 两点刻意的保留：
    /// - **已标崩溃的租约不动**：进程早已死透，租约要留给 `host_runtime_health`
    ///   把这次崩溃报出去（在那里摘掉租约，调用方只会拿到 `E_LEASE_EXPIRED`，
    ///   崩溃事实连同计数一起消失）；
    /// - 未安装/未 spawn 过的插件本就没有租约，这里是空操作。
    fn reclaim_if_unusable(&self, id: &PluginId) {
        if self.find(id).map(|e| e.is_active()).unwrap_or(false) {
            return;
        }
        self.runtime.lock().remove_if_live(id.as_str());
    }

    /// 管理操作（`host_registry_admin`，D15：主窗特权，非插件命令面）。
    pub fn admin_op(
        &self,
        id: &PluginId,
        op: crate::authz::RegistryAdminOp,
    ) -> HostResult<TransitionOutcome> {
        let event = match op {
            crate::authz::RegistryAdminOp::Enable => Event::Enable,
            crate::authz::RegistryAdminOp::Disable => Event::Disable,
            crate::authz::RegistryAdminOp::Uninstall => Event::Uninstall,
            crate::authz::RegistryAdminOp::Purge => Event::Purge,
        };
        let out = self.report_event(id, event)?;
        if !out.illegal
            && out.from != out.to
            && matches!(event, Event::Uninstall | Event::Purge)
        {
            self.entries.write().remove(id);
            self.call_end_all(id);
            // 运行期租约随插件条目一起回收（P0-2）：租约是**插件身份**的句柄，
            // 条目没了它就成了没人认领的孤儿——重装同名插件会与旧租约撞车。
            // 三把锁按域内锁序**顺序**取（pending/streams 已在上一步取完并释放），
            // 不嵌套持有。
            self.runtime.lock().remove_plugin(id.as_str());
            self.active_order.lock().retain(|x| x != id);
        }
        Ok(out)
    }

    // ────────────────────────────────────────────────────────────
    // pending call 表
    // ────────────────────────────────────────────────────────────

    /// 登记一次调用的 pending 条目并分配单调序号。
    ///
    /// 调用方必须是已启用/运行中的插件（ADR-05：`enabled` 标志即时拒绝）。
    pub fn call_begin(
        &self,
        id: &PluginId,
        cmd: &str,
        args: serde_json::Value,
    ) -> HostResult<PendingCall> {
        let entry = self.require(id)?;
        if !entry.is_active() {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_DISABLED,
                format!("插件 `{id}` 当前状态 {}，不允许发起调用", entry.state.state),
            ));
        }
        if cmd.trim().is_empty() {
            return Err(HostError::new(ErrorCode::E_AUTH_DENIED, "调用命令为空"));
        }
        let mut pending = self.pending.lock();
        if pending.len() >= self.config.max_pending_calls {
            // 表满时先做 TTL GC：僵尸条目（前端已超时却从未 callEnd 的调用）
            // 不得永久占用容量位——否则一次永久挂起累积起来就会把表占满，
            // 使后续所有调用永久 `E_CALL_PENDING_FULL`（ADR-04 的挂起会累积）。
            //
            // 只在**满时**驱逐：稳态（表未满）不做 GC，保证 `call_status` 对
            // 过期条目仍报 `E_CALL_TIMEOUT` 而非 `E_CALL_NOT_FOUND`——
            // 两者可重试性不同（前者可重试），语义不能塌缩。
            drop(pending);
            self.gc_expired();
            pending = self.pending.lock();
        }
        if pending.len() >= self.config.max_pending_calls {
            return Err(HostError::new(
                ErrorCode::E_CALL_PENDING_FULL,
                format!(
                    "pending call 表已达上限 {}（计划 §4.1）",
                    self.config.max_pending_calls
                ),
            ));
        }
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let call_id = Uuid::new_v4().to_string();
        let now = Instant::now();
        let call = PendingCall {
            call_id: call_id.clone(),
            plugin_id: id.as_str().to_string(),
            cmd: cmd.to_string(),
            args,
            seq,
            created_at: now,
            expires_at: now + self.config.pending_ttl,
        };
        pending.insert(call_id, call.clone());
        Ok(call)
    }

    /// 结束一次调用并回收条目（返回原条目供结果路由）。
    ///
    /// **顺带回收该调用上的流并补发 `end` 终帧**：handler 忘了关流时，接收方靠这一帧
    /// 才会结束等待（R5 不变量 3）。这是唯一兜底，因此不能省。
    pub fn call_end(&self, call_id: &str) -> HostResult<PendingCall> {
        self.end_call(call_id, StreamKind::End, None)
    }

    /// 取消调用（仅回收，不返回条目）。流以 `error` 终帧收尾并带上原因。
    pub fn call_cancel(&self, call_id: &str) -> HostResult<()> {
        self.end_call(
            call_id,
            StreamKind::Error,
            Some(serde_json::json!({ "reason": "canceled" })),
        )?;
        Ok(())
    }

    /// 摘 pending 条目 + 回收其流。**锁序 `pending → streams`，两步不嵌套持有**
    /// （符合本文件「锁不嵌套」的并发模型）。
    fn end_call(
        &self,
        call_id: &str,
        terminal: StreamKind,
        reason: Option<serde_json::Value>,
    ) -> HostResult<PendingCall> {
        let call = self.pending.lock().remove(call_id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("pending call `{call_id}` 不存在或已结束"),
            )
        })?;
        self.streams.lock().close_for_call(call_id, terminal, reason);
        Ok(call)
    }

    /// **窗口关闭清理**（ADR-04）：一次清空该插件的全部 pending 条目。
    /// 不清理会导致 JS 侧 `invoke()` 永久挂起。返回清除数量。
    pub fn call_end_all(&self, id: &PluginId) -> usize {
        let removed = {
            let mut pending = self.pending.lock();
            let before = pending.len();
            pending.retain(|_, c| c.plugin_id != id.as_str());
            before - pending.len()
        };
        // 同一订阅者的流整组回收（补 `error` 终帧 + 原因）：窗口没了，接收方
        // 也就不用再等了——不补的话那些流会永远挂在「等 next frame」上。
        self.streams.lock().close_for_subscriber(id.as_str());
        removed
    }

    /// TTL GC：回收全部过期条目。返回回收数量（计划 §4.1：必须有 TTL GC）。
    pub fn gc_expired(&self) -> usize {
        let now = Instant::now();
        let expired: Vec<String> = {
            let mut pending = self.pending.lock();
            let ids: Vec<String> = pending
                .iter()
                .filter(|(_, c)| c.expires_at <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &ids {
                pending.remove(id);
            }
            ids
        };
        let mut streams = self.streams.lock();
        for call_id in &expired {
            streams.close_for_call(
                call_id,
                StreamKind::Error,
                Some(serde_json::json!({ "reason": "call_timeout" })),
            );
        }
        expired.len()
    }

    // ────────────────────────────────────────────────────────────
    // 流式帧（R5 / P0-1）
    // ────────────────────────────────────────────────────────────

    /// 登记一次调用的帧载体（`host_plugin_call` 带 `channel` 时调用）。
    ///
    /// 调用必须存在：给一个不存在的 callId 登记载体意味着「帧给谁都说不清」，
    /// 宁可显式失败，也不要留下一条永远不会有接收方的流。
    ///
    /// **订阅者必须是调用归属者**：否则知道 callId 的插件就能把别人的帧劫持到
    /// 自己的 channel 上（`stream_open` 会挡住「开流」，但挡不住「换载体」——
    /// 换载体等于偷走后续所有帧）。
    pub fn stream_bind(
        &self,
        call_id: &str,
        subscriber: &str,
        sink: Arc<dyn StreamSink>,
    ) -> HostResult<()> {
        let owner = {
            let pending = self.pending.lock();
            pending.get(call_id).map(|c| c.plugin_id.clone())
        };
        let Some(owner) = owner else {
            return Err(HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("pending call `{call_id}` 不存在或已结束，无法登记帧载体"),
            ));
        };
        if owner != subscriber {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("调用 `{call_id}` 属于插件 `{owner}`，`{subscriber}` 不得挂载帧载体"),
            ));
        }
        self.streams.lock().bind(call_id, subscriber, sink);
        Ok(())
    }

    /// 为一次调用开流（`host_stream_open`）。
    pub fn stream_open(&self, call_id: &str, subscriber: &str) -> HostResult<String> {
        if !self.pending.lock().contains_key(call_id) {
            return Err(HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("pending call `{call_id}` 不存在或已结束"),
            ));
        }
        self.streams.lock().open(call_id, subscriber)
    }

    /// 写一帧 `data`（`host_stream_write`），`seq` 由宿主铸。
    pub fn stream_write(
        &self,
        stream_id: &str,
        subscriber: &str,
        args_json: Option<serde_json::Value>,
        args_raw: Option<Vec<u8>>,
    ) -> HostResult<StreamFrame> {
        self.streams
            .lock()
            .write(stream_id, subscriber, args_json, args_raw)
    }

    /// 发终帧并失效句柄（`host_stream_close`）。
    pub fn stream_close(
        &self,
        stream_id: &str,
        subscriber: &str,
        kind: StreamKind,
    ) -> HostResult<StreamFrame> {
        self.streams.lock().close(stream_id, subscriber, kind)
    }

    /// 句柄是否仍可写（诊断/测试）。
    pub fn stream_is_open(&self, stream_id: &str) -> bool {
        self.streams.lock().is_open(stream_id)
    }

    /// 活跃/残留句柄数（诊断/测试）。
    pub fn stream_len(&self) -> usize {
        self.streams.lock().len()
    }

    /// 调用是否已挂帧载体（测试用）。
    pub fn stream_call_bound(&self, call_id: &str) -> bool {
        self.streams.lock().is_bound(call_id)
    }

    /// 查询单个 pending 条目；过期即返回 `E_CALL_TIMEOUT`。
    pub fn call_status(&self, call_id: &str, now: Instant) -> HostResult<PendingCall> {
        let pending = self.pending.lock();
        let c = pending.get(call_id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_CALL_NOT_FOUND,
                format!("pending call `{call_id}` 不存在或已结束"),
            )
        })?;
        if now >= c.expires_at {
            return Err(HostError::new(
                ErrorCode::E_CALL_TIMEOUT,
                format!(
                    "pending call `{call_id}` 已超时（TTL {}ms）",
                    self.config.pending_ttl.as_millis()
                ),
            ));
        }
        Ok(c.clone())
    }

    /// pending 表大小。
    pub fn pending_len(&self) -> usize {
        self.pending.lock().len()
    }

    // ────────────────────────────────────────────────────────────
    // 进程插件运行时租约（P0-2）
    //
    // 锁序 `pending → streams → runtime`：本节的每个方法**只**取 `runtime`
    // 锁（外加 `runtime_ensure_lease` 内的一次 `entries` 读取，且是在取
    // `runtime` **之前**完成的），因此不与任何既有路径构成环。
    //
    // **终止动作在 `runtime` 锁内发起**（`RuntimeTable::terminate`）：卸载与
    // 租约换新都必须"先杀旧进程、再摘表项"，两者分开就会在窗口里留下孤儿。
    // 终止器（`LeaseReaper`）是注入的纯函数式调用，不得回调注册表——装配层
    // 注入 `ProcSpawner::kill`，它只做 syscall。
    // ────────────────────────────────────────────────────────────

    /// 某插件当前的运行时租约句柄（无则 `None`）。
    ///
    /// 与 [`Self::runtime_lease`] 的区别：这里按**插件**查（"它现在还跑着吗"），
    /// 那里按 **lease** 查（"这个句柄还有效吗"）。前者查不到不是错误（插件本就
    /// 没 spawn 过），后者查不到是租约语义的错误（`E_LEASE_EXPIRED`）。
    pub fn runtime_handle_of(&self, id: &PluginId) -> Option<RuntimeHandle> {
        self.runtime.lock().handle_of_plugin(id.as_str())
    }

    /// 按 lease 取租约条目；未知 / 已失效 → `E_LEASE_EXPIRED`。
    ///
    /// **刻意不用 `E_CALL_NOT_FOUND`**：租约失效与"这次调用不存在"是两回事，
    /// 调用方该做的事也不同（重新 spawn vs 放弃这次调用）。混用错误码会让
    /// 前端把"进程没了"当成"调用登记丢了"，于是走错恢复分支。
    pub fn runtime_lease(&self, lease: &str) -> HostResult<RuntimeLease> {
        self.runtime.lock().get(lease).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_LEASE_EXPIRED,
                format!(
                    "运行时租约 `{lease}` 不存在或已失效（进程已回收 / 宿主已重启）；\
                     需要新的进程请重新 spawn"
                ),
            )
        })
    }

    /// 确保某插件有运行时租约：**已有活租约则原样返回既有 lease（幂等）**，
    /// 否则在**同一把 `runtime` 锁内**调用 `start_pid` 并登记。
    ///
    /// 重复 spawn 的语义在这里定死为「返回既有 lease」，理由：
    /// - `spawn` 的调用方要的是"这个插件现在有进程"，不是"再起一个进程"。两个
    ///   并发入口（用户连点两次、失败重试与自动恢复同时到达）若都能通过
    ///   "尚无租约"的判定，就会起两个进程，而租约只能指向其中一个——另一个是
    ///   **孤儿 PID**：没人探测它、卸载也回收不掉，正好违背本任务的不变量
    ///   「lease 与 PID 一一绑定」；
    /// - 另一种选择（显式拒绝第二个）会让调用方必须处理一个它无法避免的竞态，
    ///   且把"幂等"这一有用性质换成"要么成功要么报错"二义行为。
    /// 因此这里只有一种行为：**永远不启动第二个进程**。
    ///
    /// **"已有"的定义是"有活租约"**：已标崩溃的租约不算——那时进程已经没了，
    /// 调用方是来**重试**的（P0-2 的崩溃重试预算就靠这条路径才有意义）。
    /// 换新时会先终止旧 pid（[`RuntimeTable::register`] 内做，不允许直接覆盖）。
    ///
    /// 启动动作放在锁内（`start_pid`）是同一个理由的延伸：启动与登记必须是一个
    /// 原子步，否则"起来了但没登记"的窗口就是孤儿 PID 的窗口。代价是临界区里
    /// 含一次 `Command::spawn`（毫秒级），且**仍只持这一把锁**。
    ///
    /// 返回 `(句柄, 本次是否真的启动了新进程)`。第二个值必须是**锁内**判定并带出来
    /// 的：调用方据此决定"要不要把状态推到 RUNNING"（新进程 = 要；幂等返回 = 不要，
    /// 那条路径上状态本来就已经是 RUNNING）。若让调用方自己在锁外比较"之前有没有
    /// 活租约"，并发 spawn 时就可能两边都以为自己起了进程，从而投出重复事件。
    pub fn runtime_ensure_lease<F>(
        &self,
        id: &PluginId,
        start_pid: F,
    ) -> HostResult<(RuntimeHandle, bool)>
    where
        F: FnOnce() -> HostResult<u32>,
    {
        let mut runtime = self.runtime.lock();
        if let Some(existing) = runtime.live_handle_of_plugin(id.as_str()) {
            return Ok((existing, false));
        }
        let pid = start_pid()?;
        Ok((runtime.register(id.as_str(), pid), true))
    }

    /// 该插件是否需要（重新）启动进程：无租约，或租约对应的进程已经崩过。
    ///
    /// 调用方（`cmd_runtime_spawn`）据此决定"要不要过崩溃预算那道门"：已有活租约
    /// 时是纯查询语义（把既有句柄交回去），不该被预算拦下。
    pub fn runtime_needs_restart(&self, id: &PluginId) -> bool {
        self.runtime.lock().needs_restart(id.as_str())
    }

    /// 注入租约终止能力（装配层调用：适配层把它接到 `ProcSpawner::kill`）。
    ///
    /// 未注入时租约回收仍会摘表项，但每次终止都按**失败**留痕
    /// （见 [`Self::runtime_reap_stats`]）——不注入不等于没发生。
    pub fn set_lease_reaper(&self, reaper: Arc<dyn crate::runtime::LeaseReaper>) {
        self.runtime.lock().set_reaper(reaper);
    }

    /// 租约回收留痕快照（诊断/测试）：`attempts / terminated / already_gone / failures`。
    ///
    /// 终止失败**不会**让卸载失败（租约必须消失），所以这里是它唯一的可查出口。
    pub fn runtime_reap_stats(&self) -> crate::runtime::ReapStats {
        self.runtime.lock().reap_stats()
    }

    /// 标记该租约的进程崩溃，返回是否为**首次**观测到（`true` = 本次首见）。
    ///
    /// 首次的判定必须在锁内完成：调用方据此决定"计一次崩溃 / 投一次
    /// `RuntimeCrash` / `consecutiveFailures` +1"，两个并发 health 轮询若各自
    /// 在锁外判定，就会把同一次死亡记两次（崩溃计数与安全模式判定一起失真）。
    pub fn runtime_mark_crashed(&self, lease: &str) -> HostResult<bool> {
        self.runtime.lock().mark_crashed(lease).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_LEASE_EXPIRED,
                format!("运行时租约 `{lease}` 不存在或已失效，无法标记崩溃"),
            )
        })
    }

    /// 回收某插件的租约（**只动租约表，不动状态机**）。
    ///
    /// **生产调用点只有一处**：`tauron-adapter::attach_after_spawn` 在"启动期间条目
    /// 被并发卸载/清除"的错误路径上回收刚铸的租约——那种情形下插件条目已经不存在，
    /// 因此不会留下"状态说在跑、实际没进程"的不一致。其余生产路径的租约回收都发生在
    /// [`Registry::reclaim_if_unusable`]（状态离开 ENABLED/RUNNING 时，见
    /// `report_event`）与 [`RuntimeTable::register`]（换新租约时先终止旧 pid），
    /// 那两条自带状态语义。
    ///
    /// **在插件仍 `RUNNING` 时调用本函数会留下反向不一致**：状态说"在运行"、
    /// 实际已经没有进程（本函数不投任何事件，迁移表里也没有可用的 `DETACH` 迁移）。
    /// 这是一个已知的、**有意不修**的边界——若将来要把它接成"杀掉某插件进程"的命令，
    /// 必须先定义状态该落到哪里（`ErrorRetryable` / `Disabled` 还是新增 `DETACH`），
    /// 否则就是把断链接进产品路径。
    pub fn runtime_remove(&self, id: &PluginId) -> Option<RuntimeHandle> {
        self.runtime.lock().remove_plugin(id.as_str())
    }

    /// 租约表大小（诊断/测试）。
    pub fn runtime_len(&self) -> usize {
        self.runtime.lock().len()
    }

    // ────────────────────────────────────────────────────────────
    // 活跃身份 LRU
    // ────────────────────────────────────────────────────────────

    /// 当前活跃身份顺序（最旧在前）。测试/诊断用。
    pub fn active_ids(&self) -> Vec<PluginId> {
        self.active_order.lock().clone()
    }

    /// 维护活跃身份集：更新 LRU 顺序并在超限时淘汰最旧者。
    ///
    /// **必须在 `entries` 锁外调用**——淘汰动作本身要再取 `entries` 锁，
    /// 嵌套持有会死锁（`parking_lot` 不重入）。
    fn maintain_active(&self, id: &PluginId) {
        let active = self
            .entries
            .read()
            .get(id)
            .is_some_and(|e| e.is_active());
        {
            let mut order = self.active_order.lock();
            order.retain(|x| x != id);
            if active {
                order.push(id.clone());
            }
        }
        self.evict_overflow();
    }

    /// 淘汰超出上限的最旧活跃身份（发 `Disable`）。
    ///
    /// 直接内联迁移而不经 [`Self::report_event`]，避免
    /// "淘汰 → 再维护 → 再淘汰"的递归；每次迭代至少移除一项，必然终止。
    fn evict_overflow(&self) {
        let mut guard = 0u32;
        while self.active_order.lock().len() > self.config.max_active_identities {
            let evict = self.active_order.lock().first().cloned();
            let Some(evict) = evict else { break };

            {
                let mut entries = self.entries.write();
                if let Some(e) = entries.get_mut(&evict) {
                    if e.is_active() {
                        let o = transition(&mut e.state, Event::Disable);
                        if !o.illegal && o.from != o.to {
                            e.last_accessed = Instant::now();
                        }
                    }
                }
            }
            // 无论淘汰成功与否都移出顺序表（成功则已非活跃，失败则本就无效）。
            self.active_order.lock().retain(|x| x != &evict);

            guard += 1;
            if guard > 64 {
                break;
            }
        }
    }

    // ────────────────────────────────────────────────────────────
    // 内部
    // ────────────────────────────────────────────────────────────

    fn put_entry(&self, entry: PluginEntry, id: PluginId) {
        self.entries.write().insert(id.clone(), entry);
        self.maintain_active(&id);
    }

    /// 注册表条目数。
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(default_config())
    }
}

/// 构造仅含 id 的最小 manifest（`record_install_failure` 用）。
fn minimal_manifest(id: PluginId) -> PluginManifest {
    PluginManifest {
        id,
        name: "install-failed".to_string(),
        version: semver::Version::new(0, 0, 0),
        plugin_type: crate::manifest::PluginType::Js,
        entry: crate::manifest::EntrySpec::default(),
        permissions: Vec::new(),
        scopes: serde_json::Map::new(),
        platforms: Vec::new(),
        framework: crate::manifest::parse_version_range(">=2.0 <3.0")
            .expect("静态 range 必可解析"),
        abi: None,
        contributes: crate::manifest::Contributes::default(),
        settings_schema: None,
        events: crate::manifest::EventsDecl::default(),
        host_functions: Vec::new(),
        min_allowed_version: None,
        signature: None,
        publisher: None,
    }
}

fn summary_of(e: &PluginEntry) -> PluginSummary {
    PluginSummary {
        id: e.id.as_str().to_string(),
        name: e.manifest.name.clone(),
        version: e.manifest.version.to_string(),
        state: e.state.state,
        plugin_type: e.manifest.plugin_type.to_string(),
        published_topics: e
            .manifest
            .events
            .publish
            .iter()
            .filter(|p| p.public)
            .map(|p| p.topic.clone())
            .collect(),
        disabled_by_safemode: e.state.disabled_by_safemode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Contributes, EntrySpec, EventsDecl, Permission, PermissionEntry, PluginType, Risk};
    use std::sync::{Arc, atomic::AtomicUsize};

    fn index() -> PermissionIndex {
        PermissionIndex {
            version: 1,
            generated_at: None,
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
            ],
        }
    }

    fn manifest(id: &str, topic: Option<&str>) -> PluginManifest {
        let mut ev = EventsDecl::default();
        if let Some(t) = topic {
            ev.publish.push(crate::manifest::EventDecl {
                topic: t.into(),
                public: true,
            });
        }
        PluginManifest {
            id: PluginId::new(id).unwrap(),
            name: format!("plugin {id}"),
            version: semver::Version::new(1, 0, 0),
            plugin_type: PluginType::Js,
            entry: EntrySpec {
                js: Some("dist/index.js".into()),
                ..Default::default()
            },
            permissions: vec![Permission::new("store:allow-get")],
            scopes: serde_json::Map::new(),
            platforms: Vec::new(),
            framework: crate::manifest::parse_version_range(">=2.0 <3.0").unwrap(),
            abi: None,
            contributes: Contributes::default(),
            settings_schema: None,
            events: ev,
            host_functions: Vec::new(),
            min_allowed_version: None,
            signature: None,
            publisher: None,
        }
    }

    fn enable(r: &Registry, id: &PluginId) {
        r.admin_op(id, crate::authz::RegistryAdminOp::Enable).unwrap();
    }

    #[test]
    fn install_then_find() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        assert_eq!(r.find(&id).unwrap().manifest.name, "plugin com.example.a");
        assert_eq!(r.len(), 1);
        assert_eq!(r.find(&id).unwrap().state.state, State::Installed);
    }

    #[test]
    fn duplicate_install_fails() {
        let r = Registry::default();
        r.install(&index(), manifest("com.example.a", None)).unwrap();
        let e = r.install(&index(), manifest("com.example.a", None)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_PLUGIN_EXISTS);
    }

    #[test]
    fn registry_full_is_hard_failure() {
        let cfg = RegistryConfig {
            max_plugins: 2,
            ..default_config()
        };
        let r = Registry::new(cfg);
        r.install(&index(), manifest("com.example.a", None)).unwrap();
        r.install(&index(), manifest("com.example.b", None)).unwrap();
        let e = r.install(&index(), manifest("com.example.c", None)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_REGISTRY_FULL);
        assert!(e.message.contains("硬失败"));
    }

    #[test]
    fn unknown_plugin_is_structured_error() {
        let r = Registry::default();
        let id = PluginId::new("com.example.nope").unwrap();
        let e = r.require(&id).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_UNKNOWN_PLUGIN);
    }

    #[test]
    fn install_failure_lands_in_install_failed() {
        // D3：安装期失败落 INSTALL_FAILED，不自动重试。
        let r = Registry::default();
        let mut m = manifest("com.example.bad", None);
        m.permissions = vec![Permission::new("fs:allow-delete")]; // 表外权限
        let e = r.install(&index(), m).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("INSTALL_FAILED"));

        let entry = r.find(&PluginId::new("com.example.bad").unwrap()).unwrap();
        assert_eq!(entry.state.state, State::InstallFailed);

        // 不自动重试：Enable 在 INSTALL_FAILED 上非法。
        let o = r.report_event(&PluginId::new("com.example.bad").unwrap(), Event::Enable);
        assert!(o.is_ok());
        let o = o.unwrap();
        assert!(o.illegal);
        assert_eq!(r.find(&PluginId::new("com.example.bad").unwrap()).unwrap().state.state, State::InstallFailed);
    }

    #[test]
    fn record_install_failure_creates_entry() {
        let r = Registry::default();
        let id = PluginId::new("com.example.parsedfail").unwrap();
        r.record_install_failure(id.clone(), "hash mismatch".into()).unwrap();
        assert_eq!(r.find(&id).unwrap().state.state, State::InstallFailed);
        assert_eq!(
            r.find(&id).unwrap().state.last_reason.as_deref(),
            Some("hash mismatch")
        );
    }

    #[test]
    fn lifecycle_report_ignores_claimed_id_and_rejects_spoof() {
        let r = Registry::default();
        r.install(&index(), manifest("com.example.a", None)).unwrap();
        r.install(&index(), manifest("com.example.b", None)).unwrap();

        // label 指向 a、声称 a → 通过。
        let o = r
            .lifecycle_report("plugin-com.example.a", Some("com.example.a"), Event::Enable)
            .unwrap();
        assert!(!o.illegal);
        assert_eq!(
            r.find(&PluginId::new("com.example.a").unwrap()).unwrap().state.state,
            State::Enabled
        );

        // label 指向 a、声称 b → 硬拒（伪造 plugin_id）。
        let e = r
            .lifecycle_report("plugin-com.example.a", Some("com.example.b"), Event::Enable)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
        assert!(e.message.contains("com.example.b"));
    }

    #[test]
    fn non_plugin_label_is_rejected() {
        let r = Registry::default();
        let e = r.lifecycle_report("main", None, Event::Enable).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn unknown_plugin_lifecycle_report_errors() {
        let r = Registry::default();
        let e = r
            .lifecycle_report("plugin-com.example.ghost", None, Event::Enable)
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::E_UNKNOWN_PLUGIN);
    }

    #[test]
    fn admin_ops_drive_state_machine() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();

        let o = r.admin_op(&id, crate::authz::RegistryAdminOp::Enable).unwrap();
        assert_eq!(o.to, State::Enabled);
        let o = r.admin_op(&id, crate::authz::RegistryAdminOp::Disable).unwrap();
        assert_eq!(o.to, State::Disabled);
        let o = r.admin_op(&id, crate::authz::RegistryAdminOp::Uninstall).unwrap();
        assert_eq!(o.to, State::Uninstalled);
        assert!(r.find(&id).is_none(), "卸载后条目必须移除");
    }

    #[test]
    fn enable_after_safemode_disable_reports_clean_summary() {
        // 回归测试（用户可达路径）：安全模式禁用 → 用户 `admin_op(Enable)`。
        // Enable 必须把 `disabled_by_safemode` 一起清掉，否则 `PluginSummary`
        // 在插件已 Enabled 时仍报告 safemode 禁用，插件管理 UI 从此失真，
        // 且恢复对账的 `SafemodeExit` 只认 DISABLED 态、无法自愈。
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        r.report_event(&id, Event::SafemodeEnter).unwrap();
        assert!(r.find(&id).unwrap().state.disabled_by_safemode);

        r.admin_op(&id, crate::authz::RegistryAdminOp::Enable).unwrap();
        let s = r.find(&id).unwrap();
        assert_eq!(s.state.state, State::Enabled);
        assert!(
            !s.state.disabled_by_safemode,
            "显式启用后不得仍报告 safemode 禁用"
        );
    }

    #[test]
    fn purge_removes_entry_and_pending() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        r.call_begin(&id, "cmd", serde_json::json!({})).unwrap();
        assert_eq!(r.pending_len(), 1);
        r.admin_op(&id, crate::authz::RegistryAdminOp::Purge).unwrap();
        assert_eq!(r.pending_len(), 0, "purge 必须清空该插件的 pending");
        assert!(r.find(&id).is_none());
    }

    #[test]
    fn illegal_admin_op_counts_but_never_panics() {
        // DISABLED 上没有 Enable→? 其实有；用 INSTALL_FAILED 上的非法事件。
        let r = Registry::default();
        let id = PluginId::new("com.example.f").unwrap();
        r.record_install_failure(id.clone(), "x".into()).unwrap();
        let o = r.report_event(&id, Event::Enable).unwrap();
        assert!(o.illegal);
    }

    // ── pending call 表 ─────────────────────────────────────────

    #[test]
    fn pending_seq_is_monotonic() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let mut seqs = Vec::new();
        for _ in 0..5 {
            seqs.push(r.call_begin(&id, "c", serde_json::json!({})).unwrap().seq);
        }
        let mut sorted = seqs.clone();
        sorted.sort();
        assert_eq!(seqs, sorted, "seq 必须单调递增（§2.1）");
        assert_eq!(seqs[0], 1);
        assert_eq!(seqs.last().copied(), Some(5));
    }

    #[test]
    fn pending_full_is_hard_failure() {
        let cfg = RegistryConfig {
            max_pending_calls: 2,
            ..default_config()
        };
        let r = Registry::new(cfg);
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        r.call_begin(&id, "b", serde_json::json!({})).unwrap();
        let e = r.call_begin(&id, "c", serde_json::json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_PENDING_FULL);
    }

    #[test]
    fn pending_gc_on_pressure_unwedges_full_table() {
        // 僵尸条目（前端已超时却从未 callEnd）不得把容量位永久占住：
        // 否则一次永久挂起累积起来会让后续所有调用永久 E_CALL_PENDING_FULL。
        let cfg = RegistryConfig {
            max_pending_calls: 2,
            pending_ttl: Duration::from_nanos(1),
            ..default_config()
        };
        let r = Registry::new(cfg);
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        r.call_begin(&id, "b", serde_json::json!({})).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        // 两条均已过期 → 表满时先 GC 再判容量，新调用必须成功。
        let c = r.call_begin(&id, "c", serde_json::json!({})).unwrap();
        assert_eq!(c.cmd, "c");
        assert_eq!(r.pending_len(), 1, "过期条目必须被回收");
    }

    #[test]
    fn pending_gc_does_not_evict_in_flight_calls() {
        // 反向约束：GC 只能清过期条目，不能赶走合法在途调用。
        let cfg = RegistryConfig {
            max_pending_calls: 1,
            ..default_config()
        };
        let r = Registry::new(cfg);
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let in_flight = r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        assert_eq!(r.gc_expired(), 0, "未过期条目不得被回收");
        let e = r.call_begin(&id, "b", serde_json::json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_PENDING_FULL);
        assert_eq!(r.pending_len(), 1, "在途条目必须仍在");
        let _ = in_flight.call_id;
    }

    #[test]
    fn pending_gc_reclaims_expired_and_reports_timeout() {
        let cfg = RegistryConfig {
            pending_ttl: Duration::from_millis(1),
            ..default_config()
        };
        let r = Registry::new(cfg);
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let call = r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        assert_eq!(r.pending_len(), 1);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(r.gc_expired(), 1, "TTL GC 必须回收过期条目（计划 §4.1）");
        assert_eq!(r.pending_len(), 0);
        // 超时查询返回 E_CALL_TIMEOUT（可重试）。
        let call2 = r.call_begin(&id, "b", serde_json::json!({})).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let e = r.call_status(&call2.call_id, Instant::now()).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_CALL_TIMEOUT);
        assert!(e.retryable);
        // 不超时前查询成功。
        let call3 = r.call_begin(&id, "c", serde_json::json!({})).unwrap();
        let c = r.call_status(&call3.call_id, Instant::now()).unwrap();
        assert_eq!(c.cmd, "c");
        let _ = call.call_id;
    }

    #[test]
    fn call_end_removes_entry() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let c = r.call_begin(&id, "a", serde_json::json!({"k": 1})).unwrap();
        let back = r.call_end(&c.call_id).unwrap();
        assert_eq!(back.cmd, "a");
        assert_eq!(back.args, serde_json::json!({"k": 1}));
        assert_eq!(r.pending_len(), 0);
        assert_eq!(r.call_end(&c.call_id).unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
        assert_eq!(r.call_cancel(&c.call_id).unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    // ────────────────────────────────────────────────────────────
    // R5 / P0-1：流式帧与调用生命周期的联动
    // ────────────────────────────────────────────────────────────

    /// 本地记录型载体（与 `stream.rs` 的同名测试件等价，但走完整 Registry 面）。
    #[derive(Default)]
    struct RecSink {
        frames: std::sync::Mutex<Vec<StreamFrame>>,
    }

    impl StreamSink for RecSink {
        fn send(&self, frame: &StreamFrame) -> HostResult<()> {
            self.frames.lock().unwrap().push(frame.clone());
            Ok(())
        }
    }

    fn rec() -> Arc<RecSink> {
        Arc::new(RecSink::default())
    }

    #[test]
    fn call_end_sweeps_bound_stream_and_delivers_terminal_frame() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let sink = rec();
        let call = r.call_begin(&id, "a", serde_json::json!({})).unwrap();

        r.stream_bind(&call.call_id, "com.example.a", sink.clone())
            .unwrap();
        assert!(r.stream_call_bound(&call.call_id));
        let stream = r.stream_open(&call.call_id, "com.example.a").unwrap();
        assert_eq!(
            r.stream_write(&stream, "com.example.a", Some(serde_json::json!({"i":1})), None)
                .unwrap()
                .seq,
            1
        );

        // 调用结束（handler 没关流）→ 必须补终帧，否则接收方永远等下去。
        r.call_end(&call.call_id).unwrap();
        let frames = sink.frames.lock().unwrap().clone();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].kind, StreamKind::End);
        assert_eq!(frames[1].seq, 2);
        assert!(!r.stream_call_bound(&call.call_id), "载体登记随调用回收");
        assert!(!r.stream_is_open(&stream), "句柄随调用失效");
        assert_eq!(r.stream_len(), 0, "句柄表不留残留");
    }

    #[test]
    fn stream_bind_rejects_foreign_subscriber_and_unknown_call() {
        let r = Registry::default();
        let a = r.install(&index(), manifest("com.example.a", None)).unwrap();
        let b = r.install(&index(), manifest("com.example.b", None)).unwrap();
        enable(&r, &a);
        enable(&r, &b);
        let call = r.call_begin(&a, "a", serde_json::json!({})).unwrap();

        // 换载体 = 偷走后续所有帧：知道 callId 的别的插件不得挂载。
        let err = r
            .stream_bind(&call.call_id, "com.example.b", rec())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(!r.stream_call_bound(&call.call_id), "被拒的挂载不得留下载体");

        // 归属者可以挂载；不存在的调用不行。
        r.stream_bind(&call.call_id, "com.example.a", rec()).unwrap();
        assert!(r.stream_call_bound(&call.call_id));
        let err = r.stream_bind("c-nope", "com.example.a", rec()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn call_cancel_closes_stream_with_reason() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let sink = rec();
        let call = r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        r.stream_bind(&call.call_id, "com.example.a", sink.clone())
            .unwrap();
        let stream = r.stream_open(&call.call_id, "com.example.a").unwrap();

        r.call_cancel(&call.call_id).unwrap();
        let frames = sink.frames.lock().unwrap().clone();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].kind, StreamKind::Error, "取消不是正常结束");
        assert_eq!(frames[0].args_json.clone().unwrap()["reason"], "canceled");
        assert!(!r.stream_is_open(&stream));
    }

    #[test]
    fn gc_leaves_unexpired_streams_alone() {
        // 判别性：GC 只能动过期条目。若它「顺手」扫掉活流，这条会红。
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let sink = rec();
        let call = r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        r.stream_bind(&call.call_id, "com.example.a", sink.clone())
            .unwrap();
        let stream = r.stream_open(&call.call_id, "com.example.a").unwrap();

        assert_eq!(r.gc_expired(), 0);
        assert!(r.stream_is_open(&stream), "未过期的流不得被 GC 终结");
        assert!(sink.frames.lock().unwrap().is_empty(), "GC 不得凭空补帧");
        assert_eq!(r.stream_write(&stream, "com.example.a", None, None).unwrap().seq, 1);
    }

    #[test]
    fn gc_expired_delivers_timeout_terminal_frame() {
        let cfg = RegistryConfig {
            pending_ttl: std::time::Duration::from_millis(1),
            ..RegistryConfig::default()
        };
        let r = Registry::new(cfg);
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);
        let sink = rec();
        let call = r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        r.stream_bind(&call.call_id, "com.example.a", sink.clone())
            .unwrap();
        let stream = r.stream_open(&call.call_id, "com.example.a").unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(r.gc_expired(), 1);
        let frames = sink.frames.lock().unwrap().clone();
        assert_eq!(frames.len(), 1, "GC 必须补一帧，且只补一帧");
        assert_eq!(frames[0].kind, StreamKind::Error);
        assert_eq!(frames[0].args_json.clone().unwrap()["reason"], "call_timeout");
        assert!(!r.stream_is_open(&stream));
    }

    #[test]
    fn window_close_clears_pending() {
        // ADR-04：窗口关闭必须清理 pending，否则 JS 侧 invoke() 永久挂起。
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        let other = r.install(&index(), manifest("com.example.b", None)).unwrap();
        enable(&r, &id);
        enable(&r, &other);
        for _ in 0..3 {
            r.call_begin(&id, "a", serde_json::json!({})).unwrap();
        }
        r.call_begin(&other, "b", serde_json::json!({})).unwrap();
        assert_eq!(r.pending_len(), 4);
        assert_eq!(r.call_end_all(&id), 3, "只清该插件");
        assert_eq!(r.pending_len(), 1, "其他插件的 pending 保留");
    }

    #[test]
    fn disabled_plugin_cannot_begin_call() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        // INSTALLED（非活跃）→ ADR-05 即时拒绝。
        let e = r.call_begin(&id, "a", serde_json::json!({})).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_PLUGIN_DISABLED);
        // 启用后可用；空命令仍被拒（ADR-05 与命令校验是两个独立关卡）。
        enable(&r, &id);
        assert!(r.call_begin(&id, "a", serde_json::json!({})).is_ok());
        assert_eq!(
            r.call_begin(&id, "", serde_json::json!({})).unwrap_err().code,
            ErrorCode::E_AUTH_DENIED
        );
    }

    // ── 可见性过滤（§4.21 测试点）───────────────────────────────

    #[test]
    fn list_visible_filters_by_subscriptions() {
        let r = Registry::default();
        r.install(&index(), manifest("com.example.a", Some("t1"))).unwrap();
        r.install(&index(), manifest("com.example.b", None)).unwrap();
        r.install(&index(), manifest("com.example.c", Some("t2"))).unwrap();

        let caller = PluginId::new("com.example.a").unwrap();
        let list = r.list_visible(Some(&caller), &["t2".to_string()]);
        let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
        assert!(ids.contains(&"com.example.a"), "必须包含自己");
        assert!(ids.contains(&"com.example.c"), "订阅了对方 public topic 即可见");
        assert!(!ids.contains(&"com.example.b"), "未订阅其 topic 的插件不可见");

        // 空订阅 → 仅自己。
        let list = r.list_visible(Some(&caller), &[]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "com.example.a");
        // 无调用者 → 空。
        assert!(r.list_visible(None, &[]).is_empty());
    }

    #[test]
    fn list_visible_ignores_private_topics() {
        let r = Registry::default();
        let mut m = manifest("com.example.pvt", Some("t9"));
        m.events.publish[0].public = false;
        r.install(&index(), m).unwrap();
        let caller = PluginId::new("com.example.a").unwrap();
        let list = r.list_visible(Some(&caller), &["t9".to_string()]);
        assert!(list.iter().all(|p| p.id != "com.example.pvt"), "private topic 不可见");
    }

    #[test]
    fn list_all_is_unfiltered() {
        let r = Registry::default();
        r.install(&index(), manifest("com.example.a", None)).unwrap();
        r.install(&index(), manifest("com.example.b", None)).unwrap();
        assert_eq!(r.list_all().len(), 2);
    }

    // ── 身份绑定 ────────────────────────────────────────────────

    #[test]
    fn bind_identity_sets_label_and_token() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        let ident = r.bind_identity(&id).unwrap();
        assert_eq!(ident.id, id);
        assert_eq!(ident.webview_label, "plugin-com.example.a");
        assert!(ident.token >= 1);
        let e = r.bind_identity(&PluginId::new("com.example.x").unwrap()).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_UNKNOWN_PLUGIN);
    }

    // ── 活跃身份 LRU ─────────────────────────────────────────────

    #[test]
    fn active_identity_lru_evicts_oldest() {
        let cfg = RegistryConfig {
            max_active_identities: 2,
            max_plugins: 8,
            ..default_config()
        };
        let r = Registry::new(cfg);
        let a = r.install(&index(), manifest("com.example.a", None)).unwrap();
        let b = r.install(&index(), manifest("com.example.b", None)).unwrap();
        let c = r.install(&index(), manifest("com.example.c", None)).unwrap();
        enable(&r, &a);
        enable(&r, &b);
        assert_eq!(r.active_ids().len(), 2, "INSTALLED 不占活跃槽");
        enable(&r, &c);
        assert!(
            r.active_ids().len() <= 2,
            "活跃身份不得超过上限：{:#?}",
            r.active_ids()
        );
        // a 最旧 → 被淘汰为 DISABLED。
        assert_eq!(r.find(&a).unwrap().state.state, State::Disabled);
        assert_eq!(r.find(&b).unwrap().state.state, State::Enabled);
        assert_eq!(r.find(&c).unwrap().state.state, State::Enabled);
    }

    #[test]
    fn lru_refreshes_on_new_event() {
        let cfg = RegistryConfig {
            max_active_identities: 2,
            max_plugins: 8,
            ..default_config()
        };
        let r = Registry::new(cfg);
        let a = r.install(&index(), manifest("com.example.a", None)).unwrap();
        let b = r.install(&index(), manifest("com.example.b", None)).unwrap();
        enable(&r, &a);
        enable(&r, &b);
        // 刷新 a（Attach/HealthOk 均使状态变化或保持在活跃集）。
        r.report_event(&a, Event::HealthOk).unwrap();
        let c = r.install(&index(), manifest("com.example.c", None)).unwrap();
        enable(&r, &c);
        // b 最旧 → 被淘汰；a 因刚被访问而保留。
        assert_eq!(r.find(&a).unwrap().state.state, State::Enabled);
        assert_eq!(r.find(&b).unwrap().state.state, State::Disabled);
    }

    #[test]
    fn uninstall_removes_from_active_order() {
        let r = Registry::default();
        let a = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &a);
        assert!(!r.active_ids().is_empty());
        r.admin_op(&a, crate::authz::RegistryAdminOp::Uninstall).unwrap();
        assert!(r.active_ids().iter().all(|x| x != &a));
    }

    // ── 状态单写者 ───────────────────────────────────────────────

    #[test]
    fn state_changes_only_via_registry_api() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        assert_eq!(r.find(&id).unwrap().state.state, State::Installed);
        r.report_event(&id, Event::Enable).unwrap();
        assert_eq!(r.find(&id).unwrap().state.state, State::Enabled);
    }

    #[test]
    fn len_and_empty_track_entries() {
        let r = Registry::default();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        r.install(&index(), manifest("com.example.a", None)).unwrap();
        assert_eq!(r.len(), 1);
        assert!(!r.is_empty());
    }

    // ── 100 并发调用（计划 §4.1 测试要求）────────────────────────

    #[test]
    fn hundred_concurrent_invokes_are_consistent() {
        let r = Arc::new(Registry::default());
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);

        const N: usize = 100;
        let mut handles = Vec::with_capacity(N);
        let ok = Arc::new(AtomicUsize::new(0));
        for _ in 0..N {
            let reg = r.clone();
            let id = id.clone();
            let ok = ok.clone();
            handles.push(std::thread::spawn(move || {
                match reg.call_begin(&id, "cmd", serde_json::json!({"n": 1})) {
                    Ok(c) => {
                        ok.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let _ = reg.call_end(&c.call_id);
                    }
                    Err(e) => panic!("并发 call_begin 失败：{e}"),
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(ok.load(std::sync::atomic::Ordering::Relaxed), N);
        assert_eq!(r.pending_len(), 0, "所有 pending 必须已回收");
        assert_eq!(r.gc_expired(), 0);
    }

    #[test]
    fn concurrent_begin_and_gc_do_not_deadlock() {
        let cfg = RegistryConfig {
            pending_ttl: Duration::from_millis(50),
            ..default_config()
        };
        let r = Arc::new(Registry::new(cfg));
        let id = r.install(&index(), manifest("com.example.a", None)).unwrap();
        enable(&r, &id);

        let mut handles = Vec::new();
        for _ in 0..8 {
            let reg = r.clone();
            let id = id.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..25 {
                    if let Ok(c) = reg.call_begin(&id, "c", serde_json::json!({})) {
                        let _ = reg.call_end(&c.call_id);
                    }
                    let _ = reg.gc_expired();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(r.pending_len() <= 200);
    }

    // ── 序列化 / 配置 ───────────────────────────────────────────

    #[test]
    fn summaries_serialize_for_js() {
        let r = Registry::default();
        r.install(&index(), manifest("com.example.a", Some("t1"))).unwrap();
        let list = r.list_all();
        let s = serde_json::to_string(&list).unwrap();
        assert!(s.contains("com.example.a"));
        assert!(s.contains("INSTALLED"));
        assert!(s.contains("t1"));
        let back: Vec<PluginSummary> = serde_json::from_str(&s).unwrap();
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn registry_config_defaults() {
        let c = default_config();
        assert_eq!(c.max_plugins, 8);
        assert_eq!(c.max_active_identities, 8);
        assert_eq!(c.max_pending_calls, 1000);
        assert_eq!(c.pending_ttl, Duration::from_secs(30));
    }

    #[test]
    fn install_failure_does_not_leak_active_slot() {
        let cfg = RegistryConfig {
            max_active_identities: 2,
            ..default_config()
        };
        let r = Registry::new(cfg);
        let mut m = manifest("com.example.bad", None);
        m.permissions = vec![Permission::new("nope:allow")];
        assert!(r.install(&index(), m).is_err());
        assert!(r.active_ids().is_empty(), "安装失败不占活跃槽");
    }

    // ────────────────────────────────────────────────────────────
    // 配置化插件选择加载（PluginFilter）
    // ────────────────────────────────────────────────────────────

    #[test]
    fn no_filter_loads_all_plugins() {
        let r = Registry::new(default_config());
        let m = manifest("com.example.a", None);
        assert!(r.install(&index(), m).is_ok());
        assert!(r.find(&PluginId::new("com.example.a").unwrap()).is_some());
    }

    #[test]
    fn allow_filter_loads_only_listed_plugins() {
        let cfg = RegistryConfig {
            plugin_filter: Some(PluginFilter {
                allow: vec!["com.example.a".into()],
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // 允许的插件可以安装
        let m = manifest("com.example.a", None);
        assert!(r.install(&index(), m).is_ok());
        // 未列出的插件被过滤
        let m2 = manifest("com.example.b", None);
        let err = r.install(&index(), m2).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
    }

    #[test]
    fn deny_filter_excludes_listed_plugins() {
        let cfg = RegistryConfig {
            plugin_filter: Some(PluginFilter {
                deny: vec!["com.example.bad".into()],
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // 未禁止的插件可以安装
        let m = manifest("com.example.ok", None);
        assert!(r.install(&index(), m).is_ok());
        // 被禁止的插件被过滤
        let m2 = manifest("com.example.bad", None);
        let err = r.install(&index(), m2).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
    }

    #[test]
    fn allow_and_deny_combined() {
        let cfg = RegistryConfig {
            plugin_filter: Some(PluginFilter {
                allow: vec!["com.example.a".into(), "com.example.b".into()],
                deny: vec!["com.example.a".into()],
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // allow 中被 deny 排除的插件被过滤
        let m = manifest("com.example.a", None);
        let err = r.install(&index(), m).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
        // allow 中未被 deny 排除的插件可以安装
        let m2 = manifest("com.example.b", None);
        assert!(r.install(&index(), m2).is_ok());
    }

    #[test]
    fn type_filter_rejects_wrong_type() {
        let cfg = RegistryConfig {
            plugin_filter: Some(PluginFilter {
                types: vec!["js".into()],
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // JS 插件可以安装
        let m = manifest("com.example.js", None);
        assert!(r.install(&index(), m).is_ok());
        // Rust 插件被过滤
        let mut m2 = manifest("com.example.rust", None);
        m2.plugin_type = PluginType::Rust;
        let err = r.install(&index(), m2).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
    }

    #[test]
    fn platform_filter_rejects_wrong_platform() {
        let cfg = RegistryConfig {
            plugin_filter: Some(PluginFilter {
                platforms: vec!["win".into()],
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // 声明 win 平台的插件可以安装
        let mut m = manifest("com.example.win", None);
        m.platforms = vec!["win".into()];
        assert!(r.install(&index(), m).is_ok());
        // 未声明 win 平台的插件被过滤
        let mut m2 = manifest("com.example.linux", None);
        m2.platforms = vec!["linux".into()];
        let err = r.install(&index(), m2).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
        // 未声明平台（空）的插件也被过滤
        let m3 = manifest("com.example.any", None);
        let err2 = r.install(&index(), m3).unwrap_err();
        assert_eq!(err2.code, ErrorCode::E_PLUGIN_FILTERED);
    }

    #[test]
    fn builtins_excluded_when_include_builtins_false() {
        let cfg = RegistryConfig {
            plugin_filter: Some(PluginFilter {
                include_builtins: false,
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // 内置插件（oc. 前缀）被过滤
        let m = manifest("oc.core", None);
        let err = r.install(&index(), m).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
        // 非内置插件可以安装
        let m2 = manifest("com.example.plugin", None);
        assert!(r.install(&index(), m2).is_ok());
    }

    #[test]
    fn builtins_included_by_default() {
        let r = Registry::new(default_config());
        // 默认加载内置插件
        let m = manifest("oc.core", None);
        assert!(r.install(&index(), m).is_ok());
    }

    #[test]
    fn filtered_plugin_does_not_occupy_capacity() {
        let cfg = RegistryConfig {
            max_plugins: 1,
            plugin_filter: Some(PluginFilter {
                deny: vec!["com.example.denied".into()],
                ..Default::default()
            }),
            ..default_config()
        };
        let r = Registry::new(cfg);
        // 被过滤的插件不占容量
        let m1 = manifest("com.example.denied", None);
        assert!(r.install(&index(), m1).is_err());
        // 容量仍可用
        let m2 = manifest("com.example.ok", None);
        assert!(r.install(&index(), m2).is_ok());
    }

    #[test]
    fn filter_validate_rejects_overlapping_allow_deny() {
        let filter = PluginFilter {
            allow: vec!["com.example.a".into()],
            deny: vec!["com.example.a".into()],
            ..Default::default()
        };
        let err = filter.validate().unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn filter_validate_accepts_disjoint_allow_deny() {
        let filter = PluginFilter {
            allow: vec!["com.example.a".into(), "com.example.b".into()],
            deny: vec!["com.example.c".into()],
            ..Default::default()
        };
        assert!(filter.validate().is_ok());
    }

    #[test]
    fn filter_matches_empty_manifest_platforms_with_empty_filter() {
        // 空平台过滤 = 所有平台通过
        let filter = PluginFilter::default();
        let m = manifest("com.example.a", None);
        assert!(filter.matches(&m));
    }

    #[test]
    fn filter_matches_manifest_with_platforms_against_specific_platform() {
        let filter = PluginFilter {
            platforms: vec!["win".into(), "mac".into()],
            ..Default::default()
        };
        let mut m = manifest("com.example.a", None);
        m.platforms = vec!["win".into()];
        assert!(filter.matches(&m));
    }

    #[test]
    fn filter_matches_manifest_platform_intersection() {
        let filter = PluginFilter {
            platforms: vec!["win".into(), "mac".into()],
            ..Default::default()
        };
        let mut m = manifest("com.example.a", None);
        m.platforms = vec!["linux".into()];
        assert!(!filter.matches(&m));
    }

    #[test]
    fn plugin_filter_is_serializable() {
        let filter = PluginFilter {
            allow: vec!["com.example.a".into()],
            deny: vec!["com.example.b".into()],
            types: vec!["js".into()],
            platforms: vec!["win".into()],
            include_builtins: false,
        };
        let json = serde_json::to_string(&filter).unwrap();
        let back: PluginFilter = serde_json::from_str(&json).unwrap();
        assert_eq!(filter.allow, back.allow);
        assert_eq!(filter.deny, back.deny);
        assert_eq!(filter.types, back.types);
        assert_eq!(filter.platforms, back.platforms);
        assert_eq!(filter.include_builtins, back.include_builtins);
    }

    // ── 运行时租约表（P0-2）─────────────────────────────────────

    #[test]
    fn runtime_ensure_lease_is_idempotent_and_starts_at_most_once() {
        use std::cell::Cell;
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();

        let starts = Cell::new(0);
        let (first, started_first) = r
            .runtime_ensure_lease(&id, || {
                starts.set(starts.get() + 1);
                Ok(4242)
            })
            .unwrap();
        assert_eq!(first.pid, 4242);
        assert_eq!(starts.get(), 1);
        assert!(started_first, "首次必须如实报「本次真的起了新进程」（调用方据此推 RUNNING）");

        // 第二次：必须**原样返回既有 lease**，且绝不再次启动进程。
        let (second, started_second) = r
            .runtime_ensure_lease(&id, || {
                starts.set(starts.get() + 1);
                Ok(9999)
            })
            .unwrap();
        assert_eq!(second, first, "重复 spawn 必须返回既有 lease");
        assert_eq!(starts.get(), 1, "重复 spawn 不得再启动进程");
        assert!(
            !started_second,
            "幂等返回必须报 `started = false`：调用方据此不投重复的 ATTACH"
        );
        assert_eq!(r.runtime_len(), 1, "同一插件至多一条租约");
    }

    #[test]
    fn runtime_ensure_lease_propagates_start_failure_without_lease() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let err = r
            .runtime_ensure_lease(&id, || {
                Err(HostError::new(ErrorCode::E_INSTALL_FAILED, "启动失败"))
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED);
        assert_eq!(r.runtime_len(), 0, "启动失败绝不得留下租约");
        assert!(r.runtime_handle_of(&id).is_none());
    }

    #[test]
    fn unknown_lease_is_expired_not_call_not_found() {
        let r = Registry::default();
        let err = r.runtime_lease("no-such-lease").unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::E_LEASE_EXPIRED,
            "租约边界必须报 E_LEASE_EXPIRED（不是 E_CALL_NOT_FOUND）"
        );
        assert_eq!(r.runtime_mark_crashed("no-such-lease").unwrap_err().code, ErrorCode::E_LEASE_EXPIRED);
    }

    #[test]
    fn runtime_crash_is_counted_once_per_lease() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let (h, _started) = r.runtime_ensure_lease(&id, || Ok(77)).unwrap();
        assert!(r.runtime_mark_crashed(&h.lease).unwrap(), "首次必须是 true");
        assert!(!r.runtime_mark_crashed(&h.lease).unwrap(), "重复轮询不得再计数");
        assert_eq!(r.runtime_lease(&h.lease).unwrap().pid, 77);
        assert!(r.runtime_lease(&h.lease).unwrap().crash_recorded);
    }

    #[test]
    fn purge_recycles_runtime_lease() {
        let r = Registry::default();
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let (h, _started) = r.runtime_ensure_lease(&id, || Ok(5)).unwrap();
        r.admin_op(&id, crate::authz::RegistryAdminOp::Purge).unwrap();
        assert_eq!(r.runtime_len(), 0, "卸载必须回收租约（否则是孤儿 PID）");
        assert_eq!(
            r.runtime_lease(&h.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED
        );
    }

    /// 崩溃后的租约**不算"已有"**：重试必须真的能起新进程（否则重试预算形同虚设），
    /// 且换新时旧 pid 必须先被终止。
    #[test]
    fn crashed_lease_is_replaced_on_retry_and_old_pid_is_terminated() {
        #[derive(Default)]
        struct Recorder {
            killed: parking_lot::Mutex<Vec<u32>>,
        }
        impl crate::runtime::LeaseReaper for Recorder {
            fn kill(&self, pid: u32) -> Result<crate::runtime::ReapOutcome, String> {
                self.killed.lock().push(pid);
                Ok(crate::runtime::ReapOutcome::AlreadyGone)
            }
        }
        let recorder = Arc::new(Recorder::default());
        let r = Registry::default();
        r.set_lease_reaper(recorder.clone());
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();

        let (first, started) = r.runtime_ensure_lease(&id, || Ok(100)).unwrap();
        assert!(started);
        assert!(r.runtime_mark_crashed(&first.lease).unwrap());
        assert!(r.runtime_needs_restart(&id), "崩溃租约必须视为需要重启");

        let (second, started_retry) = r.runtime_ensure_lease(&id, || Ok(200)).unwrap();
        assert_eq!(second.pid, 200, "重试必须起新进程");
        assert!(started_retry, "换新租约也是「真的起了新进程」");
        assert_ne!(second.lease, first.lease, "新进程必须是新租约");
        assert_eq!(recorder.killed.lock().clone(), vec![100], "旧 pid 必须先终止");
        assert_eq!(r.runtime_len(), 1);
        assert!(!r.runtime_needs_restart(&id));
        assert_eq!(
            r.runtime_lease(&first.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED,
            "旧租约必须失效"
        );
    }

    /// 卸载路径必须**真的调终止器**（只删表项 = 孤儿进程），且失败要留痕、
    /// 但不得让卸载失败。
    #[test]
    fn uninstall_terminates_pid_and_traces_kill_failure() {
        struct Failing;
        impl crate::runtime::LeaseReaper for Failing {
            fn kill(&self, _pid: u32) -> Result<crate::runtime::ReapOutcome, String> {
                Err("权限不足".into())
            }
        }
        let r = Registry::default();
        r.set_lease_reaper(Arc::new(Failing));
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let (h, _started) = r.runtime_ensure_lease(&id, || Ok(31)).unwrap();

        // 卸载必须成功：终止失败是"进程可能还在"，不是"插件没卸载掉"。
        r.admin_op(&id, crate::authz::RegistryAdminOp::Uninstall).unwrap();
        assert_eq!(r.runtime_len(), 0, "租约必须消失");
        assert!(r.runtime_handle_of(&id).is_none());
        assert_eq!(
            r.runtime_lease(&h.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED
        );
        let s = r.runtime_reap_stats();
        assert_eq!((s.attempts, s.failures), (1, 1), "终止失败必须留痕");
        assert!(s.last_error.as_deref().unwrap_or("").contains("权限不足"));
    }

    /// 租约回收的**唯一**执行点：任何"离开可用态"的迁移都必须终止仍在跑的进程。
    ///
    /// "不可用即无进程"是一条规则，不是逐个命令补的补丁——这里挑**最容易被漏掉**的
    /// 离场路径（安全模式对账用的 `SafemodeEnter`，它不经过任何 `admin_op`）来验证：
    /// 只要状态不再是 ENABLED/RUNNING，活租约就被终止。
    #[test]
    fn any_transition_out_of_active_reclaims_a_live_lease() {
        #[derive(Default)]
        struct Recorder {
            killed: parking_lot::Mutex<Vec<u32>>,
        }
        impl crate::runtime::LeaseReaper for Recorder {
            fn kill(&self, pid: u32) -> Result<crate::runtime::ReapOutcome, String> {
                self.killed.lock().push(pid);
                Ok(crate::runtime::ReapOutcome::Terminated)
            }
        }
        let recorder = Arc::new(Recorder::default());
        let r = Registry::default();
        r.set_lease_reaper(recorder.clone());
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let (h, started) = r.runtime_ensure_lease(&id, || Ok(4242)).unwrap();
        assert!(started);
        // 进程插件的常态：启用 → 宿主投 ATTACH（真起了进程）→ RUNNING。
        r.report_event(&id, Event::Enable).unwrap();
        r.report_event(&id, Event::Attach).unwrap();
        assert!(!r.runtime_needs_restart(&id), "活租约 = 不需要重启");
        assert!(recorder.killed.lock().is_empty(), "还在可用状态时不得终止");

        let out = r.report_event(&id, Event::SafemodeEnter).unwrap();
        assert!(!out.illegal);
        assert_eq!(
            recorder.killed.lock().clone(),
            vec![h.pid],
            "状态离开可用态 → 必须终止仍在跑的 sidecar"
        );
        assert_eq!(r.runtime_len(), 0, "租约必须一起回收");
        assert!(
            r.runtime_handle_of(&id).is_none(),
            "回收后不得留下指向已死进程的租约"
        );
    }

    /// 对照：**崩溃**这条离场路径不得回收租约——进程已经死了，租约要留给
    /// `host_runtime_health` 把这次崩溃报出去（在那里摘掉，调用方只会拿到
    /// `E_LEASE_EXPIRED`，崩溃事实连同计数一起消失）。
    #[test]
    fn a_crashed_lease_survives_the_transition_so_health_can_report_it() {
        #[derive(Default)]
        struct Recorder {
            killed: parking_lot::Mutex<Vec<u32>>,
        }
        impl crate::runtime::LeaseReaper for Recorder {
            fn kill(&self, pid: u32) -> Result<crate::runtime::ReapOutcome, String> {
                self.killed.lock().push(pid);
                Ok(crate::runtime::ReapOutcome::Terminated)
            }
        }
        let recorder = Arc::new(Recorder::default());
        let r = Registry::default();
        r.set_lease_reaper(recorder.clone());
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let (h, _) = r.runtime_ensure_lease(&id, || Ok(77)).unwrap();
        r.report_event(&id, Event::Enable).unwrap();
        r.report_event(&id, Event::Attach).unwrap();

        assert!(r.runtime_mark_crashed(&h.lease).unwrap(), "首次观测到死亡");
        let out = r.report_event(&id, Event::RuntimeCrash).unwrap();
        assert!(!out.illegal, "RUNNING + RUNTIME_CRASH 有规则");

        assert_eq!(r.runtime_len(), 1, "崩溃租约必须留着给 health 报账");
        assert_eq!(r.runtime_lease(&h.lease).unwrap().pid, 77);
        assert!(
            r.runtime_needs_restart(&id),
            "崩溃租约 = 需要（重新）启动"
        );
        assert!(
            recorder.killed.lock().is_empty(),
            "进程已死，不得在崩溃路径上再调一次终止（那是无谓的杀空 pid）"
        );
    }

    /// 终止器按租约里的 pid 精确调用一次（pid 张冠李戴 = 杀错进程）。
    #[test]
    fn terminate_uses_the_pid_bound_to_the_lease() {
        #[derive(Default)]
        struct Recorder {
            killed: parking_lot::Mutex<Vec<u32>>,
        }
        impl crate::runtime::LeaseReaper for Recorder {
            fn kill(&self, pid: u32) -> Result<crate::runtime::ReapOutcome, String> {
                self.killed.lock().push(pid);
                Ok(crate::runtime::ReapOutcome::Terminated)
            }
        }
        let recorder = Arc::new(Recorder::default());
        let r = Registry::default();
        r.set_lease_reaper(recorder.clone());
        let id = r.install(&index(), manifest("com.example.proc", None)).unwrap();
        let (h, _started) = r.runtime_ensure_lease(&id, || Ok(8080)).unwrap();
        let removed = r.runtime_remove(&id);
        assert_eq!(removed, Some(h));
        assert_eq!(recorder.killed.lock().clone(), vec![8080]);
        assert_eq!(r.runtime_reap_stats().terminated, 1, "真杀必须计入 terminated");
    }
}
