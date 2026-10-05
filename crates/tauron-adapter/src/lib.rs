// Tauri 命令适配层（§4.1 适配器）。
//
// 关键约束（ADR-04）：每个入口 `catch_unwind` → `E_HOST_PANIC`，
// 结构化错误，从不让 panic 抵达 JS。
//
// 本模块**不**依赖 `tauri` crate：命令逻辑保持平台无关、可离线测试。
// `#[tauri::command]` 注册在 feature-gated 的 `tauri.rs` 中（`tauri` feature）。
//
// 集成方式（第三方 Tauri 应用）：
// ```toml
// [dependencies]
// tauron-adapter = { path = "...", features = ["tauri"] }
// ```
// ```rust,ignore
// tauri::Builder::default()
//     .plugin(tauron_adapter::tauri::init())
//     .run(tauri::generate_context!())
// ```

#[cfg(feature = "tauri")]
pub mod tauri;

/// 进程插件投递实现（0.4-A1：Process 形态的 `CallDelivery` + sidecar 回帧接收器）。
pub mod process_delivery;

/// WASM broker is optional in V4 minimal profile. Without it, Wasm delivery is
/// intentionally absent and select_delivery() returns the canonical unwired result.
#[cfg(feature = "runtime-wasm-broker")]
pub mod wasm_delivery;

/// 启动恢复的持久化与崩溃检测（平台无关，纯 std）。
mod recovery;

pub use recovery::{BootRecord, LoadSource, RecoveryStore};

/// 菜单点击回传的路由表（`host_menu_*` / `host_tray_*` 共用的唯一事实源）。
mod menu_routes;

pub use menu_routes::{menu_routes, menu_routes_of, MenuLane, MenuRouteTable, MENU_CLICK_TOPIC};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use tauron_host::{
    call_delivery::{select_delivery, CallDelivery, CallOutcome, DeliveryKind, JsCallDelivery},
    eventbus::{ChannelKind, EventBus, Frame, PublishResult, SubscribeOutcome},
    guard,
    lifecycle::{Event, State as LifecycleStateName, TransitionOutcome},
    manifest::{EventDecl, PluginId, PluginManifest, PluginType},
    registry::{PluginSummary, Registry, RegistryConfig},
    runtime::{LeaseReaper, ReapOutcome, ReapStats, RuntimeHandle},
    stream::{StreamFrame, StreamKind},
    PendingCall,
};
// R8 §2：装配器宏的**消费者**要在自己的函数签名里命名这些类型（返回 `HostResult`、
// 匹配 `ErrorCode`），而 `tauron_plugin_as_host_command!` 展开时引用的
// `$crate::tauri::HostResult` 只在 feature `tauri` 下存在。因此在 crate 根重新导出：
// 「写一条宿主形态命令」不必先自己依赖 `tauron-host`，也不必打开某个 feature。
pub use tauron_host::{ErrorCode, HostError, HostResult};
// 装配方需要它来调 `cmd_registry_admin`（启用/禁用/卸载/清除）。不导出的话
// 下游宿主只能自己 `tauron_host::authz::RegistryAdminOp`——即多一条隐式依赖。
pub use tauron_host::authz::RegistryAdminOp;
// 第三方集成用的客户端配置（`AdapterConfig::from_client_config` 的输入）。
// 不导出的话接入方得直接依赖 `tauron-host` 才能构造它。
pub use tauron_host::config::ClientConfig;
use tauron_i18n::{I18nEngine, ResourceBundle};
use tauron_notify::{dispatch, DispatchSink, NotifyEntry, NotifyKind, NotifyStore};
use tauron_proc::{
    current_abi_contract, validate_abi, validate_spawn_config,
    AbiFingerprint as ProcAbiFingerprint, BinarySignature, CrashLimit, CrashTracker, ProcError,
    ProcSpawner, SpawnConfig,
};
use tauron_recovery::{
    BootContextEntry, BootPhase, EffectRecord, PluginState as RecoveryPluginState, RecoveryAction,
    RecoveryEngine,
};
use tauron_settings::{
    Migration, MigrationContract, MigrationReceipt, SettingsError, SettingsStore,
};

use crate::process_delivery::ProcessCallDelivery;
#[cfg(feature = "runtime-wasm-broker")]
use crate::wasm_delivery::WasmCallDelivery;

/// 贡献注册条目（命令/菜单/面板/设置Tab）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributeEntry {
    pub plugin_id: String,
    pub kind: String,
    pub id: String,
    pub label: String,
}

/// 贡献表容量上限。
///
/// `host_contributes_register` 是 `self` 档命令：任何插件窗口都能反复调用。
/// 只按 `(plugin_id, id)` 去重是不够的——换个 `id` 就能再插一条，`Vec` 会无界
/// 增长（宿主内存被插件单方面拖垮）。到顶后**如实拒绝**，不静默丢弃。
pub const MAX_CONTRIBUTES: usize = 4096;

/// 贡献注册表：管理所有已注册贡献。
#[derive(Debug, Default)]
pub struct ContributesRegistry {
    entries: Vec<ContributeEntry>,
}

impl ContributesRegistry {
    /// 注册贡献。
    pub fn register(&mut self, entry: ContributeEntry) -> HostResult<()> {
        // 检查重复
        if self.entries.iter().any(|e| e.plugin_id == entry.plugin_id && e.id == entry.id) {
            return Err(tauron_host::HostError::new(
                ErrorCode::E_PLUGIN_EXISTS,
                format!("贡献 {} 已存在", entry.id),
            ));
        }
        // 容量闸（与 `Registry::try_put_entry` 同一口径：判定与插入同临界区，
        // 本方法已持 `&mut self`，天然满足）。
        if self.entries.len() >= MAX_CONTRIBUTES {
            return Err(tauron_host::HostError::new(
                ErrorCode::E_REGISTRY_FULL,
                format!("贡献表已达上限 {MAX_CONTRIBUTES}；先 `clear_plugin` 或卸载插件再注册"),
            ));
        }
        self.entries.push(entry);
        Ok(())
    }

    /// 获取所有贡献。
    pub fn list_all(&self) -> &[ContributeEntry] {
        &self.entries
    }

    /// 按类型获取贡献。
    ///
    /// 注：曾有一个 `list_by_plugin` 兄弟方法，全仓零调用（连测试都没有），已删除。
    /// 要按插件过滤，需要先让 `host_contributes_list` 的线格式支持插件维度——
    /// 那是协议变更，不是在这里加一个没人调的 getter 就能解决的。
    pub fn list_by_kind(&self, kind: &str) -> Vec<&ContributeEntry> {
        self.entries.iter().filter(|e| e.kind == kind).collect()
    }

    /// 清除插件的所有贡献。
    pub fn clear_plugin(&mut self, plugin_id: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.plugin_id != plugin_id);
        before - self.entries.len()
    }

    /// 贡献数量。
    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

/// V4 deployment posture re-export（canonical 定义在 `tauron-host::production`）。
/// Development/Test preserve the historical permissive integration defaults;
/// Production requires an explicit fail-closed readiness check.
pub use tauron_host::DeploymentMode;

impl AdapterConfig {
    /// Construct an explicitly production-scoped configuration. Callers must still
    /// provide the required security/durability settings before readiness validation.
    pub fn production() -> Self {
        Self { deployment_mode: DeploymentMode::Production, ..Self::default() }
    }

    /// Legacy V4 entry point kept for substrate-style callers; delegates to the
    /// canonical fail-closed [`Self::validate_for_start`] gate.
    pub fn validate_production_readiness(&self) -> Result<(), String> {
        self.validate_for_start().map_err(|e| e.message)
    }

    /// Select the deployment posture. Production activates fail-closed readiness gates.
    pub fn with_deployment_mode(mut self, mode: tauron_host::DeploymentMode) -> Self {
        self.deployment_mode = mode;
        self
    }

    /// Declare that a non-origin transport has a verified caller identity policy.
    pub fn with_caller_identity_policy(mut self, enabled: bool) -> Self {
        self.caller_identity_policy_enabled = enabled;
        self
    }

    /// Explicitly disable durable recovery instead of silently falling back to memory-only.
    pub fn with_recovery_unsupported(mut self, unsupported: bool) -> Self {
        self.recovery_explicitly_unsupported = unsupported;
        self
    }

    /// 配置特权操作审计事实的**落盘目录**（V4 轮 11 / F3）。
    ///
    /// 这里刻意不再提供 `with_admin_audit(bool)`：宿主自己写一个布尔位，正是
    /// 「声明了审计、实际什么都没记」的根因。就绪判定现在由装配出来的 sink
    /// （能落盘 + 哈希链完整 + 无写失败）推导，不再是开关位。
    pub fn with_admin_audit_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.admin_audit_dir = Some(dir.into());
        self
    }

    /// Declare which webview labels count as the main window (V4 轮 10 / F1).
    pub fn with_main_window_labels(mut self, labels: Vec<String>) -> Self {
        self.main_window_labels = labels;
        self
    }

    /// 装配用的主窗 label 集合：未显式配置时展开 Tauri 约定缺省（`["main"]`）。
    pub fn effective_main_window_labels(&self) -> Vec<String> {
        if self.main_window_labels.is_empty() {
            tauron_host::authz::default_main_window_labels()
        } else {
            self.main_window_labels.clone()
        }
    }

    /// Test-only/development provider declaration. Production rejects this flag.
    pub fn with_mock_provider(mut self, enabled: bool) -> Self {
        self.mock_provider_enabled = enabled;
        self
    }

    /// Derive the canonical V4 production-readiness facts from actual adapter configuration.
    pub fn production_readiness(&self) -> tauron_host::ProductionReadiness {
        #[cfg(feature = "plugin-install")]
        let install_trust_configured = self.plugin_install_dir.is_some()
            && !self.plugin_signing_keys.is_empty()
            && self.acl_signing_key.as_ref().is_some_and(|key| key.len() >= 32);
        #[cfg(not(feature = "plugin-install"))]
        let install_trust_configured = true;

        #[cfg(feature = "plugin-install")]
        let trusted_time_available = self.trusted_time_provider.as_ref().is_some_and(|provider| {
            provider.trusted_time().state == tauron_host::TimeTrustState::Trusted
        });
        #[cfg(not(feature = "plugin-install"))]
        let trusted_time_available = true;

        tauron_host::ProductionReadiness {
            caller_identity_policy_enabled: self.caller_identity_policy_enabled
                || !self.origin_allowlist.is_empty(),
            // 轮 10 / F2：`caller_identity_policy_enabled` 可被「非 origin 传输的显式
            // 声明」满足，但那并不给 origin 门装弹。Production 必须看到清单真的非空。
            origin_gate_armed: !self.origin_allowlist.is_empty(),
            durable_recovery_available: self.recovery_data_dir.is_some(),
            recovery_explicitly_unsupported: self.recovery_explicitly_unsupported,
            install_feature_enabled: cfg!(feature = "plugin-install"),
            install_trust_configured,
            trusted_time_available,
            audit_for_admin_operations_available: self.admin_audit_dir.is_some(),
            writable_data_dir_available: self.recovery_data_dir.is_some(),
            // AdapterConfig describes the substrate. The process runtime is attached later,
            // once the concrete ProcSpawner (and therefore its sandbox descriptor) is known.
            process_runtime_enabled: false,
            hard_process_sandbox_available: false,
            mock_provider_enabled: self.mock_provider_enabled,
        }
    }

    /// Fail-closed production startup validation. Development/Test compatibility is unchanged.
    pub fn validate_for_start(&self) -> HostResult<()> {
        // 轮 11 / F3：审计目录必须**真的打得开**。配置里写一个路径不等于有审计——
        // 否则 `ADMIN_AUDIT_REQUIRED` 就又被降级成「填了个字符串」。`open` 会建目录、
        // 读回既有日志并同时校验 durable 校验和与哈希链，撕裂/篡改在这里就拒启。
        if let Some(dir) = self.admin_audit_dir.as_ref() {
            if let Err(error) = tauron_host::AdminAuditSink::open(dir) {
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("admin audit sink rejected host startup: {error}"),
                ));
            }
        }
        let violations = tauron_host::validate_production_readiness(
            self.deployment_mode,
            &self.production_readiness(),
        );
        if violations.is_empty() {
            return Ok(());
        }
        let detail = violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.message))
            .collect::<Vec<_>>()
            .join("; ");
        Err(HostError::new(
            ErrorCode::E_STATE_INVALID_TRANSITION,
            format!("production readiness rejected host startup: {detail}"),
        ))
    }

    /// Re-run production readiness once the concrete process runtime is known.
    ///
    /// Substrate-only/headless hosts do not need a process sandbox. The moment a
    /// `ProcSpawner` is attached, Production requires that exact spawner to report A97
    /// `hard` enforcement. Development/Test keep compatibility behavior.
    fn validate_process_runtime_for_start(
        &self,
        descriptor: &tauron_proc::ProcessSandboxDescriptor,
    ) -> HostResult<()> {
        let mut readiness = self.production_readiness();
        readiness.process_runtime_enabled = true;
        readiness.hard_process_sandbox_available =
            matches!(descriptor.enforcement, tauron_proc::ProcessSandboxEnforcement::Hard);
        let violations =
            tauron_host::validate_production_readiness(self.deployment_mode, &readiness);
        if violations.is_empty() {
            return Ok(());
        }
        let detail = violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.message))
            .collect::<Vec<_>>()
            .join("; ");
        Err(HostError::new(
            ErrorCode::E_STATE_INVALID_TRANSITION,
            format!("production plugin-runtime readiness rejected startup: {detail}"),
        ))
    }

    /// 从第三方集成用的 [`ClientConfig`] 派生装配配置。
    ///
    /// **这是 `ClientConfig` 的生产消费点**。在此之前 `ClientConfig`
    /// （`tauron-host` 里 500 余行、文档称"第三方集成的唯一入口"）只被 `pub use`
    /// 再导出、从未被任何生产代码读过——配置写得再对也不生效，
    /// `plugin_filter` 这类"配置化选择加载"等于没实现。
    ///
    /// 映射关系（只映射**有落点**的字段，其余见 `config.rs` 的诚实边界说明）：
    /// - `registry` → [`AdapterConfig::registry`]（含 `plugin_filter` 容量与过滤）；
    /// - `data_dir` → [`AdapterConfig::recovery_data_dir`]（相对路径按当前目录解析；
    ///   解析不出来时回落到调用方给的 `fallback_data_dir`）；
    /// - `env_overrides` → [`AdapterConfig::plugin_env_overrides`]，最终落到
    ///   `tauron_proc::SpawnConfig::env`（每次 spawn 注入子进程）。
    ///
    /// **写了不生效的键不会在这里被静默丢掉**：`auto_update` /
    /// `update_check_interval_secs` / `crash_report_enabled` / `brand_id` /
    /// `plugin_paths` / `performance_monitoring` 至今没有宿主落点（原因逐条写在
    /// `tauron_host::config::CLIENT_CONFIG_LANDING`），由
    /// [`tauron_host::config::ClientConfig::unwired_fields`] 如实报出，宿主在启动
    /// 横幅里打印。装配层**不**为它们造布尔位——那会把"没实现"重新包装成"已生效"。
    ///
    /// `log_level` 不由本函数消费：日志初始化属于宿主进程的事（`tracing` 订阅者
    /// 在宿主侧装配），调用方可自行 `cfg.log_level()` 取用。
    pub fn from_client_config(cfg: &ClientConfig, fallback_data_dir: Option<PathBuf>) -> Self {
        let recovery_data_dir = cfg
            .data_dir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or(fallback_data_dir);

        Self {
            deployment_mode: tauron_host::DeploymentMode::Development,
            registry: Some(cfg.registry_config()),
            caller_identity_policy_enabled: false,
            recovery_explicitly_unsupported: false,
            admin_audit_dir: None,
            mock_provider_enabled: false,
            recovery_data_dir,
            required_plugins: HashSet::new(),
            origin_allowlist: Vec::new(),
            main_window_labels: Vec::new(),
            // `ClientConfig` 没有 fs 根目录字段（它管注册表/数据目录）：由宿主用
            // [`AdapterConfig::with_fs_roots`] 显式配置；缺省 = 该域不可用（如实）。
            fs_allowed_roots: Vec::new(),
            http_policy: tauron_host::NetworkPolicy::default(),
            http_sink: None,
            plugin_env_overrides: cfg.env_overrides.clone().unwrap_or_default(),
            #[cfg(feature = "plugin-install")]
            plugin_install_dir: None,
            #[cfg(feature = "plugin-install")]
            plugin_signing_keys: std::collections::BTreeMap::new(),
            #[cfg(feature = "plugin-install")]
            acl_signing_key: None,
            #[cfg(feature = "plugin-install")]
            trusted_time_provider: None,
        }
    }

    /// Configure signed package installation with host-owned trust material.
    #[cfg(feature = "plugin-install")]
    pub fn with_plugin_install(
        mut self,
        root: PathBuf,
        signing_keys: std::collections::BTreeMap<String, Vec<u8>>,
        acl_signing_key: Vec<u8>,
    ) -> Self {
        self.plugin_install_dir = Some(root);
        self.plugin_signing_keys = signing_keys;
        self.acl_signing_key = Some(acl_signing_key);
        self
    }

    /// A100: inject the host-owned trusted-time source used by supply-chain expiry decisions.
    #[cfg(feature = "plugin-install")]
    pub fn with_trusted_time_provider(
        mut self,
        provider: Arc<dyn tauron_host::TrustedTimeProvider>,
    ) -> Self {
        self.trusted_time_provider = Some(provider);
        self
    }

    /// 配置 `host_fs_*` 域的允许根目录（空 = 该域不可用）。
    ///
    /// 装配期会逐个 `canonicalize`；不存在的根被忽略（见
    /// [`AdapterConfig::fs_allowed_roots`] 的语义说明）。
    pub fn with_fs_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.fs_allowed_roots = roots;
        self
    }

    /// Configure the V4 HTTP/network scope. Default is deny-all.
    pub fn with_http_policy(mut self, policy: tauron_host::NetworkPolicy) -> Self {
        self.http_policy = policy;
        self
    }

    /// 注入 HTTP provider（轮 48 / A96）：生产宿主装配期调用一次。
    pub fn with_http_sink(mut self, sink: Arc<dyn HttpSink>) -> Self {
        self.http_sink = Some(sink);
        self
    }
}

/// 适配器装配配置（宿主启动时传入一次）。
///
/// 全部字段都有默认值：`AdapterConfig::default()` = 恢复持久化关闭 + 空必需
/// 集合，即 `CommandState::new` 的既有行为（单元测试不需要磁盘）。
#[derive(Debug, Clone, Default)]
pub struct AdapterConfig {
    /// Deployment posture. Development is the backwards-compatible default for tests/local use.
    pub deployment_mode: tauron_host::DeploymentMode,
    /// A custom/non-origin transport can assert only after it installs verified caller identity.
    pub caller_identity_policy_enabled: bool,
    /// Production may explicitly declare recovery unsupported instead of pretending it is durable.
    pub recovery_explicitly_unsupported: bool,
    /// Privileged admin operations must be auditable in production.
    ///
    /// `None` = 没有审计 sink（Production 因此 not-ready）。见
    /// [`AdapterConfig::with_admin_audit_dir`] 与 `tauron_host::admin_audit`。
    pub admin_audit_dir: Option<PathBuf>,
    /// Production forbids mock/test providers.
    pub mock_provider_enabled: bool,
    /// 注册表配置（上限、TTL、加载过滤器）。`None` = [`RegistryConfig::default()`]。
    pub registry: Option<RegistryConfig>,
    /// 宿主数据目录：恢复标记（崩溃检测）落盘位置。
    ///
    /// 生产宿主应传 Tauri 的 `app.path().app_config_dir()`。为 `None` 时恢复
    /// 引擎只有内存态：进程一退计数器即失，安全模式永不触发。
    pub recovery_data_dir: Option<PathBuf>,
    /// 安全模式/修复模式下仍必须加载的插件 id（引擎的必需集合）。
    ///
    /// 这里是必需性的**权威来源**，每次启动都会用它整体替换引擎里的集合。
    pub required_plugins: HashSet<String>,
    /// **origin 允许清单（R4-D2）**：允许调用 `host_*` 命令的 webview origin。
    ///
    /// 语义（见 [`tauron_host::authz::origin_allowed`]）：
    /// - **空清单 = 不启用**（默认）→ 一律放行，兼容既有装配；
    /// - 非空 = **fail-closed**：宿主侧从 `Invoke` 取真实 webview origin，逐字命中
    ///   清单项才放行；取不到 origin 的调用方一律拒绝。
    ///
    /// 条目应为规范化形式 `scheme://host[:port]`（如 `http://127.0.0.1:63896`）；
    /// 比较前会去掉首尾空白与尾随 `/`。这是多宿主/混淆代理场景的防线：把非官方
    /// origin 的窗口挡在特权命令之外。
    pub origin_allowlist: Vec<String>,
    /// **可信主窗 label 集合（V4 轮 10 / F1）**。
    ///
    /// `tauron_host::authz::resolve_principal` 把任何非 `plugin-` 前缀的 label 判成
    /// 主窗——那是 label 形状判定，不是策略。Development/Test 沿用该兼容语义；
    /// **Production** 下只有落在此集合内的 label 才算主窗，其余（次级窗、自造
    /// label）在 dispatch 前被拒。
    ///
    /// 空 = 使用 Tauri 约定缺省 `tauron_host::authz::DEFAULT_MAIN_WINDOW_LABELS`
    /// （`["main"]`），装配时展开一次。
    pub main_window_labels: Vec<String>,
    /// **宿主文件系统允许根目录（`host_fs_*` 域）**。
    ///
    /// 语义（与 `origin_allowlist` 的 fail-closed 同精神，但更严）：
    /// - **空 = 该域整体不可用**——每条 `host_fs_*` 命令返回
    ///   [`UnsupportedBody`]（`supported:false`），**不是**"放行任意路径"；
    /// - 非空 = 只允许访问这些根目录**之内**的路径：调用方给的路径先 `canonicalize`
    ///   （解析符号链接、消除 `..`），再校验 `starts_with` 命中某个根。任一根目录
    ///   本身 canonicalize 失败（不存在）时该根被**忽略**（不因为配错一个不存在的
    ///   根而放开全盘，也不整体拒绝启动）。
    ///
    /// 装配方（生产宿主）应传宿主自己的数据/导出目录；测试传 tempdir。
    pub fs_allowed_roots: Vec<PathBuf>,
    /// V4 A96 network scope. Empty/default is fail-closed even when a custom HTTP sink exists.
    pub http_policy: tauron_host::NetworkPolicy,
    /// **HTTP provider 注入点（轮 48 / A96）**：接入方把自己的 [`HttpSink`] 实现
    /// 放进来即启用该域；`None` = 缺省 [`UnavailableHttpSink`]（如实 Unsupported）。
    pub http_sink: Option<Arc<dyn HttpSink>>,
    /// **运维注入的 sidecar 环境变量**（`ClientConfig.env_overrides` 的落点）。
    ///
    /// 每次 `host_runtime_spawn` 都会把这些键合进 [`tauron_proc::SpawnConfig::env`]，
    /// 因此它们真的到达子进程（`CommandSpawner` 的 `.envs()`）。同名键上**本字段优先**：
    /// 运维配置不能被调用方 `profile.env` 悄悄撤掉（见 [`apply_host_env_overrides`]）。
    ///
    /// 空 = 不注入（既有行为）。这是 `ClientConfig` 里少数**有落点**的非注册表键之一，
    /// 其余键的落点见 `tauron_host::config::CLIENT_CONFIG_LANDING`。
    pub plugin_env_overrides: std::collections::HashMap<String, String>,
    /// Package installation root. Installation remains unavailable when unset.
    #[cfg(feature = "plugin-install")]
    pub plugin_install_dir: Option<PathBuf>,
    /// Explicit trusted signer keys, keyed by the sidecar `kid`.
    #[cfg(feature = "plugin-install")]
    pub plugin_signing_keys: std::collections::BTreeMap<String, Vec<u8>>,
    /// Host-provided ACL HMAC key. Never generated from a public constant.
    #[cfg(feature = "plugin-install")]
    pub acl_signing_key: Option<Vec<u8>>,
    /// A100 host-owned trusted-time source. Production install requires a currently Trusted value.
    #[cfg(feature = "plugin-install")]
    pub trusted_time_provider: Option<Arc<dyn tauron_host::TrustedTimeProvider>>,
}

/// Result for a committed signed plugin installation.
#[cfg(feature = "plugin-install")]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallResult {
    pub plugin_id: String,
    pub version: String,
    pub install_path: String,
    pub approved_permissions: Vec<String>,
}

#[cfg(feature = "plugin-install")]
const PLUGIN_UI_ACTIVATION_FILE: &str = ".tauron-ui-activation.json";

#[cfg(feature = "plugin-install")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SignedPluginActivation {
    records: Vec<tauron_host::ActivationRecord>,
    hmac_sha256: String,
}

#[cfg(feature = "plugin-install")]
fn plugin_asset_activation_resource(manifest: &PluginManifest, relative: &str) -> String {
    format!("plugin:{}@{}:asset:{}", manifest.id, manifest.version, relative)
}

#[cfg(feature = "plugin-install")]
fn collect_plugin_activation_records(
    plugin_dir: &std::path::Path,
    manifest: &PluginManifest,
) -> HostResult<Vec<tauron_host::ActivationRecord>> {
    let mut files = Vec::<(String, std::path::PathBuf)>::new();
    let mut dirs = vec![plugin_dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("枚举插件激活目录失败 {}: {error}", dir.display()),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("读取插件激活目录项失败：{error}"),
                )
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("读取插件激活文件元数据失败 {}: {error}", path.display()),
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("插件激活内容包含符号链接，拒绝激活：{}", path.display()),
                ));
            }
            if metadata.is_dir() {
                dirs.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("插件激活内容不是普通文件：{}", path.display()),
                ));
            }
            let relative = path.strip_prefix(plugin_dir).map_err(|error| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("插件激活路径无法归一化：{error}"),
                )
            })?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative == PLUGIN_UI_ACTIVATION_FILE {
                continue;
            }
            files.push((relative, path));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut records = Vec::with_capacity(files.len());
    for (relative, path) in files {
        // 轮 44（A94）：摘要在流上算——激活腿此前把每个文件整读进内存，
        // 安装流内存会随包内最大文件线性增长（RSS/堆门禁正是抓这个）。
        let file = std::fs::File::open(&path).map_err(|error| {
            HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("读取插件激活内容失败 {}: {error}", path.display()),
            )
        })?;
        let content = tauron_host::ContentIdentity::from_reader(file).map_err(|error| {
            HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("读取插件激活内容失败 {}: {error}", path.display()),
            )
        })?;
        records.push(tauron_host::ActivationRecord {
            resource: plugin_asset_activation_resource(manifest, &relative),
            generation: tauron_host::Generation::INITIAL,
            content,
        });
    }
    Ok(records)
}

#[cfg(feature = "plugin-install")]
fn write_plugin_ui_activation(
    plugin_dir: &std::path::Path,
    manifest: &PluginManifest,
    key: &[u8],
) -> HostResult<()> {
    if manifest.entry.ui.is_none() {
        return Ok(());
    }
    let records = collect_plugin_activation_records(plugin_dir, manifest)?;
    if records.is_empty() {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "插件 activation record 为空；拒绝提交安装",
        ));
    }
    let record_bytes = serde_json::to_vec(&records).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("序列化插件 activation records 失败：{error}"),
        )
    })?;
    let signed = SignedPluginActivation {
        hmac_sha256: tauron_acl::hmac_sha256_hex(&record_bytes, key)?,
        records,
    };
    let encoded = serde_json::to_vec_pretty(&signed).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("序列化插件 activation metadata 失败：{error}"),
        )
    })?;
    let path = plugin_dir.join(PLUGIN_UI_ACTIVATION_FILE);
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(|error| {
            HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("创建插件 activation metadata 失败 {}: {error}", path.display()),
            )
        })?;
    use std::io::Write as _;
    file.write_all(&encoded).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("写入插件 activation metadata 失败：{error}"),
        )
    })?;
    file.sync_all().map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("同步插件 activation metadata 失败：{error}"),
        )
    })
}

/// 取出并**认证**插件目录里密封的 activation 记录集。
///
/// 这是密封记录集的**唯一**取出入口：文件形态（普通文件、非符号链接）、HMAC 与
/// generation 三道判定都在这里。加载入口页与逐资产服务因此共用同一份"什么叫可信
/// 记录"的判定，不存在第二条更松的解析路径。
#[cfg(feature = "plugin-install")]
fn load_sealed_activation(
    plugin_dir: &std::path::Path,
    acl_signing_key: Option<&[u8]>,
) -> HostResult<Vec<tauron_host::ActivationRecord>> {
    let key = acl_signing_key.filter(|key| key.len() >= 32).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "缺少可验证插件 activation metadata 的宿主 HMAC 密钥",
        )
    })?;
    let path = plugin_dir.join(PLUGIN_UI_ACTIVATION_FILE);
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("插件缺少 activation metadata {}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "插件 activation metadata 不是安全的普通文件",
        ));
    }
    let encoded = std::fs::read(&path).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("读取插件 activation metadata 失败：{error}"),
        )
    })?;
    let signed: SignedPluginActivation = serde_json::from_slice(&encoded).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("解析插件 activation metadata 失败：{error}"),
        )
    })?;
    let record_bytes = serde_json::to_vec(&signed.records).map_err(|error| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            format!("序列化插件 activation records 失败：{error}"),
        )
    })?;
    if !tauron_acl::hmac_sha256_matches(&record_bytes, &signed.hmac_sha256, key)? {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "插件 activation metadata HMAC 不匹配；拒绝加载可能被篡改的内容",
        ));
    }
    if signed.records.iter().any(|record| record.generation != tauron_host::Generation::INITIAL) {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "插件 activation generation 与已安装版本不匹配",
        ));
    }
    Ok(signed.records)
}

/// 加载入口页前的全目录激活复核：密封记录集必须与**当前**磁盘文件集逐条相符。
///
/// 与 [`PluginAssetTrust::verify_asset`] 的差别只在范围：这里防的是"整目录被换掉/
/// 多出文件"，每次 GET 的逐资产复核防的是"这一条字节被改过"。两道都要有。
#[cfg(feature = "plugin-install")]
fn verify_plugin_ui_activation(
    config: &InstallRuntimeConfig,
    manifest: &PluginManifest,
    plugin_dir: &std::path::Path,
) -> HostResult<()> {
    let sealed = load_sealed_activation(plugin_dir, config.acl_signing_key.as_deref())?;
    let current = collect_plugin_activation_records(plugin_dir, manifest)?;
    if current != sealed {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "插件 activation integrity 校验失败：安装后的文件集合或内容已变化",
        ));
    }
    Ok(())
}

/// A84：插件 asset **读侧**的信任根（安装根目录 + 宿主 ACL 密钥）。
///
/// 装配方必须在**注册 URI scheme 之前**构造它：拿不到 ≥32 字节宿主密钥就没有可信的
/// activation 记录，正确处置是**不注册** asset 协议，而不是注册一个"只校验路径、
/// 不校验摘要"的读侧。此前的缺口正在于此：入口页在 [`installed_plugin_ui`] 里做过
/// 全目录摘要复核，入口页加载后浏览器逐个 GET 的 `src/*.js`、`*.css`、图片走的却是
/// asset 协议，那一条字节都没复核过——安装完成后篡改任意资产，宿主照原样服务端出。
#[cfg(feature = "plugin-install")]
pub struct PluginAssetTrust {
    root: PathBuf,
    acl_signing_key: Vec<u8>,
}

/// 宿主 HMAC 密钥**不进** Debug 输出：装配日志与 panic 打印都不该泄露它。
#[cfg(feature = "plugin-install")]
impl std::fmt::Debug for PluginAssetTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginAssetTrust")
            .field("root", &self.root)
            .field(
                "acl_signing_key",
                &format_args!("<{} bytes redacted>", self.acl_signing_key.len()),
            )
            .finish()
    }
}

#[cfg(feature = "plugin-install")]
impl PluginAssetTrust {
    /// 用与安装侧**同一个**宿主 ACL 密钥建立读侧信任根。
    pub fn new(root: impl Into<PathBuf>, acl_signing_key: Vec<u8>) -> HostResult<Self> {
        if acl_signing_key.len() < 32 {
            return Err(HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                "插件 asset 读侧需要 ≥32 字节的宿主 HMAC 密钥（与安装侧 TAURON_ACL_SIGNING_KEY 同源）",
            ));
        }
        Ok(Self { root: root.into(), acl_signing_key })
    }

    /// URI scheme 挂载的只读根目录。
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 服务端出的每个字节都必须先过这里：请求路径要在**已认证的**密封记录里有一条，
    /// 且内容与该记录逐字节相符。无记录（安装后新增/注入的文件）与摘要不符同样拒绝。
    pub fn verify_asset(&self, plugin_id: &str, relative: &str, bytes: &[u8]) -> HostResult<()> {
        let id = tauron_host::manifest::PluginId::new(plugin_id)?;
        let install_root = self.root.canonicalize().map_err(|error| {
            HostError::new(ErrorCode::E_INSTALL_FAILED, format!("插件安装根目录不可用：{error}"))
        })?;
        let plugin_dir = install_root.join(id.as_str()).canonicalize().map_err(|error| {
            HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("插件目录不可用 `{plugin_id}`：{error}"),
            )
        })?;
        if !plugin_dir.starts_with(&install_root) {
            return Err(HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                "插件目录符号链接越出安装根目录",
            ));
        }
        let owner = format!("plugin:{}@", id.as_str());
        let record = load_sealed_activation(&plugin_dir, Some(self.acl_signing_key.as_slice()))?
            .into_iter()
            .find(|record| {
                record.resource.starts_with(&owner)
                    && activation_asset_path(&record.resource) == Some(relative)
            })
            .ok_or_else(|| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!(
                        "插件 asset `{plugin_id}/{relative}` 没有密封的 activation 记录；\
                         拒绝服务未经摘要复核的内容"
                    ),
                )
            })?;
        record.verify_bytes(bytes).map_err(|error| {
            HostError::new(
                ErrorCode::E_INSTALL_FAILED,
                format!("插件 asset integrity 校验失败 `{plugin_id}/{relative}`：{error}"),
            )
        })
    }
}

/// 从 `plugin:{id}@{version}:asset:{relative}` 取出资产相对路径。
///
/// 用 `rsplit_once` 而非 `split_once`：相对路径本身允许出现 `:asset:` 这段字符，
/// 从右切分才不会被路径里的它骗掉。
#[cfg(feature = "plugin-install")]
fn activation_asset_path(resource: &str) -> Option<&str> {
    let (_, relative) = resource.rsplit_once(":asset:")?;
    (!relative.is_empty()).then_some(relative)
}

/// Validated filesystem location for an installed JS plugin's entry page.
#[cfg(feature = "plugin-install")]
#[derive(Debug, Clone)]
pub struct InstalledPluginUi {
    pub plugin_id: String,
    pub entry: PathBuf,
}

/// Resolve an enabled plugin's UI entry while constraining every path beneath the install root.
#[cfg(feature = "plugin-install")]
pub fn installed_plugin_ui(
    state: &PluginRuntimeState,
    plugin_id: &str,
) -> HostResult<InstalledPluginUi> {
    let config = state.install_config.as_ref().ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "plugin-install 未配置安装目录（拒绝加载插件页面）",
        )
    })?;
    let id = tauron_host::manifest::PluginId::new(plugin_id)?;
    let registered = state.registry.find(&id).ok_or_else(|| {
        HostError::new(ErrorCode::E_UNKNOWN_PLUGIN, format!("插件 `{plugin_id}` 不存在"))
    })?;
    if !matches!(
        registered.state.state,
        tauron_host::lifecycle::State::Enabled | tauron_host::lifecycle::State::Running
    ) {
        return Err(HostError::new(
            ErrorCode::E_PLUGIN_DISABLED,
            format!("插件 `{plugin_id}` 未启用"),
        ));
    }
    let relative =
        registered.manifest.entry.ui.as_deref().ok_or_else(|| {
            HostError::new(ErrorCode::E_INVALID_MANIFEST, "JS 插件未声明 entry.ui")
        })?;
    let relative = PathBuf::from(relative);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "插件 UI 路径非法"));
    }
    let install_root = config.root.canonicalize().map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("插件安装根目录不可用：{e}"))
    })?;
    let root = install_root
        .join(plugin_id)
        .canonicalize()
        .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, format!("插件目录不可用：{e}")))?;
    if !root.starts_with(&install_root) {
        return Err(HostError::new(ErrorCode::E_INSTALL_FAILED, "插件目录符号链接越出安装根目录"));
    }
    let entry = root.join(relative).canonicalize().map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("插件 UI 文件不可用：{e}"))
    })?;
    if !entry.starts_with(&root) || !entry.is_file() {
        return Err(HostError::new(
            ErrorCode::E_INSTALL_FAILED,
            "插件 UI 文件越出安装目录或不是普通文件",
        ));
    }
    verify_plugin_ui_activation(config, &registered.manifest, &root)?;
    Ok(InstalledPluginUi { plugin_id: plugin_id.to_string(), entry })
}

/// 由已安装的 UI 绝对路径反推插件窗口的资产相对路径（`window_create` 的取径侧）。
///
/// 为什么要在**比较点**再 canonicalize 一次：`entry` 出自 [`installed_plugin_ui`]，
/// 一路都是 canonical 的；而装配期存进状态的根**可能**还是集成方原样传入的串——
/// 首次安装时目录还不存在，那边的 canonicalize 只能按原样保留。两个串不同形时
/// `strip_prefix` 必然失配（`..` 段、符号链接、Windows 的 `\\?\` 长前缀与 8.3 短名
/// 都算；纯 `./` 段不算——`Path` 按组件比较，会忽略它），
/// 表现是「装得上、打不开窗口」。canonicalize 失败（目录又被删了）就退回原值：
/// 那样与 `entry` 不同形，照走 `E_INSTALL_FAILED` 拒绝，**不会**放行越出安装根的路径。
#[cfg(feature = "plugin-install")]
#[cfg_attr(not(feature = "tauri"), allow(dead_code))] // 唯一生产调用点在 tauri 侧的 `window_create`
pub(crate) fn plugin_window_asset_relative(
    install_root: &std::path::Path,
    plugin_id: &str,
    entry: &std::path::Path,
) -> HostResult<String> {
    let root = install_root.canonicalize().unwrap_or_else(|_| install_root.to_path_buf());
    let relative = entry.strip_prefix(root.as_path()).map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("插件 UI 越出安装目录：{e}"))
    })?;
    let relative = relative.strip_prefix(plugin_id).unwrap_or(relative);
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(feature = "plugin-install")]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPermissionReview {
    pub permission: String,
    pub risk: String,
    pub description: String,
    pub default_checked: bool,
    /// scope 的可读形式（无 scope 的权限缺席）。由 `tauron_acl::build_approval_rows` 给出。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// 高危行的确认词（**文案唯一来源是 acl 审批构造器**，前端不得另写一份）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_hint: Option<String>,
}

#[cfg(feature = "plugin-install")]
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallPreview {
    pub plugin_id: String,
    pub plugin_name: String,
    pub version: String,
    pub permissions: Vec<PluginPermissionReview>,
    /// One-time cryptographic binding between what the user reviewed and what commit installs.
    pub review_token: InstallReviewToken,
}

/// V4 install approval token. It is one-time, bounded and bound to verified package facts.
#[cfg(feature = "plugin-install")]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallReviewToken {
    pub package_digest: String,
    pub manifest_digest: String,
    pub permission_digest: String,
    pub key_id: String,
    pub publisher_id: Option<String>,
    pub plugin_id: String,
    pub version: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: String,
}

#[cfg(feature = "plugin-install")]
const INSTALL_REVIEW_TTL_SECS: u64 = 10 * 60;
#[cfg(feature = "plugin-install")]
const MAX_INSTALL_REVIEWS: usize = 64;

#[cfg(feature = "plugin-install")]
struct VerifiedPluginPackage {
    manifest: PluginManifest,
    /// Open handle to the exact archive bytes that were verified. Commit extracts from this
    /// handle, not by reopening the path, closing the verify→extract TOCTOU window.
    archive: std::fs::File,
    package_digest: String,
    manifest_digest: String,
    permission_digest: String,
    key_id: String,
    publisher_id: Option<String>,
}

#[cfg(feature = "plugin-install")]
fn digest_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

#[cfg(feature = "plugin-install")]
fn digest_file_and_rewind(file: &mut std::fs::File) -> HostResult<String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom};

    file.seek(SeekFrom::Start(0))
        .map_err(|e| HostError::new(ErrorCode::E_INSTALL_FAILED, format!("定位安装包失败：{e}")))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| {
            HostError::new(ErrorCode::E_INSTALL_FAILED, format!("读取安装包失败：{e}"))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| {
        HostError::new(ErrorCode::E_INSTALL_FAILED, format!("重置安装包位置失败：{e}"))
    })?;
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(feature = "plugin-install")]
fn permission_digest(manifest: &PluginManifest) -> String {
    let mut permissions: Vec<&str> = manifest.permissions.iter().map(|p| p.as_str()).collect();
    permissions.sort_unstable();
    digest_bytes(permissions.join("\n").as_bytes())
}

#[cfg(feature = "plugin-install")]
fn mint_install_review(
    state: &PluginRuntimeState,
    verified: &VerifiedPluginPackage,
) -> HostResult<InstallReviewToken> {
    run_review_boundary(state, "install_review_mint", || {
        // 轮 43：TTL 的锚点从墙钟改为 A100 可信时间（`review_now` 生产档失败关闭）。
        let issued_at = review_now(state)?;
        Ok(InstallReviewToken {
            package_digest: verified.package_digest.clone(),
            manifest_digest: verified.manifest_digest.clone(),
            permission_digest: verified.permission_digest.clone(),
            key_id: verified.key_id.clone(),
            publisher_id: verified.publisher_id.clone(),
            plugin_id: verified.manifest.id.to_string(),
            version: verified.manifest.version.to_string(),
            issued_at,
            expires_at: issued_at.saturating_add(INSTALL_REVIEW_TTL_SECS),
            nonce: uuid::Uuid::new_v4().to_string(),
        })
    })
}

#[cfg(feature = "plugin-install")]
fn review_matches_verified(token: &InstallReviewToken, verified: &VerifiedPluginPackage) -> bool {
    token.package_digest == verified.package_digest
        && token.manifest_digest == verified.manifest_digest
        && token.permission_digest == verified.permission_digest
        && token.key_id == verified.key_id
        && token.publisher_id == verified.publisher_id
        && token.plugin_id == verified.manifest.id.as_str()
        && token.version == verified.manifest.version.to_string()
}

// ──────────────────────────────────────────────────────────────────────────
// A83（轮 43）：破坏性管理操作（uninstall/purge）的审批令牌
//
// 与安装域 [`InstallReviewToken`] **同构**的第二个消费者：预览铸发一次性令牌、
// commit 消费并重核事实、TTL 与容量都有界。差别只在绑定对象——安装域绑包摘要，
// 这里绑「注册表条目事实」（id + 安装版本 + 被审阅的操作）。update 域没有独立
// 执行路径（`E_PLUGIN_EXISTS` 拒绝覆盖安装，插件级更新按 V4 方案 §9 推迟 1.3），
// 因此「更新」= uninstall + install 两条组合腿，两端都已被令牌覆盖。
//
// 令牌机制**不挂** `plugin-install` 特性（uninstall/purge 的命令面在底座-only
// 构建里也存在）；但「生产档必须带令牌」的强制条件与安装域同域：
// `cfg!(feature = "plugin-install") && Production`——底座-only 构建没有安装目录
// 与 ACL 这两个真正的破坏面，注册表状态翻转保持既有语义。
// ──────────────────────────────────────────────────────────────────────────

/// uninstall/purge 的审批令牌（线形与 [`InstallReviewToken`] 同径：camelCase、
/// `deny_unknown_fields`）。
///
/// 绑定事实：目标插件 id、被审阅的破坏性操作、预览时刻的安装版本。commit 时三项
/// 全部重核——预览之后插件换版本或换了操作，令牌作废，必须重新预览。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdminReviewToken {
    pub plugin_id: String,
    /// 被审阅的破坏性操作（`uninstall` / `purge`）。
    pub op: RegistryAdminOp,
    /// 预览时插件的安装版本（取自注册表条目的 manifest）。
    pub version: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: String,
}

const ADMIN_REVIEW_TTL_SECS: u64 = 10 * 60;
const MAX_ADMIN_REVIEWS: usize = 64;

/// `host_registry_admin` 的线返回（轮 43）。
///
/// `Executed` 变体采用 internally-tagged 平铺：既有 `TransitionOutcome` 的
/// `event`/`from`/`to`/`depth`/`illegal`/`actions` 字段**仍在顶层**，只多一个
/// `kind` 判别字段——老调用方（含外部集成）读法不变。`Review` 是预览路径的返回，
/// 描述「将发生什么」并携带一次性令牌，不产生任何注册表副作用。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RegistryAdminResponse {
    Executed(TransitionOutcome),
    Review(RegistryAdminReview),
}

/// `RegistryAdminResponse::Review` 的载荷（轮 43）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryAdminReview {
    /// 被审阅的破坏性操作。
    pub op: RegistryAdminOp,
    pub plugin_id: String,
    /// 预览时刻的安装版本（与令牌绑定值同源）。
    pub version: String,
    /// 预览时刻的生命周期状态（SCREAMING_SNAKE_CASE，与 `PluginSummary.state` 同源）。
    pub state: LifecycleStateName,
    /// 一次性审批令牌；commit 时必须原样带回。
    pub review_token: AdminReviewToken,
}

#[cfg(feature = "plugin-install")]
fn trusted_time_provider(
    state: &PluginRuntimeState,
) -> Option<&Arc<dyn tauron_host::TrustedTimeProvider>> {
    state.install_config.as_ref().and_then(|config| config.trusted_time_provider.as_ref())
}

fn system_time_to_unix_secs(time: std::time::SystemTime) -> u64 {
    time.duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

/// 审批令牌的「现在」（轮 43 的 A100 收口点）。
///
/// 装配了可信时间源（`with_trusted_time_provider`）时以它为准，且**时钟不可信
/// 即失败关闭**（Suspicious/Unknown 不铸发令牌——TTL 建立在不可信时钟上等于没有
/// TTL）；生产档没有时间源同样失败关闭；只有开发/测试档无源时回落墙钟（既有的
/// 本地兼容姿态，与生产就绪校验的口径一致：`TRUSTED_TIME_REQUIRED` 只在生产档
/// 且安装特性开启时成立）。
fn review_now(state: &PluginRuntimeState) -> HostResult<u64> {
    #[cfg(feature = "plugin-install")]
    if let Some(provider) = trusted_time_provider(state) {
        let trusted = tauron_host::TrustedTimeProvider::trusted_time(provider.as_ref());
        if trusted.state != tauron_host::TimeTrustState::Trusted {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("审批令牌时钟不可信（{:?}）：拒绝铸发/校验破坏性操作令牌", trusted.state),
            ));
        }
        return Ok(system_time_to_unix_secs(trusted.now));
    }
    if state.deployment_mode == tauron_host::DeploymentMode::Production {
        return Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            "生产档缺少可信时间源（A100）：无法为破坏性操作铸发/校验审批令牌",
        ));
    }
    Ok(system_time_to_unix_secs(std::time::SystemTime::now()))
}

/// 令牌 TTL 检查（A100 收口：与 `package_signature` 走**同一个**
/// `require_unexpired` 判定，而不是各自比墙钟）。
///
/// 失败关闭语义与 `time_trust.rs` 一致：时钟不可信 → 拒；过期 → 拒。开发/测试档
/// 无时间源时回落墙钟比较（`now > expires_at` 与 `require_unexpired` 同口径）。
fn review_unexpired(state: &PluginRuntimeState, expires_at: u64) -> HostResult<()> {
    #[cfg(feature = "plugin-install")]
    if let Some(provider) = trusted_time_provider(state) {
        let expires = std::time::UNIX_EPOCH + std::time::Duration::from_secs(expires_at);
        return tauron_host::require_unexpired(provider.as_ref(), expires).map_err(|error| {
            HostError::new(ErrorCode::E_AUTH_DENIED, format!("审批令牌已失效：{error}"))
        });
    }
    if state.deployment_mode == tauron_host::DeploymentMode::Production {
        return Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            "生产档缺少可信时间源（A100）：无法校验破坏性操作审批令牌的有效期",
        ));
    }
    if system_time_to_unix_secs(std::time::SystemTime::now()) > expires_at {
        return Err(HostError::new(ErrorCode::E_AUTH_DENIED, "审批令牌已过期（墙钟）"));
    }
    Ok(())
}

/// 生产档破坏性操作的令牌强制条件（与安装域的 `trusted_time_available`
/// 生产条件同域：安装特性开启的构建里才存在安装目录/ACL 这两个真正破坏面）。
pub fn admin_review_required(state: &PluginRuntimeState) -> bool {
    cfg!(feature = "plugin-install")
        && state.deployment_mode == tauron_host::DeploymentMode::Production
}

fn is_destructive_admin_op(op: RegistryAdminOp) -> bool {
    matches!(op, RegistryAdminOp::Uninstall | RegistryAdminOp::Purge)
}

/// 预览铸发：绑定当前注册表条目事实。铸发前**不**做任何注册表改动。
fn mint_admin_review(
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
    version: &str,
) -> HostResult<AdminReviewToken> {
    run_review_boundary(state, "admin_review_mint", || {
        let issued_at = review_now(state)?;
        let token = AdminReviewToken {
            plugin_id: plugin_id.to_string(),
            op,
            version: version.to_string(),
            issued_at,
            expires_at: issued_at.saturating_add(ADMIN_REVIEW_TTL_SECS),
            nonce: uuid::Uuid::new_v4().to_string(),
        };
        let mut reviews = state.admin_reviews.lock();
        reviews.retain(|_, stored| stored.expires_at > issued_at);
        if reviews.len() >= MAX_ADMIN_REVIEWS {
            if let Some(oldest) = reviews
                .values()
                .min_by_key(|stored| stored.issued_at)
                .map(|stored| stored.nonce.clone())
            {
                reviews.remove(&oldest);
            }
        }
        reviews.insert(token.nonce.clone(), token.clone());
        Ok(token)
    })
}

/// commit 校验：一次性消费 + 事实重核，全部发生在核心迁移（副作用）之前。
///
/// 消费顺序与安装域一致：**先按 nonce 摘除**再比对——伪造/重放的令牌不可能留下
/// 可用条目；真实令牌一旦被提交过（或提交失败）nonce 即失效，必须重新预览。
fn validate_admin_review(
    state: &PluginRuntimeState,
    plugin_id: &str,
    op: RegistryAdminOp,
    token: &AdminReviewToken,
) -> HostResult<()> {
    run_review_boundary(state, "admin_review_consume", || {
        let stored = state.admin_reviews.lock().remove(&token.nonce).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_AUTH_DENIED,
                "admin review token is unknown, expired, or already consumed",
            )
        })?;
        if stored != *token {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                "admin review token is stale or has been tampered with; preview again",
            ));
        }
        review_unexpired(state, token.expires_at)?;
        if token.op != op || token.plugin_id != plugin_id {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                "admin review token 与本次操作不匹配：令牌只能用于被审阅的同一操作与同一插件",
            ));
        }
        let id = PluginId::new(plugin_id)?;
        let entry = state.registry.find(&id).ok_or_else(|| {
            HostError::new(
                ErrorCode::E_UNKNOWN_PLUGIN,
                format!("admin review token 指向的插件 `{plugin_id}` 已不存在；preview again"),
            )
        })?;
        if entry.manifest.version.to_string() != token.version {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "插件 `{plugin_id}` 在预览后已变更（审阅版本 {}，当前版本 {}）；破坏性操作必须重新预览",
                    token.version, entry.manifest.version
                ),
            ));
        }
        Ok(())
    })
}

/// 可选能力未装配时的明确说明。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnsupportedDomain {
    pub domain: String,
    pub reason: String,
}

/// 平台能力未装配时的线协议结果；不能把空值伪装成用户取消或成功。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnsupportedBody {
    pub supported: bool,
    pub reason: String,
    pub fallback: Option<String>,
}

/// 提供者结果：成功时保留原线形，缺少平台提供者时显式返回 UnsupportedBody。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProviderResult<T> {
    Value(T),
    Unsupported(UnsupportedBody),
}

/// 运行了明确降级路径并保留其结果（例如进程内剪贴板缓冲区）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DegradedValue<T> {
    pub supported: bool,
    pub reason: String,
    pub fallback: String,
    pub value: T,
}

fn unsupported_body(reason: &str, fallback: Option<&str>) -> UnsupportedBody {
    UnsupportedBody {
        supported: false,
        reason: reason.to_string(),
        fallback: fallback.map(str::to_string),
    }
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

/// 底座 handler 的命令面；wire-gate 与 `tauron_substrate_handler!` 比对。
pub const SUBSTRATE_COMMANDS: &[&str] = &[
    "host_window_minimize",
    "host_window_maximize",
    "host_window_restore",
    "host_window_close",
    "host_window_quit",
    "host_window_relaunch",
    "host_window_set_position",
    "host_window_set_size",
    "host_clipboard_write",
    "host_clipboard_read",
    "host_deep_link_register",
    "host_dialog_open",
    "host_dialog_save",
    "host_dialog_message",
    "host_dialog_confirm",
    "host_market_check",
    "host_market_download",
    "host_market_install",
    "host_events_publish",
    "host_events_subscribe",
    "host_events_unsubscribe",
    "host_events_drain",
    "host_events_approve",
    "host_events_revoke",
    "host_events_approvals",
    "host_i18n_t",
    "host_i18n_t_params",
    "host_i18n_set_locale",
    "host_i18n_load",
    "host_i18n_stats",
    "host_i18n_cleanup_plugin",
    "host_notify",
    "host_notifications_list",
    "host_notifications_read",
    "host_settings_get",
    "host_settings_set",
    "host_settings_adopt_legacy",
    "host_settings_migrate",
    "host_recover_boot",
    "host_recover_report",
    "host_brand_info",
    // R9：五个宿主能力域（menu / tray / fs / http / updater）+ 品牌/主题孤儿 crate 接通。
    // 全部为**主窗专属**（代码层 `require_main_window`），不进 authz 档位表。
    "host_menu_set",
    "host_menu_popup",
    "host_menu_reset",
    "host_tray_create",
    "host_tray_set_menu",
    "host_tray_remove",
    "host_fs_read",
    "host_fs_write",
    "host_fs_list",
    "host_fs_stat",
    "host_fs_mkdir",
    "host_fs_remove",
    "host_http_request",
    "host_updater_check",
    "host_updater_status",
    "host_theme_list",
    "host_theme_get",
    "host_theme_set",
    "host_production_doctor",
    "host_capabilities",
];

/// 插件运行时 handler 相对底座增加的命令面；wire-gate 与 handler 宏比对。
pub const PLUGIN_RUNTIME_COMMANDS: &[&str] = &[
    "host_lifecycle_report",
    "host_plugin_call",
    "host_call_end",
    "host_cancel",
    "host_registry_list",
    "host_registry_list_all",
    "host_registry_admin",
    "host_contributes_register",
    "host_contributes_list",
    "host_contributes_reconcile",
    "host_recover_trial_enable",
    "host_stream_open",
    "host_stream_write",
    "host_stream_grant",
    "host_stream_close",
    "host_runtime_spawn",
    "host_runtime_health",
    "host_resource_stats",
    "host_window_create",
    // 0.4-A1 调用投递闭环：跨主体调用 / 结果回填 / 结果取件。
    "host_call_plugin",
    "host_call_result",
    "host_call_take",
];

#[cfg(feature = "plugin-install")]
pub const PLUGIN_INSTALL_COMMANDS: &[&str] =
    &["host_registry_install", "host_registry_install_preview"];

/// Feature-gated privileged command auth entries owned by this adapter. The core
/// `tauron-host::authz` table intentionally knows nothing about optional domains.
#[cfg(feature = "plugin-install")]
pub const PLUGIN_INSTALL_AUTH: &[tauron_host::authz::CommandAuth] = &[
    tauron_host::authz::CommandAuth {
        command: "host_registry_install",
        tier: tauron_host::authz::AuthTier::Privileged,
        consumer: "宿主 UI 主窗（插件安装与权限审批）",
        description: "安装经过签名验证的本地插件包并写入精确授权集",
    },
    tauron_host::authz::CommandAuth {
        command: "host_registry_install_preview",
        tier: tauron_host::authz::AuthTier::Privileged,
        consumer: "宿主 UI 主窗（插件安装与权限审批）",
        description: "验证签名插件包并返回安装前权限审批摘要",
    },
];

/// 命令状态：宿主进程生命周期内的共享状态。
///
/// Tauri 通过 `State<CommandState>` 注入到每个命令处理器。
/// 恢复阶段对账的**插件侧写回口**（底座只依赖本 trait，不依赖注册表）。
///
/// 为什么需要这层：`reconcile_recovery_phase` 做的事横跨两个域——读引擎判定
/// （底座）、把判定补发成注册表事件（插件运行时）。若让它直接拿注册表，底座命令
/// 就必须触达 `registry`，R1 的编译期隔离当场失效；若把它整个划归插件域，
/// `host_recover_report`（宿主上报**自身**启动结果，底座语义）就无法触发对账。
///
/// 于是底座只依赖本 trait：`SubstrateState::plugin_flags` 由**多插件宿主**在装配时
/// 注入，底座-only 宿主保持空——没有插件就没有对账对象，这是语义正确，不是静默失败。
pub trait PluginFlagSink: Send + Sync {
    /// 当前插件快照：`(plugin_id, 是否被安全模式禁用)`。
    fn flag_snapshots(&self) -> Vec<(String, bool)>;
    /// 补发安全模式事件；`Some(illegal)` = 注册表已处理（非法迁移只计数不报错），
    /// `None` = 注册表拒绝该次迁移。
    fn report_flag_event(&self, plugin_id: &str, enter_safemode: bool) -> Option<bool>;
}

// ──────────────────────────────────────────────────────────────────────────
// R8：宿主能力 Sink 抽象（窗口 / 对话框 / 深链接）
//
// **为什么需要这一层**：这三类能力的**平台部分**（Tauri 窗口 API、原生对话框
// 插件、深链接插件）此前直接写在 `tauri.rs` 的 `#[tauri::command]` 包装器里。
// 后果有两条，且都是实测出来的：
//  1. 非 Tauri 宿主（harness 壳 / 底座-only 宿主 / 单测）**无法替换实现**——
//     `lib.rs` 里只能留 `Ok(())` / `Ok(None)` 桩，而「桩」与「真实现」的差别
//     没有任何类型或测试能表达；
//  2. 「命令真的调了平台能力」在单测里**不可判定**：`WebviewWindow` /
//     `AppHandle` 构造不出来，只能做类型证据（见 `tauri.rs` 里既有的
//     `tauri_dispatch_sink_implements_the_trait`）。
//
// 抽象存在的**唯一理由**就是这两条：让宿主与单测注入假实现，并让行为可断言
// （`lib.rs` 的 `sink_tests` 模块里「换 sink 即换命令结果」就是验收标准）。
//
// **注入方式**：三个字段都是 [`SubstrateState`] 上的 `pub Arc<dyn …>`，装配时
// 直接替换（生产注入点是 `tauri.rs` 的 `command_state_with_dir_and_config`）。
// 缺省值是**进程内降级实现**（[`MemoryWindowSink`] / [`NoopDialogSink`] /
// [`NoopDeepLinkSink`]）——它们如实降级，不伪造任何平台行为。
//
// **边界（本轮如实登记）**：Tauri 实现里**只有窗口操作是真实的**；
// 原生对话框与 OS 级深链接注册都是**降级路径**（`tauri-plugin-dialog` /
// `tauri-plugin-deep-link` 不在本仓依赖闭包内，硬约束不许新增依赖）。
// 各自的降级语义写在对应实现的文档注释里，一个字都不美化。
// ──────────────────────────────────────────────────────────────────────────

/// 窗口操作的一次留痕（[`MemoryWindowSink`] 的记录单元）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowOpRecord {
    /// 操作线名：`minimize` / `maximize` / `restore` / `close` / `set_position`
    /// / `set_size` / `quit` / `relaunch` / `create`。
    pub op: &'static str,
    /// 目标 webview label；应用级操作（`quit` / `relaunch`）为 `None`。
    pub label: Option<String>,
    /// 诊断细节（几何数值 / 新窗口 url）；无则空串。
    pub detail: String,
}

/// **窗口能力**（平台部分）。与既有命令面逐一对齐。
///
/// 七条既有操作（`host_window_minimize` / `maximize` / `restore` / `close` /
/// `set_position` / `set_size` / `quit`）加 R8 §3 的两条新操作
/// （`relaunch` / `create`，对应 `host_window_relaunch` / `host_window_create`）。
///
/// `label` 是**目标 webview 的 label**，由 wire 层从真实 `WebviewWindow` 取
/// （不是前端入参）：`minimize` / `close` 等操作的就是**调用方自己**那个窗口，
/// 与 R8 之前包装器里 `window.close()` 的语义逐字一致。sink 是**状态级**对象、
/// 不知道"当前调用来自哪个窗口"，所以目标必须显式传进来。
///
/// **错误码（19 码封闭词表内复用）**：目标窗口不存在、或平台拒绝该操作 →
/// `E_STATE_INVALID_TRANSITION`（词表里没有"平台操作失败"这一类，最贴近的语义是
/// 「该操作在当前状态下不成立」；`deep_link_delivered` 已有同样用法）。宁可如实
/// 报"没做成"，也不新增错误码。
pub trait WindowSink: Send + Sync {
    /// 最小化 `label` 窗口。
    fn minimize(&self, label: &str) -> HostResult<()>;
    /// 最大化 `label` 窗口。
    fn maximize(&self, label: &str) -> HostResult<()>;
    /// 还原 `label` 窗口（从最大化/最小化）。
    fn restore(&self, label: &str) -> HostResult<()>;
    /// 关闭 `label` 窗口。
    fn close(&self, label: &str) -> HostResult<()>;
    /// 把 `label` 窗口移到 `(x, y)`（逻辑像素 / DIP）。
    fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()>;
    /// 把 `label` 窗口缩放到 `width × height`（逻辑像素 / DIP）。
    fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()>;
    /// 退出整个应用（应用级，无目标窗口）。
    fn quit(&self) -> HostResult<()>;
    /// 请求宿主**重启**（应用级，R8 §3）。
    ///
    /// # 返回（诚实口径）
    /// - `Ok(true)` = 已向宿主请求重启（Tauri 实现走 `AppHandle::restart()`）；
    /// - `Ok(false)` = **未实现**：宿主没有重启原语（非 Tauri 宿主 = 降级），
    ///   调用方据此如实上报，**不得**把 `false` 当成"已重启"。
    fn relaunch(&self) -> HostResult<bool>;
    /// 创建 webview 窗口（R8 §3）。
    ///
    /// # 返回（诚实口径）
    /// - `Ok(true)` = 真的创建了窗口；
    /// - `Ok(false)` = **未实现**（非 Tauri 宿主 = 降级），命令据此回
    ///   `created: false`，而不是假装创建成功。
    ///
    /// 目标窗口已存在、或平台拒绝创建 → `Err`（不返回"半态成功"）。
    fn create(&self, spec: &WindowCreateSpec) -> HostResult<bool>;
}

/// 进程内窗口 sink（**缺省实现**）：只留痕，**不操作任何窗口**。
///
/// 用途：非 Tauri 宿主（底座-only / harness 壳）的缺省装配；以及单测里断言
/// 「命令真的调了 sink」（抽象存在的验收标准）。
///
/// ⚠️ **它不是窗口实现的替代品**：不创建、不移动、不关闭、不退出任何东西。
/// 要真实窗口行为必须注入平台实现（`tauri.rs` 的 `TauriWindowSink`）。
#[derive(Debug, Default)]
pub struct MemoryWindowSink {
    ops: Mutex<Vec<WindowOpRecord>>,
}

/// 进程内窗口 sink 的留痕上限（环形：超限丢**最旧**一条）。
///
/// 为什么需要：未注入平台 sink 的宿主（非 Tauri / 降级装配）用的就是本实现，
/// 每次窗口操作都会永久追加一条——长跑客户端的 `Vec` 只增不减。留痕是诊断
/// 用途，最近 512 条足够定位；丢最旧与 `NotifyStore` 的溢出策略一致。
pub const MAX_WINDOW_OPS: usize = 512;

impl MemoryWindowSink {
    /// 空记录器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次操作（环形：到顶后丢最旧，保持最近 [`MAX_WINDOW_OPS`] 条）。
    fn record(&self, op: &'static str, label: Option<&str>, detail: String) {
        let mut ops = self.ops.lock();
        if ops.len() >= MAX_WINDOW_OPS {
            ops.remove(0);
        }
        ops.push(WindowOpRecord { op, label: label.map(str::to_string), detail });
    }

    /// 已记录的操作（按发生顺序）。
    pub fn ops(&self) -> Vec<WindowOpRecord> {
        self.ops.lock().clone()
    }

    /// 某操作是否发生过。
    pub fn recorded(&self, op: &str) -> bool {
        self.ops.lock().iter().any(|r| r.op == op)
    }
}

impl WindowSink for MemoryWindowSink {
    fn minimize(&self, label: &str) -> HostResult<()> {
        self.record("minimize", Some(label), String::new());
        Ok(())
    }

    fn maximize(&self, label: &str) -> HostResult<()> {
        self.record("maximize", Some(label), String::new());
        Ok(())
    }

    fn restore(&self, label: &str) -> HostResult<()> {
        self.record("restore", Some(label), String::new());
        Ok(())
    }

    fn close(&self, label: &str) -> HostResult<()> {
        self.record("close", Some(label), String::new());
        Ok(())
    }

    fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()> {
        self.record("set_position", Some(label), format!("x={x},y={y}"));
        Ok(())
    }

    fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()> {
        self.record("set_size", Some(label), format!("width={width},height={height}"));
        Ok(())
    }

    fn quit(&self) -> HostResult<()> {
        self.record("quit", None, String::new());
        Ok(())
    }

    /// 降级：记录请求，但**不重启任何东西**（返回 `false`）。
    fn relaunch(&self) -> HostResult<bool> {
        self.record("relaunch", None, String::new());
        Ok(false)
    }

    /// 降级：记录请求，但**不创建窗口**（返回 `false`）。
    fn create(&self, spec: &WindowCreateSpec) -> HostResult<bool> {
        self.record("create", Some(&spec.label), spec.url.clone());
        Ok(false)
    }
}

/// **对话框能力**（平台部分）。
///
/// 四个方法与既有命令面对齐（`host_dialog_open` / `save` / `message` /
/// `confirm`）。`native_supported() == false` 时命令返回 `UnsupportedBody`，
/// 不把缺少 UI 伪装成用户取消。真正的原生对话框需要 provider；本仓 Tauri sink
/// 当前也未接入该 provider。
pub trait DialogSink: Send + Sync {
    /// 对话框能力是否真实可用；默认缺省实现不支持。
    fn native_supported(&self) -> bool {
        false
    }
    /// 打开文件（`directory = true` 时选目录）。`Ok(None)` = 取消。
    ///
    /// `multiple` 的**多选结果如何回传**当前无法表达：线形（`host_dialog_open`）
    /// 是单个 `string | null`，多选需要数组；本轮不改线形，故平台实现即使支持
    /// 多选也只能回第一个。如实登记为未实现，不假装支持。
    fn open_file(&self, multiple: bool, directory: bool) -> HostResult<Option<String>>;
    /// 保存文件。`Ok(None)` = 取消。
    fn save_file(&self, default_name: Option<&str>) -> HostResult<Option<String>>;
    /// 消息框（`kind` ∈ `info` / `error` / `warning`，缺省 `info`）。
    fn message(&self, kind: &str, title: &str, body: &str) -> HostResult<()>;
    /// 确认框。`Ok(false)` 的语义以实现文档为准（降级实现 = 取消，**不是**
    /// "用户点了否"）。
    fn confirm(&self, title: &str, body: &str) -> HostResult<bool>;
}

/// 对话框 sink 的**降级**实现：没有任何原生 UI（缺省实现）。
///
/// 方法体只提供默认内部值；命令层先检查 `native_supported()` 并对外返回
/// `UnsupportedBody`，所以这些内部值不会被误认为用户操作结果。
#[derive(Debug, Default)]
pub struct NoopDialogSink;

impl DialogSink for NoopDialogSink {
    fn open_file(&self, _multiple: bool, _directory: bool) -> HostResult<Option<String>> {
        Ok(None)
    }

    fn save_file(&self, _default_name: Option<&str>) -> HostResult<Option<String>> {
        Ok(None)
    }

    fn message(&self, _kind: &str, _title: &str, _body: &str) -> HostResult<()> {
        Ok(())
    }

    fn confirm(&self, _title: &str, _body: &str) -> HostResult<bool> {
        Ok(false)
    }
}

/// **深链接能力**（平台部分）：注册 / 注销协议目标是**OS 级**动作。
///
/// 应用层那一半（记录协议 + 声明 `deep-link` 公共 topic）**不在本 trait 里**——
/// 它是平台无关的状态与总线操作，由 [`cmd_deep_link_register`] 自己完成。
/// 本 trait 只负责"让 OS 把这个协议交给本应用"这一件平台事。
pub trait DeepLinkSink: Send + Sync {
    /// 向 OS 注册协议（如 `tauron`）。
    fn register(&self, protocol: &str) -> HostResult<()>;
    /// 向 OS 注销协议。
    ///
    /// 真实消费者是 [`cmd_deep_link_register`] 自己：协议**换值**时必须先注销旧值，
    /// 否则旧协议的 OS 关联会留在系统里（换名后旧链接仍然会拉起本应用，而应用层
    /// 已不认识它）。这不是预留接口，是被调用的路径。
    fn unregister(&self, protocol: &str) -> HostResult<()>;
    /// 本实现是否真的做了 **OS 级**注册（诚实标注，供诊断与测试断言）。
    ///
    /// 全仓当前实现都返回 `false`——见 [`NoopDeepLinkSink`] 与
    /// `tauri.rs` 的 `TauriDeepLinkSink` 的文档注释。
    fn native_supported(&self) -> bool;
}

/// 深链接 sink 的**降级**实现：只记账，不碰 OS（缺省实现）。
///
/// ⚠️ 本仓**没有任何 OS 级深链接注册**：`tauri-plugin-deep-link` 不在依赖闭包内。
/// 应用层的记录与 topic 声明由 [`cmd_deep_link_register`] 完成（那是真实的，
/// 前端能收到 `deep-link` 帧的前提），但"OS 把 `tauron://` 交给本应用"这件事
/// **没有做**——`native_supported()` 恒 `false` 就是这句话的机器可判定形式。
#[derive(Debug, Default)]
pub struct NoopDeepLinkSink;

impl DeepLinkSink for NoopDeepLinkSink {
    fn register(&self, _protocol: &str) -> HostResult<()> {
        Ok(())
    }

    fn unregister(&self, _protocol: &str) -> HostResult<()> {
        Ok(())
    }

    fn native_supported(&self) -> bool {
        false
    }
}

// ──────────────────────────────────────────────────────────────────────────
// R9：五个宿主能力域（menu / tray / fs / http / updater）的 Sink 抽象
//
// 与 R8 的窗口/对话框/深链接**同一套模式**：平台无关 trait 在 `lib.rs`，
// 平台实现（`TauriMenuSink` / `TauriTraySink`）在 feature-gated `tauri.rs`，
// 通过 [`SubstrateState`] 的 `pub Arc<dyn …>` 字段注入；缺省值是**如实降级**的
// 进程内实现（`Memory*` / `Noop*` / `Unavailable*`），绝不伪造平台行为。
//
// **每域的真实性口径（本轮实测结论，不美化）**：
// - `menu`：`tauri::menu`（core，无需额外依赖）→ **真实现**；
// - `tray`：`tauri` 的 `tray-icon` feature（离线缓存有 `tray-icon 0.24.2`）
//   → **真实现**（本轮只在 Windows 验证编译）；
// - `fs`：`std::fs` → **真实现**（无平台依赖，故缺省即真实现；"不可用"体现为
//   [`AdapterConfig::fs_allowed_roots`] 为空时的 `UnsupportedBody`）；
// - `http`：**诚实降级**——`reqwest 0.13.4` 虽在离线缓存，但其 TLS 后端
//   `hyper-tls`（native-tls）与 `hyper-rustls`（rustls）**都不在离线缓存中**，
//   离线解析失败（实测报 `no matching package named hyper-tls/hyper-rustls`）。
//   故本域只留可注入的 `HttpSink`，缺省 [`UnavailableHttpSink`] 返回
//   `UnsupportedBody("未装配 HTTP 提供者")`；**不引入任何新依赖**（硬约束）；
// - `updater`：接 `tauron-distribute`（`check_for_update` / 灰度 / 崩溃门禁）
//   → **真实现**，但 `EndpointClient` 需宿主注入；缺省无端点故 `native_supported()
//   = false`（如实降级）。
// ──────────────────────────────────────────────────────────────────────────

/// 一个菜单项（线形：camelCase；`MenuSpec` 的成员）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MenuItemSpec {
    /// 稳定 id（`host_menu_*` 返回的点击事件里带上它）。
    pub id: String,
    /// 显示文本。
    pub label: String,
    /// 点击时由宿主 `emit` 到该 topic 的帧即为一次真实点击；`None` = 只记录不发布。
    ///
    /// 走的是 Tauri 的**事件通道**（`AppHandle::emit`，前端 `listen`），不是
    /// `host_events_*` 那套底座总线——见 [`crate::MenuRouteTable`]。
    #[serde(default)]
    pub event: Option<String>,
    /// 是否可点（缺省 true）。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 菜单规格（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MenuSpec {
    /// 顶层菜单项（按序）。
    #[serde(default)]
    pub items: Vec<MenuItemSpec>,
}

/// 菜单操作结果（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MenuOutcome {
    /// 平台是否真的应用了该菜单（降级实现恒 `false`）。
    pub applied: bool,
    /// 菜单项数量（诊断用）。
    pub item_count: usize,
    /// `applied == false` 时说明原因；成功时为 `None`。
    pub reason: Option<String>,
}

/// **菜单能力**（平台部分）。
///
/// 菜单点击的回传**不新造传输**，但也不是 `host_events_*` 总线：`TauriMenuSink` 注册
/// 一个 Tauri 全局菜单监听，命中 [`crate::MenuRouteTable`] 后直接用
/// `AppHandle::emit` 把 `{ id, source, native: true }` 发到 `MenuItemSpec::event`
/// 指定的 topic，前端用 `listen` 收。`host_events_drain` 取不到这类帧。
pub trait MenuSink: Send + Sync {
    /// 菜单能力是否真实可用；默认缺省实现不支持。
    fn native_supported(&self) -> bool {
        false
    }
    /// 设置应用菜单（`tauri::menu` 的 `MenuBuilder`）。
    fn set_menu(&self, spec: &MenuSpec) -> HostResult<bool>;
    /// 弹出上下文菜单（`Menu::popup`）。
    fn popup(&self, spec: &MenuSpec) -> HostResult<bool>;
    /// 移除应用菜单。
    fn reset(&self) -> HostResult<bool>;
}

/// 进程内菜单 sink（**降级缺省**）：只留痕，**不建任何菜单**。
///
/// ⚠️ 它不是菜单实现的替代品：要真实菜单必须注入 `tauri.rs` 的 `TauriMenuSink`。
/// `native_supported()` 恒 `false`，命令层据此返回 `UnsupportedBody`。
#[derive(Debug, Default)]
pub struct MemoryMenuSink {
    ops: Mutex<Vec<&'static str>>,
}

/// 菜单/托盘 sink 的留痕上限（环形：超限丢最旧）。
pub const MAX_MENU_OPS: usize = 256;

impl MemoryMenuSink {
    /// 空记录器。
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, op: &'static str) {
        let mut ops = self.ops.lock();
        if ops.len() >= MAX_MENU_OPS {
            ops.remove(0);
        }
        ops.push(op);
    }

    /// 是否记录过某操作。
    pub fn recorded(&self, op: &str) -> bool {
        self.ops.lock().contains(&op)
    }
}

impl MenuSink for MemoryMenuSink {
    fn set_menu(&self, _spec: &MenuSpec) -> HostResult<bool> {
        self.record("set_menu");
        Ok(false)
    }

    fn popup(&self, _spec: &MenuSpec) -> HostResult<bool> {
        self.record("popup");
        Ok(false)
    }

    fn reset(&self) -> HostResult<bool> {
        self.record("reset");
        Ok(false)
    }
}

/// 托盘规格（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraySpec {
    /// 悬浮提示文本。
    #[serde(default)]
    pub tooltip: Option<String>,
    /// 托盘右键菜单。
    #[serde(default)]
    pub menu: Option<MenuSpec>,
}

/// 托盘操作结果（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrayOutcome {
    /// 平台是否真的应用了该操作（降级实现恒 `false`）。
    pub applied: bool,
    /// `applied == false` 时说明原因；成功时为 `None`。
    pub reason: Option<String>,
}

/// **系统托盘能力**（平台部分）。
///
/// 真实现需 `tauri` 的 `tray-icon` feature（本轮仅在 Windows 验证编译；
/// Linux 还需额外系统依赖，**未在本机验证**）。
pub trait TraySink: Send + Sync {
    /// 托盘能力是否真实可用；默认缺省实现不支持。
    fn native_supported(&self) -> bool {
        false
    }
    /// 创建/更新托盘图标。
    fn create(&self, spec: &TraySpec) -> HostResult<bool>;
    /// 设置托盘菜单。
    fn set_menu(&self, spec: &MenuSpec) -> HostResult<bool>;
    /// 移除托盘。
    fn remove(&self) -> HostResult<bool>;
}

/// 进程内托盘 sink（**降级缺省**）：只留痕，**不建任何托盘图标**。
#[derive(Debug, Default)]
pub struct MemoryTraySink {
    ops: Mutex<Vec<&'static str>>,
}

impl MemoryTraySink {
    /// 空记录器。
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, op: &'static str) {
        let mut ops = self.ops.lock();
        if ops.len() >= MAX_MENU_OPS {
            ops.remove(0);
        }
        ops.push(op);
    }

    /// 是否记录过某操作。
    pub fn recorded(&self, op: &str) -> bool {
        self.ops.lock().contains(&op)
    }
}

impl TraySink for MemoryTraySink {
    fn create(&self, _spec: &TraySpec) -> HostResult<bool> {
        self.record("create");
        Ok(false)
    }

    fn set_menu(&self, _spec: &MenuSpec) -> HostResult<bool> {
        self.record("set_menu");
        Ok(false)
    }

    fn remove(&self) -> HostResult<bool> {
        self.record("remove");
        Ok(false)
    }
}

/// 目录项（`host_fs_list` 的行）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsEntry {
    /// 文件名（不含父路径）。
    pub name: String,
    /// 绝对路径。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 文件字节数（目录为 0）。
    pub size: u64,
}

/// `host_fs_stat` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsStat {
    /// 绝对路径。
    pub path: String,
    /// 是否目录。
    pub is_dir: bool,
    /// 是否普通文件。
    pub is_file: bool,
    /// 字节数。
    pub size: u64,
    /// 宿主视角是否只读（`permissions().readonly()`）。
    pub readonly: bool,
}

/// `host_fs_read` 的结果（**文本**；超限时 `truncated: true`）。
///
/// 不引入 base64（硬约束）：二进制文件按 UTF-8 有损解码，`truncated` 如实标注。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsReadResult {
    /// 绝对路径。
    pub path: String,
    /// 文本内容（UTF-8 有损）。
    pub text: String,
    /// 实际读取的字节数。
    pub bytes: u64,
    /// 是否因超过上限而被截断。
    pub truncated: bool,
}

/// `host_fs_write` 的结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FsWriteResult {
    /// 绝对路径。
    pub path: String,
    /// 写入的字节数。
    pub bytes: u64,
}

/// **文件系统能力**（宿主允许根目录内的 I/O）。
///
/// 本 trait 只做**裸 I/O**；路径归属校验（canonicalize + `starts_with`）在命令层
/// 完成——那是安全关键点，必须与 [`AdapterConfig::fs_allowed_roots`] 同源。
///
/// 缺省实现是 [`StdFsSink`]（`std::fs`，**真实现**，无平台依赖）。本域因此
/// **没有"进程内降级实现"**：`std::fs` 就是真实行为；"不可用"体现为允许根为空时
/// 命令层的 `UnsupportedBody`（如实，不伪造）。可注入假实现用于单测。
pub trait FsSink: Send + Sync {
    /// 读取文件（至多 `max_bytes`）。V4 A95：安全关键 I/O 接收 root-scoped handle path。
    fn read(&self, path: &tauron_host::ScopedPath, max_bytes: u64) -> HostResult<(Vec<u8>, bool)>;
    /// 写入文件（覆盖）。V4 A95：Unix 实现通过 openat/O_NOFOLLOW。
    fn write(&self, path: &tauron_host::ScopedPath, bytes: &[u8]) -> HostResult<u64>;
    /// 列目录。Unix 使用已打开 directory handle，不重新解释绝对路径。
    fn list(&self, path: &tauron_host::ScopedPath) -> HostResult<Vec<FsEntry>>;
    /// 取元数据。V4 A95：最终对象必须通过 root-relative handle 打开。
    fn stat(&self, path: &tauron_host::ScopedPath) -> HostResult<FsStat>;
    /// 建目录。Unix 使用 mkdirat；recursive 逐级 no-follow 打开/创建。
    fn mkdir(&self, path: &tauron_host::ScopedPath, recursive: bool) -> HostResult<()>;
    /// 删除文件或（空）目录。Unix 使用 statat(no-follow)+unlinkat。
    fn remove(&self, path: &tauron_host::ScopedPath) -> HostResult<()>;
}

/// `std::fs` 的真实实现（**缺省**）。
#[derive(Debug, Default)]
pub struct StdFsSink;

impl StdFsSink {
    /// 新建。
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(unix))]
fn fs_io_error(op: &str, path: &std::path::Path, e: std::io::Error) -> HostError {
    HostError::new(
        ErrorCode::E_STATE_INVALID_TRANSITION,
        format!("文件系统操作 `{op}` 失败（{}）：{e}", path.display()),
    )
}

#[cfg(unix)]
fn scoped_fs_error(
    op: &str,
    path: &tauron_host::ScopedPath,
    e: tauron_host::ScopedFsError,
) -> HostError {
    HostError::new(
        ErrorCode::E_STATE_INVALID_TRANSITION,
        format!("scoped filesystem `{op}` failed（{}）：{e}", path.display_path().display()),
    )
}

impl FsSink for StdFsSink {
    fn read(&self, path: &tauron_host::ScopedPath, max_bytes: u64) -> HostResult<(Vec<u8>, bool)> {
        #[cfg(unix)]
        {
            tauron_host::scoped_fs_read_hard(path, max_bytes)
                .map_err(|e| scoped_fs_error("read", path, e))
        }
        #[cfg(not(unix))]
        {
            let display = path.display_path();
            use std::io::Read;
            let file =
                std::fs::File::open(&display).map_err(|e| fs_io_error("read", &display, e))?;
            let mut buf = Vec::new();
            let mut limited = file.take(max_bytes.saturating_add(1));
            limited.read_to_end(&mut buf).map_err(|e| fs_io_error("read", &display, e))?;
            let truncated = buf.len() as u64 > max_bytes;
            if truncated {
                buf.truncate(max_bytes as usize);
            }
            Ok((buf, truncated))
        }
    }

    fn write(&self, path: &tauron_host::ScopedPath, bytes: &[u8]) -> HostResult<u64> {
        #[cfg(unix)]
        {
            tauron_host::scoped_fs_write_hard(path, bytes)
                .map_err(|e| scoped_fs_error("write", path, e))
        }
        #[cfg(not(unix))]
        {
            let display = path.display_path();
            std::fs::write(&display, bytes).map_err(|e| fs_io_error("write", &display, e))?;
            Ok(bytes.len() as u64)
        }
    }

    fn list(&self, path: &tauron_host::ScopedPath) -> HostResult<Vec<FsEntry>> {
        #[cfg(unix)]
        {
            let base = path.display_path();
            tauron_host::scoped_fs_list_hard(path)
                .map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| FsEntry {
                            path: base.join(&entry.name).to_string_lossy().into_owned(),
                            name: entry.name,
                            is_dir: entry.is_dir,
                            size: entry.size,
                        })
                        .collect()
                })
                .map_err(|e| scoped_fs_error("list", path, e))
        }
        #[cfg(not(unix))]
        {
            let display = path.display_path();
            let mut out = Vec::new();
            for entry in
                std::fs::read_dir(&display).map_err(|e| fs_io_error("list", &display, e))?
            {
                let entry = entry.map_err(|e| fs_io_error("list", &display, e))?;
                let meta = entry.metadata().map_err(|e| fs_io_error("list", &display, e))?;
                out.push(FsEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: entry.path().to_string_lossy().into_owned(),
                    is_dir: meta.is_dir(),
                    size: if meta.is_file() { meta.len() } else { 0 },
                });
            }
            out.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(out)
        }
    }

    fn stat(&self, path: &tauron_host::ScopedPath) -> HostResult<FsStat> {
        #[cfg(unix)]
        let meta =
            tauron_host::scoped_fs_stat_hard(path).map_err(|e| scoped_fs_error("stat", path, e))?;
        #[cfg(not(unix))]
        let meta = {
            let display = path.display_path();
            std::fs::symlink_metadata(&display).map_err(|e| fs_io_error("stat", &display, e))?
        };
        Ok(FsStat {
            path: path.display_path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            size: if meta.is_file() { meta.len() } else { 0 },
            readonly: meta.permissions().readonly(),
        })
    }

    fn mkdir(&self, path: &tauron_host::ScopedPath, recursive: bool) -> HostResult<()> {
        #[cfg(unix)]
        {
            tauron_host::scoped_fs_mkdir_hard(path, recursive)
                .map_err(|e| scoped_fs_error("mkdir", path, e))
        }
        #[cfg(not(unix))]
        {
            let display = path.display_path();
            let result = if recursive {
                std::fs::create_dir_all(&display)
            } else {
                std::fs::create_dir(&display)
            };
            result.map_err(|e| fs_io_error("mkdir", &display, e))
        }
    }

    fn remove(&self, path: &tauron_host::ScopedPath) -> HostResult<()> {
        #[cfg(unix)]
        {
            tauron_host::scoped_fs_remove_hard(path).map_err(|e| scoped_fs_error("remove", path, e))
        }
        #[cfg(not(unix))]
        {
            let display = path.display_path();
            let meta = std::fs::symlink_metadata(&display)
                .map_err(|e| fs_io_error("remove", &display, e))?;
            let result = if meta.is_dir() {
                std::fs::remove_dir(&display)
            } else {
                std::fs::remove_file(&display)
            };
            result.map_err(|e| fs_io_error("remove", &display, e))
        }
    }
}

/// HTTP 请求规格（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpRequestSpec {
    /// 方法（仅 `GET` / `POST`）。
    #[serde(default = "default_http_method")]
    pub method: String,
    /// 目标 URL（**仅** `http` / `https`）。
    pub url: String,
    /// 请求头。
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    /// 请求体（`POST`）。
    #[serde(default)]
    pub body: Option<String>,
    /// 超时（毫秒，缺省 30000）。
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// 响应体字节上限（缺省 1 MiB）。
    #[serde(default)]
    pub max_bytes: Option<u64>,
}

fn default_http_method() -> String {
    "GET".to_string()
}

/// HTTP 响应（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpResponseSpec {
    /// 状态码。
    pub status: u16,
    /// 响应头（小写键）。
    pub headers: std::collections::BTreeMap<String, String>,
    /// 响应体（UTF-8 有损）。
    pub body: String,
    /// 是否因超过上限而被截断。
    pub truncated: bool,
    /// 本跳实际拨号的解析地址（provider 契约，轮 48 / A96）：宿主用它复检
    /// 私网/字面 IP 规则（DNS rebinding 面）。空 = provider 未回报。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolved_addrs: Vec<String>,
}

/// **HTTP 能力**（平台/网络部分）。
///
/// 本仓**没有装配真实 HTTP 提供者**：如实降级（见下方 `UnavailableHttpSink` 与
/// 本域块的说明）。接入方经 [`AdapterConfig::with_http_sink`] 注入自己的实现即可启用。
pub trait HttpSink: Send + Sync + std::fmt::Debug {
    /// HTTP 能力是否真实可用；默认缺省实现不支持。
    fn native_supported(&self) -> bool {
        false
    }

    /// Provider-side enforcement level. Production HTTP must cover redirects and DNS/private IP.
    fn network_enforcement(&self) -> tauron_host::NetworkEnforcement {
        tauron_host::NetworkEnforcement::UrlOnly
    }

    /// 发起**单跳**请求。
    ///
    /// **单跳契约（轮 48 / A96）**：实现**不得**自行跟随 redirect——3xx 必须原样
    /// 返回（含 `location` 头），由宿主的 [`cmd_http_request`] 逐跳授权后再发下一跳；
    /// 实现**必须**在响应里回报本跳解析地址（`resolved_addrs`），宿主用
    /// [`tauron_host::NetworkPolicy::authorize_resolution`] 复检私网/字面 IP。
    /// 此前这两项只是文档里的"信任我"承诺（`authorize_redirect` /
    /// `authorize_resolution` 除模块自测外零调用者）；现在宿主在生产命令路径上
    /// 逐跳执行这两条规则。
    fn request(
        &self,
        spec: &HttpRequestSpec,
        policy: &tauron_host::NetworkPolicy,
    ) -> HostResult<ProviderResult<HttpResponseSpec>>;
}

/// HTTP sink 的**降级**实现（缺省）：恒返回 `UnsupportedBody`。
///
/// 真原因（实测）：`reqwest 0.13.4` 的两种 TLS 后端 `hyper-tls` 与 `hyper-rustls`
/// 都不在离线缓存中，`cargo check --offline` 报 `no matching package named
/// hyper-tls/hyper-rustls`；硬约束又不许新增依赖，故不接入 HTTP 客户端。
#[derive(Debug, Default)]
pub struct UnavailableHttpSink;

impl HttpSink for UnavailableHttpSink {
    fn request(
        &self,
        _spec: &HttpRequestSpec,
        _policy: &tauron_host::NetworkPolicy,
    ) -> HostResult<ProviderResult<HttpResponseSpec>> {
        Ok(ProviderResult::Unsupported(unsupported_body(
            "未装配 HTTP 提供者（reqwest 的 TLS 后端 hyper-tls/hyper-rustls 不在离线缓存中）",
            Some("注入自定义 HttpSink 实现"),
        )))
    }
}

/// `host_updater_check` 的结果（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdaterCheckOutcome {
    /// 是否有可用更新。
    pub available: bool,
    /// 可用版本号；无则 `null`。
    pub version: Option<String>,
    /// 下载 URL；无则 `null`。
    pub url: Option<String>,
    /// 发布日期；无则 `null`。
    pub released_at: Option<String>,
    /// 是否走了**降级/不可用**路径（端点不可达、签名非法、响应体非法等）。
    pub degraded: bool,
    /// 说明（降级原因 / 灰度未覆盖 / 已是最新等）；无则 `null`。
    pub reason: Option<String>,
}

/// `host_updater_status` 的结果（线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdaterStatus {
    /// 更新提供者是否已装配（`EndpointClient` 已注入）。
    pub available: bool,
    /// 进程内更新状态机（来自 [`ShellExtState::update_state`]）。
    pub state: Option<String>,
    /// `state` 是否来自**模拟**推进（见 [`cmd_market_download`] / [`cmd_market_install`]）。
    ///
    /// 这条线字段存在的唯一理由：`state` 的两个写入方（两条命令各自的分派腿）推进
    /// `update_state` 是**如实**的进程内账本行为（写侧的 `reason` 说清楚了），但
    /// `state` 字符串本身不带这个信息——读侧若直接上屏，"模拟点了一下安装"就会显示成
    /// "已安装 2.0.0"。缺省装配（[`NoUpgradeInstaller`]）下本值恒为 `true`（有 state
    /// 时）；轮 40 装配腿（[`DistributeUpgradeInstaller`]）注入后，**真实效果已发生**
    /// 的推进由写入方置 `false`——由写入方的 provenance 位决定，不由此处推断。
    ///
    /// 本值由 [`UpdaterSink::status`] 从调用方（[`cmd_updater_status`]）传入的
    /// 账本对**推导**（`state.is_some() && 写入方标记`），仓库里不存在把它硬写成
    /// `false` 的地方——那等于无中生有地断言"这条状态是真的"。
    pub state_simulated: bool,
    /// 当前灰度批次百分比。
    pub grayscale_percent: u32,
    /// 崩溃门禁是否已停发。
    pub crash_gate_stopped: bool,
    /// `available == false` 时说明原因；否则 `None`。
    pub reason: Option<String>,
}

/// **更新通道能力**（接 `tauron-distribute`）。
///
/// 真实现 [`DistributeUpdaterSink`] 会真的跑 `check_for_update`（灰度 + 签名 +
/// 清单校验）；但 `EndpointClient` 必须由宿主注入——缺省无端点故
/// `native_supported() == false`。
pub trait UpdaterSink: Send + Sync {
    /// 更新通道是否真实可用（端点已注入）；默认缺省实现不支持。
    fn native_supported(&self) -> bool {
        false
    }
    /// 检查更新。
    fn check(&self, current_version: &str) -> HostResult<ProviderResult<UpdaterCheckOutcome>>;
    /// 当前更新通道状态（灰度批次 / 崩溃门禁 / 提供者可用性）+ 进程内更新账本。
    ///
    /// **账本与 provenance 必须由调用方成对传入**（轮 33）：sink 只拥有通道，
    /// `update_state` 的写入方是 `shell_ext`（`cmd_market_download` / `cmd_market_install`
    /// 的分派腿——缺省模拟，装配腿注入后为真）。sink 自己填这两个字段就等于替写入方
    /// 回答"这条状态是模拟来的还是真的"——那是断言，不是推导，
    /// `check-simulated-never-commits.mjs` 只允许 `simulated: false` 出现在装配腿真路径
    /// （`_wired` 腿）里。
    fn status(&self, ledger_state: Option<String>, ledger_simulated: bool) -> UpdaterStatus;
}

/// 未配置端点时的 `EndpointClient`：如实报"未配置"，**不假装**"已是最新"。
///
/// （`Ok(None)` 在 `tauron-distribute::check_for_update` 里被解释为 `UpToDate`——
/// 那是"端点说已最新"的语义，与"没有端点"是两件事，不能混用。）
struct UnconfiguredEndpointClient;

impl tauron_distribute::EndpointClient for UnconfiguredEndpointClient {
    fn fetch_manifest(
        &self,
        _current_version: &str,
    ) -> tauron_distribute::DistributeResult<Option<tauron_distribute::UpdateManifest>> {
        Err(tauron_distribute::DistributeError::EndpointError(
            "未配置更新端点（EndpointClient 未注入）".into(),
        ))
    }
}

/// `tauron-distribute` 支撑的更新 sink（**真实现**）。
///
/// 持有 `EndpointClient`（可注入）+ 安装身份（§9.1：灰度分桶 =
/// `hash(stableInstallationId)`，取 `AdapterConfig::recovery_data_dir` 下持久化）+
/// 灰度策略 + 崩溃门禁；`check` 直接调 `tauron_distribute::check_for_update`。
/// 装配方用 [`Self::with_endpoint`] 注入真实/模拟端点与安装身份，
/// 用 [`Self::unconfigured`]（缺省）表示"无端点"。
pub struct DistributeUpdaterSink {
    client: Arc<dyn tauron_distribute::EndpointClient>,
    installation: Arc<tauron_distribute::InstallationIdentity>,
    grayscale: Mutex<tauron_distribute::GrayscalePolicy>,
    crash_gate: Mutex<tauron_distribute::CrashGate>,
    configured: bool,
}

impl DistributeUpdaterSink {
    /// **缺省**：无端点（`native_supported() == false`，命令层如实降级）。
    pub fn unconfigured() -> Self {
        Self::unconfigured_with_identity(Arc::new(
            tauron_distribute::InstallationIdentity::ephemeral(),
        ))
    }

    /// 无端点、但携带**持久化安装身份**的缺省形态。
    ///
    /// 为什么要单独给这个口：身份（§9.1）和端点是两件事——端点没配时更新检查
    /// 仍然如实报「不可用」，但身份不该因此变成每次进程重启换桶的临时值。
    /// 宿主装配（[`SubstrateState::with_adapter_config`]）在有数据目录时就用这条，
    /// 让「这一份安装是谁」先于「有没有更新服务器」成立。
    pub fn unconfigured_with_identity(
        installation: Arc<tauron_distribute::InstallationIdentity>,
    ) -> Self {
        Self {
            client: Arc::new(UnconfiguredEndpointClient),
            installation,
            grayscale: Mutex::new(tauron_distribute::GrayscalePolicy::default()),
            crash_gate: Mutex::new(tauron_distribute::CrashGate::default()),
            configured: false,
        }
    }

    /// 注入端点与安装身份（**全量灰度**起步的便捷口）。
    ///
    /// 正式分发路径的身份必须由装配方用
    /// [`tauron_distribute::InstallationIdentity::load_or_create`] 持久化提供；
    /// `ephemeral()` 只用于测试（进程重启换桶）。
    ///
    /// ⚠️ 这条便捷口把灰度钉在 `Batch100`：100% 覆盖下分桶判定
    /// （`user_hash % 100 >= percentage`）**永远不会拒绝任何人**，R2-8 的灰度能力
    /// 等于没接。要真正按安装身份放量，请用
    /// [`Self::with_endpoint_in_grayscale`] 显式给批次。
    pub fn with_endpoint(
        client: Arc<dyn tauron_distribute::EndpointClient>,
        installation: Arc<tauron_distribute::InstallationIdentity>,
    ) -> Self {
        Self::with_endpoint_in_grayscale(
            client,
            installation,
            tauron_distribute::GrayscalePolicy {
                current: tauron_distribute::GrayscaleBatch::Batch100,
                ..Default::default()
            },
        )
    }

    /// 注入端点、安装身份**与灰度策略**（R2-8 分桶在真实链路上的唯一可控入口）。
    ///
    /// 灰度策略是装配方的运维事实（先 1%、停留满再推进），不是宿主的内部常量：
    /// `check` 用它 + 持久化安装身份的 `user_hash()` 一起决定「这次更新对该安装
    /// 是否可见」，`status` 把当前百分比与崩溃停发位如实吐给
    /// `host_updater_status`。推进批次请调 [`Self::advance_grayscale`]（达停留时间
    /// 才成功），崩溃率上报后请调 [`Self::update_crash_gate`]。
    pub fn with_endpoint_in_grayscale(
        client: Arc<dyn tauron_distribute::EndpointClient>,
        installation: Arc<tauron_distribute::InstallationIdentity>,
        grayscale: tauron_distribute::GrayscalePolicy,
    ) -> Self {
        Self {
            client,
            installation,
            grayscale: Mutex::new(grayscale),
            crash_gate: Mutex::new(tauron_distribute::CrashGate::default()),
            configured: true,
        }
    }

    /// 推进灰度批次（达停留时间才成功）。
    pub fn advance_grayscale(&self, now: u64) -> tauron_distribute::DistributeResult<()> {
        self.grayscale.lock().advance(now).map(|_| ())
    }

    /// 更新崩溃门禁（返回是否触发停发）。
    pub fn update_crash_gate(&self, crashes: u32, total: u32) -> bool {
        self.crash_gate.lock().update(crashes, total)
    }
}

impl UpdaterSink for DistributeUpdaterSink {
    fn native_supported(&self) -> bool {
        self.configured
    }

    fn check(&self, current_version: &str) -> HostResult<ProviderResult<UpdaterCheckOutcome>> {
        use tauron_distribute::UpdateCheckResult as R;
        let policy = self.grayscale.lock().clone();
        let result = tauron_distribute::check_for_update(
            self.client.as_ref(),
            current_version,
            &policy,
            // §9.1：灰度分桶来自持久化的本机安装身份（R2-8 修复，非固定 0）。
            self.installation.user_hash(),
        );
        let outcome = match result {
            Ok(R::UpdateAvailable(m)) => UpdaterCheckOutcome {
                available: true,
                version: Some(m.version),
                url: Some(m.url),
                released_at: Some(m.release_date),
                degraded: false,
                reason: None,
            },
            Ok(R::UpToDate) => UpdaterCheckOutcome {
                available: false,
                version: None,
                url: None,
                released_at: None,
                degraded: false,
                reason: Some("已是最新版本".into()),
            },
            Ok(R::NotInGrayscale) => UpdaterCheckOutcome {
                available: false,
                version: None,
                url: None,
                released_at: None,
                degraded: false,
                reason: Some("灰度批次未覆盖该用户".into()),
            },
            Ok(R::EndpointUnavailable(s)) => UpdaterCheckOutcome {
                available: false,
                version: None,
                url: None,
                released_at: None,
                degraded: true,
                reason: Some(format!("更新端点不可用：{s}")),
            },
            Ok(R::SignatureInvalid) => UpdaterCheckOutcome {
                available: false,
                version: None,
                url: None,
                released_at: None,
                degraded: true,
                reason: Some("更新清单签名非法（已拒绝）".into()),
            },
            Ok(R::InvalidBody(s)) => UpdaterCheckOutcome {
                available: false,
                version: None,
                url: None,
                released_at: None,
                degraded: true,
                reason: Some(format!("更新清单格式非法：{s}")),
            },
            Err(e) => UpdaterCheckOutcome {
                available: false,
                version: None,
                url: None,
                released_at: None,
                degraded: true,
                reason: Some(format!("更新检查失败：{e}")),
            },
        };
        Ok(ProviderResult::Value(outcome))
    }

    fn status(&self, ledger_state: Option<String>, ledger_simulated: bool) -> UpdaterStatus {
        // provenance 只能从"有没有账本"推导：无账本时无从谈起，有账本时听写入方的。
        let state_simulated = ledger_state.is_some() && ledger_simulated;
        UpdaterStatus {
            available: self.configured,
            state: ledger_state,
            state_simulated,
            grayscale_percent: self.grayscale.lock().current_percentage(),
            crash_gate_stopped: self.crash_gate.lock().is_stopped(),
            reason: if self.configured {
                None
            } else {
                Some("未配置更新端点（EndpointClient 未注入）".into())
            },
        }
    }
}

/// `host_market_download` / `host_market_install` 的**装配腿注入面**（轮 40）。
///
/// 这是 `tauron_distribute::UpgradeRunner` 执行侧的**仓内生产消费者**：装配方
/// （宿主）注入真实组件后，两条商城命令从模拟桩变为真下载 / 真安装；缺省
/// [`NoUpgradeInstaller`] 保持如实模拟（`simulated: true`）。
///
/// **为什么拆两条而非一条 `run()`**：命令面就是两条（下载 / 安装），账本也分两格
/// （`downloaded:<v>` / `installed:<v>`）；合成一条会把两段的失败语义糊在一起。
///
/// **为什么不收调用方版本号**：URL / 版本 / 摘要的权威来源是宿主装配的清单——与
/// `host_market_check` 不读调用方 `endpoints` / `pubkey` 同一条口径。让 webview
/// 指定"装哪个版本"等于让调用方指定供应链输入。
///
/// **实现方必须承担的语义**（命令层按此推进账本）：
/// - [`UpgradeInstaller::download`]：只在「字节落盘 + SHA-256 核对 + 验签」全部
///   通过后返回 staged 事实；失败必须零 staged 残留（清理失败上浮）；
/// - [`UpgradeInstaller::install`]：只在「备份 → 解压 → 原子交换 → 健康检查 →
///   提交日志（+ 重启）」全部成功后返回；失败路径的回滚由 runner 自带，命令层
///   账本只在成功后推进。
pub trait UpgradeInstaller: Send + Sync {
    /// 是否已装配真实现。缺省 `false` → 两条命令走模拟路径（如实 `simulated: true`）。
    fn native_supported(&self) -> bool {
        false
    }
    /// 下载 + 校验更新包（staged）。返回宿主侧清单的**真实**版本。
    fn download(&self) -> HostResult<StagedUpgrade>;
    /// 安装已 staged 的更新包（无 staged 时报错，必须零账本）。
    fn install(&self) -> HostResult<InstalledUpgrade>;
}

/// [`UpgradeInstaller::download`] 成功的事实（staged 包已校验落盘）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpgrade {
    /// 真实版本（来自宿主侧清单，**不是**调用方入参）。
    pub version: String,
    /// 已校验的包 SHA-256（hex 小写）。
    pub sha256: String,
    /// staged 包路径（宿主发侧事实）。
    pub path: PathBuf,
}

/// [`UpgradeInstaller::install`] 成功的事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledUpgrade {
    /// 真实版本（来自宿主侧清单）。
    pub version: String,
    /// 是否已向宿主请求重启（runner `RestartProvider` 的结果；未注入/未开启为 false）。
    pub restarted: bool,
}

/// 缺省装配腿：未接入（两条命令因此走模拟路径）。方法被直接调用时给类型化硬失败，
/// 绝不返回 success-shaped 结果。
pub struct NoUpgradeInstaller;

impl UpgradeInstaller for NoUpgradeInstaller {
    fn download(&self) -> HostResult<StagedUpgrade> {
        Err(HostError::new(
            ErrorCode::E_STATE_INVALID_TRANSITION,
            "未装配升级执行器（UpgradeInstaller 未注入）：下载腿不可用",
        ))
    }
    fn install(&self) -> HostResult<InstalledUpgrade> {
        Err(HostError::new(
            ErrorCode::E_STATE_INVALID_TRANSITION,
            "未装配升级执行器（UpgradeInstaller 未注入）：安装腿不可用",
        ))
    }
}

/// staged 槽位文件名（`download` 写、`install` 读）。
///
/// **固定名，不掺调用方输入**：版本号来自宿主清单、不进路径，槽位语义是"一份
/// staged 更新"（新的下载替换旧的）。
pub const STAGED_PACKAGE_FILE: &str = "staged-update.zip";

/// 一个按目标平台细分的更新产物（A71）：目标规格 + 该平台专属清单。
///
/// `download()` / `install()` 在**任何下载、解压或 OS 加载动作之前**先做变体解析
/// （[`tauron_host::ArtifactVariantResolver`]）；选不出兼容本机目标的一支就硬拒，
/// 不静默回退到 [`tauron_distribute::UpgradeOptions`] 里的单包默认值。
#[derive(Debug, Clone)]
pub struct UpdateArtifactVariant {
    pub target: tauron_host::TargetSpec,
    pub manifest: tauron_distribute::UpdateManifest,
}

impl AsRef<tauron_host::TargetSpec> for UpdateArtifactVariant {
    fn as_ref(&self) -> &tauron_host::TargetSpec {
        &self.target
    }
}

/// `tauron-distribute` 支撑的**真实**升级装配腿（轮 40）。
///
/// 持宿主装配的清单与目录（[`tauron_distribute::UpgradeOptions`]）＋五个注入组件：
/// - `download`：`download_bounded`（deadline + 协作取消）→ `verify_package`
///   （SHA-256 + 验签），失败清理 staged、清理失败上浮；
/// - `install`：把 staged 包经 [`StagedPackageDownloader`] 喂给完整
///   [`tauron_distribute::UpgradeRunner`]——备份 / 解压 / 原子交换 / 健康检查 /
///   提交 / 重启与自动回滚全部真实执行，journal 落盘。
/// - 变体过滤（A71）：注入 [`UpdateArtifactVariant`] 列表后，`download` / `install`
///   先在**任何下载、解压或 OS 加载动作之前**解析出本机兼容变体；无兼容硬拒，
///   不静默回退单包。
///
/// **边界（不夸大）**：生产 HTTP `Downloader` 与 ed25519 `SignatureVerifier` 仍由
/// 装配方注入（仓内没有联网实现）；`download` / `install` 由内部锁串行化（staged
/// 槽位单份，并发调用不得互踩）。
pub struct DistributeUpgradeInstaller {
    options: tauron_distribute::UpgradeOptions,
    variants: Vec<UpdateArtifactVariant>,
    downloader: Arc<dyn tauron_distribute::Downloader>,
    verifier: Arc<dyn tauron_distribute::SignatureVerifier>,
    extractor: Arc<dyn tauron_distribute::ArchiveExtractor>,
    health_check: Arc<dyn tauron_distribute::UpgradeHealthCheck>,
    restart_provider: Option<Arc<dyn tauron_distribute::RestartProvider>>,
    staged_lock: Mutex<()>,
}

impl DistributeUpgradeInstaller {
    /// 注入清单/目录与全部组件。
    ///
    /// `restart_provider` 为 `None` 时 `options.auto_restart` 必须为 `false`——
    /// 否则 `install` 的前置校验会以 `UpgradeComponentMissing` 硬拒（不静默跳过）。
    pub fn new(
        options: tauron_distribute::UpgradeOptions,
        downloader: Arc<dyn tauron_distribute::Downloader>,
        verifier: Arc<dyn tauron_distribute::SignatureVerifier>,
        extractor: Arc<dyn tauron_distribute::ArchiveExtractor>,
        health_check: Arc<dyn tauron_distribute::UpgradeHealthCheck>,
        restart_provider: Option<Arc<dyn tauron_distribute::RestartProvider>>,
    ) -> Self {
        Self {
            options,
            variants: Vec::new(),
            downloader,
            verifier,
            extractor,
            health_check,
            restart_provider,
            staged_lock: Mutex::new(()),
        }
    }

    /// staged 槽位路径（`download` 的落点 / `install` 的读点）。
    pub fn staged_path(&self) -> PathBuf {
        self.options.download_dir.join(STAGED_PACKAGE_FILE)
    }

    /// 注入按目标细分的更新变体（A71）。非空时下载/安装都只走变体解析；
    /// 空列表保持单清单语义（既有装配不变）。
    pub fn with_variants(mut self, variants: Vec<UpdateArtifactVariant>) -> Self {
        self.variants = variants;
        self
    }

    /// 选出本次动作使用的清单：变体列表非空 → 按**本机编译目标**解析（无兼容即硬拒）；
    /// 空 → 单清单默认。判定发生在下载/解压/OS 加载之前，失败关闭。
    ///
    /// 选出之后还要过**降级门禁**（轮 54）：清单版本低于 `installed_version` 即硬拒，
    /// 与 `UpgradeRunner::validate` 同一口径（同一个 `ensure_not_downgrade`）。
    /// 放在这里是必要的——`download` 腿不经 runner，只靠 `validate` 会在拒绝前
    /// 已经把整包下载并落 staged 文件。
    fn select_manifest(&self) -> HostResult<&tauron_distribute::UpdateManifest> {
        let manifest = if self.variants.is_empty() {
            &self.options.manifest
        } else {
            let host = tauron_host::current_target_spec();
            match tauron_host::ArtifactVariantResolver::resolve(&self.variants, &host) {
                Some(variant) => &variant.manifest,
                None => {
                    return Err(HostError::new(
                        ErrorCode::E_INVALID_MANIFEST,
                        format!(
                            "更新清单提供 {} 个变体，无一兼容本机目标（os={:?} arch={:?}）：拒绝下载，不静默回退单包",
                            self.variants.len(),
                            host.os,
                            host.arch
                        ),
                    ))
                }
            }
        };
        tauron_distribute::ensure_not_downgrade(
            self.options.installed_version.as_deref(),
            &manifest.version,
        )
        .map_err(map_distribute_error)?;
        Ok(manifest)
    }

    /// 下载 + 校验，返回（路径，实际 SHA-256）。下载腿的纯逻辑（失败清理在调用方）。
    fn download_and_verify(
        &self,
        manifest: &tauron_distribute::UpdateManifest,
    ) -> Result<(PathBuf, String), tauron_distribute::DistributeError> {
        let mut progress = |_downloaded: u64, _total: u64| {};
        let path = tauron_distribute::download_bounded(
            self.downloader.clone(),
            manifest.url.clone(),
            self.staged_path(),
            self.options.download_timeout_secs,
            &mut progress,
        )?;
        let sha256 = tauron_distribute::verify_package(manifest, &path, self.verifier.as_ref())?;
        Ok((path, sha256))
    }
}

impl UpgradeInstaller for DistributeUpgradeInstaller {
    fn native_supported(&self) -> bool {
        true
    }

    fn download(&self) -> HostResult<StagedUpgrade> {
        let _slot = self.staged_lock.lock();
        let manifest = self.select_manifest()?;
        if manifest.url.is_empty() {
            return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "更新清单缺少下载 URL"));
        }
        if manifest.version.is_empty() {
            return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "更新清单缺少版本号"));
        }
        if manifest.signature.is_empty() {
            return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "更新清单缺少签名"));
        }
        if manifest.sha256.as_deref().unwrap_or("").trim().is_empty() {
            return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "更新清单缺少 sha256"));
        }
        if self.options.download_timeout_secs == 0 {
            return Err(HostError::new(ErrorCode::E_INVALID_MANIFEST, "下载超时必须 > 0"));
        }
        let staged = self.staged_path();
        if let Some(parent) = staged.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("创建 staged 目录 `{}` 失败：{e}", parent.display()),
                )
            })?;
        }
        // 单槽语义：旧 staged 先清。留着旧包会让"本次失败"还残留一个可被 install
        // 读到的候选——失败路径不得留下会被误当成本次结果的文件。
        match std::fs::remove_file(&staged) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(HostError::new(
                    ErrorCode::E_INSTALL_FAILED,
                    format!("清理旧 staged `{}` 失败：{e}", staged.display()),
                ));
            }
        }
        match self.download_and_verify(manifest) {
            Ok((path, sha256)) => {
                Ok(StagedUpgrade { version: manifest.version.clone(), sha256, path })
            }
            Err(error) => {
                let mapped = map_distribute_error(error);
                // 失败路径清 staged；清理失败必须上浮（不静默留脏）。
                if let Err(cleanup) = std::fs::remove_file(&staged) {
                    if cleanup.kind() != std::io::ErrorKind::NotFound {
                        return Err(HostError::new(
                            ErrorCode::E_INSTALL_FAILED,
                            format!(
                                "{}；且清理 staged `{}` 失败：{cleanup}",
                                mapped.message,
                                staged.display()
                            ),
                        ));
                    }
                }
                Err(mapped)
            }
        }
    }

    fn install(&self) -> HostResult<InstalledUpgrade> {
        let _slot = self.staged_lock.lock();
        // 与 download 同一解析口径：runner 会用**被选中的**清单重验 staged 包的
        // sha256/签名；这里回退单清单会让变体下载的包在重验时被判不一致。
        let manifest = self.select_manifest()?.clone();
        let staged = self.staged_path();
        if !staged.is_file() {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                "没有已 staged 的更新包：先执行 host_market_download",
            ));
        }
        let mut options = self.options.clone();
        options.manifest = manifest;
        let mut runner = tauron_distribute::create_upgrade_runner(options.clone())
            .with_downloader_arc(Arc::new(StagedPackageDownloader { staged }))
            .with_verifier_arc(self.verifier.clone())
            .with_extractor_arc(self.extractor.clone())
            .with_health_check_arc(self.health_check.clone());
        if let Some(provider) = &self.restart_provider {
            runner = runner.with_restart_provider_arc(provider.clone());
        }
        let result = runner.run().map_err(map_distribute_error)?;
        Ok(InstalledUpgrade {
            version: options.manifest.version.clone(),
            restarted: result.restarted,
        })
    }
}

/// 把「已 staged 且已校验」的包当作下载结果的 [`tauron_distribute::Downloader`]：
/// `run_phases` 的下载阶段要求真实产出文件，这里从 staged 复制到本次操作的下载
/// 目录；stage 自身已过摘要 + 验签，runner 拿到后会**再验一次**（同一条
/// `verify_package`），两层都真跑。
struct StagedPackageDownloader {
    staged: PathBuf,
}

impl tauron_distribute::Downloader for StagedPackageDownloader {
    fn download(
        &self,
        _url: &str,
        dest_path: &Path,
        control: &tauron_distribute::PhaseControl,
        progress_callback: &mut dyn FnMut(u64, u64),
    ) -> tauron_distribute::DistributeResult<PathBuf> {
        use tauron_distribute::DistributeError as D;
        if control.is_cancelled() {
            return Err(D::PhaseTimeout { phase: "Download", timeout_secs: 0 });
        }
        if !self.staged.is_file() {
            return Err(D::UpgradeComponentMissing {
                component: "StagedPackage（先执行 host_market_download）",
            });
        }
        let copied = std::fs::copy(&self.staged, dest_path)
            .map_err(|e| D::FileOperationFailed(format!("staged 复制到下载目录失败：{e}")))?;
        progress_callback(copied, copied);
        Ok(dest_path.to_path_buf())
    }
}

/// `tauron_distribute::DistributeError` → 冻结错误面（24 码）的映射（轮 40）。
///
/// 不新增错误码：包摘要/签名/归档/清单类 → `E_INVALID_MANIFEST`（清单或产物不
/// 合格）；阶段超时 → `E_CALL_TIMEOUT`；组件缺失/安装根缺失/日志损坏 →
/// `E_STATE_INVALID_TRANSITION`（装配或状态前提不成立）；其余（文件/交换/备份/
/// 回滚/重启失败）→ `E_INSTALL_FAILED`。
fn map_distribute_error(error: tauron_distribute::DistributeError) -> HostError {
    use tauron_distribute::DistributeError as D;
    let code = match &error {
        D::PackageHashMismatch { .. }
        | D::SignatureInvalid
        | D::ArchiveRejected(_)
        | D::DowngradeRejected { .. }
        | D::InvalidBody(_)
        | D::ManifestParse(_) => ErrorCode::E_INVALID_MANIFEST,
        D::PhaseTimeout { .. } => ErrorCode::E_CALL_TIMEOUT,
        D::UpgradeComponentMissing { .. } | D::InstallRootMissing { .. } | D::Journal(_) => {
            ErrorCode::E_STATE_INVALID_TRANSITION
        }
        _ => ErrorCode::E_INSTALL_FAILED,
    };
    HostError::new(code, format!("升级装配腿失败：{error}"))
}

/// `host_window_create` 的**核心规格**（平台无关；由核心按注册表解析后交给 sink）。
///
/// 字段来源**全部是宿主侧**：`plugin_id` 来自入参但必须先在注册表里存在，
/// `label` 由核心按身份约定铸造（`plugin-<id>`，**不是**入参），
/// `url` 来自 manifest 的 `entry.ui`，`title` / 尺寸来自入参或核心缺省。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCreateSpec {
    /// 窗口 label，恒为 `plugin-<插件 id>`（身份模型的载体）。
    pub label: String,
    /// 目标插件 id（已在注册表中存在）。
    pub plugin_id: String,
    /// 插件主面板 UI 入口（`manifest.entry.ui`）。
    pub url: String,
    /// 窗口标题（入参缺省 = manifest 的 `name`）。
    pub title: String,
    /// 内尺寸宽（逻辑像素）。
    pub width: u32,
    /// 内尺寸高（逻辑像素）。
    pub height: u32,
}

/// `host_window_create` 的线形入参（camelCase）。
///
/// 最小形态刻意**不含 url**：让调用方指定新窗口的 URL 等于把一个"带插件身份的
/// webview"指向任意地址——那个窗口会被宿主当作 `plugin-<id>` 身份单元（label 决定
/// 身份），于是主窗就成了提权跳板。URL 只能来自 manifest 的 `entry.ui`。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowCreateRequest {
    /// 目标插件 id（必须已在注册表中）。
    pub plugin_id: String,
    /// 窗口标题；缺省 = manifest 的 `name`。
    #[serde(default)]
    pub title: Option<String>,
    /// 内尺寸宽（逻辑像素）；缺省 [`DEFAULT_WINDOW_WIDTH`]。
    #[serde(default)]
    pub width: Option<u32>,
    /// 内尺寸高（逻辑像素）；缺省 [`DEFAULT_WINDOW_HEIGHT`]。
    #[serde(default)]
    pub height: Option<u32>,
}

/// 新窗口的缺省内尺寸（与示例应用 `tauri.conf.json` 的主窗尺寸同源：
/// 1024×768；这里取 720 高以避开小屏任务栏遮挡）。
pub const DEFAULT_WINDOW_WIDTH: u32 = 1024;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 720;

/// `host_window_create` 的返回（camelCase 线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowCreateOutcome {
    /// 铸造出的窗口 label，恒为 `plugin-<插件 id>`。
    ///
    /// 这个 label 就是 [`crate::tauri::cleanup_closed_window`] 的回收键
    /// （该函数用 `plugin-` 前缀推出插件 id），因此**创建后的清理不需要新钩子**：
    /// 宿主在 App Builder 的 `on_window_event` 里已有的那一行就够了。
    pub label: String,
    /// 目标插件 id。
    pub plugin_id: String,
    /// 是否**真的**创建了窗口。`false` = 宿主未实现（降级路径）。
    pub created: bool,
    /// `created = false` 时的原因；成功时为 `null`。
    pub reason: Option<String>,
}

/// `host_window_relaunch` 的返回（camelCase 线形）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowRelaunchOutcome {
    /// 重启**之前**完成的阶段对账结果。
    ///
    /// 它出现在返回里不是装饰：这是"先对账、后重启"这个顺序在运行时可观测的
    /// 证据（顺序反了会把"上一次未干净结束"的标记带进下一次启动）。
    pub reconcile: PhaseReconcileOutcome,
    /// 是否**真的**向宿主请求了重启。`false` = 宿主没有重启原语（降级路径）。
    pub relaunch_requested: bool,
    /// `relaunchRequested = false` 时的原因；成功时为 `null`。
    pub reason: Option<String>,
}

/// **底座状态**（substrate）：任意宿主都需要的那部分，**不含插件注册表**。
///
/// R1b 的核心：插件运行时状态与底座状态分开持有，于是底座命令的实现体在**类型上**
/// 就触达不到 `registry` / `contributes`——不是靠约定、也不靠门禁正则，而是编译期。
/// 底座-only 宿主（harness 壳这类不跑插件运行时的客户端）只装配本结构，
/// **一份插件状态都不建**。
#[derive(Clone)]
pub struct SubstrateState {
    /// V4 deployment posture carried into command paths (not only checked at construction).
    pub deployment_mode: tauron_host::DeploymentMode,
    /// V4 A109 production-readiness facts derived from the adapter config at assembly.
    /// `host_production_doctor` recomputes process-runtime facts from the actually
    /// attached sandbox descriptor, so the report cannot drift from the startup gate.
    pub production_readiness: tauron_host::ProductionReadiness,
    pub bus: Arc<Mutex<EventBus>>,
    /// 本份安装的身份（V4 §9.1 / R2-8）：灰度分桶与「这一份安装是谁」的依据。
    ///
    /// 配了数据目录就是**持久化**身份（`installation.id`，重开不换桶、不含 PII）；
    /// 纯内存装配是临时身份（`is_durable() == false`）。装配方注入更新端点时
    /// （[`DistributeUpdaterSink::with_endpoint`]）应把这份额身份一并传过去。
    pub installation_identity: Arc<tauron_distribute::InstallationIdentity>,
    /// V4 A87: process-wide durable-state writer lease. When a data directory is configured,
    /// exactly one Host process may own recovery/settings writes for that directory at a time.
    /// Clones share the same lease handle; dropping the last SubstrateState releases the OS lock.
    pub storage_writer_lease: Option<Arc<tauron_host::PersistentWriterLease>>,
    /// **特权操作的审计 sink**（V4 轮 11 / F3，`tauron_host::admin_audit`）。
    ///
    /// `None` = 未配置 [`AdapterConfig::admin_audit_dir`]：特权判定照常执行，但**没有
    /// 审计事实**，于是 `production_readiness` 的 `audit_for_admin_operations_available`
    /// 为假、Production 启动被拒（不再由宿主自己声明布尔位）。
    ///
    /// 写入点是唯一咽喉点 [`admin_gate`]；读取点是 `host_production_doctor`
    /// 的 `adminAudit` 快照（条数 / 裁剪 / 写失败 / 链完整性），不是布尔位。
    pub admin_audit: Option<Arc<tauron_host::AdminAuditSink>>,
    /// 宿主设置文档（R7-2：`tauron-settings` 的 [`SettingsStore`]，激活孤儿 crate）。
    ///
    /// **不再是裸 `HashMap`**：键的合法性、值的类型、以及跨 schema 版本的迁移
    /// 都由 Store 负责（见 [`cmd_settings_set`] 与 [`host_settings_migrate`]）。
    /// 命名空间是单个伪插件 id [`HOST_SETTINGS_NAMESPACE`]——宿主设置的键是
    /// 开放集合，不需要 per-plugin 隔离。
    ///
    /// **落盘**：`SettingsStore` 自身按设计只管内存态（见其模块头），磁盘 I/O 由
    /// 调用方承担——这里就是那个调用方。装配时从
    /// [`SubstrateState::settings_path`] 读回，每次写成功后落盘。`None` = 不落盘
    /// （测试与底座-only 宿主），行为与之前一致。
    pub settings: Arc<Mutex<SettingsStore>>,
    /// Serialize settings mutations across stage → durable write → commit/rollback without
    /// holding the SettingsStore mutex across filesystem I/O.
    ///
    /// **「跨 I/O」在这里是两个不同的东西**：这把租约**故意**横跨整段落盘——它是单写者
    /// 事务的边界，缺了它，A 的 `restore(&before)` 回滚会把 B 刚成功写入的键一起抹掉
    /// （回滚的是快照，不是增量）。而被禁止的是另一件事：`settings` 互斥量**绝不**进入
    /// `persist_settings_doc`，那里只取一次性快照，读写方不会被磁盘拖住。
    ///
    /// 消费点：`cmd_settings_set`、`cmd_settings_adopt_legacy`、`cmd_settings_migrate`、
    /// `reconcile_settings_boundary`。租约随 `SubstrateState` 克隆共享（同一 `Arc`），
    /// 因此「多个 state 句柄」不等于「多个写者」。
    pub settings_write_lock: Arc<Mutex<()>>,
    /// 设置文档的落盘位置（`None` = 纯内存，重启即丢）。
    pub settings_path: Option<std::path::PathBuf>,
    /// V4 A93 durable generation for host-settings.json. Serialized settings mutations already
    /// hold settings_write_lock, so this counter advances exactly once after each successful rename.
    pub settings_generation: Arc<Mutex<u64>>,
    /// V4 A91: settings-engine panic containment. Once faulted, ordinary settings work is
    /// rejected until the main-window migration/reconcile path proves a durable rebuild.
    settings_fault: Arc<Mutex<tauron_host::FaultBoundary>>,
    /// V4 A91（轮 47）: event-bus panic containment. 故障后所有事件命令拒绝服务，
    /// 直到 [`reconcile_events_boundary`] 做确定性修复（`host_recover_boot` 触发）：
    /// 会话态清零 = 完整一致（订阅可重建）；topic 声明属装配期事实，重置保留。
    events_fault: Arc<Mutex<tauron_host::FaultBoundary>>,
    /// V4 A91（轮 47）: approval-token panic containment（安装/管理两域一次性令牌）。
    /// 故障后令牌操作拒绝服务；修复入口是两条 preview 命令（重新预览 = 清空旧令牌、
    /// 重新铸发）——令牌本就是一次性的，清空是完整语义，不是折衷。
    review_fault: Arc<Mutex<tauron_host::FaultBoundary>>,
    /// V4 A91（轮 47）: registry panic containment（管理面：list/admin/安装提交）。
    /// 注册表条目没有持久镜像、会话内无法证明重建一致 → 修复尝试只做**如实隔离**
    /// （Quarantined），修复路径是重启重新装配（装配重装 + 轮 41 启动孤儿清扫）。
    registry_fault: Arc<Mutex<tauron_host::FaultBoundary>>,
    /// **只写不读**的兼容通知日志（R8 之前的 `host_notify` 产物）。
    ///
    /// ⚠️ **接入状态：没有任何命令返回它**——`host_notify` 返回 `void`，
    /// 通知中心的读取端是 [`SubstrateState::notify_store`]（`host_notifications_list`）。
    /// 因此它是有上限的兼容缓冲（见 [`cmd_notify`] 的环形裁剪），前端
    /// `shell-client.ts` 的 `NotificationRecord` 注释也自认"它今天不在线上"。
    pub notifications: Arc<Mutex<Vec<NotificationRecord>>>,
    /// 通知存储（P0-5：对接 tauron-notify crate）。
    pub notify_store: Arc<Mutex<NotifyStore>>,
    /// 系统通知分发通道（R7-3：`DispatchSink` 的 Tauri 实现）。
    ///
    /// 与 [`SubstrateState::plugin_flags`] 同一注入模式：底座本身**不依赖 tauri**，
    /// 装配时由 feature-gated 的 `tauri` 模块 `OnceLock::set` 一次。
    /// 未注入（底座-only 宿主、单测）= 没有系统通知通道：`host_notify` 只入
    /// 应用内环形缓冲，**不**产生 dispatch 记录（没有尝试就没有日志）。
    pub notify_sink: Arc<std::sync::OnceLock<Arc<dyn DispatchSink>>>,
    /// 恢复引擎（§4.14：启动阶段判定）。
    pub recovery: Arc<Mutex<RecoveryEngine>>,
    /// 恢复持久化载体（崩溃检测标记 + 落盘状态）。`dir = None` 时不落盘。
    pub recovery_store: Arc<Mutex<RecoveryStore>>,
    /// 本轮启动恢复状态的加载来源（诊断用，见 [`LoadSource::as_str`]）。
    pub recovery_source: LoadSource,
    /// i18n 引擎（§4.20：语言状态单一来源 + 资源包 + 缺失键计数）。
    pub i18n: Arc<Mutex<I18nEngine>>,
    /// 壳扩展状态（窗口几何/剪贴板/深链接/更新状态，P2）。
    pub shell_ext: Arc<Mutex<ShellExtState>>,
    /// 应用层订阅分组：多选择器订阅（`host_events_subscribe` 的 `sub` 数组）
    /// 返回的分组 token → [`GroupSubscription`]，供一次 `host_events_unsubscribe`
    /// 整体退订。单选择器订阅不建组（直接透传核心 token，与核心行为一致）。
    /// 窗口关闭 / 插件卸载由 [`prune_subscription_groups`] 按订阅者整组回收。
    ///
    /// 归属**底座**而不是插件运行时：它是事件总线（ipc 域）的订阅登记，任何 webview
    /// 都能订阅，与注册表无关。
    pub subscription_groups: Arc<Mutex<std::collections::HashMap<String, GroupSubscription>>>,
    /// 恢复阶段对账的插件侧写回口（见 [`PluginFlagSink`]）。
    ///
    /// 用 `OnceLock` 而不是普通字段：装配时由 [`PluginRuntimeState::with_substrate`]
    /// 注入一次。宿主把**同一份** `Arc<SubstrateState>` 同时 `manage` 给底座命令、
    /// 交给插件运行时——普通字段无法在事后注入，会让底座命令读到「没注入写回口」的
    /// 副本，对账静默失效。
    pub plugin_flags: Arc<std::sync::OnceLock<Arc<dyn PluginFlagSink>>>,
    /// V4 A97 process sandbox descriptor supplied by the installed plugin runtime.
    /// Bottom-only hosts keep this unset; multi-plugin hosts inject it from the same ProcSpawner
    /// that performs spawn, so capability reporting cannot drift from the execution path.
    pub process_sandbox: Arc<std::sync::OnceLock<tauron_proc::ProcessSandboxDescriptor>>,
    /// 插件运行时**单例装配凭证**（V7-P1-01）。
    ///
    /// 归属**底座**：不变量是「一份底座只装配一份插件运行时」，所以门禁必须长在
    /// 底座上，而不是长在某个 `PluginRuntimeState` 实例上。
    /// [`PluginRuntimeState::with_substrate_and_spawner`] 用 `set` 的返回值领取凭证，
    /// 领取失败即 `AlreadyAssembled` **且不返回 Runtime**——此前只打印冲突日志，
    /// 调用方照样拿到第二个注册表，而恢复对账仍写向第一个（日志替代不了状态不变量）。
    pub plugin_runtime_assembly: Arc<std::sync::OnceLock<AssemblyToken>>,
    /// **窗口能力**（R8 §1）：平台部分的可替换实现。
    ///
    /// 与 [`Self::plugin_flags`] 的 `OnceLock` **不同**，这里是普通 `pub` 字段：
    /// 装配方在 `Arc` 化**之前**直接替换即可（`tauri.rs` 的
    /// `command_state_with_dir_and_config` 就是这么做的），单测也能在同一个位置
    /// 注入假实现——`OnceLock` 的"只设一次"在这里没有必要（没有第二份状态会读到
    /// 旧值），反而会让单测里的替换变成不可达。
    pub window_sink: Arc<dyn WindowSink>,
    /// **对话框能力**（R8 §1）：缺省 = [`NoopDialogSink`]（降级：恒返回取消）。
    pub dialog_sink: Arc<dyn DialogSink>,
    /// **深链接能力**（R8 §1）：缺省 = [`NoopDeepLinkSink`]（无 OS 级注册）。
    pub deep_link_sink: Arc<dyn DeepLinkSink>,
    /// **菜单能力**（R9）：缺省 = [`MemoryMenuSink`]（降级：不建菜单）。
    pub menu_sink: Arc<dyn MenuSink>,
    /// **系统托盘能力**（R9）：缺省 = [`MemoryTraySink`]（降级：不建托盘）。
    pub tray_sink: Arc<dyn TraySink>,
    /// **文件系统能力**（R9）：缺省 = [`StdFsSink`]（`std::fs` 真实现）。
    pub fs_sink: Arc<dyn FsSink>,
    /// 文件系统允许根目录（canonicalize 后的**权威副本**，与
    /// [`AdapterConfig::fs_allowed_roots`] 同源；空 = 该域不可用）。
    pub fs_allowed_roots: Arc<Vec<PathBuf>>,
    /// **HTTP 能力**（R9）：缺省 = [`UnavailableHttpSink`]（诚实降级）。
    pub http_sink: Arc<dyn HttpSink>,
    /// Parsed/validated V4 network scope shared with the concrete provider.
    pub http_policy: Arc<tauron_host::NetworkPolicy>,
    /// **更新通道能力**（R9）：缺省 = [`DistributeUpdaterSink::unconfigured`]。
    pub updater_sink: Arc<dyn UpdaterSink>,
    /// **升级装配腿能力**（轮 40）：缺省 = [`NoUpgradeInstaller`]（两条商城命令
    /// 如实走模拟路径）。装配方在 Arc 化**之前**替换为
    /// [`DistributeUpgradeInstaller`]（注入清单/目录与五组件），下载/安装腿即真。
    pub upgrade_installer: Arc<dyn UpgradeInstaller>,
    /// **主题注册表**（任务二：接通孤儿 crate `tauron-theme`）。
    ///
    /// 与既有 settings 的 `theme` 键**不是同一事实源**：`settings` 存的是"用户选了
    /// 哪个 id"，这里存的是"有哪些主题可用 + 当前激活项"。两者互补而非重复。
    /// 缺省 = [`tauron_theme::ThemeRegistry::default`]（含内置 light/dark）。
    pub themes: Arc<Mutex<tauron_theme::ThemeRegistry>>,
}

// ──────────────────────────────────────────────────────────────────────────
// 进程插件执行器（P0-2：激活 tauron-proc，让 `PluginType::Process` 有真实入口）
//
// 分工：
// - **启动面**（`ProcSpawner`）是可注入 trait：生产 = `std::process::Command`
//   （`tauron_proc::CommandSpawner`），单测 = fake。真起进程的执行器语义由
//   `crates/tauron-test-sidecar/tests/sidecar_e2e.rs` 承担（轮 22 / V7-P1-05：仓内
//   有自己的 sidecar 夹具二进制），单元层因此不必与真实进程时序共舞。
// - **租约**（`plugin_id ↔ lease ↔ pid`）在 `tauron_host::registry::Registry`
//   的 `runtime` 表里（同域锁，见该文件锁序文档）——它是**插件身份**的句柄。
// - **崩溃窗口计数**用 `tauron_proc::CrashTracker`（缺省 3 次 / 5min），不新造计数。
// ──────────────────────────────────────────────────────────────────────────

/// `host_runtime_spawn` 的 `profile` 载荷（camelCase 线形）。
///
/// 为什么启动参数要由调用方给：宿主**不能凭空造出验签材料**。§4.7 要求
/// 「spawn 前校验二进制签名与哈希」，而 manifest 只声明 `entry.sidecar`（名字），
/// 不含可信 hash——于是这里的 `signature` / `binaryHash` / `abi` 是必填项，
/// 缺失即拒（见 [`RuntimeSpawnProfile::to_spawn_config`] 的调用点）。将来由安装器
/// 维护可信指纹库时，这个载荷可以由宿主侧补齐，但**今天不假装它存在**。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSpawnProfile {
    /// sidecar 二进制路径。缺省 = manifest 的 `entry.sidecar`。
    #[serde(default)]
    pub binary_path: Option<String>,
    /// 传给 sidecar 的参数。
    #[serde(default)]
    pub args: Vec<String>,
    /// 追加的环境变量。
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// 二进制签名（算法 / 签名 / 签名者），三项都不得为空。
    pub signature: RuntimeBinarySignature,
    /// 二进制 sha256（64 位 hex）。
    pub binary_hash: String,
    /// ABI 指纹。
    pub abi: RuntimeAbiFingerprint,
}

/// 把**运维**注入的环境变量合进待启动 sidecar 的 env。
///
/// 优先级是本函数的全部难点，规则只有一条：**运维 > 调用方**。
/// `profile.env` 由发起 spawn 的调用方（前端 / 插件装配方）自报，若同名的
/// `ClientConfig.env_overrides` 能被它覆盖，运维就失去了唯一一根确定的杠杆
/// （想给全部 sidecar 强制 `TAURON_PROFILE=prod`，结果任何一个调用方都能改）。
/// 反过来，调用方独有的键照原样保留——本字段只做注入，不做白名单。
///
/// 空 `overrides` = 原样不动（默认装配零影响）。
fn apply_host_env_overrides(
    env: &mut std::collections::HashMap<String, String>,
    overrides: &std::collections::HashMap<String, String>,
) {
    for (key, value) in overrides {
        env.insert(key.clone(), value.clone());
    }
}

/// 线形签名（与 `tauron_proc::BinarySignature` 同构，但走 camelCase）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBinarySignature {
    pub algorithm: String,
    pub signature: String,
    pub signer_id: String,
}

/// 线形 ABI 指纹。
///
/// **不含 `generatedAt`**：校验时刻由宿主时钟决定（`tauron_proc::AbiFingerprint::now`），
/// 让前端自报"何时校验过"等于让诊断依据失去意义。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAbiFingerprint {
    /// crate 版本指纹。
    pub rust_version: String,
    /// 接口哈希。
    pub interface_hash: String,
}

impl RuntimeSpawnProfile {
    /// 落到 `tauron_proc::SpawnConfig`。
    ///
    /// 纯映射：校验（签名 / hash / abi 是否为空、hash 是否 64 位 hex）交给
    /// `tauron_proc::validate_spawn_config`——**同一份规则**不能有两份实现，
    /// 否则"经适配层"与"直接调 tauron-proc"两条路的严格程度会悄悄分叉。
    fn to_spawn_config(&self, binary_path: &str) -> SpawnConfig {
        SpawnConfig {
            binary_path: binary_path.to_string(),
            args: self.args.clone(),
            env: self.env.clone(),
            signature: BinarySignature {
                algorithm: self.signature.algorithm.clone(),
                signature: self.signature.signature.clone(),
                signer_id: self.signature.signer_id.clone(),
            },
            binary_hash: self.binary_hash.clone(),
            abi: ProcAbiFingerprint::now(
                self.abi.rust_version.clone(),
                self.abi.interface_hash.clone(),
            ),
        }
    }
}

/// `host_runtime_health` 的返回。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeHealth {
    /// Proven-alive flag. Unknown is deliberately false rather than optimistic true.
    pub alive: bool,
    /// V4 tri-state process probe. Unknown means ownership/probe could not prove either state.
    pub status: tauron_proc::ProcessStatus,
    /// 该租约绑定的进程号。
    pub pid: u32,
    /// 崩溃窗口内的崩溃次数（`tauron_proc::CrashTracker`，**唯一**计数来源）。
    pub crashes: u32,
    /// 恢复引擎的连续失败计数（`BootCounter.consecutive_failures`）。
    ///
    /// 这是**全局**计数（恢复引擎按"启动连续失败"定义，用于决定 normal /
    /// safemode / repairmode），不是"该插件的崩溃次数"——后者看 `crashes`。
    /// 两个数一起给出，正是为了让调用方不必猜（历史断链的典型症状就是拿错一个
    /// 当另一个用）。崩溃对它的贡献路径见 [`deliver_runtime_crash`]。
    pub consecutive_failures: u32,
    /// 租约回收的留痕（`tauron_host::runtime::ReapStats`）。
    ///
    /// **全局计数**（宿主进程内所有插件的累计：`attempts` / `terminated` /
    /// `alreadyGone` / `failures` / `lastError`），**不是本条租约的统计**——
    /// 挂在 health 上是因为终止失败的留痕必须能被宿主 UI 看到，而为此新开一条
    /// 命令会再牵动能力表/授权档位/TS 镜像一整套面。要按插件看回收情况目前做不到
    /// （表里只留全局计数），如实标注。
    pub reap: ReapStats,
    /// V4 A103 canonical liveness/readiness/degradation report.
    pub health: tauron_host::HealthReport,
}

/// 进程插件运行时：可注入启动面 + 崩溃窗口计数。
pub struct ProcRuntime {
    spawner: Arc<dyn ProcSpawner>,
    /// 崩溃预算（**唯一**定义处：窗口与上限都取自这里，不另造常量）。
    limit: CrashLimit,
    crashes: Mutex<CrashTracker>,
}

impl ProcRuntime {
    /// 用给定启动面装配（缺省崩溃窗口：3 次 / 5min，见 `tauron_proc::CrashLimit`）。
    ///
    /// 崩溃预算**不做成可配置项**：窗口与上限的唯一来源是 `CrashLimit::default()`
    /// （§4.7 的定稿数值），多一个旋钮就多一处可以配置成"永不超限"的地方——那正是
    /// "崩溃预算形同虚设"的成因（缺省参数写错一次，门就永远不会关上）。
    pub fn new(spawner: Arc<dyn ProcSpawner>) -> Self {
        let limit = CrashLimit::default();
        Self { spawner, crashes: Mutex::new(CrashTracker::new(limit.clone())), limit }
    }

    /// 启动面（生产 = `std::process::Command`）。
    pub fn spawner(&self) -> &Arc<dyn ProcSpawner> {
        &self.spawner
    }

    /// V4 A97 sandbox enforcement from the exact spawner used for process execution.
    pub fn sandbox_descriptor(&self) -> tauron_proc::ProcessSandboxDescriptor {
        self.spawner.sandbox_descriptor()
    }

    /// 崩溃预算（窗口秒数 + 窗口内允许的崩溃次数上限）。
    pub fn crash_limit(&self) -> &CrashLimit {
        &self.limit
    }

    /// 记一次崩溃（窗口内计数 + 是否已超限）。返回值 = 未超限。
    pub fn record_crash(&self, plugin_id: &str) -> bool {
        self.crashes.lock().record_crash(plugin_id)
    }

    /// 窗口内崩溃次数。
    pub fn crash_count(&self, plugin_id: &str) -> u32 {
        self.crashes.lock().crash_count(plugin_id)
    }

    /// 窗口内崩溃是否已超限（`CrashLimit`：缺省 3 次 / 5min）。
    ///
    /// 判定边界**完全**由 `tauron_proc::CrashTracker` 定义：窗口内崩溃数 **>**
    /// `max_crashes` 才算超限（即上限是"允许的崩溃次数"，不是"允许的重启次数"）。
    /// 这里不另造一套阈值，免得两处判定各自为政。
    pub fn is_crash_exceeded(&self, plugin_id: &str) -> bool {
        self.crashes.lock().is_exceeded(plugin_id)
    }

    /// 清空该插件的崩溃窗口记录。
    ///
    /// **唯一调用者是"用户显式确认"**（`host_registry_admin` 的 Enable）：状态机的
    /// `ResetCounters` 只重置状态机自己的预算计数，管不到进程侧窗口。不一起清，
    /// 就会出现"用户已确认、状态机说 Enabled、spawn 仍被崩溃窗口拒绝"的死结。
    pub fn reset_crashes(&self, plugin_id: &str) {
        self.crashes.lock().clear(plugin_id);
    }
}

impl Default for ProcRuntime {
    /// 缺省 = 生产启动面（真进程）。测试请用 [`ProcRuntime::new`] 注入 fake。
    fn default() -> Self {
        Self::new(Arc::new(tauron_proc::CommandSpawner::new()))
    }
}

/// 把注册表的租约回收接到进程执行器上（[`LeaseReaper`] 的进程实现）。
///
/// **为什么需要这一层**：宿主核心（`tauron-host`）不认识进程，它只知道"要终止某
/// 个 pid"；进程知识（`Child::kill` + `wait`）住在 `tauron-proc`。装配层是唯一
/// 同时认识两者的地方，因此桥接代码放在这里，而不是让核心反向依赖进程 host。
///
/// 两种终止结果的语义差别在核心侧被保留（`Terminated` / `AlreadyGone`），
/// 失败原因转成 `String` 供核心留痕（核心不依赖 `ProcError`）。
struct SpawnerReaper(Arc<dyn ProcSpawner>);

impl LeaseReaper for SpawnerReaper {
    fn kill(&self, pid: u32) -> Result<ReapOutcome, String> {
        match self.0.kill(pid) {
            Ok(tauron_proc::KillOutcome::Terminated) => Ok(ReapOutcome::Terminated),
            Ok(tauron_proc::KillOutcome::AlreadyGone) => Ok(ReapOutcome::AlreadyGone),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 插件运行时单例装配凭证（V7-P1-01）。
///
/// **不可伪造**：`assembly_id` 来自进程内单调递增计数器，唯一领取点是
/// [`PluginRuntimeState::with_substrate_and_spawner`]；`AssemblyToken::next` 只在
/// 领取凭证时调用一次。字段是公开的、类型本身不含任何能改回「未装配」状态的操作
/// ——底座上的凭证一旦落下，只能被读到，不能被替换或清除。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssemblyToken {
    pub assembly_id: u64,
}

/// 凭证 id 计数器：从 1 起，只在领取凭证时自增（见 [`AssemblyToken::next`]）。
static NEXT_ASSEMBLY_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl std::fmt::Display for AssemblyToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.assembly_id)
    }
}

impl AssemblyToken {
    fn next() -> Self {
        Self { assembly_id: NEXT_ASSEMBLY_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst) }
    }
}

/// 装配失败原因（V7-P1-01 单例不变量的结构化错误面）。
///
/// 实现 [`std::error::Error`] 是为了让 Tauri `setup` 闭包能把它作为
/// `Box<dyn Error>` 原样交回框架：**装配冲突必须在 setup 阶段结构化失败**，
/// 而不是继续 `manage` 到 managed state 的运行时 panic。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyError {
    /// 同一底座已经装配过插件运行时。`existing` 是持有凭证的那次装配，
    /// `attempted` 是被拒的这次——两个 id 不同就能证明「第二次 Runtime 从未存在」。
    AlreadyAssembled { existing: AssemblyToken, attempted: AssemblyToken },
    /// 宿主把同一份 managed state 注册了两次（`init()` 与 `state_init()` 同时使用）。
    /// 与 [`Self::AlreadyAssembled`] 不同：这一份冲突发生在 Tauri 的 state 表上，
    /// 底座凭证还没被领取就已经注定要冲突。
    AlreadyManaged { state: &'static str },
    /// 进程运行时配置在装配前被拒（`AdapterConfig::validate_process_runtime_for_start`）。
    /// 保留 [`HostError`] 而不是只留消息：错误码是宿主语义的一部分，装配失败也要能
    /// 被原样读出（`E_CONFIG_*` / `E_SANDBOX_*` 之类）。
    ProcessRuntimeRejected(tauron_host::HostError),
    /// 跨重启回收台账被拒（IO / 完整性）：`Registry::open_reap_ledger` 的失败原文。
    /// 这份台账是"上一轮宿主是否有杀不掉的进程"的唯一跨重启来源，读不开就不启动
    /// ——静默重置成空账等于把孤儿洗白。
    ReapLedgerRejected(String),
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyAssembled { existing, attempted } => write!(
                f,
                "同一底座已装配过插件运行时（既有凭证 #{existing}，本次尝试 #{attempted}）。\
                 一份底座只允许一份插件运行时：第二次装配不会返回 Runtime，\
                 否则恢复对账仍写向第一个注册表，而调用方手里拿的是第二个。"
            ),
            Self::AlreadyManaged { state } => write!(
                f,
                "managed state `{state}` 已注册：`tauron_adapter::tauri::init()` 与 \
                 `state_init()` 只能二选一，同时使用会重复注册同一份状态。"
            ),
            Self::ProcessRuntimeRejected(reason) => {
                write!(f, "进程运行时配置在装配前被拒：{reason}")
            }
            Self::ReapLedgerRejected(reason) => {
                write!(f, "跨重启回收台账被拒（不静默重置成空账）：{reason}")
            }
        }
    }
}

impl std::error::Error for AssemblyError {}

/// **插件运行时状态**：多插件宿主才装配的部分（注册表 + 贡献登记）。
///
/// 持 [`SubstrateState`] 的**同一份** Arc（不是拷贝）：插件命令既要注册表，也要底座
/// 能力（通知 / i18n / 恢复），因此这里显式给出唯一底座入口。装配时两个 managed 值
/// 共享同一份底座——不存在「第二份状态」这类「改了没生效」的温床。
///
/// 装配入口是 [`Self::with_substrate`] / [`Self::with_substrate_and_spawner`]，
/// 两者返回 `Result<Self, AssemblyError>`：**同一底座上的第二次装配拿不到 Runtime**。
#[derive(Clone)]
pub struct PluginRuntimeState {
    pub substrate: Arc<SubstrateState>,
    pub registry: Arc<Registry>,
    pub contributes: Arc<Mutex<ContributesRegistry>>,
    /// 进程插件运行时（P0-2）：可注入的启动面 + 崩溃追踪。
    ///
    /// 进程细节（`std::process::Command`、存活探测）与崩溃窗口计数都在这一侧，
    /// 不进 `tauron-host`；核心只持有**租约**（`Registry` 的 `runtime` 表）。
    pub proc_runtime: Arc<ProcRuntime>,
    /// 调用投递实现表（0.4-A1）：按插件形态选 `CallDelivery`。
    ///
    /// 未登记的类型（Rust/Wasm 在 A4 之前、或任何无实现者）落到
    /// [`select_delivery`] 的 `UnwiredDelivery`，返回有类型的 `Unsupported`
    /// 而非假装成功——这是「未装配 delivery」负向测试的落地点。
    pub deliveries: Arc<HashMap<DeliveryKind, Box<dyn CallDelivery>>>,
    /// 运维注入的 sidecar 环境变量（[`AdapterConfig::plugin_env_overrides`] 的运行时副本）。
    ///
    /// 放在运行时状态而不是底座：只有 `host_runtime_spawn` 读它，底座-only 宿主没有
    /// 子进程可注入。装配时从 config 拷一次，之后只读——避免每个 spawn 再走一遍
    /// `AdapterConfig`。
    plugin_env_overrides: Arc<HashMap<String, String>>,
    #[cfg(feature = "plugin-install")]
    install_config: Option<InstallRuntimeConfig>,
    /// One-time bounded install-review tokens. Preview mints; commit consumes.
    #[cfg(feature = "plugin-install")]
    install_reviews: Arc<Mutex<HashMap<String, InstallReviewToken>>>,
    /// A83（轮 43）：uninstall/purge 的一次性审批令牌（预览铸发、commit 消费）。
    /// 不挂特性开关：破坏性管理操作在底座-only 构建里也存在（见本文件 A83 段注释）。
    admin_reviews: Arc<Mutex<HashMap<String, AdminReviewToken>>>,
}

#[cfg(feature = "plugin-install")]
#[derive(Clone)]
struct InstallRuntimeConfig {
    root: PathBuf,
    signing_keys: std::collections::BTreeMap<String, Vec<u8>>,
    acl_signing_key: Option<Vec<u8>>,
    trusted_time_provider: Option<Arc<dyn tauron_host::TrustedTimeProvider>>,
}

impl core::ops::Deref for PluginRuntimeState {
    type Target = SubstrateState;

    fn deref(&self) -> &SubstrateState {
        &self.substrate
    }
}

#[cfg(feature = "plugin-install")]
impl PluginRuntimeState {
    /// Install root used by the host's read-only plugin asset protocol.
    pub fn install_config_root(&self) -> Option<&PathBuf> {
        self.install_config.as_ref().map(|config| &config.root)
    }
}

/// 兼容名：`CommandState` ≡ **插件运行时状态**。
///
/// R1b 之前它是 12 字段的 god object；拆分后只是 [`PluginRuntimeState`] 的别名，
/// 并通过 `Deref` 暴露底座字段。保留别名是为了让既有调用点（约 70 处测试构造 +
/// 全部命令包装器）零改动，**不是**因为还存在第二份状态。
///
/// 底座-only 宿主改用 [`SubstrateState`]（`SubstrateState::with_adapter_config`）
/// 并注册 `tauron_substrate_handler!`——类型上就拿不到插件状态。
pub type CommandState = PluginRuntimeState;

/// `host_notify` 的兼容日志上限（见 `cmd_notify` 的说明）。
///
/// 这份日志**当前没有任何读取方**（没有命令返回它，生产代码只在测试里读），
/// 但每次 `host_notify` 都会写入：没有上限时它是一个纯写入的无限增长点。
pub const MAX_NOTIFICATION_LOG: usize = 256;

/// `host_notify` 单条通知的文本字节上限（§33 R3-5：count + bytes 双配额）。
/// 环形缓冲只约束条数，不约束单条大小；不封此口时一条巨型 body 即可
/// 与 256 条正常通知占据同一量级的内存。
pub const MAX_NOTIFY_TITLE_BYTES: usize = 512;
/// 见 [`MAX_NOTIFY_TITLE_BYTES`]。
pub const MAX_NOTIFY_BODY_BYTES: usize = 4096;

/// 兼容用通知记录。
///
/// ⚠️ **不在线上**：没有任何命令返回它（`host_notify` 返回 `()`）。保留是为了
/// 兼容既有调用面。`rename_all = "camelCase"` 是为了让潜在的线上形态与
/// `@tauron/host` 的 TS `NotificationRecord`（`pluginId` 等）一致——两侧字段
/// 名此前不一致（Rust 蛇形 / TS 驼峰），一旦有人把它接上线就会静默读到
/// `undefined`。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub plugin_id: String,
    pub title: String,
    pub body: String,
    pub timestamp: u64,
}

/// 多选择器订阅的分组登记项。
///
/// `subscriber` 必须随行：分组 token（`grp:<首token>`）本身不含归属信息，
/// 而核心订阅会被 `dispose_subscriber`（窗口关闭 / 插件卸载）直接清掉——
/// 登记项若不按订阅者回收，「开窗 → 多选择器订阅 → 关窗」循环会让这张表
/// 无限累积（§8-3 悬挂状态：token 指向已消亡的订阅）。
#[derive(Debug, Clone)]
pub struct GroupSubscription {
    /// 建组的订阅者（wire 层为 `plugin_id_of(webview.label())`，与
    /// [`prune_subscription_groups`] 的回收键同源）。
    pub subscriber: String,
    /// 组内的核心订阅 token（按选择器顺序）。
    pub tokens: Vec<String>,
}

/// §8-17 授权档位表的**生产自检入口**（解 P1-10）。
///
/// `tauron_host::authz::{COMMANDS, ADMIN_COMMANDS}` 是编译期常量表：若有空字段或
/// 重复命令，属**构建缺陷**——授权层对某条命令没有定义，是安全相关的坏状态。
/// 此前 `validate_command_registry` 只在测试里被调用（生产零调用），表错了要等到
/// 有人写测试才暴露。本函数让底座装配**每次进程只跑一次**校验：
///
/// - `OnceLock` 缓存结果：静态表不需要每次构造状态都重算；
/// - 结果可读（测试可断言 `is_ok()`），因此这条链路是**可测的生产调用**，不是
///   只在 `#[cfg(test)]` 里存在的摆设。
fn authz_table_selfcheck() -> &'static Result<(), String> {
    static CHECK: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    CHECK.get_or_init(|| tauron_host::authz::validate_command_registry().map_err(|e| e.message))
}

fn canonical_substrate_service_graph() -> tauron_host::ServiceGraph {
    use tauron_host::{ServiceGraph, ServiceNode};
    let mut graph = ServiceGraph::default();
    for node in [
        ServiceNode { id: "contract".into(), requires: vec![] },
        ServiceNode { id: "policy".into(), requires: vec!["contract".into()] },
        ServiceNode { id: "recovery".into(), requires: vec!["contract".into()] },
        ServiceNode { id: "settings".into(), requires: vec!["contract".into()] },
        ServiceNode { id: "capability".into(), requires: vec!["contract".into(), "policy".into()] },
        ServiceNode { id: "provider".into(), requires: vec!["capability".into()] },
        ServiceNode { id: "message".into(), requires: vec!["policy".into(), "capability".into()] },
    ] {
        graph.insert(node).expect("canonical Tauron service IDs are unique");
    }
    graph
}

impl SubstrateState {
    /// 底座装配：恢复持久化 + i18n + origin 清单 + 通知存储。
    ///
    /// 崩溃检测（上一次未干净结束 → 计一次失败）和「本轮进行中」的落盘都由
    /// [`RecoveryStore::load`] 一次完成，调用方无法跳过这一步（§4.14：判定只
    /// 依赖标记文件与计数器，禁止解析日志字符串）。
    ///
    /// 必需插件集合以 [`AdapterConfig::required_plugins`] 为准整体替换，替换后
    /// 再落盘一次——否则宿主改了必需集合也不跨进程。落盘失败不会静默：原因留在
    /// `last_error`，由 `host_recover_boot` 的 `persistence.lastError` 暴露。
    ///
    /// **不消费 `cfg.registry`**：注册表属插件运行时（[`PluginRuntimeState`]），
    /// 底座装配不该顺带建一份——那正是 R1 要拆掉的东西。
    pub fn with_adapter_config(cfg: &AdapterConfig) -> Self {
        // V4 Production Profile: compatibility fallbacks are allowed only outside production.
        // Production startup must fail before any service/state side effect is created.
        if let Err(error) = cfg.validate_for_start() {
            panic!("[tauron] {error}");
        }

        // V4 ServiceGraph（轮 11 收口 A75）：装配期认的是这张图的**拓扑有效性**——
        // 环或缺依赖是构建缺陷，必须当场 panic，而不是留到运行期重试。
        //
        // 此前它还把 `startup_order()`/`shutdown_order()` 存成两个公开字段，但**没有
        // 任何执行点**（唯一消费者是测试自己），属于"看起来在按拓扑序装配/回收"的
        // 装饰。图的边描述的是**运行期能力依赖**（`message` 依赖 `capability` 的审批
        // 面），不是构造顺序：事件总线在底座装配时就建好，而 capability/provider 属
        // 插件运行时。本仓也没有可排序的服务级回收动作——底座状态都是进程生命周期
        // 资源（随进程释放），唯一有外部副作用的退出动作（回收 sidecar 子进程）由
        // `tauron_proc::CommandSpawner::drop` 承担。
        //
        // 因此两个字段按「删除孤儿逻辑」处置，A75 的"逆序执行"等 A74/A88 给 provider
        // 生命周期补上真实回收动作后再立项（见缺口计划 Batch 4' 与未接线台账）。
        if let Err(error) = canonical_substrate_service_graph().startup_order() {
            panic!("[tauron] service dependency graph invalid: {error:?}");
        }

        // §8-17 生产自检（P1-10）：档位表是构建期常量，错了就是构建缺陷。
        // 失败**立即 panic**（与同一构造函数里 `NotifyStore::new(512).expect(..)` 的
        // 失败姿态一致）——让缺陷在第一次启动就暴露，而不是带病运行到越权发生。
        if let Err(msg) = authz_table_selfcheck() {
            panic!("[tauron] 授权档位表自检失败（§8-17 构建缺陷）：{msg}");
        }

        let notify_store = NotifyStore::new(512).expect("NotifyStore::new(512) should succeed");

        // V4 A87: the same durable data directory is the canonical owner for recovery + settings.
        // Acquire a real OS-backed writer lease before reading or mutating any persistent state.
        // A second process therefore fails at construction instead of racing on JSON/marker files.
        let storage_writer_lease = cfg.recovery_data_dir.as_ref().map(|dir| {
            let namespace = tauron_host::StorageNamespace {
                tenant: "local".into(),
                application: "tauron-substrate".into(),
                principal: "durable-state".into(),
            };
            let owner = format!("pid:{}", std::process::id());
            Arc::new(
                tauron_host::PersistentWriterLease::acquire(
                    &dir.join(".tauron-locks"),
                    namespace,
                    &owner,
                )
                .unwrap_or_else(|error| {
                    panic!("[tauron] durable-state single-writer lease rejected startup: {error}")
                }),
            )
        });

        let mut store = match cfg.recovery_data_dir.clone() {
            Some(dir) => RecoveryStore::new(dir),
            None => RecoveryStore::disabled(),
        };
        let required = cfg.required_plugins.clone();

        let record = store.load(required.clone());
        let mut engine = record.engine;
        // 必需性是宿主配置说了算（引擎自身的 `set_plugin` 只增不减）。
        engine.set_required_plugins(required);
        store.save(&engine);

        // 宿主设置命名空间：注册 schema + v1→v2 迁移（R7-2）。
        // 装配期一次，之后 `host_settings_get`/`set` 才有 schema 可依。
        let mut settings = SettingsStore::new();
        install_host_settings_schema(&mut settings);

        // 设置落盘：与恢复标记共用数据目录。读不回来（首次启动 / 文件损坏）时
        // 保留空文档并**如实记录**——不静默吞掉，也不因为一个坏文件拒绝启动。
        let settings_path = cfg.recovery_data_dir.as_ref().map(|d| d.join(HOST_SETTINGS_FILE));
        let mut settings_generation = 0;
        if let Some(path) = settings_path.as_ref() {
            match load_settings_doc(path) {
                Ok(Some((entries, generation))) => {
                    settings.restore(&entries);
                    settings_generation = generation;
                    // 轮 16 R3（A101 收口）：**健康载入即回执**。回滚镜像只保
                    // 「迁移提交 → 文档第一次被证明读得动」这个窗口；此后它相对
                    // 不断前进的正式文档永远是陈旧的，留着 = 未来任何一次无关损坏
                    // 都会「好心」把用户带回任意久远的过去。一次性语义的完整形式
                    // 是「消费即删 ∧ 回执即删」。
                    let rollback_path = path.with_file_name(HOST_SETTINGS_ROLLBACK_FILE);
                    if rollback_path.exists() {
                        clear_settings_rollback_image(&rollback_path);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    // V4 A101：正式文档读不动时，先问迁移回滚镜像。有且校验通过就带回
                    // **迁移前**的状态启动——这比「生产档拒绝启动」和「降级成空文档」都
                    // 好：用户设置的最后一次已知良好状态是真实存在的，不该被一次坏写吃掉。
                    // 镜像是一次性的：消费即作废，否则下一次的偶然读失败会把用户带回更久以前。
                    let rollback_path = path.with_file_name(HOST_SETTINGS_ROLLBACK_FILE);
                    match load_settings_rollback_image(&rollback_path) {
                        Ok(Some((entries, generation))) => {
                            settings.restore(&entries);
                            settings_generation = generation;
                            clear_settings_rollback_image(&rollback_path);
                            eprintln!(
                                "[tauron] 设置文档 {} 完整性失败（{}），已按 A101 从迁移回滚镜像 {} 恢复到迁移前状态",
                                path.display(),
                                e.message,
                                rollback_path.display()
                            );
                        }
                        Ok(None) => report_settings_load_failure(
                            path,
                            &e.message,
                            None,
                            cfg.deployment_mode,
                        ),
                        Err(rollback_error) => report_settings_load_failure(
                            path,
                            &e.message,
                            Some(&rollback_error.message),
                            cfg.deployment_mode,
                        ),
                    }
                }
            }
        }

        // §33 R2-4（W6）：宿主设置的**提交**要接进消息面——插件侧 `onSettingsChanged`
        // 走 EventBus 订阅，不是新增命令。声明方是宿主自己（publisher = "host"）。
        // topic 刻意**不开 public**：宿主设置是单命名空间、键却可能归属不同插件
        // （`plugin:p…` 形态的键），无条件广播等于允许跨插件观察——可见性交给
        // 审批面（`host_events_approve`，与私有 topic 五步链同一套授权事实）。
        let bus = Arc::new(Mutex::new(EventBus::default()));
        bus.lock()
            .declare_topics(
                "host",
                &[EventDecl { topic: HOST_SETTINGS_CHANGED_TOPIC.to_string(), public: false }],
            )
            .expect("host-owned mirror topic cannot collide (reserved namespace `host:`)");

        // §33 R2-8（§9.1）：安装身份**先于**更新端点成立。有数据目录就地持久化
        // （首用随机 UUIDv4，重开不换桶，不含 PII）；文件损坏时 `load_or_create`
        // 拒绝静默重置——这里按降级处理（临时身份 + 留痕），因为「灰度桶跨进程
        // 稳定」是优化项，不是启动不变量，拒启代价太大。无数据目录（纯内存装配）
        // 才用临时身份。
        let installation_identity = Arc::new(
            match cfg
                .recovery_data_dir
                .as_ref()
                .map(|dir| tauron_distribute::InstallationIdentity::load_or_create(dir))
            {
                Some(Ok(identity)) => identity,
                Some(Err(err)) => {
                    eprintln!(
                        "[tauron] 安装身份不可用，本轮以临时身份启动（灰度桶不跨进程）：{err}"
                    );
                    tauron_distribute::InstallationIdentity::ephemeral()
                }
                None => tauron_distribute::InstallationIdentity::ephemeral(),
            },
        );

        // V4 轮 11 / F3：审计 sink 在**装配期**打开。打不开就拒绝启动而不是降级——
        // 一份可能被篡改/撕裂的审计日志比"明知道没有日志"更危险（读回时同时校验
        // durable 校验和与哈希链，见 `tauron_host::admin_audit::AdminAuditSink::open`）。
        let admin_audit = cfg.admin_audit_dir.as_ref().map(|dir| {
            Arc::new(tauron_host::AdminAuditSink::open(dir).unwrap_or_else(|error| {
                panic!("[tauron] admin audit sink rejected startup: {error}")
            }))
        });

        Self {
            deployment_mode: cfg.deployment_mode,
            production_readiness: cfg.production_readiness(),
            bus,
            installation_identity: installation_identity.clone(),
            storage_writer_lease,
            admin_audit,
            settings: Arc::new(Mutex::new(settings)),
            settings_write_lock: Arc::new(Mutex::new(())),
            settings_path,
            settings_generation: Arc::new(Mutex::new(settings_generation)),
            settings_fault: Arc::new(Mutex::new(tauron_host::FaultBoundary::new("settings"))),
            events_fault: Arc::new(Mutex::new(tauron_host::FaultBoundary::new("events"))),
            review_fault: Arc::new(Mutex::new(tauron_host::FaultBoundary::new("approval"))),
            registry_fault: Arc::new(Mutex::new(tauron_host::FaultBoundary::new("registry"))),
            notifications: Arc::new(Mutex::new(Vec::new())),
            notify_store: Arc::new(Mutex::new(notify_store)),
            notify_sink: Arc::new(std::sync::OnceLock::new()),
            recovery: Arc::new(Mutex::new(engine)),
            recovery_store: Arc::new(Mutex::new(store)),
            recovery_source: record.source,
            i18n: Arc::new(Mutex::new(I18nEngine::default())),
            shell_ext: Arc::new(Mutex::new(ShellExtState {
                origin_allowlist: cfg.origin_allowlist.clone(),
                main_window_labels: cfg.effective_main_window_labels(),
                ..ShellExtState::default()
            })),
            subscription_groups: Arc::new(Mutex::new(std::collections::HashMap::new())),
            plugin_flags: Arc::new(std::sync::OnceLock::new()),
            process_sandbox: Arc::new(std::sync::OnceLock::new()),
            plugin_runtime_assembly: Arc::new(std::sync::OnceLock::new()),
            // R8：三类平台能力的**降级缺省**。宿主（`tauri.rs`）在装配时替换为
            // Tauri 实现；不替换 = 进程内留痕 / 取消 / 无 OS 注册（如实降级）。
            window_sink: Arc::new(MemoryWindowSink::new()),
            dialog_sink: Arc::new(NoopDialogSink),
            deep_link_sink: Arc::new(NoopDeepLinkSink),
            // R9：五域的降级/真实现缺省（见本域块的"真实性口径"）。
            menu_sink: Arc::new(MemoryMenuSink::new()),
            tray_sink: Arc::new(MemoryTraySink::new()),
            fs_sink: Arc::new(StdFsSink::new()),
            // 允许根目录先 canonicalize：不存在/不可解析的根被**忽略**（不整体
            // 拒绝启动，也不放开全盘）。留存的都是可用于 `starts_with` 的绝对路径。
            fs_allowed_roots: Arc::new(
                cfg.fs_allowed_roots.iter().filter_map(|p| p.canonicalize().ok()).collect(),
            ),
            // 轮 48 / A96：注入式 provider（`with_http_sink`）缺省 = 诚实降级。
            http_sink: cfg.http_sink.clone().unwrap_or_else(|| Arc::new(UnavailableHttpSink)),
            http_policy: Arc::new(cfg.http_policy.clone()),
            updater_sink: Arc::new(DistributeUpdaterSink::unconfigured_with_identity(
                installation_identity,
            )),
            // 轮 40：升级装配腿缺省不接入（与 updater_sink 同一装配姿态：宿主
            // 注入才为真；`with_adapter_config` 不替装配方虚构组件）。
            upgrade_installer: Arc::new(NoUpgradeInstaller),
            themes: Arc::new(Mutex::new(tauron_theme::ThemeRegistry::default())),
        }
    }
}

impl PluginRuntimeState {
    /// 创建默认状态（测试用）。
    ///
    /// **恢复持久化关闭**：不碰磁盘，因此测试无副作用、无残留文件。生产宿主
    /// 请用 [`Self::with_adapter_config`] 并传入宿主数据目录。
    pub fn new() -> Self {
        Self::with_adapter_config(AdapterConfig::default())
    }

    /// 创建指定注册表配置的状态（恢复持久化仍关闭）。
    pub fn with_config(config: RegistryConfig) -> Self {
        Self::with_adapter_config(AdapterConfig {
            registry: Some(config),
            ..AdapterConfig::default()
        })
    }

    /// 自建底座并装配插件运行时（测试 + 「自己全都要」的宿主）。
    ///
    /// 底座是本次调用新建的，因此单例凭证必然由这次装配领取，`Err` 分支不可达。
    pub fn with_adapter_config(cfg: AdapterConfig) -> Self {
        let substrate = Arc::new(SubstrateState::with_adapter_config(&cfg));
        Self::with_substrate(substrate, cfg)
            .unwrap_or_else(|error| panic!("[tauron] 自建底座装配失败：{error}"))
    }

    /// 在**既有**底座状态上装配插件运行时。
    ///
    /// 宿主装配走这里（而不是 [`Self::with_adapter_config`]）：底座 Arc 由宿主创建
    /// 并同时 `manage`，插件运行时复用**同一份**——两个 managed 值不可能指向两份
    /// 底座状态。
    ///
    /// **单例语义（V7-P1-01）**：返回 `Result`，同一份底座的第二次装配得到
    /// [`AssemblyError::AlreadyAssembled`]，**不会**拿到第二个 Runtime。此前这里只
    /// 打印冲突日志就照样返回新注册表，于是「调用方手里的注册表」与「恢复对账写入
    /// 的注册表」分成两份——日志替代不了状态不变量。
    ///
    /// 进程执行器用生产启动面（`std::process::Command`）。**测试请用**
    /// [`Self::with_spawner`]，否则会在 CI 上真起进程。
    pub fn with_substrate(
        substrate: Arc<SubstrateState>,
        cfg: AdapterConfig,
    ) -> Result<Self, AssemblyError> {
        Self::with_substrate_and_spawner(
            substrate,
            cfg,
            Arc::new(tauron_proc::CommandSpawner::new()),
        )
    }

    /// [`Self::with_substrate`] 的可注入变体：进程启动面由调用方给出。
    ///
    /// 这是「测试里绝不真起 sidecar」的落地点：fake 启动面既能证明**真**调用了
    /// 启动面（记录调用），也能凭空制造崩溃（把 pid 标记为已死），而执行器语义
    /// （租约、`E_LEASE_EXPIRED`、崩溃计数、`RuntimeCrash` 投递）全部照原样走。
    pub fn with_substrate_and_spawner(
        substrate: Arc<SubstrateState>,
        cfg: AdapterConfig,
        spawner: Arc<dyn ProcSpawner>,
    ) -> Result<Self, AssemblyError> {
        let sandbox_descriptor = spawner.sandbox_descriptor();
        cfg.validate_process_runtime_for_start(&sandbox_descriptor)
            .map_err(AssemblyError::ProcessRuntimeRejected)?;

        // **领取单例凭证**：`OnceLock::set` 只接受第一次写入，所以这一步就是
        // 「同一底座只有一份插件运行时」的不变量本身。领取必须在建注册表**之前**，
        // 否则并发第二次装配会先建出一个没人认领的 Registry（真进程启动面还会长出
        // 一个 reaper），然后把冲突写成一条日志。
        let attempted = AssemblyToken::next();
        if substrate.plugin_runtime_assembly.set(attempted).is_err() {
            // `set` 失败 ⇒ 凭证已被别人领取，`get()` 必返回 `Some`。
            let existing = *substrate
                .plugin_runtime_assembly
                .get()
                .expect("凭证领取失败说明底座上已有装配记录");
            return Err(AssemblyError::AlreadyAssembled { existing, attempted });
        }

        let registry = Arc::new(Registry::new(cfg.registry.unwrap_or_default()));
        // 把租约回收接到进程执行器（P0-2 缺口一）：卸载/清除/崩溃换新都必须**真的**
        // 终止 sidecar，否则留下没人认领的孤儿进程。未注入时核心仍会摘表项，
        // 但每次终止都按失败留痕（`Registry::runtime_reap_stats`），不会静默。
        registry.set_lease_reaper(Arc::new(SpawnerReaper(spawner.clone())));
        // 跨重启孤儿扫描（V7 §7「startup orphan sweep」）：平台存活探测固定注入，
        // 回收台账落在宿主的恢复数据目录。上一轮宿主"杀不掉"的 pid 在启动时逐条
        // 定性——**只探测不杀**（跨重启无法验明进程身份，盲杀可能命中复用同号的
        // 新进程，见 `RuntimeTable::sweep_restart_reaps` 的安全封口）。
        // 台账撕裂/篡改/IO 失败 = 拒绝启动：它是"上一轮是否有孤儿"的唯一跨重启
        // 事实来源，静默重置成空账等于把孤儿洗白。
        registry.set_system_pid_probe();
        if let Some(dir) = cfg.recovery_data_dir.as_ref() {
            registry
                .open_reap_ledger(dir)
                .map_err(|e| AssemblyError::ReapLedgerRejected(e.to_string()))?;
        }
        // 注入恢复对账的插件侧写回口 + 进程沙箱事实。两者都长在这份新注册表 /
        // 这次领取的凭证上，所以写入必须成功——凭证已独占，同一底座不可能有第二次
        // 装配走到这里（见 [`AssemblyError::AlreadyAssembled`]）。
        //
        // 用 `is_err()` + `panic!` 而不是 `expect`：`OnceLock::set` 的错误类型
        // `SetError<T>` 要 `T: Debug` 才可实现，而这里的 T 是
        // `Arc<dyn PluginFlagSink>`（trait object 没有 Debug）。
        if substrate
            .plugin_flags
            .set(Arc::new(RegistryFlagSink { registry: registry.clone() }))
            .is_err()
        {
            panic!("[tauron] 单例凭证已领取，插件侧写回口不可能被他人注入");
        }
        if substrate.process_sandbox.set(sandbox_descriptor).is_err() {
            panic!("[tauron] 单例凭证已领取，process sandbox descriptor 不可能被他人注入");
        }
        let proc_runtime = Arc::new(ProcRuntime::new(spawner));
        let deliveries = Self::default_deliveries(&substrate, &registry, &proc_runtime);
        Ok(Self {
            substrate,
            registry,
            contributes: Arc::new(Mutex::new(ContributesRegistry::default())),
            proc_runtime,
            deliveries: Arc::new(deliveries),
            plugin_env_overrides: Arc::new(cfg.plugin_env_overrides.clone()),
            #[cfg(feature = "plugin-install")]
            install_config: match cfg.plugin_install_dir {
                // 轮 16：装成 **canonical 权威副本**（与 `fs_allowed_roots` 同一
                // 条规则：见本文件对允许根目录的 canonicalize 注释）。
                // 存原始串时，`installed_plugin_ui` 返回的 `entry`（canonicalize 过）
                // 与 `install_config_root()`（原始串）不同形，`window_create` 对
                // `plugin-*` 标签做的 `entry.strip_prefix(root)` 必然失配 →
                // E_INSTALL_FAILED。集成方给相对路径、经 env 变量传入、或路径里
                // 含符号链接 / Windows 8.3 短名时会踩到；CI 的干净临时目录踩不到。
                // 目录此刻尚不存在（首次安装才创建）时按原样保留——**但这句话本身
                // 不构成保证**：轮 17 复查指出 `window_create` 拿的是这个存储值，而
                // `entry` 已 canonical 过，所以比较点自己也得 canonicalize 一次
                // （见 [`plugin_window_asset_relative`]），否则「首装 + 目录当时不存在」
                // 就把原 bug 原样保留在一条更窄的路径上。
                Some(raw_root) => {
                    let root = raw_root.canonicalize().unwrap_or_else(|_| raw_root.clone());
                    Some(InstallRuntimeConfig {
                        root,
                        signing_keys: cfg.plugin_signing_keys,
                        acl_signing_key: cfg.acl_signing_key,
                        trusted_time_provider: cfg.trusted_time_provider,
                    })
                }
                None => None,
            },
            #[cfg(feature = "plugin-install")]
            install_reviews: Arc::new(Mutex::new(HashMap::new())),
            admin_reviews: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// 构建默认投递实现表（0.4-A1）。
    ///
    /// - `Js`：复用事件总线 request 通道（`JsCallDelivery`）。
    /// - `Process`：经 sidecar stdin/stdout 帧回路（`ProcessCallDelivery`）；sidecar
    ///   必须在 `cmd_runtime_spawn` 后处于运行中，`live_pid_of` 才能解析出 pid。
    /// - `Wasm`：`WasmCallDelivery`——投递路径**真的经过** `tauron-wasm` 的配置 /
    ///   ABI / 崩溃预算校验层（任务二接线），但**执行层没有可执行通路**：仓内的
    ///   wasmi provider 已落地却没接到投递侧，且它只支持整数 host_fn ABI
    ///   （JSON 参数走 linear memory 的完整 ABI 未实现）。因此仍诚实返回
    ///   `delivered: false`（上层转 `E_PLUGIN_TYPE_NO_RUNTIME`）。
    /// - `Rust`（Native）：A4 之前无执行器，保持 unwired（落 `UnwiredDelivery`，
    ///   诚实返回 `Unsupported`，不假装能调起）。
    pub fn default_deliveries(
        substrate: &Arc<SubstrateState>,
        registry: &Arc<Registry>,
        proc_runtime: &Arc<ProcRuntime>,
    ) -> HashMap<DeliveryKind, Box<dyn CallDelivery>> {
        let mut map: HashMap<DeliveryKind, Box<dyn CallDelivery>> = HashMap::new();
        map.insert(
            DeliveryKind::Js,
            Box::new(JsCallDelivery::new(substrate.bus.clone(), registry.clone())),
        );
        map.insert(
            DeliveryKind::Process,
            Box::new(ProcessCallDelivery::new(proc_runtime.clone(), registry.clone())),
        );
        #[cfg(feature = "runtime-wasm-broker")]
        {
            // Broker feature only wires validation/delivery metadata. The actual WASM
            // engine remains an on-demand runtime pack and must still report unsupported
            // until that engine is installed/ready.
            map.insert(DeliveryKind::Wasm, Box::new(WasmCallDelivery::new(registry.clone())));
        }
        map
    }

    /// 按目标插件形态选投递实现并投递（0.4-A1）。
    ///
    /// 返回 `ProviderResult::Value(call)` = 已投递待应答；`ProviderResult::Unsupported`
    /// = 未装配投递（无通路）。投递传输错误（队列满等）以 `Err` 透传。
    pub fn deliver_call(&self, call: &PendingCall) -> HostResult<ProviderResult<PendingCall>> {
        let target = match PluginId::new(&call.target) {
            Ok(id) => id,
            Err(_) => {
                return Ok(ProviderResult::Unsupported(UnsupportedBody {
                    supported: false,
                    reason: format!("调用目标 `{}` 不是合法插件 id", call.target),
                    fallback: None,
                }));
            }
        };
        let entry = self.registry.require(&target)?;
        let kind = delivery_kind_of(entry.manifest.plugin_type);
        let delivery = select_delivery(kind, &self.deliveries);
        let receipt = delivery.deliver(call)?;
        if receipt.delivered {
            Ok(ProviderResult::Value(call.clone()))
        } else {
            Ok(ProviderResult::Unsupported(unsupported_body(
                &receipt.reason.unwrap_or_else(|| "无可用调用投递通路".into()),
                None,
            )))
        }
    }

    /// 按目标插件形态选投递实现并**结算**（与 [`Self::deliver_call`] 对称）。
    ///
    /// 为什么结算也必须走投递表：`CallDelivery` 是「一次调用的通路」的唯一抽象面，
    /// 投递与结算是同一条通路的两端。结算若绕开投递表直连
    /// [`Registry::settle_call`](tauron_host::registry::Registry::settle_call)，
    /// `CallDelivery::settle` 上的语义（`UnwiredDelivery` 的诚实拒绝、未来 Process
    /// 形态按 sidecar 协议解析回执）就成了**永远不会被执行的孤儿分支**——写的人
    /// 以为它生效，实际任何形态的结算都走另一条直连。
    ///
    /// 兜底：目标插件已不在注册表（卸载后仍残留的 pending call）时直接走注册表结算
    /// ——投递实现只是"按形态分流"，形态信息消失不该让发起方拿不到结果。
    pub fn settle_call_for(
        &self,
        target: &str,
        call_id: &str,
        outcome: CallOutcome,
    ) -> HostResult<PendingCall> {
        let entry = PluginId::new(target).ok().and_then(|id| self.registry.require(&id).ok());
        let Some(entry) = entry else {
            return self.registry.settle_call(call_id, outcome);
        };
        let kind = delivery_kind_of(entry.manifest.plugin_type);
        select_delivery(kind, &self.deliveries).settle(call_id, outcome)
    }

    /// 自建底座 + 注入进程启动面（测试用；恢复持久化关闭）。
    ///
    /// 与 [`Self::with_adapter_config`] 同理：底座是新建的，单例凭证必然由这次领取。
    pub fn with_spawner(spawner: Arc<dyn ProcSpawner>) -> Self {
        Self::with_substrate_and_spawner(
            Arc::new(SubstrateState::with_adapter_config(&AdapterConfig::default())),
            AdapterConfig::default(),
            spawner,
        )
        .unwrap_or_else(|error| panic!("[tauron] 自建底座装配失败：{error}"))
    }
}

/// 插件形态 → 投递实现键。
///
/// **投递侧与结算侧共用**这一个映射：两处各写一遍 `match` 就会漂移，届时
/// 「投递走 Js、结算走 Unwired」这种半通半断的链路没有任何测试能提前发现。
fn delivery_kind_of(plugin_type: PluginType) -> DeliveryKind {
    match plugin_type {
        PluginType::Js => DeliveryKind::Js,
        PluginType::Process => DeliveryKind::Process,
        PluginType::Rust => DeliveryKind::Native,
        PluginType::Wasm => DeliveryKind::Wasm,
    }
}

/// [`PluginFlagSink`] 的注册表实现（插件运行时装配时注入底座）。
struct RegistryFlagSink {
    registry: Arc<Registry>,
}

impl PluginFlagSink for RegistryFlagSink {
    fn flag_snapshots(&self) -> Vec<(String, bool)> {
        self.registry.list_all().into_iter().map(|p| (p.id, p.disabled_by_safemode)).collect()
    }

    fn report_flag_event(&self, plugin_id: &str, enter_safemode: bool) -> Option<bool> {
        // 快照阶段已成功解析过同一个 id（确定性），这里再解析一次不会失败；
        // 真失败就当作注册表拒绝（计数 ignored），不 panic。
        let id = PluginId::new(plugin_id).ok()?;
        let event = if enter_safemode { Event::SafemodeEnter } else { Event::SafemodeExit };
        self.registry.report_event(&id, event).ok().map(|o| o.illegal)
    }
}

impl Default for PluginRuntimeState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod substrate_only_tests {
    use super::*;

    /// 方案 R1「验证」项的**运行期**证据：只有底座状态时，底座命令可用。
    ///
    /// 编译期证据同样在这个函数里——若底座命令的签名还能看见 `registry` /
    /// `contributes`，本函数就**编译不过**（`state.registry` 在 `&SubstrateState`
    /// 上不存在）。这不是约定，是类型系统。
    #[test]
    fn production_readiness_is_fail_closed_but_development_stays_compatible() {
        assert!(AdapterConfig::default().validate_production_readiness().is_ok());

        let prod = AdapterConfig::production();
        let err =
            prod.validate_production_readiness().expect_err("empty production config must fail");
        assert!(err.contains("caller identity"), "err={err}");
        assert!(err.contains("recovery"), "err={err}");

        let temp = tempfile::tempdir().unwrap();
        let mut ready = AdapterConfig::production();
        ready.origin_allowlist = vec!["tauri://localhost".into()];
        ready.recovery_data_dir = Some(temp.path().to_path_buf());
        ready.admin_audit_dir = Some(temp.path().join("audit"));
        #[cfg(feature = "plugin-install")]
        {
            // plugin-install 打开时，安装信任材料与可信时间也是生产必要条件。
            let mut keys = std::collections::BTreeMap::new();
            keys.insert("fixture-key".to_string(), vec![0x4b; 32]);
            ready = ready
                .with_plugin_install(temp.path().join("plugins"), keys, vec![0x5a; 32])
                .with_trusted_time_provider(Arc::new(tauron_host::SystemTimeProvider::new(
                    tauron_host::TimeTrustState::Trusted,
                )));
        }
        assert!(ready.validate_production_readiness().is_ok());
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn production_readiness_rejects_partial_install_trust_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AdapterConfig::production();
        cfg.origin_allowlist = vec!["tauri://localhost".into()];
        cfg.recovery_data_dir = Some(temp.path().to_path_buf());
        cfg.plugin_install_dir = Some(temp.path().join("plugins"));

        let err = cfg.validate_production_readiness().expect_err("partial trust must fail");
        // 失败面用 readiness 规范码：装了目录但没有签名密钥 → INSTALL_TRUST_REQUIRED；
        // 未注入可信时间源 → TRUSTED_TIME_REQUIRED。
        assert!(err.contains("INSTALL_TRUST_REQUIRED"), "err={err}");
        assert!(err.contains("TRUSTED_TIME_REQUIRED"), "err={err}");
    }

    #[test]
    fn substrate_only_state_serves_base_commands() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());

        // 底座命令：品牌桩 + 只读恢复查询，都不需要插件运行时。
        assert!(cmd_brand_info(&state).is_ok());
        let boot = cmd_recover_boot(&state).expect("recover_boot 应成功");
        assert!(boot.get("phase").is_some(), "恢复阶段必须可读");

        // i18n / 设置 / 通知等底座域命令同样在只有底座时可用。
        assert!(cmd_settings_get(&state, "k").is_ok());
        assert!(cmd_i18n_stats(&state).is_ok());

        // 插件侧写回口未注入 → 对账无事可做（空结果，而不是 panic 或静默失败）。
        assert!(state.plugin_flags.get().is_none(), "底座-only 装配不得注入插件侧写回口");
    }

    /// P1-10 回归：授权档位表自检必须是**生产调用**（构造状态就会跑），
    /// 且结果是可读的（不是只在 `#[cfg(test)]` 里存在的摆设）。
    ///
    /// 断链回归：`validate_command_registry` 此前只在测试里被调用——档位表写错了
    /// （空字段 / 重复命令）要等到有人写测试才暴露。现在构造任何底座状态都会跑它。
    #[test]
    fn authz_table_selfcheck_is_wired_into_production_construction() {
        // 构造一次底座即触发自检（panic 就不会走到这里）。
        let _ = SubstrateState::with_adapter_config(&AdapterConfig::default());
        let checked = authz_table_selfcheck();
        assert!(checked.is_ok(), "内置授权档位表必须自洽（§8-17）：{:?}", checked.as_ref().err());
        // 自检对象确实是 canonical 的命令面（不是空表通过）。
        assert!(tauron_host::authz::COMMANDS.len() >= 15);
        assert!(!tauron_host::authz::ADMIN_COMMANDS.is_empty());
    }

    /// 插件运行时装配会注入写回口，且**共享同一份底座**（不是两份状态）。
    #[test]
    fn plugin_runtime_shares_one_substrate_and_injects_sink() {
        let plugin = PluginRuntimeState::new();
        assert!(plugin.substrate.plugin_flags.get().is_some(), "多插件装配必须注入恢复对账写回口");
        // Deref：从底座侧写入，插件运行时侧立刻可见——同一份状态，不是拷贝。
        // 设置走 `SettingsStore`（R7-2），故这里用 Store 的 API 而不是裸 map。
        plugin.substrate.settings.lock().set_layer(
            HOST_SETTINGS_NAMESPACE,
            tauron_settings::LayerKind::Builtin,
            serde_json::json!({"probe": 1}),
        );
        assert_eq!(
            plugin.settings.lock().get_key(HOST_SETTINGS_NAMESPACE, "probe").unwrap(),
            Some(serde_json::json!(1)),
            "插件运行时与底座必须是同一份状态（同一 Arc，不是拷贝）"
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 命令调用主体与身份判定（R7 收口）
// ──────────────────────────────────────────────────────────────────────────
//
// **背景（缺口原文）**：R4 的身份模型是「`self` 档命令的 `pluginId` 一律不由
// 调用方传入，由宿主从 webview label 解析」。它已经落在插件面命令上
// （`authz::resolve_self_identity` / `resolve_principal` 用于
// `host_lifecycle_report` / `host_plugin_call` / events / streams），**但没有
// 落在设置族与特权命令上**。后果是「特权 = 仅主窗」这句话此前只在**部署配置**
// （origin 白名单 / Tauri ACL）里成立，代码里没有任何判定——`host_registry_admin`
// / `host_registry_list_all` / `host_runtime_spawn` / `host_runtime_health` 的包装器
// 直接转调，不解析身份；`host_settings_*` 更是任何窗口都能读写任何键。
//
// 本节把那条判定变成**代码**：主体一律从 label 解析（复用
// [`tauron_host::authz::resolve_principal`]，不另造一套解析），畸形 label 一律
// 拒绝（绝不降级成主窗），特权命令仅主窗，设置族按身份绑定键空间。
//
// 判定入口都放在无 tauri 依赖的这一层：`tauron-adapter` 的底座必须能在没有
// Tauri 运行时的环境里被测试（单测构造不出 `WebviewWindow`），所以「解析 label →
// 判定 → 转调」里只有**第一跳**（`window.label()`）留在 wire 层。

/// 命令调用主体。
///
/// 由 wire 层从 `WebviewWindow` 的 label 解析得到，再交给 `*_as` 系列命令核心判定。
/// **没有 `Invalid` 变体**：畸形 label 在 [`Caller::from_label`] 就被拒绝，
/// 构造不出主体——这是刻意的，`authz::Principal::Invalid` 的注释要求调用方拒绝它，
/// 让它成为一个可继续传递的枚举值迟早会有人忘了判。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// 主窗 / 应用自身的非插件 webview（特权主体）。
    MainWindow,
    /// 插件 webview（label = `plugin-<合法 id>`），携带经校验的插件 id。
    Plugin(String),
}

impl Caller {
    /// 从 webview label 解析主体。
    ///
    /// 安全要点：[`tauron_host::authz::Principal::Invalid`]（`plugin-` 前缀但 id
    /// 非法）一律**拒绝**，绝不降级成 [`Caller::MainWindow`]——否则伪造一个畸形
    /// label 就能拿到主窗档权力（提权）。错误码复用既有的身份不符/越权码
    /// [`ErrorCode::E_AUTH_DENIED`]，**不新增错误码**。
    pub fn from_label(label: &str) -> HostResult<Self> {
        match tauron_host::authz::resolve_principal(label) {
            tauron_host::authz::Principal::MainWindow => Ok(Caller::MainWindow),
            tauron_host::authz::Principal::Plugin(id) => {
                Ok(Caller::Plugin(id.as_str().to_string()))
            }
            tauron_host::authz::Principal::Invalid(raw) => Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "webview label `{raw}` 不是合法身份（`plugin-` 前缀但 id 非法）；\
                     畸形 label 绝不降级成主窗"
                ),
            )),
        }
    }

    /// 是否为主窗主体。
    pub fn is_main_window(&self) -> bool {
        matches!(self, Caller::MainWindow)
    }

    /// 插件 id；主窗返回 `None`。
    pub fn plugin_id(&self) -> Option<&str> {
        match self {
            Caller::Plugin(id) => Some(id.as_str()),
            Caller::MainWindow => None,
        }
    }

    /// 诊断用的主体描述（错误消息里用）。
    fn describe(&self) -> String {
        match self {
            Caller::MainWindow => "主窗".to_string(),
            Caller::Plugin(id) => format!("插件 `{id}`"),
        }
    }
}

/// **主窗专属命令的代码层判定**（特权 / 管理面 / 迁移入口）。
///
/// 与 [`tauron_host::authz::ADMIN_COMMANDS`] 同源：表里登记为
/// [`tauron_host::authz::AuthTier::Privileged`] 的命令必须全部被本函数拒绝插件主体
/// （有测试逐条遍历该表守住这一点，防止表与代码漂移）。表里**未登记**的主窗管理面
/// 命令（`host_registry_list_all`）与新增的迁移入口用同一条判定，语义同为「仅主窗」。
pub fn require_main_window(caller: &Caller, command: &str) -> HostResult<()> {
    match caller {
        Caller::MainWindow => Ok(()),
        Caller::Plugin(_) => Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "宿主命令 `{command}` 是主窗专属（privileged 档），{} 无权调用；\
                 该判定在代码层执行（不只是 origin ACL / Tauri ACL）",
                caller.describe()
            ),
        )),
    }
}

/// **特权操作的授权 + 审计唯一咽喉点**（V4 轮 11 / Batch 0-3，F3）。
///
/// 为什么是一个函数而不是「每条命令各自 `require_main_window` + 各自记一条审计」：
/// 与 origin ACL 同理（见 `wire-gate` 的「判定必须落在唯一分发入口」）——N 个记录点
/// 必然漏，漏掉的那条既不会编译失败也没有门禁提示，只会静默不留痕。审计集与判定
/// 代码的对账由 [`tauron_host::admin_audit_required`] 的命令名集合 + 一条 wire-gate
/// 共同守住：表里的命令必须真的走本函数，走本函数的命令必须在表里。
///
/// 审计的记录范围是**授权判定**（谁在动用/试图动用特权），不是业务返回值：判定在
/// 副作用之前，因此被拒的尝试同样留痕（提权尝试是首要审计信号）。
pub fn admin_gate(state: &SubstrateState, caller: &Caller, command: &str) -> HostResult<()> {
    let verdict = require_main_window(caller, command);
    record_admin_audit(state, caller, command, &verdict);
    verdict
}

/// 写一条审计事实。未配置 sink、或命令不在审计集里时**零副作用**。
fn record_admin_audit(
    state: &SubstrateState,
    caller: &Caller,
    command: &str,
    verdict: &HostResult<()>,
) {
    if !tauron_host::admin_audit_required(command) {
        return;
    }
    let Some(sink) = state.admin_audit.as_ref() else {
        return;
    };
    let (outcome, error_code) = match verdict {
        Ok(()) => (tauron_host::AdminAuditOutcome::Allowed, None),
        Err(error) => (
            tauron_host::AdminAuditOutcome::Denied,
            serde_json::to_value(error.code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string)),
        ),
    };
    // 线形主体标识：与 authz 档位的 consumer 口径一致，但不带中文描述——审计记录
    // 要被别的机器读，标识必须是稳定 token。
    let (caller_id, plugin_id) = match caller {
        Caller::MainWindow => ("main-window".to_string(), None),
        Caller::Plugin(id) => (format!("plugin:{id}"), Some(id.as_str())),
    };
    sink.record(command, &caller_id, plugin_id, outcome, error_code.as_deref());
}

/// 插件设置的命名空间前缀（约定：`plugin:<插件 id>`）。
///
/// 与 TS 侧既有的键约定一致（例如 `plugin:p.theme`）。
pub fn plugin_settings_namespace(plugin_id: &str) -> String {
    format!("plugin:{plugin_id}")
}

/// **设置族的键空间判定**。
///
/// - 主窗：任意键（现状不变）；
/// - 插件：键必须落在**自己的**命名空间内——等于 `plugin:<自己 id>`，或以
///   `plugin:<自己 id>.` 开头；其余（别的插件的键、`host.*` 这类宿主级键）一律拒绝。
///
/// 边界是**显式比较**而不是 `starts_with` 前缀包含：后者会让 `plugin:p.a` 的插件
/// 写穿 `plugin:p.ab.x`（`plugin:p.a` 是 `plugin:p.ab` 的前缀）。判定的落点写成
/// 「剥掉自己的命名空间后，剩下的是空串或以 `.` 开头」。
pub fn require_settings_key_scope(caller: &Caller, key: &str) -> HostResult<()> {
    let Caller::Plugin(id) = caller else {
        // 主窗：任意键（含宿主级键）。
        return Ok(());
    };
    let ns = plugin_settings_namespace(id);
    let in_scope = match key.strip_prefix(ns.as_str()) {
        Some(rest) => rest.is_empty() || rest.starts_with('.'),
        None => false,
    };
    if in_scope {
        return Ok(());
    }
    Err(HostError::new(
        ErrorCode::E_AUTH_DENIED,
        format!(
            "设置键 `{key}` 不在{}自己的命名空间 `{ns}`（或 `{ns}.…`）内；\
             越界读写别的插件键 / 宿主级键一律拒绝",
            caller.describe()
        ),
    ))
}

/// **身份绑定参数的判定**（轮 11 第二批：`host_notify` / `host_i18n_load` /
/// `host_i18n_cleanup_plugin` / `host_recover_report`）。
///
/// # 为什么是"绑参数"而不是"整条命令主窗专属"
///
/// 这四条命令都**收一个 `pluginId` 参数**，而那个参数决定"这次操作算在谁头上"：
/// 通知的署名、文案的命名空间、失败预算与故障归因的归属。插件本来就需要以
/// **自己的名义**做这些事（发自己的通知 / 装自己的文案 / 报自己的状态），一刀切成
/// 主窗专属会把插件的正常功能一起关掉。所以判定落在**参数**上，与
/// [`require_settings_key_scope`] 同一形状：
///
/// - **主窗**：任意 `pluginId`，含 `None`（宿主自己就是这些操作的另一个合法主体，
///   现状不变）；
/// - **插件**：`Some(id)` 必须**等于自己**；`None` 一律拒绝——`None` 在这四条命令里
///   是**宿主命名空间 / 应用级上报**的语义，不是"省略"，插件代为主张就是冒名。
///
/// 与 [`tauron_host::authz::resolve_self_identity`] 同一条规则（self 档只认身份、
/// 不认入参），区别只在主体模型：那边以 label 为输入且**不认主窗**，这里以
/// [`Caller`] 为输入并按上面两档判定。
///
/// # 拒绝码与副作用
///
/// 复用既有的身份/越权码 [`ErrorCode::E_AUTH_DENIED`]（**不新增错误码**）。
/// 判定必须在**任何副作用之前**执行：越界时通知不入环形缓冲、别人的 bundle 不被清、
/// 恢复计数器与阶段一动不动（每条命令都有"零副作用"断言）。
pub fn require_self_plugin_scope(
    caller: &Caller,
    command: &str,
    claimed: Option<&str>,
) -> HostResult<()> {
    let Caller::Plugin(me) = caller else {
        // 主窗：任意 pluginId（含 None），与 R7 之前的现状一致。
        return Ok(());
    };
    match claimed {
        Some(id) if id == me => Ok(()),
        Some(id) => Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "`{command}` 的 pluginId `{id}` 不是调用方自己（`{me}`）：以别的插件或宿主的\
                 名义执行该操作一律拒绝（判定在任何副作用之前）"
            ),
        )),
        None => Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!(
                "`{command}` 未指明归属主体（`pluginId`（署名 / 命名空间）或 `id`（通知）\
                 缺省 = 宿主级命名空间 / 应用级上报），{} 不得代为主张；请传自己的 id（`{me}`）",
                caller.describe()
            ),
        )),
    }
}

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

/// `host_lifecycle_report`：上报生命周期事件（self 档）。
///
/// 身份从 webview label 解析，忽略入参 pluginId（§2.1）。
///
/// **事件必须落在 [`tauron_host::lifecycle::Event::PLUGIN_REPORTABLE`] 内**——
/// 越权事件（`Enable`/`SafemodeExit`/`Uninstall`/`InstallOk` 等）在
/// `Registry::lifecycle_report` 里以 `E_AUTH_DENIED` 拒绝。这道闸不在本函数里
/// 补一遍，是为了让**所有** self 档调用点共用同一份白名单。
///
/// 上报后做一次恢复阶段对账：插件自报启用后若当前处于安全模式且该插件非必需，
/// `SafemodeEnter` 会立刻把它标记为禁用——恢复判定优先于插件自己的上报。
pub fn cmd_lifecycle_report(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    event: Event,
) -> HostResult<TransitionOutcome> {
    let out = guard("lifecycle_report", || {
        state.registry.lifecycle_report(webview_label, claimed_id, event)
    })??;

    // D28 反向同步边：试启期间插件自报错误 → 注册表按 D28 回落
    // （`IncrementTrialFailure` + 打回 DISABLED + 置安全模式标志）。引擎若
    // 不知情、仍记它 `TrialEnable`，紧接着的对账会把「引擎判启用、注册表却
    // 禁用」当成不一致，补发 `SafemodeExit` 把刚打上的标志又清掉——形成
    // 「插件报错 → 标志被清 → 插件重入」的往返抖动，且两套试验预算各自为
    // 政。这里把试验失败同步进引擎，对账立即静默、预算保持同向。
    // 只补这一条动作：其余动作（重试预算等）引擎不建模，注册表自己闭环。
    if out.actions.contains(&tauron_host::lifecycle::Action::IncrementTrialFailure) {
        if let Ok(id) = tauron_host::authz::resolve_self_identity(webview_label, claimed_id) {
            state.recovery.lock().record_trial_failure(id.as_str());
        }
    }

    reconcile_recovery_phase(state);
    Ok(out)
}

/// `host_plugin_call`：插件调用自己的 C/D 后端（self 档）。
pub fn cmd_plugin_call(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    cmd: &str,
    args: serde_json::Value,
) -> HostResult<PendingCall> {
    guard("plugin_call", || {
        let id = tauron_host::authz::resolve_self_identity(webview_label, claimed_id)?;
        let call = state.registry.call_begin(&id, cmd, args)?;
        // A1 投递：把请求送到真正能执行它的一端（Js → 事件总线 request 通道）。
        // 未装配投递（Rust/Wasm 在 A4 之前、或 Process 在 A3 之前）落到 unwired，
        // 返回诚实的 `E_PLUGIN_TYPE_NO_RUNTIME` 而非假装成功。
        match state.deliver_call(&call)? {
            ProviderResult::Value(_) => Ok(call),
            ProviderResult::Unsupported(b) => {
                Err(HostError::new(ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME, b.reason))
            }
        }
    })?
}

/// `host_call_plugin`：跨主体调用（0.4-A1）。宿主主窗与插件→插件共用。
///
/// `caller` 是发起主体（`"main"` 或插件 id），**配额记在它名下**；`target` 是执行主体。
/// 返回 [`ProviderResult`]：`Value(call)` = 已投递待应答；`Unsupported` = 未装配投递
/// （无通路）——不假装成功，前端据此读到 `reason` 而不是拿到一个永不结束的 pending。
pub fn cmd_call_plugin(
    state: &PluginRuntimeState,
    caller: &str,
    target: &str,
    cmd: &str,
    args: serde_json::Value,
) -> HostResult<ProviderResult<PendingCall>> {
    cmd_call_plugin_with_parent(state, caller, target, cmd, args, None)
}

/// V4 A77 contextual cross-principal call. Legacy callers omit parent_call_id and become roots.
pub fn cmd_call_plugin_with_parent(
    state: &PluginRuntimeState,
    caller: &str,
    target: &str,
    cmd: &str,
    args: serde_json::Value,
    parent_call_id: Option<&str>,
) -> HostResult<ProviderResult<PendingCall>> {
    guard("call_plugin", || {
        let call = state.registry.call_begin_cross_with_parent(
            caller,
            target,
            caller,
            cmd,
            args,
            parent_call_id,
        )?;
        match state.deliver_call(&call) {
            Ok(ProviderResult::Value(value)) => Ok(ProviderResult::Value(value)),
            Ok(ProviderResult::Unsupported(body)) => {
                let _ = state.registry.call_cancel(&call.call_id);
                Ok(ProviderResult::Unsupported(body))
            }
            Err(error) => {
                let _ = state.registry.call_cancel(&call.call_id);
                Err(error)
            }
        }
    })?
}

/// `host_call_result`：执行方回填一次调用的结果（0.4-A1 的结算入口）。
///
/// 只有执行方（call.target）能回填：身份必须 == target，否则伪造结果被拒。
/// 结算后条目**保留**，等待发起方 [`cmd_call_take`] 取走。
pub fn cmd_call_result(
    state: &PluginRuntimeState,
    webview_label: &str,
    claimed_id: Option<&str>,
    call_id: &str,
    ok: bool,
    result: Option<serde_json::Value>,
    error_code: Option<String>,
) -> HostResult<PendingCall> {
    guard("call_result", || {
        let id = tauron_host::authz::resolve_self_identity(webview_label, claimed_id)?;
        // 只有执行方能回填：身份必须 == 该 call 的 target。
        let target = state.registry.call_target(call_id)?;
        if target != id.as_str() {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "插件 `{id}` 不是调用 `{call_id}` 的执行方（target=`{target}`），不得回填结果"
                ),
            ));
        }
        state.settle_call_for(&target, call_id, CallOutcome { ok, result, error_code })
    })?
}

/// `host_call_take`：发起方取走一次已结算的结果（0.4-A1 的回执取件）。
///
/// 只有发起方（call.caller）能取：插件身份必须 == caller，或主窗取自己发起的调用。
/// `Settled` 取走即删；`Pending` 返回副本、保留（调用方据此知道「还没好」）。
pub fn cmd_call_take(
    state: &PluginRuntimeState,
    webview_label: &str,
    // `claimed_id` 故意不使用：self 档命令的身份只从 webview label 派生
    // （防冒充），与 `host_call_result` 不同——后者用 `claimed_id` 是因为
    // `resolve_self_identity` 同时需要 label 与 claimed 来做插件身份校验。
    // 这里 caller 既可能是插件也可能是主窗，直接用 `resolve_principal` 即可。
    _claimed_id: Option<&str>,
    call_id: &str,
) -> HostResult<PendingCall> {
    guard("call_take", || {
        let principal = tauron_host::authz::resolve_principal(webview_label);
        let caller = match &principal {
            tauron_host::authz::Principal::Plugin(id) => id.as_str().to_string(),
            tauron_host::authz::Principal::MainWindow => "main".to_string(),
            tauron_host::authz::Principal::Invalid(_) => {
                return Err(HostError::new(
                    ErrorCode::E_AUTH_DENIED,
                    "非法身份主体，不得取走调用结果",
                ));
            }
        };
        // 只有发起方能取：caller 必须 == 该 call 的 caller。
        let pending = state.registry.peek_call(call_id)?;
        if pending.caller != caller {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "主体 `{caller}` 不是调用 `{call_id}` 的发起方（caller=`{}`），不得取走结果",
                    pending.caller
                ),
            ));
        }
        state.registry.take_call(call_id)
    })?
}

/// 调用归属键：pending 条目的 `plugin_id`（= 配额归属者 = 发起主体）口径。
///
/// 与 `cmd_call_take` 里的 caller 映射同源：插件 → 自己的 id，主窗 → `"main"`。
/// self 档命令的身份一律从 webview label 派生，不认调用方传入的身份字段。
fn call_owner_key(caller: &Caller) -> &str {
    match caller {
        Caller::Plugin(id) => id.as_str(),
        Caller::MainWindow => "main",
    }
}

/// `host_call_end`：stream 终帧确认（self 档）。
///
/// **归属校验（本轮补）**：只有该调用的归属主体（`pending.plugin_id`，即发起方）
/// 能结束它。此前本命令只收 `callId`、不做任何身份判定——任何插件一旦拿到或
/// 猜到别人的 callId，就能终结对方的 pending call 及其名下全部流（跨插件越权 +
/// 拒绝服务）。这与 `COMMANDS` 表里 `host_call_end` 登记为 `self` 档（"只能作用于
/// 调用者自己"）的契约相矛盾，故补上与 [`cmd_call_take`] 同一形状的归属判定。
pub fn cmd_call_end(
    state: &PluginRuntimeState,
    caller: &Caller,
    call_id: &str,
) -> HostResult<PendingCall> {
    guard("call_end", || {
        let owner = call_owner_key(caller);
        let pending = state.registry.peek_call(call_id)?;
        if pending.plugin_id != owner {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "调用 `{call_id}` 的归属主体是 `{}`，`{owner}` 不得结束它",
                    pending.plugin_id
                ),
            ));
        }
        state.registry.call_end(call_id)
    })?
}

/// `host_cancel`：取消 pending call（self 档）。
///
/// **归属校验（本轮补）**：与 [`cmd_call_end`] 同一判据——只有发起方能取消自己的
/// 调用。缺此判定时 `host_cancel` 会成为"任意插件终止任意插件调用"的越权入口
/// （`E_AUTH_DENIED` 在**任何副作用之前**返回，被取消的调用与流纹丝不动）。
pub fn cmd_cancel(state: &PluginRuntimeState, caller: &Caller, call_id: &str) -> HostResult<()> {
    guard("cancel", || {
        let owner = call_owner_key(caller);
        let pending = state.registry.peek_call(call_id)?;
        if pending.plugin_id != owner {
            return Err(HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!(
                    "调用 `{call_id}` 的归属主体是 `{}`，`{owner}` 不得取消它",
                    pending.plugin_id
                ),
            ));
        }
        state.registry.call_cancel(call_id)
    })?
}

// ────────────────────────────────────────────────────────────────
// 流式帧（R5 / P0-1）
//
// 这三条命令是「流式」从**形状**变成**通路**的地方。在此之前：
// `plugin_call` 的 `kind: 'stream'` 会被校验、`call_end` 的注释也叫「终帧确认」，
// 但没有任何路径能把帧送出去——`channel` 只当不透明 JSON 收下，写完就丢，
// 前端表现为「回调永不触发、也不报错」。三命令 + `Registry::stream_*` 补上这条链。
// ────────────────────────────────────────────────────────────────

/// `host_stream_open` 的返回：新句柄 + 归属调用。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOpened {
    pub stream_id: String,
    pub call_id: String,
}

/// `host_stream_open`：为一次**已挂帧载体**的调用开流（self 档）。
///
/// 调用侧必须带 `channel` 调 `host_plugin_call`：没有载体的调用在这里以
/// `E_CALL_NOT_FOUND` 显式失败，而不是开一条「帧进虚空」的流（那是最难查的静默断链）。
pub fn cmd_stream_open(
    state: &PluginRuntimeState,
    subscriber: &str,
    call_id: &str,
) -> HostResult<StreamOpened> {
    guard("stream_open", || {
        let stream_id = state.registry.stream_open(call_id, subscriber)?;
        Ok(StreamOpened { stream_id, call_id: call_id.to_string() })
    })?
}

/// `host_stream_write`：写一帧 `data`（self 档）。
///
/// `seq` 由宿主铸（不接受前端传入，与 pending call 的 `seq` 同规矩）：前端自报
/// 序号就能伪造乱序/重复，接收方的去重假设随即失效。
pub fn cmd_stream_write(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    args_json: Option<serde_json::Value>,
    args_raw: Option<Vec<u8>>,
) -> HostResult<StreamFrame> {
    guard("stream_write", || {
        state.registry.stream_write(stream_id, subscriber, args_json, args_raw)
    })?
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCredit {
    pub stream_id: String,
    pub credit_bytes: usize,
}

pub fn cmd_stream_grant(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    bytes: usize,
) -> HostResult<StreamCredit> {
    guard("stream_grant", || {
        let credit_bytes = state.registry.stream_grant(stream_id, subscriber, bytes)?;
        Ok(StreamCredit { stream_id: stream_id.to_string(), credit_bytes })
    })?
}

/// `host_stream_close`：发终帧并使句柄失效（self 档）。
///
/// `kind` 只接受 `end` / `error`（闭集）：`data` 不是终帧，用它关流会让接收方
/// 以为流还在继续。
pub fn cmd_stream_close(
    state: &PluginRuntimeState,
    subscriber: &str,
    stream_id: &str,
    kind: &str,
) -> HostResult<StreamFrame> {
    let parsed = StreamKind::parse(kind).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("未知帧种类 `{kind}`（合法值：data | end | error）"),
        )
    })?;
    guard("stream_close", || state.registry.stream_close(stream_id, subscriber, parsed))?
}

// ────────────────────────────────────────────────────────────────
// 进程插件运行时（P0-2）
//
// 这两条命令是 `PluginType::Process` 从「四类枚举里的一个值」变成**可执行**的
// 地方。在此之前 `tauron-proc`（1780 行、配置/校验/心跳/崩溃追踪齐备）是孤儿
// crate：没有任何 crate 依赖它，于是 `PluginType::Process` 在宿主侧没有任何入口，
// 插件只能"声明自己是 C 类"而永远跑不起来。
//
// 域归属：**插件运行时域**（依赖注册表与插件状态），只能出现在
// `tauron_plugin_handler!`，绝不进 `tauron_substrate_handler!`。
// ────────────────────────────────────────────────────────────────

/// `host_runtime_spawn`：为 **process 型**插件启动 sidecar 并铸租约。
///
/// 语义（依次）：
/// 1. 插件必须存在（否则 `E_UNKNOWN_PLUGIN`）；
/// 2. 插件类型必须是 `Process`，否则 `E_PLUGIN_TYPE_NO_RUNTIME`——**诚实失败**：
///    不为 js/rust/wasm 插件伪造 pid，也不静默返回一个"看起来成功"的句柄；
/// 3. 插件必须**可用**（`ENABLED` / `RUNNING`），否则 `E_PLUGIN_DISABLED`
///    （见下方"可用性门"）；
/// 4. 缺少可信验签材料（签名 / sha256 / ABI）时拒绝启动（§4.7 硬约束）；
/// 5. 同一插件**已有活租约 → 原样返回既有 lease**（幂等，理由见
///    `Registry::runtime_ensure_lease`）：只有一种行为，不出现"有时返回旧的、
///    有时报错"的二义；
/// 6. 需要（重新）启动且崩溃窗口超限 → `E_PLUGIN_DISABLED` 并落到
///    `ERRORED_USER_CONFIRM`（见下方"崩溃预算"）；
/// 7. **真的起了新进程**时投递既有事件 `ATTACH`，把状态推到 `RUNNING`
///    （进程插件没有 webview 可上报，只能宿主自己投；幂等返回既有租约时不投）。
///    见 [`attach_after_spawn`]。
///
/// # 可用性门（P0-2）
///
/// **一条规则**：注册表状态不是 `ENABLED` / `RUNNING` 就不许起进程。判定直接用
/// 既有的 `PluginEntry::is_active()`（`call_begin` 的即时拒绝用的是同一个谓词——
/// 不新造状态、不新造谓词，两处语义因此不可能漂移）。
///
/// 为什么把"崩溃预算的人工闸门"并进这一条：`ERRORED_USER_CONFIRM` 之所以被拒，
/// 正是因为它不 active；**人工确认闸门由本门覆盖，不再是独立分支**——两条部分
/// 重叠的规则迟早会有人只改一处。
///
/// 由此得到的**调用顺序**（宿主 UI / 恢复流程必须遵守）：
/// **先让状态可用（`host_registry_admin` 的 `enable`，或安全模式下的
/// `host_recover_trial_enable`），再 spawn。** 各场景：
/// - `INSTALLED`（装完没启用）→ 被拒：先 enable；
/// - safemode 期间（`DISABLED` + `disabledBySafemode`）→ 被拒：先
///   `host_recover_trial_enable`（试验性启用）或用户确认后的 enable；
/// - 崩溃 → `ERRORED_RETRYABLE` / `ERRORED_USER_CONFIRM` → 被拒：先 enable
///   （状态机的重试预算决定 enable 会落到 `ENABLED` 还是再次 `ERRORED_USER_CONFIRM`）。
///
/// 反向保证（在 `tauron-host` 侧，不在本命令里）：状态一旦离开 `ENABLED`/`RUNNING`
/// （禁用 / 安全模式对账 / 卸载 / 错误迁移），**仍在跑的 sidecar 会被终止**——
/// 所以本门不是"只管住新的 spawn、老的进程照跑"的半截门。
///
/// # 崩溃预算
///
/// 可用性门之外还有一道**预算**门（二者不重叠：一个管状态、一个管次数）：
/// `ProcRuntime::is_crash_exceeded`（`CrashLimit` 定稿值 3 次 / 5min）为真即拒绝，
/// 并投 `Event::ErrorFatal` 把插件落到 `ERRORED_USER_CONFIRM`（见
/// [`refuse_exhausted_crash_budget`]）。只在**需要启动新进程**时判定：已有活租约的
/// 调用是纯查询语义（把句柄交回去），不该被预算拦下。
pub fn cmd_runtime_spawn(
    state: &PluginRuntimeState,
    plugin_id: &str,
    profile: &RuntimeSpawnProfile,
) -> HostResult<RuntimeHandle> {
    guard("runtime_spawn", || {
        if state.deployment_mode == tauron_host::DeploymentMode::Production
            && !matches!(
                state.proc_runtime.sandbox_descriptor().enforcement,
                tauron_proc::ProcessSandboxEnforcement::Hard
            )
        {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                "production runtime spawn requires hard ProcessSandboxProvider enforcement",
            ));
        }

        let id = PluginId::new(plugin_id)?;
        // 条目快照在 `runtime` 锁**之外**取（锁序 `pending → streams → runtime`；
        // `entries` 不参与该链，先取完再进 runtime 域，绝不反向）。
        let entry = state.registry.require(&id)?;

        if entry.manifest.plugin_type != PluginType::Process {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME,
                format!(
                    "插件 `{id}` 的类型是 `{}`，没有进程执行器（只有 `process` 型插件可 spawn）。\
                     这不是配置问题：该类型需要自己的执行器（js → webview 运行时 / rust → 宿主内 / wasm → 待接线）",
                    entry.manifest.plugin_type
                ),
            ));
        }

        // 可用性门：状态必须 ENABLED / RUNNING（既有谓词，不另造）。
        // 复用 `PluginEntry::is_active()`——它的实现就是 `Enabled | Running`，且
        // `call_begin` 的即时拒绝用的是同一个谓词（两处语义不可能漂移）。
        // 注意耦合：该谓词的文档讲的是"占用活跃身份槽位"，若哪天它的定义被改动
        // （例如把 INSTALLED 也算进去），这道门必须跟着重新审视——这就是不新造一个
        // 平行谓词换来的代价。
        // 状态**重新读一次**再判定：上面那份快照只用于类型与 sidecar，中间的
        // 校验/锁等待期间插件可能已被禁用——按陈旧快照放行就会绕过这道门。
        let live = state.registry.require(&id)?;
        if !live.is_active() {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_DISABLED,
                format!(
                    "插件 `{id}` 当前状态 `{}` 不可用（只有 `ENABLED` / `RUNNING` 才允许有 sidecar）：\
                     拒绝启动进程；请先让状态可用（`host_registry_admin` 的 `enable`，\
                     安全模式下用 `host_recover_trial_enable`）再 spawn",
                    live.state.state
                ),
            ));
        }

        // 缺省二进制 = manifest 声明的 sidecar（§4.2：C 类必须声明 `entry.sidecar`，
        // install 期已强校验，因此这条兜底分支**在实践中不可达**——保留它是为了
        // 「不 panic、不猜路径」：manifest 规则若放宽，这里会变成真路径而不是 panic 点。
        let binary_path = profile
            .binary_path
            .clone()
            .filter(|p| !p.trim().is_empty())
            .or_else(|| entry.manifest.entry.sidecar.clone())
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!(
                        "插件 `{id}` 未声明 `entry.sidecar`，`profile.binaryPath` 也为空：\
                         拒绝启动一个来源不明的二进制"
                    ),
                )
            })?;

        let mut cfg = profile.to_spawn_config(&binary_path);
        // 运维注入的环境变量在**这里**落地（`ClientConfig.env_overrides` 的落点）：
        // 放在 `to_spawn_config` 之后、校验与启动之前，因此它既参与 §4.7 的启动前
        // 校验，也确实到达子进程（`CommandSpawner` 的 `.envs()`）。
        apply_host_env_overrides(&mut cfg.env, &state.plugin_env_overrides);
        // spawn 前校验：签名 / sha256 / ABI。校验放在判定租约**之前**——
        // 不合格的载荷连"是否已有租约"都不该被它探测到（也就不会伪造出幂等假象）。
        validate_spawn_config(&cfg).map_err(proc_error_to_host)?;

        // ABI 契约比对（`tauron_proc::validate_abi` 的生产调用点）：
        // expected = 宿主当前支持的 sidecar ABI 契约（框架常量；前端可从
        // `@tauron/host` 的 `SIDECAR_ABI_CONTRACT` 取到同一份值）；
        // actual   = 调用方在 `profile.abi` 里声明的指纹。
        // 不符 → `E_ABI_MISMATCH`（**不是** `E_INSTALL_FAILED`：ABI 不匹配是版本
        // 兼容问题，调用方该升级插件/宿主，而不是重装）。
        //
        // 诚实边界：`profile.abi` 是调用方自报的，本校验挡的是"配置错配 / 前端用了
        // 旧模板"，不是"恶意调用方伪造 ABI"——后者需要 sidecar 在 RPC 握手时自报
        // 指纹（尚未实现）。与 `validate_spawn_config` 同属"配置一致性"层。
        validate_abi(&current_abi_contract(), &cfg.abi).map_err(proc_error_to_host)?;

        // 预算门（只拦"需要启动新进程"的调用；已有活租约是纯查询语义）。
        // 状态侧已由上面的可用性门覆盖（`ERRORED_USER_CONFIRM` 也不 active）。
        if state.registry.runtime_needs_restart(&id)
            && state.proc_runtime.is_crash_exceeded(plugin_id)
        {
            return Err(refuse_exhausted_crash_budget(state, &id, plugin_id));
        }

        // `started` 是**锁内**判定出来的"本次真的起了新进程"（幂等返回既有活租约
        // 时为 false），据此决定要不要把状态推到 RUNNING——见 `attach_after_spawn`。
        let (handle, started) = state.registry.runtime_ensure_lease(&id, || {
            state.proc_runtime.spawner().spawn(&cfg).map(|p| p.pid).map_err(proc_error_to_host)
        })?;
        if started {
            attach_after_spawn(state, &id)?;
            // 0.4-A1：真起了新进程就注册 stdout 帧接收器，闭合「宿主 → sidecar →
            // 回帧 → 结算」链路。sidecar 回帧（含 `callId`）经读线程路由到这里，
            // 由 `ProcessFrameSinkImpl` 调 `settle_call`。幂等：重复注册只是覆盖。
            // 返回 `false` = 进程在读线程侧已 EOF（注册被拒绝以防泄漏表条目）：
            // 此时 stdin 登记同样已被清掉，后续 `write_frame` 会以 `NotFound`
            // 如实失败——无需在此造错误，死亡探测路径会回收租约。立即退出的
            // sidecar 走这条竞态属正常，不视为错误。
            let registered = state.proc_runtime.spawner().register_frame_sink(
                handle.pid,
                Arc::new(crate::process_delivery::ProcessFrameSinkImpl::new(
                    state.registry.clone(),
                )),
            );
            let _ = registered; // 迟到 EOF：死亡探测兜底，见上注释
        }
        Ok(handle)
    })?
}

/// `host_runtime_spawn` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 判定必须在启动面**之前**：本命令会按调用方给的 `plugin_id` 启动一个可执行
/// 文件。若插件 webview 可调用，任何插件都能启动别的插件的 sidecar（越权执行
/// 原语）——拒绝路径**一次都不许到达启动面**（有测试用 fake spawner 断言
/// `call_count == 0`）。
pub fn cmd_runtime_spawn_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
    profile: &RuntimeSpawnProfile,
) -> HostResult<RuntimeHandle> {
    admin_gate(&state.substrate, caller, "host_runtime_spawn")?;
    cmd_runtime_spawn(state, plugin_id, profile)
}

/// 真起了新进程之后，把插件从 `ENABLED` 推到 `RUNNING`（既有迁移 `ATTACH`）。
///
/// **为什么必须由宿主投**：js/rust 插件的 `ATTACH` 由 webview 上报，进程插件
/// **没有 webview**——宿主要是也不投，注册表就会停在 `ENABLED` 而进程已经在跑
/// （UI 显示"已启用"、事实是"运行中"），状态机与事实不一致。这条与可用性门合起来
/// 才是闭环：可用状态才能起进程（`ENABLED`/`RUNNING`），起了进程状态就变 `RUNNING`。
///
/// **只在真起进程时投**（幂等返回既有活租约那条路径不投）：那种情况状态本来就是
/// `RUNNING`，再投一次是无意义的重复事件。`started` 由
/// [`tauron_host::registry::Registry::runtime_ensure_lease`] 在锁内判定，避免并发
/// spawn 时两边都以为自己起了进程而各投一次。
///
/// **非法迁移不伪造事件**：唯一现实入口是"状态已经是 `RUNNING`"（`RUNNING + ATTACH`
/// 在迁移表里没有规则，例如插件自报恢复并先自己 `ATTACH` 过，随后宿主才发现旧租约已崩
/// 并换了新进程）。此时**目标态已达成**，保持原状即正确；状态机把这次非法迁移计进
/// `PluginState::illegal_transitions`（`host_registry_list` 可查），因此不静默——
/// 但这里**不会**为了"让状态好看"去伪造事件或直接改状态。
///
/// 唯一的错误路径是**条目已消失**（启动期间被并发卸载/清除）。那不是可忽略的噪声：
/// 刚铸的租约指向一个已经不存在的插件，进程再没人管——于是**回收刚铸的租约**（终止
/// 进程）后如实返回错误，而不是把一个孤儿句柄交给调用方。
fn attach_after_spawn(state: &PluginRuntimeState, id: &PluginId) -> HostResult<()> {
    match state.registry.report_event(id, Event::Attach) {
        Ok(_) => Ok(()),
        Err(e) => {
            let reclaimed = state.registry.runtime_remove(id);
            Err(HostError::new(
                e.code,
                format!(
                    "进程已启动，但插件条目在启动期间消失（并发卸载/清除），无法登记为 RUNNING：{}；\
                     已回收刚铸的租约（{}）",
                    e.message,
                    match reclaimed {
                        Some(h) => format!("pid {}", h.pid),
                        None => "租约已不在表中".to_string(),
                    }
                ),
            ))
        }
    }
}

/// 崩溃窗口超限 → 拒绝启动，并把插件落到 `ERRORED_USER_CONFIRM`（"需用户确认"）。
///
/// **为什么复用 `E_PLUGIN_DISABLED`**（不新增错误码）：这个变体的既有语义就是
/// "插件当前不可用，且只有宿主显式放行才能改变"（ADR-05：`enabled` 标志做即时
/// 拒绝）。崩溃预算耗尽正是同一件事的另一条成因，调用方的下一个动作也一样
/// （去主窗确认/启用，而不是立刻重试）——因此不该造第二个含义相同的码。同时它
/// **不可重试**（不在 `ErrorCode::retryable()` 里），正好保证框架层不会自动重试
/// 一个已经明确要求人工介入的失败。0.4 审计后 `ProcError` 不再承载崩溃预算变体
/// （孤儿 `ProcRunner` 已删），adapter 的 `CrashTracker` 超限**直接**产
/// `E_PLUGIN_DISABLED`，语义同源。
///
/// **落点复用 `Event::ErrorFatal`**（不新增事件）：既有迁移表里
/// `ENABLED/RUNNING + ERROR_FATAL → ERRORED_USER_CONFIRM`，语义就是"这个插件坏到
/// 不能再自动重试了，交给人"——正是崩溃预算耗尽该有的下场。选它而不是
/// `RetryExhausted`，是因为后者只有 `ERRORED_RETRYABLE` 一个来源，而本函数只在
/// 插件**仍可用**（`ENABLED`/`RUNNING`，可用性门已放行）时才会被调用，投
/// `RetryExhausted` 会成为非法迁移、落点只剩注释里的说法。副作用也正确：
/// `ErrorFatal` 带 `IncrementFailure`（"又失败一次"该记的账）。
/// 唯一的另一条分支是**试验期**（`TrialActive` 守卫）：此时落到 `DISABLED` +
/// 安全模式标志（D28 回落）而不是 `ERRORED_USER_CONFIRM`——这里如实报出实际落点，
/// 不假装落到了 `ERRORED_USER_CONFIRM`。
fn refuse_exhausted_crash_budget(
    state: &PluginRuntimeState,
    id: &PluginId,
    plugin_id: &str,
) -> HostError {
    let crashes = state.proc_runtime.crash_count(plugin_id);
    let limit = state.proc_runtime.crash_limit();
    let landing = match state.registry.report_event(id, Event::ErrorFatal) {
        Ok(outcome) if !outcome.illegal => format!("已落到 `{}`", outcome.to),
        // 非法迁移：状态机已计数（`illegal_transitions`）。两种子情况分开说明。
        Ok(_) => match state.registry.find(id).map(|e| e.state.state) {
            Some(landed) if landed == LifecycleStateName::ErroredUserConfirm => {
                format!("已处于 `{landed}`（无需迁移）")
            }
            Some(current) => format!(
                "未能落到 `ERRORED_USER_CONFIRM`：当前状态 `{current}` 对 `ERROR_FATAL` \
                 没有规则（非法迁移已计数），需人工处理"
            ),
            None => "未能落到 `ERRORED_USER_CONFIRM`：插件条目已消失".to_string(),
        },
        Err(e) => format!("未能投递 `ERROR_FATAL`（{}）：需人工处理", e.code),
    };
    HostError::new(
        ErrorCode::E_PLUGIN_DISABLED,
        format!(
            "插件 `{id}` 在 {}s 崩溃窗口内已崩溃 {crashes} 次（上限 {}），拒绝再次自动启动：{landing}；\
             用户在主窗确认后（`host_registry_admin` 的 `enable`）可再次 spawn",
            limit.window_secs, limit.max_crashes
        ),
    )
}

/// `host_runtime_health`：按 lease 查询进程健康。
///
/// - 未知 / 已失效的 lease → `E_LEASE_EXPIRED`（**不是** `E_CALL_NOT_FOUND`：
///   这是租约语义的边界）；
/// - 进程不存活时，**首次**观测到才记一次崩溃：投递 `RuntimeCrash` 生命周期事件
///   并让恢复引擎的 `consecutiveFailures` +1（复用既有引擎与状态机，不新造计数）；
/// - 崩溃检测是**轮询式**的（没有后台监控线程）：不被调用的 health 不会发现死亡。
///
/// 返回里带**全局**的租约回收留痕（`reap`）：终止失败的痕迹必须能被宿主 UI 看到，
/// 否则"不静默吞"只是写在注释里。挂在 health 上而不是新开命令，是为了不再牵动
/// 能力表/授权档位/TS 镜像（`RuntimeHealth` 本就是这条命令的既有返回）。
fn runtime_health_report(
    status: tauron_proc::ProcessStatus,
    lifecycle: Option<tauron_host::lifecycle::State>,
    crashes: u32,
) -> tauron_host::HealthReport {
    match status {
        tauron_proc::ProcessStatus::Alive => match lifecycle {
            Some(tauron_host::lifecycle::State::Running) => {
                if crashes == 0 {
                    tauron_host::HealthReport::ready()
                } else {
                    tauron_host::HealthReport::degraded(format!(
                        "runtime is ready but has {crashes} recent crash(es) in its budget window"
                    ))
                }
            }
            Some(state) => tauron_host::HealthReport::alive_but_not_ready(format!(
                "process is alive but plugin lifecycle is {state}"
            )),
            None => tauron_host::HealthReport::alive_but_not_ready(
                "process is alive but plugin registry entry is missing",
            ),
        },
        tauron_proc::ProcessStatus::Exited => {
            tauron_host::HealthReport::dead("sidecar process has exited")
        }
        tauron_proc::ProcessStatus::Unknown => tauron_host::HealthReport::unknown(
            "sidecar liveness could not be proven by the process provider",
        ),
    }
}

pub fn cmd_runtime_health(state: &PluginRuntimeState, lease: &str) -> HostResult<RuntimeHealth> {
    guard("runtime_health", || {
        let entry = state.registry.runtime_lease(lease)?;
        let status = state.proc_runtime.spawner().status(entry.pid);
        let alive = matches!(status, tauron_proc::ProcessStatus::Alive);

        // Unknown is a degraded diagnostic state, not a crash: never consume crash budget unless
        // the process is positively observed Exited.
        if matches!(status, tauron_proc::ProcessStatus::Exited)
            && state.registry.runtime_mark_crashed(lease)?
        {
            state.proc_runtime.record_crash(&entry.plugin_id);
            deliver_runtime_crash(state, &entry.plugin_id);
        }

        let crashes = state.proc_runtime.crash_count(&entry.plugin_id);
        let lifecycle = PluginId::new(&entry.plugin_id)
            .ok()
            .and_then(|id| state.registry.find(&id))
            .map(|plugin| plugin.state.state);
        let health = runtime_health_report(status, lifecycle, crashes);

        Ok(RuntimeHealth {
            alive,
            status,
            pid: entry.pid,
            crashes,
            consecutive_failures: state.recovery.lock().counter().consecutive_failures,
            reap: state.registry.runtime_reap_stats(),
            health,
        })
    })?
}

/// `host_runtime_health` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 为什么同属特权：返回里有 `pid`（暴露宿主侧进程标识）且会驱动崩溃检测
/// （`record_crash` + 崩溃投递），插件主体不该能借此查询/扰动别的插件的进程。
pub fn cmd_runtime_health_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    lease: &str,
) -> HostResult<RuntimeHealth> {
    require_main_window(caller, "host_runtime_health")?;
    cmd_runtime_health(state, lease)
}

/// 主窗资源快照：展示每种有界资源的总占用/上限与逐插件占用。
/// 插件明细取自对应资源表本身，不维护第二份计数状态。
pub fn cmd_resource_stats(state: &PluginRuntimeState) -> HostResult<serde_json::Value> {
    guard("resource_stats", || {
        // 诊断读 = 回收点：过期 pending / 调用图边 / 准入令牌此前只在 pending
        // 触顶（begin_call）时被顺带回收；主窗若不轮询诊断，N 笔未结束的调用
        // 会占用内存直到重启。这里随读回收一次，快照也因此与回收事实一致。
        state.registry.gc_expired();
        // V7 §7/§9 的驱动腿：终止失败的 pid 在这里被再试一轮（有界，见
        // `RuntimeTable::retry_pending_reaps`）。刻意与 `gc_expired` 同址——
        // 主窗轮询诊断本来就是这个宿主的"心跳"，重试没有驱动方等于没有重试。
        state.registry.runtime_retry_pending_reaps();
        let plugins = state.registry.list_all();
        let plugin_ids: Vec<String> = plugins.iter().map(|plugin| plugin.id.to_string()).collect();
        let pending_used = state.registry.pending_len();
        let pending_limit = state.registry.config().max_pending_calls;
        let streams_used = state.registry.stream_active_total();
        // V7 §9 leak gate：代际台账读数（有界性必须能被宿主 UI 查到，不能只活在 Rust 测试里）。
        let generations = state.registry.runtime_generation_stats();
        // V7 §9 leak gate：回收留痕（含重试队列与终态证据）同样上线。
        let reap = state.registry.runtime_reap_stats();

        let subscriptions = state.bus.lock();
        let subscriptions_used = subscriptions.subscription_total();
        let subscription_usage: std::collections::BTreeMap<String, usize> = plugin_ids
            .iter()
            .map(|id| (id.clone(), subscriptions.subscription_count(id)))
            .collect();
        drop(subscriptions);

        let notifications = state.notify_store.lock();
        let notification_usage = notifications.group_counts();
        let notification_capacity = notifications.capacity();
        let notification_plugin_capacity = notifications.plugin_capacity();
        let notification_used = notifications.len();
        let notification_evictions = notifications.eviction_counts();
        let notification_evictions_total = notifications.eviction_total();
        let notification_counts: std::collections::BTreeMap<String, usize> = notification_usage
            .into_iter()
            .filter_map(|(key, count)| {
                key.strip_prefix("plugin:").map(|id| (id.to_string(), count))
            })
            .collect();
        drop(notifications);

        let plugin_rows: Vec<serde_json::Value> = plugin_ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "pluginId": id,
                    "pendingCalls": state.registry.pending_for(id),
                    "streams": state.registry.stream_active_for(id),
                    "subscriptions": subscription_usage.get(id).copied().unwrap_or_default(),
                    "notifications": notification_counts.get(id).copied().unwrap_or_default(),
                    "notificationEvictions": notification_evictions.get(id).copied().unwrap_or_default(),
                })
            })
            .collect();

        Ok(serde_json::json!({
            "global": {
                "pendingCalls": { "used": pending_used, "limit": pending_limit },
                "streams": { "used": streams_used, "limit": tauron_host::stream::MAX_STREAMS },
                "subscriptions": {
                    "used": subscriptions_used,
                    "limit": tauron_host::eventbus::MAX_SUBSCRIPTIONS
                },
                "notifications": {
                    "used": notification_used,
                    "limit": notification_capacity,
                    "perPluginLimit": notification_plugin_capacity,
                    "evictedTotal": notification_evictions_total
                },
                "generations": generations,
                "reap": reap
            },
            "perPlugin": {
                "pendingCallsLimit": tauron_host::registry::MAX_PENDING_PER_PLUGIN,
                "streamsLimit": tauron_host::stream::MAX_STREAMS_PER_PLUGIN,
                "subscriptionsLimit": tauron_host::eventbus::MAX_SUBSCRIPTIONS_PER_PLUGIN,
                "plugins": plugin_rows
            },
            "faults": {
                "settings": {
                    "state": state.settings_fault.lock().state(),
                    "generation": state.settings_fault.lock().generation(),
                    "lastFault": state.settings_fault.lock().last_fault().cloned()
                },
                // A91（轮 47）：三个子系统边界的可观测面。主窗据此判「有没有子域
                // 在拒绝服务、修复尝试得出了什么结论」，而不是等下一次命令报错。
                "events": {
                    "state": state.events_fault.lock().state(),
                    "generation": state.events_fault.lock().generation(),
                    "lastFault": state.events_fault.lock().last_fault().cloned()
                },
                "approval": {
                    "state": state.review_fault.lock().state(),
                    "generation": state.review_fault.lock().generation(),
                    "lastFault": state.review_fault.lock().last_fault().cloned()
                },
                "registry": {
                    "state": state.registry_fault.lock().state(),
                    "generation": state.registry_fault.lock().generation(),
                    "lastFault": state.registry_fault.lock().last_fault().cloned()
                }
            }
        }))
    })?
}

/// 资源诊断数据只允许主窗读取，避免暴露其他插件的活动量。
pub fn cmd_resource_stats_as(
    caller: &Caller,
    state: &PluginRuntimeState,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_resource_stats")?;
    cmd_resource_stats(state)
}

/// 崩溃的**投递**（两条既有通道，都不新造计数）：
///
/// 1. `Event::RuntimeCrash` → 注册表状态机（`plugin.state` 的唯一写入者）。
///    崩溃进程的插件据此离开 RUNNING/ENABLED 进入 ERRORED（或按 D28 回落
///    `disabled-by-safemode`），副作用是 `retry_count` / `trial_failures` 等
///    既有预算计数——**不新增计数器**；
/// 2. `RecoveryEngine::record_boot_failure(Some(plugin_id))` → 与
///    `host_recover_report` 的 failure 分支**同一条**引擎路径，因此
///    `consecutiveFailures` 与安全模式判定天然同源（不会出现两套判定各自为政）。
///
/// 之后做一次阶段对账 + 落盘：判定必须落到插件侧（`reconcile_recovery_phase`
/// 是唯一途径），计数必须跨进程可见（重启后安全模式才可能触发）。
///
/// 事件的非法迁移（例如插件停在 INSTALLED）由状态机计数而不报错——**不静默**：
/// `illegal_transitions` 是可读的，且 `consecutiveFailures` 照样 +1。
///
/// 唯一可能的投递失败是**插件条目已消失**（`report_event` 返回 `E_UNKNOWN_PLUGIN`，
/// 只可能发生在并发卸载把条目摘掉、租约尚未回收的窗口里）。此时状态机上已经没有
/// 可迁移的对象，因此这里**不**把 health 升格成错误：崩溃计数与租约事实已经确定，
/// 返回错误只会让调用方丢掉这些事实（失败面由"插件条目已消失"本身可见）。
fn deliver_runtime_crash(state: &PluginRuntimeState, plugin_id: &str) {
    if let Ok(id) = PluginId::new(plugin_id) {
        let _ = state.registry.report_event(&id, Event::RuntimeCrash);
    }
    state.recovery.lock().record_boot_failure(Some(plugin_id));
    reconcile_recovery_phase(state);
    persist_recovery_engine(state);
}

/// `ProcError` → 宿主结构化错误（跨 IPC 的错误码表）。
///
/// **映射是粗的**（错误码只有两位数的粒度），但方向是确定的，且 `message` 保留
/// 原始原因：
/// - 配置不合格 → `E_INVALID_MANIFEST`（调用方改载荷即可）；
/// - **ABI 契约不匹配 → `E_ABI_MISMATCH`**（独立成码，不与安装失败合并：这是
///   版本兼容问题，调用方该升级插件/宿主，而不是重装——前端据此才能给出正确提示）；
/// - 其余（启动失败 / 进程终止）→ `E_INSTALL_FAILED`。
///
/// 未映射成 `E_HOST_PANIC`：那会**谎报**“宿主 panic”并把它标成可重试
/// （`retryable() == true`），前端会拿一个确定性的启动失败去无限重试。
///
/// 0.4 审计：`ProcError` 的孤儿变体（验签/帧上限/心跳/并发/崩溃预算等零产生点）
/// 已随 `ProcRunner` 模拟器删除；adapter 自己的崩溃预算耗尽**直接**产
/// `E_PLUGIN_DISABLED`（`cmd_runtime_spawn` 内），不经 `ProcError` 中转。
fn proc_error_to_host(e: ProcError) -> HostError {
    let code = match &e {
        ProcError::InvalidSpawnConfig(_) => ErrorCode::E_INVALID_MANIFEST,
        ProcError::AbiMismatch { .. } => ErrorCode::E_ABI_MISMATCH,
        ProcError::SpawnFailed(_) | ProcError::ProcessTerminated(_) => ErrorCode::E_INSTALL_FAILED,
    };
    HostError::new(code, format!("进程执行器失败：{e}"))
}

/// 把恢复引擎状态落盘（**锁顺序 `recovery → recovery_store`，两把锁绝不重叠**，
/// 与 `cmd_recover_report` 同一写法）。
fn persist_recovery_engine(state: &SubstrateState) {
    let engine_snapshot = state.recovery.lock().to_json();
    state.recovery_store.lock().save_json(&engine_snapshot);
}

/// `host_events_publish`：发布事件（self 档）。
///
/// 使用可靠语义（`publish_request`）：溢出返回结构化错误。
pub fn cmd_events_publish(
    state: &SubstrateState,
    publisher: &str,
    topic: &str,
    payload: serde_json::Value,
) -> HostResult<PublishResult> {
    cmd_events_publish_with_causation(state, publisher, topic, payload, None)
}

/// V4 A78 event publication with an optional parent causation context.
pub fn cmd_events_publish_with_causation(
    state: &SubstrateState,
    publisher: &str,
    topic: &str,
    payload: serde_json::Value,
    causation: Option<&tauron_host::EventCausation>,
) -> HostResult<PublishResult> {
    run_events_boundary(state, "events_publish", || {
        let bus = state.bus.lock();
        bus.publish_request_with_causation(publisher, topic, payload, causation)
    })
}

/// `host_events_subscribe`：订阅事件（self 档）。
pub fn cmd_events_subscribe(
    state: &SubstrateState,
    subscriber: &str,
    window: &str,
    topic: &str,
) -> HostResult<SubscribeOutcome> {
    run_events_boundary(state, "events_subscribe", || {
        let bus = state.bus.lock();
        bus.subscribe(subscriber, window, topic)
    })
}

/// `host_events_unsubscribe`：退订事件（self 档）。
pub fn cmd_events_unsubscribe(state: &SubstrateState, token: &str) -> HostResult<()> {
    run_events_boundary(state, "events_unsubscribe", || {
        let bus = state.bus.lock();
        bus.unsubscribe(token)
    })
}

/// 一条 EventBus 私有 topic 审批事实。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventApproval {
    pub subscriber: String,
    pub topic: String,
}

/// 主窗批准插件订阅私有 topic。
pub fn cmd_events_approve_as(
    caller: &Caller,
    state: &SubstrateState,
    subscriber: &str,
    topic: &str,
) -> HostResult<()> {
    admin_gate(state, caller, "host_events_approve")?;
    if subscriber.trim().is_empty() || topic.trim().is_empty() {
        return Err(HostError::new(ErrorCode::E_AUTH_DENIED, "subscriber 与 topic 均不可为空"));
    }
    // `approve` 自身的拒绝（未声明 topic / 审批表满）原样上线——闸门只把 panic
    // 转成 `E_HOST_PANIC`，不吞业务错误。
    run_events_boundary(state, "events_approve", || state.bus.lock().approve(subscriber, topic))
}

/// 主窗撤销插件私有 topic 审批。幂等；返回是否真实删除。
///
/// A81：撤销即失效——该 `(subscriber, topic)` 的既有订阅一并退订、队列里该 topic 的
/// 待取帧一并作废（详见 `tauron_host::eventbus::EventBus::revoke`）。
pub fn cmd_events_revoke_as(
    caller: &Caller,
    state: &SubstrateState,
    subscriber: &str,
    topic: &str,
) -> HostResult<bool> {
    admin_gate(state, caller, "host_events_revoke")?;
    run_events_boundary(state, "events_revoke", || Ok(state.bus.lock().revoke(subscriber, topic)))
}

/// 主窗读取全部 EventBus 审批事实。
pub fn cmd_events_approvals_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<Vec<EventApproval>> {
    require_main_window(caller, "host_events_approvals")?;
    run_events_boundary(state, "events_approvals", || {
        Ok(state
            .bus
            .lock()
            .approvals()
            .into_iter()
            .map(|(subscriber, topic)| EventApproval { subscriber, topic })
            .collect())
    })
}

/// 主窗读取机器可读的生产就绪自检报告（V4 A109）。
///
/// 与启动门禁同源：底座的 `production_readiness` 在装配时由 [`AdapterConfig`] 推导；
/// 进程运行时事实以**实际注入**的 sandbox descriptor 为准重算（与
/// `validate_process_runtime_for_start` 同一判定），报告不会与启动路径漂移。
pub fn cmd_production_doctor_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<tauron_host::ProductionDoctorReport> {
    require_main_window(caller, "host_production_doctor")?;
    guard("production_doctor", || production_doctor_report(state))
}

/// 就绪报告的**唯一**装配点（`host_production_doctor` 与宿主启动后的自检共用）。
///
/// 轮 11 / F3：`admin-audit` 检查项的真值只来自审计 sink 的
/// [`tauron_host::AdminAuditFacts::healthy`]，报告同时把快照本身带上线（条数 /
/// 裁剪 / 写失败 / 链完整性）——读取侧因此可独立复核，而不必相信一个布尔位。
fn production_doctor_report(state: &SubstrateState) -> tauron_host::ProductionDoctorReport {
    let mut readiness = state.production_readiness.clone();
    if let Some(descriptor) = state.process_sandbox.get() {
        readiness.process_runtime_enabled = true;
        readiness.hard_process_sandbox_available =
            matches!(descriptor.enforcement, tauron_proc::ProcessSandboxEnforcement::Hard);
    }
    let audit = state.admin_audit.as_ref().map(|sink| sink.facts());
    readiness.audit_for_admin_operations_available =
        audit.as_ref().is_some_and(tauron_host::AdminAuditFacts::healthy);
    let mut report = tauron_host::production_doctor(state.deployment_mode, &readiness);
    report.admin_audit = audit;
    report
}

/// 按订阅者回收多选择器订阅的分组登记（§8-3 零悬挂）。
///
/// 两个必调点：**窗口关闭**（`cleanup_closed_window`）与**插件卸载**
/// （`cmd_registry_admin` 的 Uninstall/Purge）。两处都会先经
/// `dispose_subscriber` 清掉核心订阅——分组登记若不跟着回收，表里留下的
/// 全是指向已消亡 token 的死条目，随开/关窗循环无限累积。
/// 回收键与 wire 层的建组键同源（`plugin_id_of(label)`），主窗的分组也会
/// 在主窗销毁时随 label 原样值被回收。
pub fn prune_subscription_groups(state: &SubstrateState, subscriber: &str) {
    state.subscription_groups.lock().retain(|_, g| g.subscriber != subscriber);
}

/// `host_events_drain`：拉取本插件待投递帧（self 档）。
///
/// 这是订阅链路的**取件步骤**：`host_events_subscribe` 只是把帧排进
/// 每订阅者队列，前端必须周期性（或订阅后）调用本命令取走帧，
/// 否则队列只进不出。`kind` 为 `event` / `request` / `state`。
pub fn cmd_events_drain(
    state: &SubstrateState,
    subscriber: &str,
    kind: &str,
) -> HostResult<Vec<Frame>> {
    let parsed_kind = ChannelKind::parse(kind).ok_or_else(|| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("未知通道类型 `{kind}`（expected: event | request | state）"),
        )
    })?;
    run_events_boundary(state, "events_drain", || {
        let bus = state.bus.lock();
        bus.drain(subscriber, parsed_kind)
    })
}

// ──────────────────────────────────────────────────────────────────────────
// 设置（R7-2：走 `tauron-settings` 的 Store，激活孤儿 crate）
// ──────────────────────────────────────────────────────────────────────────

/// 宿主设置的命名空间（`tauron-settings` 的伪插件 id）。
///
/// 一个就够：宿主设置的键是开放集合（插件各自带自己的键），不需要 per-plugin
/// 命名空间隔离；`SettingsStore` 的四层合并与版本迁移在单命名空间内同样成立。
pub const HOST_SETTINGS_NAMESPACE: &str = "host.settings";

/// §33 R2-4：宿主设置**提交**（持久化成功之后）镜像到消息面的 topic，
/// publisher 为宿主（`"host"`）。**私有**：订阅前须经 `host_events_approve`
/// 审批（宿主设置单命名空间但键可能归属不同插件，见 SubstrateState 装配处注释）。
/// 帧载荷：`{key, value, source, revision}`（Event 通道：可丢、收敛到最新 revision）。
pub const HOST_SETTINGS_CHANGED_TOPIC: &str = "host:settings:changed";

/// 宿主设置文档的 **v1** schema 版本。
///
/// v1 的键是**裸键**（可以含 `.`，例如 `plugin:p.theme`），文档平铺存储。
/// 这正是 R7 之前裸 `HashMap` 的契约。
pub const HOST_SETTINGS_SCHEMA_V1: &str = "1.0.0";

/// 设置文档的落盘文件名（放在宿主数据目录下，与恢复标记同目录）。
pub const HOST_SETTINGS_FILE: &str = "host-settings.json";
const HOST_SETTINGS_DURABLE_SCHEMA: &str = "tauron.host-settings/2";

/// 从磁盘读回设置文档。
///
/// 返回 `Ok(None)` = 文件不存在（首次启动，不是错误）；`Err` = 文件存在但读不动
/// 或解析不了（**不静默当成空文档**——那会让用户以为设置还在，其实被清了）。
fn load_settings_doc(
    path: &std::path::Path,
) -> HostResult<Option<(Vec<(String, tauron_settings::PluginState)>, u64)>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("设置文档读取失败：{e}"),
            ));
        }
    };

    match tauron_host::decode_durable::<Vec<(String, tauron_settings::PluginState)>>(&bytes) {
        Ok(envelope) => {
            if envelope.schema != HOST_SETTINGS_DURABLE_SCHEMA {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("设置文档 durable schema 不支持：{}", envelope.schema),
                ));
            }
            Ok(Some((envelope.payload, envelope.generation)))
        }
        Err(durable_error) => {
            // One-way legacy reader: pre-V4 files were a raw Vec<(namespace, PluginState)>.
            // A corrupted V4 envelope cannot accidentally validate as this shape.
            match serde_json::from_slice::<Vec<(String, tauron_settings::PluginState)>>(&bytes) {
                Ok(entries) => Ok(Some((entries, 0))),
                Err(legacy_error) => {
                    let quarantined =
                        path.with_extension(format!("json.corrupt-{}", crate::recovery::now_ms()));
                    let quarantine_note = match std::fs::rename(path, &quarantined) {
                        Ok(()) => format!("；已隔离到 {}", quarantined.display()),
                        Err(error) => format!("；隔离失败：{error}"),
                    };
                    Err(HostError::new(
                        ErrorCode::E_INVALID_MANIFEST,
                        format!(
                            "设置文档完整性校验失败：durable={durable_error}; legacy={legacy_error}{quarantine_note}"
                        ),
                    ))
                }
            }
        }
    }
}

/// 设置文档读不回来、回滚镜像也救不了时的**如实记录**（A101 的降级出口）。
///
/// 生产档仍然 fail-closed：带着读不动的数据启动，等于把「设置还在」这个假象继续卖给用户。
/// 非生产档降级为空文档 + 日志。两条路径都把原因点名，有镜像时连镜像的失败一起报——
/// 「静默变空文档」和「只报一半原因」都是这条链上最常见的漏诊。
fn report_settings_load_failure(
    path: &std::path::Path,
    doc_error: &str,
    rollback_error: Option<&str>,
    mode: tauron_host::DeploymentMode,
) {
    let rollback_note =
        rollback_error.map(|error| format!("；回滚镜像也不可用：{error}")).unwrap_or_default();
    if mode == tauron_host::DeploymentMode::Production {
        panic!(
            "[tauron] production settings durable-state validation failed for {}: {}{rollback_note}",
            path.display(),
            doc_error
        );
    }
    eprintln!(
        "[tauron] 设置文档 {} 完整性失败，本轮以空文档降级启动：{}{rollback_note}",
        path.display(),
        doc_error
    );
}

/// 原子落盘：临时文件全量写 + `fsync`，再 rename 到位。
///
/// `label` 只进错误文案（`设置文档` / `设置回滚镜像`），两条落盘路径保持同一口径。
///
/// **必须在 `settings_write_lock` 之下调用**：临时名是 `<目标>.tmp`，不带 per-writer
/// 后缀，两个并发调用会踩同一个文件——Windows 上 `File::create` 直接 sharing violation，
/// 于是两次本该都成功的写变成两次都失败。换言之：**单写者不是性能选项，是这段代码的正确性前提**。
fn atomic_write_settings_file(path: &std::path::Path, bytes: &[u8], label: &str) -> HostResult<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("设置目录创建失败 {}：{e}", dir.display()),
            )
        })?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("{label}写入失败 {}：{e}", tmp.display()),
        )
    })?;
    std::io::Write::write_all(&mut file, bytes).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("{label}写入失败 {}：{e}", tmp.display()),
        )
    })?;
    file.sync_all().map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("{label}同步失败 {}：{e}", tmp.display()),
        )
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("{label}落位失败 {}：{e}", path.display()),
        )
    })?;
    Ok(())
}

/// 把设置文档写回磁盘（原子写：先写临时文件再 rename）。
///
/// **必须在 `settings_write_lock` 之下调用**（见 [`atomic_write_settings_file`]）。
///
/// **失败必须让调用方知道**：设置写成功但落盘失败，是"重启后设置消失"的根因。
/// 这里返回 `Err`，由 [`cmd_settings_set`] 冒泡给前端——不静默吞掉。
fn persist_settings_doc(state: &SubstrateState) -> HostResult<()> {
    let Some(path) = state.settings_path.as_ref() else {
        return Ok(());
    };
    let entries = state.settings.lock().snapshot_all();
    let next_generation = state.settings_generation.lock().saturating_add(1);
    let envelope =
        tauron_host::DurableEnvelope::seal(HOST_SETTINGS_DURABLE_SCHEMA, next_generation, entries)
            .map_err(|e| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("设置文档 durable envelope 构造失败：{e}"),
                )
            })?;
    let bytes = tauron_host::encode_durable(&envelope).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("设置文档 durable envelope 编码失败：{e}"),
        )
    })?;
    atomic_write_settings_file(path, &bytes, "设置文档")?;
    *state.settings_generation.lock() = next_generation;
    Ok(())
}

/// V4 A101：迁移**回滚镜像**的文件名（与正式文档同目录）。
pub const HOST_SETTINGS_ROLLBACK_FILE: &str = "host-settings.rollback.json";
const HOST_SETTINGS_ROLLBACK_SCHEMA: &str = "tauron.host-settings-rollback/1";

/// 回滚镜像的落盘路径；宿主没配数据目录时返回 `None`（纯内存宿主没有可回滚的事实）。
fn settings_rollback_path(state: &SubstrateState) -> Option<std::path::PathBuf> {
    state.settings_path.as_ref().map(|p| p.with_file_name(HOST_SETTINGS_ROLLBACK_FILE))
}

/// V4 A101：把**迁移前**的用户层落成磁盘回滚镜像。
///
/// 为什么必须有磁盘这一份：v1→v2 迁移的合同注释写着「保留快照到 probation/commit」
/// （见 [`install_host_settings_schema`]），而只存在于内存的快照一重启就没了——下一次
/// 启动面对的是迁移后的文档，没有任何东西能把它带回迁移前。`requiresSnapshot` 于是
/// 只是一个没人兑现的形容词。
///
/// 镜像带自己的 durable 信封（schema + generation + checksum），所以「恢复」这条路径
/// 消费的是**已验证**的数据，不是任意坏文件——这也是生产档位允许用它启动的唯一理由。
fn stage_settings_rollback_image(
    state: &SubstrateState,
    entries: &[(String, tauron_settings::PluginState)],
) -> HostResult<()> {
    let Some(path) = settings_rollback_path(state) else {
        return Ok(());
    };
    let generation = *state.settings_generation.lock();
    let envelope = tauron_host::DurableEnvelope::seal(
        HOST_SETTINGS_ROLLBACK_SCHEMA,
        generation,
        entries.to_vec(),
    )
    .map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("设置回滚镜像 durable envelope 构造失败：{e}"),
        )
    })?;
    let bytes = tauron_host::encode_durable(&envelope).map_err(|e| {
        HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("设置回滚镜像 durable envelope 编码失败：{e}"),
        )
    })?;
    atomic_write_settings_file(&path, &bytes, "设置回滚镜像")
}

/// 读回滚镜像。`Ok(None)` = 没有镜像（没迁过 / 已被消费）；`Err` = 镜像存在但校验不过
/// （此时**绝不**拿它恢复——半对的镜像比没有镜像更危险）。
fn load_settings_rollback_image(
    path: &std::path::Path,
) -> HostResult<Option<(Vec<(String, tauron_settings::PluginState)>, u64)>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("设置回滚镜像读取失败：{e}"),
            ));
        }
    };
    let envelope: tauron_host::DurableEnvelope<Vec<(String, tauron_settings::PluginState)>> =
        tauron_host::decode_durable(&bytes).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("设置回滚镜像完整性校验失败：{e}"),
            )
        })?;
    if envelope.schema != HOST_SETTINGS_ROLLBACK_SCHEMA {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("设置回滚镜像 schema 不支持：{}", envelope.schema),
        ));
    }
    Ok(Some((envelope.payload, envelope.generation)))
}

/// 作废回滚镜像（幂等：文件本就不存在也不报错）。
///
/// 两个作废点共用这一个出口，避免「一次性」语义在两处分叉：① 装配消费成功之后——在
/// `SubstrateState` 构造里按路径作废，那时还没有 `state`；② 迁移落盘失败并 rewind 内存
/// 之后。留着已消费的镜像，下一次偶然的读失败会把用户带回更久以前的状态；迁移失败后留着
/// 它，等于在磁盘上伪造一次没发生过的迁移。
///
/// 轮 17：**删不掉不等于不用管**。`let _ = remove_file(path)` 会把权限/占用/目标是个目录
/// 这类错误原样咽下，结果正是本函数要防的那件事——一份可被后续读回的陈旧镜像。
/// 镜像的可消费性来自**固定文件名**，所以删除失败就把它改名挪开（改名后
/// `load_settings_rollback_image` 再也找不到它），两条路都走不通时才留一行痕迹。
fn clear_settings_rollback_image(path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(remove_error) => {
            let name =
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let discarded =
                path.with_file_name(format!("{name}.discarded-{}", crate::recovery::now_ms()));
            if let Err(rename_error) = std::fs::rename(path, &discarded) {
                eprintln!(
                    "[tauron] 设置回滚镜像作废失败且挪开也失败（删除：{remove_error}；改名：{rename_error}）：\
                     它仍在 {} 上，后续读损坏时可能被当成回滚点消费",
                    path.display()
                );
            }
        }
    }
}

/// 宿主设置文档的**当前**（v2）schema 版本。
///
/// v2 的键是**单段转义**键（见 [`settings_path`]）：一键 = 一段 = 一叶。
pub const HOST_SETTINGS_SCHEMA_V2: &str = "2.0.0";

/// v1 与 v2 共用的文档 schema。
///
/// 两份都是 `additionalProperties: true` 的开放对象——宿主设置的键宿主编译期
/// 枚举不出来。版本号在这里承载的是**键的编码契约**（v1 裸键 / v2 转义键），
/// 不是字段集合；升版的目的是让「键编码变了」这件事可迁移、可审计，
/// 而不是把开放键集合假装成封闭的。
fn host_settings_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": true,
    })
}

/// 把一个设置键编码成 `SettingsStore` 的**单段**点路径。
///
/// 为什么必须转义：`SettingsStore` 用点路径寻址，`.` 是分隔符。既有键
/// （`plugin:p.theme`）含 `.`，直接透传会被拆成嵌套对象——
/// - 前缀键读取会从 `Null` 变成对象（平铺语义变了）；
/// - `a` 已是标量时再写 `a.b`，`merge::write_path` 的
///   「中间节点必须是对象」断言会 panic（被 `guard` 拦成 `E_HOST_PANIC`）。
///
/// 转义是**注入式**的（`%` 先转），因此可逆且不产生碰撞：
/// `a.b` → `a%2Eb`，而字面键 `a%2Eb` → `a%252Eb`。
///
/// 转义表：`%` → `%25`、`.` → `%2E`、`$` → `%24`
/// （`$` 是 `$unset` 的保留前缀，`validate_path` 禁止段以它开头）。
pub fn settings_path(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for ch in key.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '.' => out.push_str("%2E"),
            '$' => out.push_str("%24"),
            _ => out.push(ch),
        }
    }
    out
}

/// [`settings_path`] 的**逆**：把 Store 的编码点路径还原成调用方使用的线形键。
///
/// 为什么需要它：消息面（`host:settings:changed`）给订阅方的必须是线形键
/// ——他们拿这个键直接 `host_settings_get/set` 回查回写；给编码路径
/// （`plugin:p%2Etheme`）就成了「读得到、用不了」。编码是单字符→`%XX`，
/// 因此按 `%` 引导的三字符序列解码即可，无需回溯。
pub fn settings_wire_key(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        // 不完整/未知的 `%` 序列原样保留（编码产物只可能是下面三种，
        // 出现别的说明键本来就不含编码语义，不该被改写）。
        let seq: String = chars.clone().take(2).collect();
        match seq.as_str() {
            "25" => {
                out.push('%');
                chars.next();
                chars.next();
            }
            "2E" => {
                out.push('.');
                chars.next();
                chars.next();
            }
            "24" => {
                out.push('$');
                chars.next();
                chars.next();
            }
            _ => out.push('%'),
        }
    }
    out
}

/// **适配器内唯一的设置提交口**：进程内 watcher + 消息面镜像，同一份事实。
///
/// 为什么收成函数而不是在每个写命令里各写一遍：这两路投递**必须同时发生**。
/// 只发 watcher 就是「主窗知道、插件不知道」的半接线（§33 R2-4 的原始形态），
/// 只发镜像就是订阅方收到 watcher 都没确认过的变更。新增写路径走这里，
/// 由 `settings_commit_has_single_mirror_site` 门禁钉住。
///
/// 调用前提是**durable 写入已成功**（回滚由调用方负责）——revision 只有在
/// 落盘后才分配，观察者不该看见后来被回滚的东西。
fn commit_settings_change(
    state: &SubstrateState,
    event: tauron_settings::ChangeEvent,
) -> tauron_settings::ChangeEvent {
    let committed = state.settings.lock().publish_committed_change(event);
    // 可丢的 Event 通道：慢消费者丢最旧、按 topic 收敛到最新 revision，
    // 与进程内 watcher 互不影响；镜像失败不影响已 durable 的提交本身。
    state.bus.lock().publish(
        "host",
        HOST_SETTINGS_CHANGED_TOPIC,
        serde_json::json!({
            "key": settings_wire_key(&committed.key),
            "value": committed.value,
            "source": committed.source,
            "revision": committed.revision,
        }),
        tauron_host::eventbus::ChannelKind::Event,
    );
    committed
}

/// v1 → v2：把裸键文档改写成转义键文档（`%unset` 保留键原样放行）。
///
/// v1 文档是平铺对象，键就是设置键本身，所以这里做的是**逐键改名**，
/// 而不是路径重写。
fn migrate_host_settings_v1_to_v2(user: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(m) = user else {
        return user.clone();
    };
    let mut out = serde_json::Map::new();
    for (k, v) in m {
        if k == tauron_settings::UNSET_KEY {
            // v1 文档由裸 `HashMap` 写入，从不含 `$unset`；真出现了就原样留下
            // （不猜它的段语义），免得把数据改坏。
            out.insert(k.clone(), v.clone());
        } else {
            out.insert(settings_path(k), v.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// 注册宿主设置命名空间的 schema（v1 → v2 升版）与 v1→v2 迁移。
fn install_host_settings_schema(store: &mut SettingsStore) {
    // 先 v1 再 v2：注册表要求版本严格递增，这条链本身就是升版的证据。
    store
        .register(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V1, &host_settings_schema())
        .expect("内置的宿主设置 v1 schema 必须可编译");
    store
        .register(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V2, &host_settings_schema())
        .expect("内置的宿主设置 v2 schema 必须可编译");
    store.register_migration(
        HOST_SETTINGS_NAMESPACE,
        Migration::new(
            HOST_SETTINGS_SCHEMA_V1,
            HOST_SETTINGS_SCHEMA_V2,
            migrate_host_settings_v1_to_v2,
        )
        .with_contract(MigrationContract::new(
            false, // no reverse function is shipped
            false, // v1 readers do not understand v2 escaped keys
            true,  // retain the pre-migration snapshot until probation/commit
        )),
    );
}

/// `SettingsError` → `HostError`。
///
/// 复用 `E_INVALID_MANIFEST`，**不新增错误码**（TS 门禁要求两侧错误码同序）；
/// 这与 `tauron-settings` 自己的约定一致（见该 crate 的模块文档）。
fn settings_to_host_error(e: SettingsError) -> HostError {
    HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("设置被拒：{e}"))
}

fn settings_fault_to_host_error(error: tauron_host::FaultError) -> HostError {
    HostError::new(
        ErrorCode::E_HOST_PANIC,
        format!(
            "settings fault boundary rejected work: {error}; main window must run host_settings_migrate to reconcile"
        ),
    )
}

/// 设置族的故障闸门（A91）：**只判就绪，不代持事务**。
///
/// 三段式而不是 `FaultBoundary::run`——后者要求整段闭包期间持有边界锁，而这里的闭包
/// 包含落盘 I/O。那样一来 `settings_fault` 就成了事实上的串行化点：`host_settings_get`
/// 会排在别人的磁盘写后面，`settings_write_lock` 也跟着变成装饰（实测：去掉写租约后
/// 并发写测试仍然全绿，因为闸门替它把一切串好了）。事务边界归写租约，这里只管
/// 「故障了就不许再干活」和「panic 事后登记」。
fn run_settings_boundary<T>(
    state: &SubstrateState,
    operation: &str,
    f: impl FnOnce() -> HostResult<T>,
) -> HostResult<T> {
    state.settings_fault.lock().ensure_ready().map_err(settings_fault_to_host_error)?;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let error = state.settings_fault.lock().record_panic(operation, payload);
            Err(settings_fault_to_host_error(error))
        }
    }
}

/// V4 A91 deterministic repair path for the Settings Engine.
///
/// A panic may have happened after an in-memory mutation. The only state we trust for recovery is
/// the durable envelope protected by the existing checksum/generation contract. If persistence is
/// disabled, consistency cannot be proven, so the boundary remains quarantined instead of
/// pretending recovery succeeded.
fn reconcile_settings_boundary(state: &SubstrateState) -> HostResult<()> {
    {
        let mut boundary = state.settings_fault.lock();
        if boundary.state() == tauron_host::FaultState::Ready {
            return Ok(());
        }
        boundary.begin_reconcile().map_err(settings_fault_to_host_error)?;
    }

    let attempt = guard("settings_reconcile", || -> HostResult<()> {
        let _write = state.settings_write_lock.lock();
        let path = state.settings_path.as_ref().ok_or_else(|| {
            HostError::new(
                ErrorCode::E_HOST_PANIC,
                "settings boundary cannot be reconciled without durable state".to_string(),
            )
        })?;

        let mut rebuilt = SettingsStore::new();
        install_host_settings_schema(&mut rebuilt);
        let generation = match load_settings_doc(path)? {
            Some((entries, generation)) => {
                rebuilt.restore(&entries);
                generation
            }
            None => 0,
        };

        *state.settings.lock() = rebuilt;
        *state.settings_generation.lock() = generation;
        Ok(())
    });

    let result = match attempt {
        Ok(result) => result,
        Err(error) => Err(error),
    };
    let mut boundary = state.settings_fault.lock();
    match result {
        Ok(()) => {
            boundary.reconcile_succeeded();
            Ok(())
        }
        Err(error) => {
            boundary.reconcile_failed();
            Err(HostError::new(
                ErrorCode::E_HOST_PANIC,
                format!("settings fault reconcile failed and boundary was quarantined: {error}"),
            ))
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// A91（轮 47）：事件 / 审批 / 注册表三个子系统的故障边界
// ──────────────────────────────────────────────────────────────────────────

/// 子系统边界的拒绝文案。修复路径必须**具名**——只报"拒绝服务"而不给出确定性
/// 修复出口，等于把故障变成不可恢复的全宿主静默降级。
fn subsystem_fault_to_host_error(
    subsystem: &str,
    error: tauron_host::FaultError,
    repair: &str,
) -> HostError {
    HostError::new(
        ErrorCode::E_HOST_PANIC,
        format!("{subsystem} fault boundary rejected work: {error}; {repair}"),
    )
}

/// 三段式闸门（与 [`run_settings_boundary`] 同构）：只判就绪 + 事后登记 panic。
///
/// **不**用 `FaultBoundary::run`——它要求整段闭包持有边界锁，闸门会变成串行化点；
/// 且该 API 按「未接线公开 API 台账」登记为 embedder 面（`FaultBoundary::run` 的
/// 唯一消费者是 fault.rs 自己的单测，本文件不得把它接成生产消费者）。
fn run_subsystem_boundary<T>(
    boundary: &Arc<Mutex<tauron_host::FaultBoundary>>,
    subsystem: &str,
    repair: &str,
    operation: &str,
    f: impl FnOnce() -> HostResult<T>,
) -> HostResult<T> {
    boundary
        .lock()
        .ensure_ready()
        .map_err(|error| subsystem_fault_to_host_error(subsystem, error, repair))?;
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let error = boundary.lock().record_panic(operation, payload);
            Err(subsystem_fault_to_host_error(subsystem, error, repair))
        }
    }
}

/// 事件总线闸门：故障修复路径 = `host_recover_boot`（确定性重置为空）。
fn run_events_boundary<T>(
    state: &SubstrateState,
    operation: &str,
    f: impl FnOnce() -> HostResult<T>,
) -> HostResult<T> {
    run_subsystem_boundary(
        &state.events_fault,
        "events",
        "run host_recover_boot to reconcile (it clears the bus session state: \
         subscriptions/approvals/queues; topic declarations are kept)",
        operation,
        f,
    )
}

/// 审批令牌闸门：修复路径 = 重新走 preview（清空旧令牌、重新铸发）。
fn run_review_boundary<T>(
    state: &SubstrateState,
    operation: &str,
    f: impl FnOnce() -> HostResult<T>,
) -> HostResult<T> {
    run_subsystem_boundary(
        &state.review_fault,
        "approval",
        "re-run the preview path to reconcile (it clears pending approval tokens)",
        operation,
        f,
    )
}

/// 注册表闸门：会话内没有确定性重建，修复路径 = 重启重新装配。
fn run_registry_boundary<T>(
    state: &SubstrateState,
    operation: &str,
    f: impl FnOnce() -> HostResult<T>,
) -> HostResult<T> {
    run_subsystem_boundary(
        &state.registry_fault,
        "registry",
        "restart the host to re-assemble the registry (no in-session rebuild exists)",
        operation,
        f,
    )
}

/// 修复的第一段：判定当前态并把边界推进到 `Reconciling`。
///
/// 返回 `Ok(true)` = 本次真的需要修复（`Faulted` / `Quarantined` / 悬空
/// `Reconciling`——上次修复中途夭折，修复动作是确定性的，重做安全）；
/// `Ok(false)` = 本来就 `Ready`（幂等无操作）。
fn begin_subsystem_reconcile(
    boundary: &Arc<Mutex<tauron_host::FaultBoundary>>,
    subsystem: &str,
    repair: &str,
) -> HostResult<bool> {
    let mut guard = boundary.lock();
    match guard.state() {
        tauron_host::FaultState::Ready => Ok(false),
        tauron_host::FaultState::Reconciling => Ok(true),
        _ => {
            guard
                .begin_reconcile()
                .map_err(|error| subsystem_fault_to_host_error(subsystem, error, repair))?;
            Ok(true)
        }
    }
}

/// 事件总线（A91 轮 47）：确定性修复 = **会话态清零**（[`EventBus::reset_session_state`]）。
///
/// 总线没有需要重建的持久真相：订阅 / 审批 / 队列 / 排序都是可重建的会话瞬态，
/// 清零是**完整**的一致状态而非猜测——留下的半状态才是不可修复的。**结构声明
/// （topic）保留**：那是装配期事实，发布端对未声明 topic 只静默丢弃不报错
/// （防存在性探测），连声明一起清等于让修复动作把可工作的消息面悄悄打哑。
/// 触发点：[`cmd_recover_boot`]（启动恢复读取 = 确定性修复时刻）。
fn reconcile_events_boundary(state: &SubstrateState) -> HostResult<bool> {
    if !begin_subsystem_reconcile(
        &state.events_fault,
        "events",
        "host_recover_boot resets the bus session state",
    )? {
        return Ok(false);
    }
    state.bus.lock().reset_session_state();
    state.events_fault.lock().reconcile_succeeded();
    Ok(true)
}

/// 审批令牌（A91 轮 47）：确定性修复 = **清空两域令牌表**。
///
/// 令牌本就是一次性、短 TTL 的瞬态物；清空让任何"消费了一半"的令牌彻底失效——
/// 比留下更安全（留下的令牌不可能再被合法消费，却可能被误判为可用）。触发点：
/// 两条 preview 命令（重新预览 = 操作者对审批域的显式恢复动作）。
fn reconcile_review_boundary(state: &PluginRuntimeState) -> HostResult<bool> {
    if !begin_subsystem_reconcile(&state.review_fault, "approval", "re-run the preview path")? {
        return Ok(false);
    }
    state.admin_reviews.lock().clear();
    #[cfg(feature = "plugin-install")]
    state.install_reviews.lock().clear();
    state.review_fault.lock().reconcile_succeeded();
    Ok(true)
}

/// 注册表（A91 轮 47）：**没有会话内确定性修复**。
///
/// 条目没有持久镜像；重新装配是进程启动期的接入方动作（`install_plugin_from_json`
/// 与轮 41 的启动孤儿清扫）。与其造一个"看起来修好了"的重建，不如如实隔离：
/// 修复尝试把边界推成 `Quarantined`（继续拒绝），修复路径写进返回错误——重启重装。
/// 触发点：[`cmd_recover_boot`]（主窗尝试修复时会得到明确结论，而不是无限"故障中"）。
fn reconcile_registry_boundary(state: &SubstrateState) -> HostResult<bool> {
    if !begin_subsystem_reconcile(&state.registry_fault, "registry", "the host will be restarted")?
    {
        return Ok(false);
    }
    state.registry_fault.lock().reconcile_failed();
    Err(HostError::new(
        ErrorCode::E_HOST_PANIC,
        "registry fault cannot be reconciled in session (no durable rebuild exists); \
         restart the host to re-assemble the registry"
            .to_string(),
    ))
}

/// **接手一份旧版（v1）宿主设置文档**（R7-2：settings 可迁移）。
///
/// 用途：宿主从磁盘读到的旧版配置走这里进 Store。写入用户层并把**数据版本**
/// 标成 [`HOST_SETTINGS_SCHEMA_V1`]——不标注的话 [`host_settings_migrate`]
/// 无从知道起点，只能拒绝迁移（宁可不迁，也不猜）。
///
/// `doc` 必须是对象（键 = 设置键，值 = 设置值）；否则返回 `E_INVALID_MANIFEST`，
/// 不静默退化成空文档。
pub fn host_settings_adopt_legacy(
    state: &SubstrateState,
    doc: serde_json::Value,
) -> HostResult<()> {
    if !doc.is_object() {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "旧版宿主设置文档必须是对象（键 = 设置键）".to_string(),
        ));
    }
    let mut store = state.settings.lock();
    store.set_layer(HOST_SETTINGS_NAMESPACE, tauron_settings::LayerKind::User, doc);
    store.set_data_version(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V1);
    Ok(())
}

/// 把宿主设置从已标注的数据版本迁到**当前** schema 版本。
///
/// # 返回
///
/// 实际应用的迁移步数（已是当前版本、或没有数据时为 `0`）。
///
/// 全有或全无：链路缺失或迁移结果过不了 schema 校验时用户层一个字节都不改，
/// 返回 `E_INVALID_MANIFEST`（不新增错误码）。
fn host_settings_migrate_transaction(state: &SubstrateState) -> HostResult<MigrationReceipt> {
    state
        .settings
        .lock()
        .migrate_transactional(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V2)
        .map_err(settings_to_host_error)
}

pub fn host_settings_migrate(state: &SubstrateState) -> HostResult<usize> {
    Ok(host_settings_migrate_transaction(state)?.steps())
}

/// 当前宿主设置的数据版本（**宿主嵌入方的 Rust 诊断读口**；`None` = 既无数据也无标注）。
///
/// 线上没有对应命令：插件侧能看到的最接近事实是镜像帧里的 `revision`，
/// 版本本身要靠 `host_settings_migrate` 的返回步数推断。
pub fn host_settings_data_version(state: &SubstrateState) -> Option<String> {
    state.settings.lock().data_version(HOST_SETTINGS_NAMESPACE).map(str::to_string)
}

/// Monotonic revision of successfully committed host-setting writes.
///
/// Rust embedding diagnostic read-out — there is **no** wire command for it;
/// wire consumers observe the same counter through the `revision` field of
/// [`HOST_SETTINGS_CHANGED_TOPIC`] frames.
pub fn host_settings_revision(state: &SubstrateState) -> u64 {
    state.settings.lock().revision()
}

/// `host_settings_get`：读取设置。
///
/// **线形不变**（前端契约）：入参 `key: string`，返回任意 JSON；未写过的键
/// 返回 `Null`（不是报错）。读路径不校验 schema——缺键不是错误。
pub fn cmd_settings_get(state: &SubstrateState, key: &str) -> HostResult<serde_json::Value> {
    run_settings_boundary(state, "settings_get", || {
        let path = settings_path(key);
        let store = state.settings.lock();
        let value =
            store.get_key(HOST_SETTINGS_NAMESPACE, &path).map_err(settings_to_host_error)?;
        Ok(value.unwrap_or(serde_json::Value::Null))
    })
}

/// `host_settings_set`：写入设置。
///
/// **线形不变**（前端契约）：入参 `key: string, value: any`，返回 `()`。
/// 写入**经 [`SettingsStore`]**（不再是裸 `HashMap`）：键会被编码成合法点路径，
/// 值要过命名空间 schema，落盘形态由 Store 的四层结构决定。
pub fn cmd_settings_set(
    state: &SubstrateState,
    key: &str,
    value: serde_json::Value,
) -> HostResult<()> {
    run_settings_boundary(state, "settings_set", || {
        if key.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "设置键不能为空（调用方可能传了 undefined/null）".to_string(),
            ));
        }

        // 单写者事务。两件事必须分清：**写租约故意横跨落盘**（否则另一个 writer 能插进
        // 「内存已改、磁盘未改」的半提交窗口，回滚时会把对方的写入一起吃掉）；被禁止的是
        // `settings` 互斥量进 `persist_settings_doc`——它在下面的块作用域里就出了临界区，
        // 落盘只消费一次性快照。
        let _write = state.settings_write_lock.lock();
        let path = settings_path(key);
        let (before, event) = {
            let mut store = state.settings.lock();
            let before = store.snapshot_all();
            let (_op, event) = store
                .set_deferred(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_NAMESPACE, &path, &value)
                .map_err(settings_to_host_error)?;
            (before, event)
        };

        if let Err(error) = persist_settings_doc(state) {
            state.settings.lock().restore(&before);
            return Err(error);
        }

        // Watchers only observe a revision after durable persistence succeeded.
        commit_settings_change(state, event);
        Ok(())
    })
}

/// `host_settings_adopt_legacy`：把宿主磁盘上读到的旧版设置文档交给 Store。
///
/// 迁移能力的**唯一入口**。缺了它，[`host_settings_adopt_legacy`] 与
/// [`host_settings_migrate`] 就只是 Rust 测试能碰到的孤儿逻辑——真实宿主宁可
/// 自己拼点路径写设置，也不会去调一个没有渠道的迁移。
///
/// **线形**：入参 `doc: object`（键 = 设置键，值 = 设置值），返回 `()`。
/// 非对象文档返回 `E_INVALID_MANIFEST`（不静默退化成空文档）。写入后数据版本
/// 标注为 [`HOST_SETTINGS_SCHEMA_V1`]，[`cmd_settings_migrate`] 才知道起点。
pub fn cmd_settings_adopt_legacy(state: &SubstrateState, doc: serde_json::Value) -> HostResult<()> {
    run_settings_boundary(state, "settings_adopt_legacy", || {
        let _write = state.settings_write_lock.lock();
        let before = state.settings.lock().snapshot_all();
        host_settings_adopt_legacy(state, doc)?;
        if let Err(error) = persist_settings_doc(state) {
            state.settings.lock().restore(&before);
            return Err(error);
        }
        Ok(())
    })
}

/// `host_settings_adopt_legacy` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// **主窗专属**：它接手的是**整份**文档（不是某一个键），会把用户层整体改写并
/// 把数据版本重标为 v1——插件主体能调它等于能清空/重置整个宿主的设置层，也等于
/// 能把 v2 数据倒回 v1（拒绝服务）。设置族的**按键**访问权限（插件只能读写自己
/// 命名空间）在这里没有意义：整份文档跨所有插件与宿主键。
pub fn cmd_settings_adopt_legacy_as(
    caller: &Caller,
    state: &SubstrateState,
    doc: serde_json::Value,
) -> HostResult<()> {
    require_main_window(caller, "host_settings_adopt_legacy")?;
    cmd_settings_adopt_legacy(state, doc)
}

/// `host_settings_migrate`：把设置迁到当前 schema 版本。
///
/// **线形**：无入参，返回迁移**步数**（数字）。`0` = 已是最新（或本就无数据），
/// 且**幂等**（重复调用始终是 0）。全有或全无：链路缺失 / 迁移结果过不了新
/// schema 校验时用户层一个字节都不改，返回 `E_INVALID_MANIFEST`。
///
/// 与 [`cmd_settings_adopt_legacy`] 一样受 [`guard`] 保护——迁移要走 schema
/// 编译与用户数据改写，一旦 panic 必须是 `E_HOST_PANIC` 而不是把 panic  unwind
/// 穿过 IPC 边界。
pub fn cmd_settings_migrate(state: &SubstrateState) -> HostResult<usize> {
    reconcile_settings_boundary(state)?;
    run_settings_boundary(state, "settings_migrate", || {
        let _write = state.settings_write_lock.lock();
        // A101：镜像必须在迁移**之前**取——迁移完成后内存里已经没有迁移前的用户层了
        // （`MigrationReceipt.before` 就是同一份，但它是私有字段，且不覆盖其它命名空间）。
        let before = state.settings.lock().snapshot_all();
        let receipt = host_settings_migrate_transaction(state)?;
        let steps = receipt.steps();
        if receipt.changed() {
            // 合同要求快照 → 先把迁移前的用户层落到磁盘，再动正式文档。**顺序不能反**：
            // 先写新文档再写镜像，中间掉电的结果是「已经迁了，且没有任何东西能回滚」。
            if receipt.contract().requires_snapshot {
                if let Err(error) = stage_settings_rollback_image(state, &before) {
                    state.settings.lock().rollback_migration(receipt);
                    return Err(error);
                }
            }
            if let Err(error) = persist_settings_doc(state) {
                state.settings.lock().rollback_migration(receipt);
                if let Some(path) = settings_rollback_path(state) {
                    clear_settings_rollback_image(&path);
                }
                return Err(error);
            }
        }
        Ok(steps)
    })
}

/// `host_settings_migrate` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// **主窗专属**：迁移会整体改写用户层（键编码契约换版），且会把数据版本推到
/// 当前版本——插件主体调它等于能对宿主设置做一次全局不可逆改写。与
/// [`cmd_settings_adopt_legacy_as`] 同一条理由：整份文档级操作无法表达
/// 「只准动自己的键」。
pub fn cmd_settings_migrate_as(caller: &Caller, state: &SubstrateState) -> HostResult<usize> {
    require_main_window(caller, "host_settings_migrate")?;
    cmd_settings_migrate(state)
}

/// `host_settings_get` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 主窗可读任意键；插件只能读自己命名空间（`plugin:<自己 id>`）内的键。
/// 越界在**读之前**拒绝（读也不放过：别人的设置值本身就是隐私）。
/// 命令的**线形不变**（入参 `key`、返回任意 JSON 或 `Null`）。
pub fn cmd_settings_get_as(
    caller: &Caller,
    state: &SubstrateState,
    key: &str,
) -> HostResult<serde_json::Value> {
    require_settings_key_scope(caller, key)?;
    cmd_settings_get(state, key)
}

/// `host_settings_set` 的**带身份判定**版本（wire 层转调的就是它）。
///
/// 主窗可写任意键；插件只能写自己命名空间内的键。越界在**写之前**拒绝——
/// 拒绝路径**不产生任何写入副作用**（有测试断言 Store 内容逐字不变）。
/// 命令的**线形不变**（入参 `key` + `value`，返回 `()`）。
pub fn cmd_settings_set_as(
    caller: &Caller,
    state: &SubstrateState,
    key: &str,
    value: serde_json::Value,
) -> HostResult<()> {
    require_settings_key_scope(caller, key)?;
    cmd_settings_set(state, key, value)
}

/// `host_notify`：发送通知（P0-5：对接 tauron-notify crate；R7-3：接
/// [`DispatchSink`]）。
///
/// **顺序不变量**：先入环形缓冲，再走系统通知分发。这条不变量的落点在
/// [`tauron_notify::dispatch`] 内部（「先 push 再 send」），本函数不再自己
/// push 一次——重复插入会被 Store 当成重复 id 拒掉，等于把顺序又倒回去了。
///
/// **降级语义**：系统通知失败（不支持 / 未授权 / 致命错误）只让
/// `DispatchOutcome` 变成 `Degraded`，通知照旧留在环形缓冲里，本命令
/// **绝不**因为系统通知失败而返回错误。
///
/// 未注入 `DispatchSink`（底座-only 宿主 / 单测）时没有系统通知通道：
/// 只入应用内缓冲，**不**记 dispatch 日志（没有尝试就没有日志）。
///
/// **入口**：wire 层转调 [`cmd_notify_as`]（署名必须是自己 / 宿主），本函数是
/// **不过身份**的核心。
pub fn cmd_notify(
    state: &SubstrateState,
    plugin_id: &str,
    title: &str,
    body: &str,
) -> HostResult<()> {
    guard("notify", || {
        if title.len() > MAX_NOTIFY_TITLE_BYTES {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "通知标题超出字节预算（{} > {MAX_NOTIFY_TITLE_BYTES}），拒绝入队",
                    title.len()
                ),
            ));
        }
        if body.len() > MAX_NOTIFY_BODY_BYTES {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "通知正文超出字节预算（{} > {MAX_NOTIFY_BODY_BYTES}），拒绝入队",
                    body.len()
                ),
            ));
        }
        let entry = NotifyEntry {
            id: uuid::Uuid::new_v4().to_string(),
            plugin_id: plugin_id.to_string(),
            kind: NotifyKind::Info,
            title: title.to_string(),
            message: body.to_string(),
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            read: false,
            data: serde_json::Value::Null,
        };
        {
            let mut store = state.notify_store.lock();
            match state.notify_sink.get() {
                // 有系统通道：交给 `dispatch`（它内部先入缓冲再 send，
                // 并落一条分发日志）。
                Some(sink) => {
                    dispatch(&mut store, sink.as_ref(), entry);
                }
                // 无系统通道：只有应用内这一条路。
                None => {
                    if let Err(e) = store.push(entry) {
                        return Err(HostError::new(
                            ErrorCode::E_HOST_PANIC,
                            format!("通知写入失败: {e}"),
                        ));
                    }
                }
            }
        }

        // 向后兼容：同时写入 NotificationRecord（**只写不读**的兼容日志，
        //    故必须有上限，否则每次 host_notify 都会永久占一份内存）。
        let mut notifications = state.notifications.lock();
        notifications.push(NotificationRecord {
            plugin_id: plugin_id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        });
        if notifications.len() > MAX_NOTIFICATION_LOG {
            let excess = notifications.len() - MAX_NOTIFICATION_LOG;
            notifications.drain(0..excess); // 保留最新 MAX_NOTIFICATION_LOG 条
        }

        Ok(())
    })?
}

/// `host_notify` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// `pluginId` 是通知的**署名**：通知中心按它显示来源、做"来自哪个插件"的过滤，
/// 用户据此决定信不信这条通知。不判定的话，任何插件都能：
///
/// - 以**宿主**名义发通知（伪装成宿主告警 → 钓鱼 / 骗点击）；
/// - 以**别的插件**名义发通知（栽赃：把恶意行为挂到别人名下）。
///
/// 所以规则不是"把命令关成主窗专属"——插件本来就得以自己名义发通知——而是
/// **署名必须是自己**（[`require_self_plugin_scope`]）。拒绝路径**零副作用**：
/// 通知不入环形缓冲、不写兼容日志、不触发系统通知分发（有测试断言条数不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_notify_as(
    caller: &Caller,
    state: &SubstrateState,
    plugin_id: &str,
    title: &str,
    body: &str,
) -> HostResult<()> {
    require_self_plugin_scope(caller, "host_notify", Some(plugin_id))?;
    cmd_notify(state, plugin_id, title, body)
}

/// 通知读端的**可见性过滤**（scoped-read 口径，与 `host_registry_list` 同族）。
///
/// # 为什么这里是"过滤"而不是"拒绝"
///
/// 与 [`require_main_window`] / [`require_self_plugin_scope`] 的语义**不同**，别混用：
/// 那两个是"拒绝或放行"，本函数是"**裁剪结果**"。插件看不见别人的通知是对的，但
/// "插件读自己的通知"本身是正当功能（通知中心就是干这个的），一刀切拒绝会把功能
/// 关掉——所以读端按可见性过滤，写端（`host_notify` / `host_notifications_read`）
/// 才按身份判定。
///
/// - **主窗**：全部可见（现状不变）；
/// - **插件**：只有 `pluginId == 自己` 的条目可见。
///
/// 返回**未分页**的可见集合（时间倒序）。分页由调用方在这之后做，原因见
/// `notifications_list_payload`：反过来先分页再过滤，别人的通知会把窗口占满，
/// 插件自己那几条反而被 `limit` 挤出去（"过滤了但自己的读不到"）。
pub fn visible_notifications<'a>(caller: &Caller, store: &'a NotifyStore) -> Vec<&'a NotifyEntry> {
    // `recent(len)` = 整个环（时间倒序）；不能用 `recent(limit)` 拿到"全部再看"。
    let all = store.recent(store.len());
    match caller.plugin_id() {
        // 主窗：全部可见。
        None => all,
        Some(me) => all.into_iter().filter(|e| e.plugin_id == me).collect(),
    }
}

/// `host_notifications_list` 的快照构造（两条入口共用）。
///
/// **计数与条目必须同源自可见集合**：`total` / `unread` 都在**过滤之后**重新数。
/// 只裁数组、留着全局 `unread`（甚至只留着全局 `total`）仍然是泄露——未读数本身
/// 就能推断"别的插件正在发通知"。
///
/// **`limit` 作用在过滤之后**：先按可见性筛出调用方的集合，再取其中最新 `limit` 条。
/// 因此插件的 `items.len()` 可以小于 `limit` 而 `total` 仍可能更大（分页语义：
/// `total` 是可见集合总数，`items` 是本页），这不矛盾——`total`/`unread` 始终是
/// 可见集合的真值，前端角标因此不会显示别人的条数。
///
/// **`dispatchLog` 同样过滤**：记录里只有 `entryId`（没有 `plugin_id`），所以按
/// `entryId` 反查条目归属。查不到（条目已被环形裁剪）时对插件**不显示**——无法归属
/// 的记录不能证明是自己的；主窗不受影响（保持"条目被裁剪后日志仍在"的既有语义）。
fn notifications_list_payload(
    state: &SubstrateState,
    caller: &Caller,
    limit: Option<usize>,
) -> HostResult<serde_json::Value> {
    guard("notifications_list", || {
        let store = state.notify_store.lock();
        let visible = visible_notifications(caller, &store);
        let total = visible.len();
        let unread = visible.iter().filter(|e| !e.read).count();
        let items: Vec<serde_json::Value> = visible
            .iter()
            .take(limit.unwrap_or(50))
            .map(|e| {
                serde_json::json!({
                    "id": e.id,
                    "pluginId": e.plugin_id,
                    "kind": e.kind.as_str(),
                    "title": e.title,
                    "message": e.message,
                    "ts": e.ts,
                    "read": e.read,
                    "data": e.data,
                })
            })
            .collect();
        let mine = caller.plugin_id();
        let dispatch_log: Vec<serde_json::Value> = store
            .dispatch_log()
            .iter()
            .filter(|r| match mine {
                // 主窗：日志不受影响（含"条目已被裁剪但日志仍在"的既有语义）。
                None => true,
                // 插件：只留能归属到自己的记录。
                Some(me) => store.get(&r.entry_id).is_some_and(|e| e.plugin_id == me),
            })
            .map(|r| {
                serde_json::json!({
                    "entryId": r.entry_id,
                    "outcome": r.outcome.as_str(),
                    "ts": r.ts,
                })
            })
            .collect();
        let plugin_usage: std::collections::BTreeMap<String, usize> = store
            .group_counts()
            .into_iter()
            .filter_map(|(group, count)| {
                let plugin_id = group.strip_prefix("plugin:")?;
                match mine {
                    None => Some((plugin_id.to_string(), count)),
                    Some(caller_plugin) if caller_plugin == plugin_id => {
                        Some((plugin_id.to_string(), count))
                    }
                    Some(_) => None,
                }
            })
            .collect();
        Ok(serde_json::json!({
            "unread": unread,
            "total": total,
            "items": items,
            "dispatchLog": dispatch_log,
            "capacity": store.capacity(),
            "pluginCapacity": store.plugin_capacity(),
            "pluginUsage": plugin_usage,
        }))
    })?
}

/// `host_notifications_list` 的**带身份过滤**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条要按身份过滤（危害原文）
///
/// 返回体里是**通知的正文**：`title` / `message` / `data`（`data` 里常见跳转链接、
/// 操作按钮的载荷）。不按身份过滤的话，任何插件调一次就能读到**别的插件**（以及
/// **宿主自己**）的通知正文——而通知正文恰恰是"宿主想对用户说什么"的通道，可能包含
/// 故障详情、待处理项、甚至带 token 的跳转链接。这不是"看见别人存在"这种元信息泄露，
/// 而是**内容**泄露。
///
/// 过滤而不是拒绝（见 [`visible_notifications`]）：插件要能读自己的通知，否则通知
/// 中心对插件不可用。`total` / `unread` / `dispatchLog` 与 `items` **同源**，避免
/// "数组裁了、角标还露着"的残留泄露。
///
/// 命令**线形不变**（入参仍是 `limit`，返回仍是 `{ unread, total, items, dispatchLog }`；
/// 只是插件视角下这四个值都变成"可见集合"上的真值）。
pub fn cmd_notifications_list_as(
    caller: &Caller,
    state: &SubstrateState,
    limit: Option<usize>,
) -> HostResult<serde_json::Value> {
    notifications_list_payload(state, caller, limit)
}

/// `host_notifications_list`：通知中心读取端（`host_notify` 的配对）。
///
/// 断链回归：通知存储此前**只写不读**——`host_notify` 落进环形缓冲就再无
/// 出口，未读计数只增不减、通知中心无处取数。`limit` 缺省 50，按时间倒序。
/// 线形键为 camelCase（`pluginId`），与 TS 契约一致（NotifyEntry 本体是
/// snake_case，此处逐字段映射）。
///
/// **`dispatchLog`（R7-3 新增字段）**：**系统通知分发尝试**的日志，一次
/// `host_notify` 一条，字段 `entryId` / `outcome`（`system` | `degraded` |
/// `failed`）/ `ts`。语义边界写清楚：
///
/// - 它是**尝试**的记录，不是通知本身：条目被环形裁剪后，它的分发记录仍留在
///   日志里（两个环互相独立，各有上限）；
/// - 未注入 `DispatchSink` 时不会有任何记录（没有尝试）；
/// - 它**不受** `limit` 影响（那是通知本身的条数上限）；日志由 Store 自己的
///   环形上限（`tauron_notify::DEFAULT_DISPATCH_LOG_CAPACITY` = 200）约束。
///
/// **入口**：wire 层转调 [`cmd_notifications_list_as`]（按身份过滤可见集合），
/// 本函数是**不过身份**的核心 = 主窗视角（全部可见），行为与 R7 之前逐字一致。
pub fn cmd_notifications_list(
    state: &SubstrateState,
    limit: Option<usize>,
) -> HostResult<serde_json::Value> {
    notifications_list_payload(state, &Caller::MainWindow, limit)
}

/// `host_notifications_read`：标记已读（`id` 缺省 = 全部）。
///
/// **入口**：wire 层转调 [`cmd_notifications_read_as`]（插件只能标记自己的通知），
/// 本函数是**不过身份**的核心。
pub fn cmd_notifications_read(
    state: &SubstrateState,
    id: Option<&str>,
) -> HostResult<serde_json::Value> {
    guard("notifications_read", || {
        let mut store = state.notify_store.lock();
        let marked = match id {
            Some(id) => usize::from(store.mark_read(id)),
            None => store.mark_all_read(),
        };
        Ok(serde_json::json!({ "marked": marked }))
    })?
}

/// `host_notifications_read` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// 这条是**写端**（改的是已读状态），所以与读端不同：不是过滤，而是判定。
///
/// - `id = None` 是"**全部标记已读**"——改的是**全局**未读状态（含宿主自己与所有
///   其他插件）。插件调一次就能把宿主的未读角标清零，用户再也看不到"有事没处理"；
/// - `id = Some(x)` 若 `x` 是**别人的**通知，改的就是别人的已读状态：那条通知在
///   通知中心里直接变成"已读"，等于**替别人把消息吞掉**（用户再也不会被提醒）。
///
/// 规则：主窗任意 `id` / `None`（宿主自己就是通知中心的完整主体）；插件
/// `Some(id)` 只能是**自己**的通知，`None` 一律拒绝（[`require_self_plugin_scope`]
/// 的 `None` 分支 = 宿主级/全局档）。
///
/// **一个必须说明的边界**：`id` 是**通知 id**（不是 pluginId），所以"是不是自己的"
/// 只能先反查条目归属。**查不到（未知 id / 已被环形裁剪）时对插件一律拒绝**——
/// 理由有两条：① 无法归属就证明不了是自己的（规则要求"只能是自己"）；② 若放行并
/// 返回 `marked: 0`，`marked` 就成了**存在性预言机**（`1` = 这个 id 存在，`0` = 不存在），
/// 那正是本批要收掉的信息泄露。主窗路径不变（未知 id → `marked: 0`，不 panic）。
///
/// 拒绝路径**零副作用**：别人的已读状态、全局未读计数一律不动（有测试断言）。
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_notifications_read_as(
    caller: &Caller,
    state: &SubstrateState,
    id: Option<&str>,
) -> HostResult<serde_json::Value> {
    // 归属反查只对插件有意义（主窗任意 id / None 都合法，不做多余查询）。
    if caller.plugin_id().is_some() {
        if let Some(nid) = id {
            let owner = state.notify_store.lock().get(nid).map(|e| e.plugin_id.clone());
            let Some(owner) = owner else {
                return Err(HostError::new(
                    ErrorCode::E_AUTH_DENIED,
                    format!(
                        "通知 `{nid}` 不存在或已过环形窗口，无法确认它是{} 自己的通知；\
                         对未知 id 放行会让 `marked` 变成存在性预言机（判定在任何副作用之前）",
                        caller.describe()
                    ),
                ));
            };
            // 已知归属：走与本批同一条判定（`Some(别人)` 拒绝）。
            require_self_plugin_scope(caller, "host_notifications_read", Some(&owner))?;
        } else {
            // `None` = 标记全部：全局状态，插件不得主张。
            require_self_plugin_scope(caller, "host_notifications_read", None)?;
        }
    }
    cmd_notifications_read(state, id)
}

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

// ──────────────────────────────────────────────────────────────────────────
// 启动恢复（§4.14：持久化 + 崩溃检测 + 驱动信号）
// ──────────────────────────────────────────────────────────────────────────

/// 阶段对账结果（诊断用）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseReconcileOutcome {
    /// 扫描到的注册表插件数。
    pub scanned: usize,
    /// 补发的 `SafemodeEnter`（引擎判定应禁用，注册表尚未标记）。
    pub entered: usize,
    /// 补发的 `SafemodeExit`（引擎判定应启用，注册表仍标记为安全模式禁用）。
    pub exited: usize,
    /// 被忽略的非法迁移（例如对已卸载插件补发，或两次调用之间状态已变）。
    pub ignored: usize,
}

fn phase_name(phase: BootPhase) -> &'static str {
    match phase {
        BootPhase::Normal => "正常启动",
        BootPhase::Safemode => "安全模式",
        BootPhase::Repairmode => "修复模式",
    }
}

/// 把恢复引擎的阶段判定**对账**到注册表生命周期状态机。
///
/// 两个 crate 各管一件事，本函数是唯一的桥：
/// - `tauron-recovery` 负责**判**——连续失败次数 → normal/safemode/repairmode；
/// - 注册表状态机负责**执行**——`Event::SafemodeEnter` →
///   `Action::SetSafemodeDisabled` → `disabled_by_safemode`。后者是
///   `PluginSummary.disabledBySafemode` 的**唯一权威来源**。
///
/// 它是**对账**而非事件处理器：逐个比较注册表实际标志与引擎判定，只在不一致时
/// 补发事件，因此可被任何路径安全、重复地调用（启动上报后、插件上报生命周期后、
/// 管理操作后）。非法迁移被计数而不报错——对已卸载插件补发 `SafemodeExit` 是
/// 合法情形，不是错误。
///
/// 引擎不认识的插件（例如本轮新装）会先按当前必需集合登记进引擎，再参与判定——
/// 决策覆盖面必须与注册表一致，否则新装插件会绕过安全模式。
fn reconcile_recovery_phase(state: &SubstrateState) -> PhaseReconcileOutcome {
    let mut out = PhaseReconcileOutcome::default();

    // 底座-only 宿主没有插件侧写回口 → 没有插件可对账。这是语义正确（无对象），
    // 不是静默跳过错误。
    let Some(sink) = state.plugin_flags.get() else {
        return out;
    };

    // 先取注册表快照（自持内存，不跨锁），再单独持 engine 锁取判定——
    // 两把锁从不重叠持有。
    let snapshots = sink.flag_snapshots();
    let plan: Vec<(PluginId, bool, bool)> = {
        let mut engine = state.recovery.lock();
        snapshots
            .into_iter()
            .filter_map(|(raw_id, disabled_by_safemode)| {
                let id = PluginId::new(&raw_id).ok()?;
                // 注册表是「有哪些插件」的唯一权威来源，恢复引擎只知道被显式
                // 登记过的插件。引擎不认识就按当前必需集合登记——不补齐的话，
                // 新装的插件会绕过安全模式判定（决策覆盖面必须与注册表一致）。
                if engine.plugin_state(id.as_str()).is_none() {
                    engine.register_plugin(id.as_str());
                }
                let should_enable = engine.plugin_state(id.as_str()).is_none_or(|s| s.is_enabled());
                Some((id, should_enable, disabled_by_safemode))
            })
            .collect()
    };

    for (id, should_enable, currently_disabled) in plan {
        out.scanned += 1;
        let event = if !should_enable && !currently_disabled {
            Some(Event::SafemodeEnter)
        } else if should_enable && currently_disabled {
            Some(Event::SafemodeExit)
        } else {
            None
        };
        let Some(event) = event else { continue };
        let entering = event == Event::SafemodeEnter;
        // 判定在底座（引擎）侧完成，写回口只负责把事件补发到插件侧。
        match sink.report_flag_event(id.as_str(), entering) {
            Some(true) => out.ignored += 1,
            Some(false) => {
                if entering {
                    out.entered += 1
                } else {
                    out.exited += 1
                }
            }
            None => out.ignored += 1,
        }
    }
    out
}

/// 恢复引擎当前状态的线形态（`host_recover_boot` 与 `host_recover_report` 共用）。
///
/// R7-1 新增字段 `lastContext`（**既有字段一个都没改名/删除**：前端已消费
/// `phase`/`counter`/`disabledPlugins`/`requiredPlugins`/`loadSource`/`persistence`）。
fn recovery_boot_payload(state: &SubstrateState) -> serde_json::Value {
    let engine = state.recovery.lock();
    let phase = engine.decide_boot_phase();
    let consecutive_failures = engine.counter().consecutive_failures;
    let safemode_failures = engine.counter().safemode_failures;
    // `disabled_plugins` 借用引擎内部 map 的 key，必须在锁内物化。
    let disabled_plugins: Vec<serde_json::Value> = engine
        .disabled_plugins()
        .into_iter()
        .map(|(id, st)| serde_json::json!({ "pluginId": id, "state": st.as_str() }))
        .collect();
    let required_plugins: Vec<String> = engine.required_plugins().iter().cloned().collect();

    let store = state.recovery_store.lock();
    let persistence = serde_json::json!({
        "enabled": store.is_enabled(),
        "dir": store.dir().map(|p| p.to_string_lossy().into_owned()),
        "bootInFlight": store.in_flight,
        "lastError": store.last_error.clone(),
    });
    // 崩溃后诊断上下文：`RecoveryStore` 从标记文件读回来的**上一轮**关键事件
    // 摘要（含本次 `load` 识别到的那次崩溃）。锁定在 store 锁内物化。
    let last_context: Vec<serde_json::Value> =
        store.last_context.iter().map(boot_context_entry_wire).collect();

    serde_json::json!({
        // 线名与 TS `RecoveryBootResult` 消费字段逐字对齐（camelCase）。
        "phase": phase.as_str(),
        "phaseName": phase_name(phase),
        "counter": {
            "consecutiveFailures": consecutive_failures,
            "safemodeFailures": safemode_failures,
        },
        "disabledPlugins": disabled_plugins,
        "requiredPlugins": required_plugins,
        "loadSource": state.recovery_source.as_str(),
        "persistence": persistence,
        "lastContext": last_context,
    })
}

/// 一条关键事件摘要的线形态（camelCase）。
///
/// `failureKind` 是 [`BootContextEntry::failure_kind`] 的**派生**线名
/// （由 `phase` + `trial` 推出），不是新概念、不参与判定——只为前端少写一份
/// 分支逻辑。`pluginId` / `pluginState` 可以是 `null`（宿主自身 / 未登记）。
fn boot_context_entry_wire(c: &BootContextEntry) -> serde_json::Value {
    serde_json::json!({
        "ts": c.ts,
        "phase": c.phase.as_str(),
        "failureKind": c.failure_kind(),
        "pluginId": c.plugin_id,
        "trial": c.trial,
        "consecutiveFailures": c.consecutive_failures,
        "safemodeFailures": c.safemode_failures,
        "pluginState": c.plugin_state.map(|s| s.as_str()),
    })
}

/// `host_recover_boot`：查询启动恢复状态（§4.14）。
///
/// 恢复引擎由三处驱动，均在本文件中：
/// - [`CommandState::with_adapter_config`]：启动时载入持久化标记并做崩溃检测；
/// - [`cmd_recover_report`]：应用上报启动结果（驱动信号）；
/// - [`cmd_recover_trial_enable`]：安全模式内逐个试验性启用插件。
///
/// 阶段判定落到插件侧的唯一途径是 [`reconcile_recovery_phase`]：它把引擎的判定
/// 补发成 `SafemodeEnter` / `SafemodeExit`，由注册表状态机写入
/// `disabled_by_safemode`——那才是 `<oc-plugin-manager>` 角标读取的值。
///
/// A91（轮 47）：本命令同时是故障子系统的**确定性修复时刻**（主窗在恢复流里调用
/// 它即触发）——事件总线会话态清零（订阅/审批/队列；topic 声明保留）、注册表修不了
/// 则如实隔离。修复成败不改变本命令的读取语义；结果落在各自边界状态里
/// （`host_resource_stats` 的 `faults` 块与后续被拒命令的错误文案都看得到）。
pub fn cmd_recover_boot(state: &SubstrateState) -> HostResult<serde_json::Value> {
    guard("recover_boot", || {
        let _ = reconcile_events_boundary(state);
        let _ = reconcile_registry_boundary(state);
        Ok(recovery_boot_payload(state))
    })?
}

/// `host_recover_boot` 的**带身份判定**版本（轮 12）。
///
/// 返回的是**应用级**恢复态势（是否进安全模式、失败计数、阶段判定）。插件读它 =
/// 侦察宿主当前是否处于降级运行态（并可据此挑时机），和 `host_production_doctor`
/// 被定为 privileged 的理由同一类：部署/运行状态情报面。启动时对账与
/// `cmd_recover_trial_enable` 的驱动全在主窗侧，插件侧没有合法读取场景。
pub fn cmd_recover_boot_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_recover_boot")?;
    cmd_recover_boot(state)
}

/// `host_recover_report`：上报启动结果（§4.14 的**驱动信号**）。
///
/// `outcome` 是闭集（`success` | `failure`），表外值直接拒绝——不静默当成
/// failure 处理。
///
/// `pluginId` 只在一种情况下改变计数路径：该插件当前处于 `TrialEnable`
/// （试验性启用）状态时，本次失败按**试验失败**记（`record_trial_failure`，
/// 只累加该插件的试验次数、不累入全局计数），1 次即回落
/// `disabled-by-safemode` 且之后不可再试。否则 `pluginId` 仅作诊断提示。
///
/// 两次上报都会**清除** `bootInFlight` 标记：上报意味着本次启动已经得出明确
/// 结论，下一次启动不应重复计数。真正的崩溃（没来得及上报）由标记文件暴露。
///
/// **入口**：wire 层转调 [`cmd_recover_report_as`]（插件只能报自己的、不能认领
/// 应用级 `None`），本函数是**不过身份**的核心。
pub fn cmd_recover_report(
    state: &SubstrateState,
    outcome: &str,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    guard("recover_report", || {
        let is_success = outcome == "success";
        if !is_success && outcome != "failure" {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "host_recover_report 的 outcome 只接受 `success` 或 `failure`，收到 `{outcome}`"
                ),
            ));
        }

        let engine_action: String = {
            let mut engine = state.recovery.lock();
            if is_success {
                engine.record_boot_success();
                "bootSuccess".to_string()
            } else {
                match plugin_id {
                    Some(pid)
                        if engine.plugin_state(pid) == Some(RecoveryPluginState::TrialEnable) =>
                    {
                        // 试验性启用插件失败：按试验记（只累加该插件的试验次数），
                        // 并把这一条带真实时间戳写进诊断上下文。
                        let st = engine.record_trial_failure_at(pid, recovery::now_ms());
                        st.as_str().to_string()
                    }
                    _ => {
                        // **归因而非入账**：`pluginId` 只是「疑似故障插件」提示，
                        // 绝不因此把该插件记进试验失败预算（那会让安全模式里
                        // 崩过一次的插件永久失去试启用机会）。
                        engine.record_boot_failure_suspected_at(plugin_id, recovery::now_ms());
                        "bootFailure".to_string()
                    }
                }
            }
        };

        // 判定落地到注册表，并清除 in-flight 标记。
        let phase = reconcile_recovery_phase(state);
        // 两把锁**顺序持有、绝不重叠**：先在 recovery 锁内取出序列化快照并释放，
        // 再单独持 recovery_store 锁落盘。若在持 recovery_store 锁时去取
        // recovery 锁，会与 `recovery_boot_payload` 的 recovery → recovery_store
        // 顺序构成 ABBA 死锁；而持 recovery_store 锁调用 `recovery_boot_payload`
        // 更是直接的自死锁（`parking_lot` 不重入）。
        let engine_snapshot = state.recovery.lock().to_json();
        {
            let mut store = state.recovery_store.lock();
            store.in_flight = false;
            store.save_json(&engine_snapshot);
        }

        let payload = recovery_boot_payload(state);
        let mut result = payload.clone();
        result["outcome"] = serde_json::json!(if is_success { "success" } else { "failure" });
        result["engineAction"] = serde_json::json!(engine_action);
        result["suspectedPlugin"] = serde_json::json!(plugin_id);
        result["phaseReconcile"] = serde_json::to_value(phase).unwrap_or_default();
        Ok(result)
    })?
}

/// `host_recover_report` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条要绑身份（危害原文）
///
/// `pluginId` 决定这次上报的后果落在谁头上，而后果**是真的、且不可逆**：
///
/// - 该插件正处于 `TrialEnable` 时，本次 `failure` 走 `record_trial_failure_at`——
///   **消耗的是它的试验预算**，1 次即把它打回 `disabled-by-safemode` 且**之后不可
///   再试**。所以不判定的话，插件 A 报一条 `"failure", pluginId = "com.b"` 就能把
///   正在被人工试启的插件 B 永久关掉（跨插件 DoS：动的是别人的恢复配额）；
/// - 非试验态时它会写进**故障归因**（`suspectedPlugin` / 诊断上下文里的
///   `pluginId`）——插件能借此把故障栽赃到别的插件名下，运维按诊断去排查错对象；
/// - 两条路径都会推进**全局**启动失败计数（一次 `success` 才清零）。`pluginId` 为
///   `None` 表示「**应用级**启动结果」，那更是插件无权主张的档位：反复报
///   `failure` 就能把整个应用推进安全模式 / 修复模式。
///
/// 规则因此是"署名必须是自己"（[`require_self_plugin_scope`]）：主窗可报任意
/// `pluginId` 或 `None`（宿主才是"这次启动成没成"的权威），插件只能报**自己**。
/// 拒绝路径**零副作用**：别人的试验预算、阶段、计数器、诊断上下文一律不动
/// （有测试拿"正处于 `TrialEnable` 的 B"当靶子断言这一点）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_recover_report_as(
    caller: &Caller,
    state: &SubstrateState,
    outcome: &str,
    plugin_id: Option<&str>,
) -> HostResult<serde_json::Value> {
    require_self_plugin_scope(caller, "host_recover_report", plugin_id)?;
    cmd_recover_report(state, outcome, plugin_id)
}

/// `host_recover_trial_enable`：安全模式内试验性启用一个插件。
///
/// 引擎只允许 `phase == Safemode` 且该插件未被试验失败过。发事件前先做一次
/// 阶段对账：刚装载、尚未被对账标记的插件停在 `Installed` 等非 DISABLED 态，
/// 直接发试启事件会撞非法迁移（事件被拒但引擎已改，两侧发散）；对账先把应
/// 禁用者标成 DISABLED + 安全模式标志，试启才有合法迁移起点。
///
/// 成功后发 `TrialEnable`（D28）而非 `SafemodeExit`：`SetTrial` 让注册表记入
/// 独立试启预算，且 `trial_from_safemode` 置位后该插件**自行上报错误**时会走
/// D28 回落（1 次即打回 DISABLED + 安全模式标志），与引擎侧的试验失败判定
/// 同向——两侧不是各自为政（反向同步见 `cmd_lifecycle_report`）。
/// 返回的 `phaseReconcile` 即本调用前置对账的结果（诊断用）。
///
/// **主窗专属（轮 11 第二批）**：试启是**恢复管理操作**（改别人的插件状态、
/// 消耗别人的试验预算），wire 层转调 [`cmd_recover_trial_enable_as`] 判定，
/// 本函数是**不过身份**的核心。
pub fn cmd_recover_trial_enable(
    state: &PluginRuntimeState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    guard("recover_trial_enable", || {
        let id = PluginId::new(plugin_id)?;
        // 先登记进引擎（恢复管辖范围内），再对账、再试启。
        {
            let mut engine = state.recovery.lock();
            engine.register_plugin(id.as_str());
        }
        let phase_reconcile = reconcile_recovery_phase(state);

        // Recovery safety policy takes precedence over action de-duplication. Once a plugin has
        // exhausted its one-shot trial budget, a replay in the same incident must remain a hard
        // E_PLUGIN_DISABLED rejection rather than being hidden as a de-duplicated success.
        if state.recovery.lock().counter().trial_exhausted(id.as_str()) {
            return Err(HostError::new(
                ErrorCode::E_PLUGIN_DISABLED,
                format!("插件 `{id}` 已试验失败，回落 disabled-by-safemode，不可再次试启"),
            ));
        }

        // 必须先取出结果再分支：`match state.recovery.lock().trial_enable(..)` 会让
        // 临时 guard 被临时值生命周期延长规则持有到整个 match 结束，于是错误分支里
        // 再次 `state.recovery.lock()` 就是自死锁（`parking_lot` 不重入，表现为
        // 空转而非挂起）。
        // V4 A92: real RecoveryExecutor wiring. The incident sequence comes from the latest
        // persisted failure context, so duplicate user clicks in one incident share a key while
        // a later independent incident gets a new sequence.
        let (action, should_execute) = {
            let mut engine = state.recovery.lock();
            let incident_seq =
                engine.last_context().map(|entry| entry.ts).unwrap_or_else(recovery::now_ms);
            let action = RecoveryAction {
                plugin_id: id.as_str().to_string(),
                action_kind: "trial-enable".to_string(),
                idempotency_key: RecoveryAction::idempotency_key(
                    id.as_str(),
                    "trial-enable",
                    incident_seq,
                ),
            };
            let should_execute = engine.execute_action(&action).map_err(|e| {
                HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("恢复动作正在执行或不可开始：{e}"),
                )
            })?;
            (action, should_execute)
        };

        if !should_execute {
            let payload = recovery_boot_payload(state);
            let mut result = payload.clone();
            result["pluginId"] = serde_json::json!(id.as_str());
            result["engineAction"] = serde_json::json!("trialEnable:deduplicated");
            result["phaseReconcile"] = serde_json::to_value(phase_reconcile).unwrap_or_default();
            return Ok(result);
        }

        let previous_plugin_state = state.recovery.lock().plugin_state(id.as_str());
        let trial = state.recovery.lock().trial_enable(id.as_str());
        match trial {
            Ok(()) => {}
            Err(tauron_recovery::RecoveryError::RestrictedInSafemode) => {
                state.recovery.lock().abort_recovery();
                let phase = state.recovery.lock().decide_boot_phase().as_str();
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("插件 `{id}` 只能在安全模式下试验性启用；当前阶段是 `{phase}`"),
                ));
            }
            Err(tauron_recovery::RecoveryError::TrialExhausted(pid)) => {
                state.recovery.lock().abort_recovery();
                return Err(HostError::new(
                    ErrorCode::E_PLUGIN_DISABLED,
                    format!("插件 `{pid}` 已试验失败，回落 disabled-by-safemode，不可再次试启"),
                ));
            }
            Err(e) => {
                state.recovery.lock().abort_recovery();
                return Err(HostError::new(
                    ErrorCode::E_STATE_INVALID_TRANSITION,
                    format!("试验性启用失败：{e}"),
                ));
            }
        }

        // 试启成功后让注册表状态机跟着走 D28 试启迁移（§4.14：引擎负责判，
        // 状态机负责执行）。外部副作用成功后才写 EffectRecord + commit action；
        // 拒绝则 abort，允许同一 incident 的下一次显式请求安全重放。
        // Apply the external registry effect. A TransitionOutcome with `illegal=true` is a
        // rejection even though report_event itself returned Ok; treating it as committed would
        // persist an EffectRecord for an effect that never happened.
        let registry_effect: HostResult<(String, bool)> =
            if let Some(out) = state.registry.find(&id).map(|e| e.state.state) {
                use tauron_host::lifecycle::State;
                if out == State::Enabled || out == State::Running {
                    // Desired external state already exists; the recovery action is satisfied,
                    // but this invocation did not create a new external effect.
                    Ok(("alreadyEnabled".to_string(), false))
                } else {
                    match state.registry.report_event(&id, Event::TrialEnable) {
                        Ok(o) if !o.illegal => Ok((format!("trialEnable:{}", o.to.as_str()), true)),
                        Ok(o) => Err(HostError::new(
                            ErrorCode::E_STATE_INVALID_TRANSITION,
                            format!(
                                "恢复试启被注册表状态机拒绝：{} -> {}",
                                o.from.as_str(),
                                o.to.as_str()
                            ),
                        )),
                        Err(error) => Err(error),
                    }
                }
            } else {
                // Engine-only recovery remains supported for a plugin that is not yet in the
                // runtime registry. There is no external effect to record in that case.
                Ok(("notInRegistry".to_string(), false))
            };

        let (engine_action, external_effect_applied) = match registry_effect {
            Ok(result) => result,
            Err(error) => {
                {
                    let mut engine = state.recovery.lock();
                    engine.rollback_trial_enable_if_unchanged(id.as_str(), previous_plugin_state);
                    engine.abort_recovery();
                }
                persist_recovery_engine(state);
                return Err(error);
            }
        };

        {
            let mut engine = state.recovery.lock();
            if external_effect_applied {
                engine.record_effect(EffectRecord {
                    effect_id: format!("registry:{}:{}", id.as_str(), action.idempotency_key),
                    action_key: action.idempotency_key.clone(),
                    ts: recovery::now_ms(),
                });
            }
            engine.complete_recovery();
        }

        // Persist the action/effect ledger together with the recovery engine. This keeps
        // deduplication valid across process restart, not only inside one in-memory session.
        persist_recovery_engine(state);

        let payload = recovery_boot_payload(state);
        let mut result = payload.clone();
        result["pluginId"] = serde_json::json!(id.as_str());
        result["engineAction"] = serde_json::json!(engine_action);
        result["phaseReconcile"] = serde_json::to_value(phase_reconcile).unwrap_or_default();
        Ok(result)
    })?
}

/// `host_recover_trial_enable` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// 试启是**恢复管理操作**，不是插件自己能发起的事：
///
/// - 它改的是**别人的**插件状态——把 `disabled-by-safemode` 的插件重新放行；
/// - 它**消耗别人的试验预算**（引擎侧 1 次即回落、之后不可再试），所以插件 A 反复
///   对插件 B 调它，等于把 B 唯一的一次试启用机会烧掉（跨插件配额攻击）；
/// - 它会推进阶段对账（`reconcile_recovery_phase`），把安全模式判定写回注册表——
///   一个插件不该有权驱动全局恢复状态机。
///
/// 判定与 [`require_main_window`] 的其余调用点同源、同码（[`ErrorCode::E_AUTH_DENIED`]），
/// 且在实际试启**之前**：拒绝时引擎阶段、插件状态、试验预算一律不动。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_recover_trial_enable_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    plugin_id: &str,
) -> HostResult<serde_json::Value> {
    require_main_window(caller, "host_recover_trial_enable")?;
    cmd_recover_trial_enable(state, plugin_id)
}

// ──────────────────────────────────────────────────────────────────────────
// 品牌（任务二：接通孤儿 crate `tauron-brand`）
// ──────────────────────────────────────────────────────────────────────────

/// `host_brand_info` 的线形返回（camelCase）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrandInfo {
    /// 应用标识符（如 `com.tauron.standard`）。
    pub identifier: String,
    /// 自定义协议名。
    pub protocol_scheme: String,
    /// 自启项名称。
    pub autostart_name: String,
    /// 数据目录名。
    pub data_dir: String,
    /// 快捷键绑定（键名 → 按键序列）。
    pub shortcuts: std::collections::BTreeMap<String, String>,
    /// 图标路径（平台字符串 → 相对路径）。
    pub icons: std::collections::BTreeMap<String, String>,
}

/// 品牌配置**内联 JSON**环境变量（优先）。
pub const BRAND_CONFIG_JSON_ENV: &str = "TAURON_BRAND_CONFIG_JSON";
/// 品牌配置**文件路径**（JSON）环境变量。
pub const BRAND_CONFIG_ENV: &str = "TAURON_BRAND_CONFIG";

fn brand_err(e: tauron_brand::BrandError) -> HostError {
    HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("品牌配置非法：{e}"))
}

/// 投影线形（`Platform` 取其 `as_str`，与 TS 侧字符串约定一致）。
fn brand_info_from_raw(cfg: &tauron_brand::BrandConfig) -> HostResult<BrandInfo> {
    cfg.validate_required().map_err(brand_err)?;
    cfg.validate_shortcuts().map_err(brand_err)?;
    Ok(BrandInfo {
        identifier: cfg.identifier.clone(),
        protocol_scheme: cfg.protocol_scheme.clone(),
        autostart_name: cfg.autostart_name.clone(),
        data_dir: cfg.data_dir.clone(),
        shortcuts: cfg.shortcuts.clone(),
        icons: cfg.icons.iter().map(|(p, v)| (p.as_str().to_string(), v.clone())).collect(),
    })
}

/// 从环境变量装载品牌配置（真实实现，接 `tauron-brand`）。
///
/// 来源二选一（内联优先）：[`BRAND_CONFIG_JSON_ENV`] / [`BRAND_CONFIG_ENV`]。
/// 都未设置 = **未配置品牌** → `None`，命令层如实返回 `UnsupportedBody`
/// （不伪造一个默认品牌）。
fn load_brand_config_from_env() -> HostResult<Option<tauron_brand::BrandConfig>> {
    let raw = match std::env::var(BRAND_CONFIG_JSON_ENV) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => match std::env::var(BRAND_CONFIG_ENV) {
            Ok(path) if !path.trim().is_empty() => std::fs::read_to_string(&path).map_err(|e| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("读取品牌配置文件 `{path}` 失败：{e}"),
                )
            })?,
            _ => return Ok(None),
        },
    };
    let cfg = serde_json::from_str::<tauron_brand::BrandConfig>(&raw).map_err(|e| {
        HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("品牌配置 JSON 解析失败：{e}"))
    })?;
    Ok(Some(cfg))
}

/// 品牌来源是否已配置（供 [`cmd_host_capabilities`] 推导 `brand` 域可用性）。
///
/// 与 [`cmd_brand_info`] **同源**：只看那两个环境变量是否非空。JSON 非法也算
/// "已配置"——域本身可用，内容问题由 `host_brand_info` 的返回如实报告。
fn brand_configured() -> bool {
    let non_empty = |key: &str| std::env::var(key).map(|v| !v.trim().is_empty()).unwrap_or(false);
    non_empty(BRAND_CONFIG_JSON_ENV) || non_empty(BRAND_CONFIG_ENV)
}

/// `host_brand_info`：品牌信息（**真实实现**，接孤儿 crate `tauron-brand`）。
///
/// 此前是恒 `UnsupportedBody` 的桩；现在未配置来源时仍如实降级，配置了则走
/// `tauron-brand` 的校验（必填字段 / 快捷键唯一性）——**不返回裸桩**。
///
/// （`host_market_check` 的实现在"壳扩展"一节，与 download/install 放在一起——
/// R8 把三条商城命令的线形统一成有类型的结构，三个定义不该分散在两处。）
pub fn cmd_brand_info(_state: &SubstrateState) -> HostResult<ProviderResult<BrandInfo>> {
    guard("brand_info", || match load_brand_config_from_env()? {
        None => Ok(ProviderResult::Unsupported(unsupported_body(
            "brand provider is not configured",
            Some("设置环境变量 TAURON_BRAND_CONFIG_JSON 或 TAURON_BRAND_CONFIG"),
        ))),
        Some(cfg) => Ok(ProviderResult::Value(brand_info_from_raw(&cfg)?)),
    })?
}

// ──────────────────────────────────────────────────────────────────────────
// R9 命令面：menu / tray / fs / http / updater / theme
//
// **全部为「主窗专属」**（`require_main_window`）：它们是宿主 UI 的编排原语，
// 不属于插件可触达命令面，因此**不进** `tauron_host::authz` 档位表、也不进
// `capabilities.ts`（见 authz.rs 的"不收录但有代码层判定"清单）。
// ──────────────────────────────────────────────────────────────────────────

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

/// 单次 `host_fs_read` 的字节上限（4 MiB）。
pub const FS_MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

fn fs_unavailable() -> UnsupportedBody {
    unsupported_body(
        "fs provider is not configured",
        Some("在 AdapterConfig.fs_allowed_roots 中配置允许根目录（空 = 不可用）"),
    )
}

/// 规范化**已存在的最近祖先**并把剩余组件原样接回：允许对尚不存在的目标
/// （如 `host_fs_write` 的新文件）做 root 前缀判定，而不用整路径 `canonicalize`
/// 失败即拒绝。
fn canonicalize_existing_prefix(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let mut pending: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor = path;
    loop {
        match cursor.canonicalize() {
            Ok(mut base) => {
                for component in pending.iter().rev() {
                    base.push(component);
                }
                return Ok(base);
            }
            Err(error) => {
                let Some(parent) = cursor.parent() else {
                    return Err(error);
                };
                let Some(name) = cursor.file_name() else {
                    return Err(error);
                };
                pending.push(name);
                cursor = parent;
            }
        }
    }
}

/// V4 A95：把授权结果转换为“可信 root + portable relative path”。
///
/// 预检查只用于选择 root；真正 read/write/stat 在 Unix 通过 openat/O_NOFOLLOW
/// 沿同一 root handle 执行，因此 check/use 期间替换 symlink 仍会被拒绝。
///
/// 候选路径先按**最近的已存在祖先**规范化再比对前缀：宿主装配期的 roots 已经
/// `canonicalize`（Windows 上会带 `\\?\` verbatim 前缀），直接用原始字符串做
/// `starts_with` 会把 root 内的合法路径误判为越界。
fn scoped_within_roots(roots: &[PathBuf], raw: &str) -> HostResult<tauron_host::ScopedPath> {
    let candidate = PathBuf::from(raw);
    if !candidate.is_absolute() {
        return Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!("fs 路径 `{raw}` 必须是允许 root 下的绝对路径"),
        ));
    }
    let candidate = canonicalize_existing_prefix(&candidate).map_err(|error| {
        HostError::new(ErrorCode::E_AUTH_DENIED, format!("fs 路径 `{raw}` 无法规范化：{error}"))
    })?;

    for root in roots {
        if !candidate.starts_with(root) {
            continue;
        }
        let relative = candidate.strip_prefix(root).map_err(|_| {
            HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("fs 路径 `{raw}` 无法转换为 root-relative path"),
            )
        })?;
        let relative = if relative.as_os_str().is_empty() {
            "."
        } else {
            relative.to_str().ok_or_else(|| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("fs 路径 `{raw}` 含无法在线协议表达的非 UTF-8 组件"),
                )
            })?
        };
        return tauron_host::ScopedPath::new(root.clone(), relative).map_err(|error| {
            HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("fs scoped path 拒绝 `{raw}`: {error}"),
            )
        });
    }

    Err(HostError::new(ErrorCode::E_AUTH_DENIED, format!("fs 路径 `{raw}` 不在宿主允许 root 内")))
}

/// `host_fs_read`：读取文本文件（允许根目录内；超限截断并如实标注）。
pub fn cmd_fs_read(
    state: &SubstrateState,
    path: &str,
    max_bytes: Option<u64>,
) -> HostResult<ProviderResult<FsReadResult>> {
    guard("fs_read", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        let limit = max_bytes.unwrap_or(FS_MAX_READ_BYTES).min(FS_MAX_READ_BYTES);
        let (bytes, truncated) = state.fs_sink.read(&scoped, limit)?;
        Ok(ProviderResult::Value(FsReadResult {
            path: scoped.display_path().to_string_lossy().into_owned(),
            text: String::from_utf8_lossy(&bytes).into_owned(),
            bytes: bytes.len() as u64,
            truncated,
        }))
    })?
}

/// `host_fs_read` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_read_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
    max_bytes: Option<u64>,
) -> HostResult<ProviderResult<FsReadResult>> {
    require_main_window(caller, "host_fs_read")?;
    cmd_fs_read(state, path, max_bytes)
}

/// `host_fs_write`：写入文本文件（覆盖；允许根目录内）。
pub fn cmd_fs_write(
    state: &SubstrateState,
    path: &str,
    text: &str,
) -> HostResult<ProviderResult<FsWriteResult>> {
    guard("fs_write", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        let bytes = state.fs_sink.write(&scoped, text.as_bytes())?;
        Ok(ProviderResult::Value(FsWriteResult {
            path: scoped.display_path().to_string_lossy().into_owned(),
            bytes,
        }))
    })?
}

/// `host_fs_write` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_write_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
    text: &str,
) -> HostResult<ProviderResult<FsWriteResult>> {
    require_main_window(caller, "host_fs_write")?;
    cmd_fs_write(state, path, text)
}

/// `host_fs_list`：列目录（允许根目录内）。
pub fn cmd_fs_list(state: &SubstrateState, path: &str) -> HostResult<ProviderResult<Vec<FsEntry>>> {
    guard("fs_list", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.list(&scoped).map(ProviderResult::Value)
    })?
}

/// `host_fs_list` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_list_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
) -> HostResult<ProviderResult<Vec<FsEntry>>> {
    require_main_window(caller, "host_fs_list")?;
    cmd_fs_list(state, path)
}

/// `host_fs_stat`：取元数据（允许根目录内）。
pub fn cmd_fs_stat(state: &SubstrateState, path: &str) -> HostResult<ProviderResult<FsStat>> {
    guard("fs_stat", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.stat(&scoped).map(ProviderResult::Value)
    })?
}

/// `host_fs_stat` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_stat_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
) -> HostResult<ProviderResult<FsStat>> {
    require_main_window(caller, "host_fs_stat")?;
    cmd_fs_stat(state, path)
}

/// `host_fs_mkdir`：建目录（允许根目录内）。
pub fn cmd_fs_mkdir(
    state: &SubstrateState,
    path: &str,
    recursive: bool,
) -> HostResult<ProviderResult<()>> {
    guard("fs_mkdir", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.mkdir(&scoped, recursive)?;
        Ok(ProviderResult::Value(()))
    })?
}

/// `host_fs_mkdir` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_mkdir_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
    recursive: bool,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_fs_mkdir")?;
    cmd_fs_mkdir(state, path, recursive)
}

/// `host_fs_remove`：删除文件或空目录（允许根目录内；**不递归**）。
pub fn cmd_fs_remove(state: &SubstrateState, path: &str) -> HostResult<ProviderResult<()>> {
    guard("fs_remove", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.remove(&scoped)?;
        Ok(ProviderResult::Value(()))
    })?
}

/// `host_fs_remove` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_remove_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_fs_remove")?;
    cmd_fs_remove(state, path)
}

/// HTTP method 的闭集校验（`GET` / `POST`，大小写不敏感）。
fn validated_http_method(method: &str) -> HostResult<String> {
    let m = method.trim().to_ascii_uppercase();
    match m.as_str() {
        "GET" | "POST" => Ok(m),
        other => Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("HTTP method `{other}` 非法：只接受 GET / POST"),
        )),
    }
}

/// `NetworkPolicy` 拒绝 → `HostError` 的统一映射（轮 48 / A96：URL 授权、逐跳
/// `authorize_redirect` 与 `authorize_resolution` 复检共用同一条判定）。
fn http_policy_denied(error: &tauron_host::NetworkPolicyError) -> HostError {
    let code = match error {
        tauron_host::NetworkPolicyError::EmptyUrl
        | tauron_host::NetworkPolicyError::InvalidUrl(_)
        | tauron_host::NetworkPolicyError::SchemeDenied(_)
        | tauron_host::NetworkPolicyError::HttpDenied
        | tauron_host::NetworkPolicyError::EmbeddedCredentialsDenied
        | tauron_host::NetworkPolicyError::HostMissing
        | tauron_host::NetworkPolicyError::PortMissing(_)
        | tauron_host::NetworkPolicyError::InvalidDomainRule(_)
        | tauron_host::NetworkPolicyError::InvalidRedirectLimit => ErrorCode::E_INVALID_MANIFEST,
        _ => ErrorCode::E_AUTH_DENIED,
    };
    HostError::new(code, format!("HTTP network policy denied request: {error}"))
}

fn authorize_http_url(
    policy: &tauron_host::NetworkPolicy,
    url: &str,
) -> HostResult<tauron_host::AuthorizedUrl> {
    policy.authorize_url(url).map_err(|error| http_policy_denied(&error))
}

/// `host_http_request`：发起一次 HTTP 请求（主窗专属）。
///
/// 命令层做**参数校验 + 逐跳授权流**；能力可用性由 sink 决定：缺省
/// [`UnavailableHttpSink`] 恒返回 `UnsupportedBody`（诚实降级，见其文档）。
///
/// **单跳契约（轮 48 / A96）**：redirect 跟随与解析复检由**宿主**逐跳执行——
/// sink 只发单跳；3xx 的每一跳先经 `authorize_redirect`（含跨源凭据剥离、
/// `max_redirects` 上界）与 `resolve_redirect_location`（相对 Location 归一），
/// 每个响应里 provider 回报的解析地址经 `authorize_resolution` 复检私网/字面 IP。
/// 由此 `authorize_redirect` / `authorize_resolution` 从"只有模块自测"变为
/// 这条生产命令路径上的真实闸门。
pub fn cmd_http_request(
    state: &SubstrateState,
    spec: &HttpRequestSpec,
) -> HostResult<ProviderResult<HttpResponseSpec>> {
    guard("http_request", || {
        validated_http_method(&spec.method)?;
        let mut hop_spec = spec.clone();
        let mut current = authorize_http_url(&state.http_policy, &hop_spec.url)?;
        let mut current_url = hop_spec.url.clone();
        if state.http_sink.native_supported()
            && state.http_sink.network_enforcement()
                != tauron_host::NetworkEnforcement::RedirectAndDns
        {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                "HTTP provider does not enforce V4 redirect/DNS/private-network policy",
                Some("use a provider with NetworkEnforcement::RedirectAndDns"),
            )));
        }
        let mut hop: u8 = 0;
        loop {
            let response = match state.http_sink.request(&hop_spec, &state.http_policy)? {
                ProviderResult::Value(response) => response,
                other => return Ok(other),
            };
            if !response.resolved_addrs.is_empty() {
                let mut resolved = Vec::with_capacity(response.resolved_addrs.len());
                for raw in &response.resolved_addrs {
                    let address = raw.parse::<std::net::IpAddr>().map_err(|_| {
                        HostError::new(
                            ErrorCode::E_AUTH_DENIED,
                            format!(
                                "HTTP network policy denied request: provider reported an \
                                 unparseable resolved address `{raw}`"
                            ),
                        )
                    })?;
                    resolved.push(address);
                }
                state
                    .http_policy
                    .authorize_resolution(&current, &resolved)
                    .map_err(|error| http_policy_denied(&error))?;
            }
            if !(300..=399).contains(&response.status) {
                return Ok(ProviderResult::Value(response));
            }
            let Some(location) = response.headers.get("location") else {
                // 3xx 而无 `location`：没有可授权的下一跳，按最终响应交还调用方。
                return Ok(ProviderResult::Value(response));
            };
            hop = hop.saturating_add(1);
            let has_credentials = hop_spec.headers.keys().any(|key| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                )
            });
            let next_url = tauron_host::resolve_redirect_location(&current_url, location)
                .map_err(|error| http_policy_denied(&error))?;
            let authorization = state
                .http_policy
                .authorize_redirect(&current, &next_url, hop, has_credentials)
                .map_err(|error| http_policy_denied(&error))?;
            if !authorization.forward_credentials {
                hop_spec.headers.retain(|key, _| {
                    !matches!(
                        key.to_ascii_lowercase().as_str(),
                        "authorization" | "cookie" | "proxy-authorization"
                    )
                });
            }
            current = authorization.target;
            current_url = next_url;
            hop_spec.url = current_url.clone();
        }
    })?
}

/// `host_http_request` 的**带身份判定**版本（仅主窗）。
pub fn cmd_http_request_as(
    caller: &Caller,
    state: &SubstrateState,
    spec: &HttpRequestSpec,
) -> HostResult<ProviderResult<HttpResponseSpec>> {
    require_main_window(caller, "host_http_request")?;
    cmd_http_request(state, spec)
}

fn updater_unavailable() -> ProviderResult<UpdaterCheckOutcome> {
    ProviderResult::Unsupported(unsupported_body(
        "updater provider is not configured",
        Some("注入 EndpointClient（DistributeUpdaterSink::with_endpoint）"),
    ))
}

/// `host_updater_check`：检查更新（主窗专属；真跑 `tauron-distribute`）。
pub fn cmd_updater_check(
    state: &SubstrateState,
    current_version: &str,
) -> HostResult<ProviderResult<UpdaterCheckOutcome>> {
    guard("updater_check", || {
        if current_version.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "current_version 不能为空".to_string(),
            ));
        }
        if !state.updater_sink.native_supported() {
            return Ok(updater_unavailable());
        }
        state.updater_sink.check(current_version)
    })?
}

/// `host_updater_check` 的**带身份判定**版本（仅主窗）。
pub fn cmd_updater_check_as(
    caller: &Caller,
    state: &SubstrateState,
    current_version: &str,
) -> HostResult<ProviderResult<UpdaterCheckOutcome>> {
    require_main_window(caller, "host_updater_check")?;
    cmd_updater_check(state, current_version)
}

/// `host_updater_status`：更新通道状态（主窗专属）。
///
/// 这是 [`ShellExtState::update_state`] 的**真实生产读取方**：把进程内更新状态机
/// （`None → downloaded → installed`）并入返回的 `state` 字段。此前该字段无读取方
/// （见其接入状态注释），本轮补上这条链路。
///
/// **`state` 与 `state_simulated` 必须成对读出**（轮 33）：`update_state` 的写入方
/// 是 `cmd_market_download` / `cmd_market_install` 的分派腿（缺省模拟，轮 40 起装配腿
/// 注入后为真），它们推进账本是如实的，但字符串本身分不出"模拟推进"与"真实推进"。
/// 把 `state` 单独上屏就会把"点了一下模拟安装"说成"已安装"，因此本命令同时带出
/// provenance 位。
///
/// **成对读出落在装配上**（轮 33）：本命令把 `(update_state, update_state_simulated)`
/// 一次性交给 [`UpdaterSink::status`]，由 sink 推导 provenance；sink 的签名里这两个
/// 参数是**必传**的，所以通道实现无法再只报 `state` 而不报它是哪来的，也无法自己
/// 断言"这条状态是真的"。
///
/// **线形**：入参无，返回
/// `{ available: bool, state: string | null, stateSimulated: bool, grayscalePercent: u32, crashGateStopped: bool, reason: string | null }`。
/// `stateSimulated` 在 `state === null` 时恒为 `false`；缺省装配有 `state` 时恒为 `true`，
/// 轮 40 起装配腿（[`DistributeUpgradeInstaller`]）真路径的推进由写入方的 provenance
/// 标记决定（真实效果成功后为 `false`）。
pub fn cmd_updater_status(state: &SubstrateState) -> HostResult<UpdaterStatus> {
    guard("updater_status", || {
        let ext = state.shell_ext.lock();
        Ok(state.updater_sink.status(ext.update_state.clone(), ext.update_state_simulated))
    })?
}

/// `host_updater_status` 的**带身份判定**版本（仅主窗）。
pub fn cmd_updater_status_as(caller: &Caller, state: &SubstrateState) -> HostResult<UpdaterStatus> {
    require_main_window(caller, "host_updater_status")?;
    cmd_updater_status(state)
}

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

// ──────────────────────────────────────────────────────────────────────────
// i18n（§4.20：语言状态单一来源 + 资源包 + 缺失键计数）
// ──────────────────────────────────────────────────────────────────────────

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

// ──────────────────────────────────────────────────────────────────────────
// 窗口管理命令（P1-1：Rust 窗口命令族；R8 §1：平台部分改走 WindowSink）
//
// **R8 之前的样子**：这里只有 `guard(...) + Ok(())` 桩，真实窗口操作写在
// `tauri.rs` 的 `#[tauri::command]` 包装器里（`window.minimize()` 等）。
// 于是"命令做了什么"分成两处，且平台那一半在单测里完全不可达。
//
// **现在**：核心命令一律转调 [`SubstrateState::window_sink`]。`label` 由 wire 层
// 从真实 `WebviewWindow` 取（沿用"身份/目标只从宿主侧来"的既有口径），核心不解析、
// 不信任任何前端入参。
// ──────────────────────────────────────────────────────────────────────────

/// `host_window_minimize`：最小化**调用方自己**的窗口。
pub fn cmd_window_minimize(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_minimize", || state.window_sink.minimize(label))?
}

/// `host_window_maximize`：最大化调用方的窗口。
pub fn cmd_window_maximize(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_maximize", || state.window_sink.maximize(label))?
}

/// `host_window_restore`：还原调用方的窗口（从最大化/最小化）。
pub fn cmd_window_restore(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_restore", || state.window_sink.restore(label))?
}

/// `host_window_close`：关闭调用方的窗口。
///
/// 窗口销毁后的回收**不在这里**：宿主在 `on_window_event(Destroyed)` 里调
/// [`crate::tauri::cleanup_closed_window`]（订阅/分组/pending 调用一起回收）。
/// 在这里顺手清会漏掉"用户点标题栏关闭"这条路径——那是同一件事，不该有两套清理。
pub fn cmd_window_close(state: &SubstrateState, label: &str) -> HostResult<()> {
    guard("window_close", || state.window_sink.close(label))?
}

/// `host_window_quit`：退出应用。
pub fn cmd_window_quit(state: &SubstrateState) -> HostResult<()> {
    guard("window_quit", || state.window_sink.quit())?
}

/// `host_window_quit` 的**带身份判定**版本（轮 12）。
///
/// # 为什么这条必须有判定
///
/// 它关的不是调用方自己的窗口，而是**整个应用**（`TauriWindowSink::quit` 走
/// `app.exit(0)`）：插件窗调一次就把宿主连同所有邻居插件一起关掉，是现成的 DoS
/// 开关。同族的 `host_window_relaunch`（重启）在轮 11 已判为仅主窗——quit 比
/// relaunch 更彻底，没有理由反而不判。其余窗口命令（close/minimize/maximize/
/// restore/set_position/set_size）传的是 `window.label()`（调用方**自己**的窗口），
/// 天然按主体隔离，所以不需要判定。
pub fn cmd_window_quit_as(caller: &Caller, state: &SubstrateState) -> HostResult<()> {
    require_main_window(caller, "host_window_quit")?;
    cmd_window_quit(state)
}

/// `host_window_relaunch`：**先对账恢复阶段、再重启**（主窗专属，R8 §3）。
///
/// # 顺序为什么不能反（代码里的显式顺序就是这条不变量）
///
/// 重启前必须先 [`reconcile_recovery_phase`]，因为对账是**把引擎的判定写回注册表**
/// 的唯一途径（`disabled_by_safemode` 的权威来源）。反过来的话：本次进程带着
/// 「注册表标志尚未与引擎判定对齐」的状态退出，而恢复标记与阶段计数器是**跨进程
/// 持久化**的——下一次启动读回的仍是旧判定，于是"该禁用的插件没被禁用 → 再次
/// 启动失败 → 再重启"，形成重启循环（正是安全模式要打断的那类循环）。
///
/// 顺序在代码里是**两行、按序执行**（先 `reconcile`，再 `relaunch`），对账结果也随
/// 返回值一起给出去，因此这个顺序在运行时可观测、可断言
/// （见 `sink_tests::window_relaunch_reconciles_before_requesting_restart`）。
///
/// # 谁能调
///
/// **仅主窗**：重启是应用级动作，插件 webview 触发它等于把"重启风暴"的开关交给
/// 任意插件（而且它同时改写全局恢复阶段判定）。判定在代码层执行（不只是 ACL）。
pub fn cmd_window_relaunch_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<WindowRelaunchOutcome> {
    require_main_window(caller, "host_window_relaunch")?;
    guard("window_relaunch", || {
        // ① **先**对账：把恢复引擎的阶段判定补发到插件侧（顺序不可交换，见文档）。
        let reconcile = reconcile_recovery_phase(state);
        // ② **后**重启。降级实现返回 `false`（非 Tauri 宿主没有重启原语）——
        //    此时如实上报，不假装已经重启。
        let requested = state.window_sink.relaunch()?;
        Ok(WindowRelaunchOutcome {
            reconcile,
            relaunch_requested: requested,
            reason: (!requested).then(|| {
                "宿主无重启原语（非 Tauri 宿主 / 未注入 TauriWindowSink）：\
                 对账已完成，但**没有**请求任何重启"
                    .to_string()
            }),
        })
    })?
}

/// `host_window_create`：为**已注册**插件创建主面板窗口（主窗专属，R8 §3）。
///
/// # 语义与拒绝面
///
/// - label **由核心铸造**，恒为 `plugin-<id>`（身份模型的载体，不由入参决定）；
/// - `<id>` 必须**已存在于注册表**：不存在的 id 一律拒绝（错误码复用
///   [`ErrorCode::E_UNKNOWN_PLUGIN`]），**不创建半态窗口**——一个 label 指向
///   不存在插件的 webview 会被身份解析当作合法单元（label 就是身份），却没有任何
///   注册表条目与它对应，之后所有 self 档命令都会以"未注册"失败；
/// - 插件必须声明 `entry.ui`：没有 UI 入口就没有可加载的页面。缺它 → 拒绝
///   （[`ErrorCode::E_INVALID_MANIFEST`]）。**不用一个编造的默认 URL 顶上**——
///   那会把"配置缺失"变成"能打开但内容是错的"；
/// - URL **只来自 manifest**（线形里没有 url 字段）：让调用方指定 URL 等于让主窗
///   把"带插件身份的 webview"指向任意地址，而 label 决定身份 → 提权跳板。
///
/// # 清理
///
/// 创建后的回收**复用既有钩子**：label 是 `plugin-<id>`，`cleanup_closed_window`
/// 正是用这个前缀推出插件 id 来回收订阅 / 分组 / pending 调用的。宿主只需保证
/// `on_window_event(Destroyed)` 那一行已接线（示例应用 `main.rs` 里有）。
///
/// # 谁能调
///
/// **仅主窗**：为任意已注册插件开窗（含被安全模式禁用的插件）是宿主管理面动作。
pub fn cmd_window_create_as(
    caller: &Caller,
    state: &PluginRuntimeState,
    req: &WindowCreateRequest,
) -> HostResult<WindowCreateOutcome> {
    require_main_window(caller, "host_window_create")?;
    guard("window_create", || {
        let id = PluginId::new(&req.plugin_id)?;
        // 注册表是"这个插件存在"的唯一权威来源。`require` 失败 = `E_UNKNOWN_PLUGIN`，
        // 且**在任何窗口副作用之前**失败。
        let entry = state.registry.require(&id)?;
        let url = entry.manifest.entry.ui.clone().ok_or_else(|| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!(
                    "插件 `{id}` 的 manifest 未声明 `entry.ui`：没有 UI 入口就没有可加载的页面，\
                     拒绝创建窗口（不编造默认 URL）"
                ),
            )
        })?;
        validate_plugin_ui_entry(&url)?;

        let spec = WindowCreateSpec {
            // 身份 label 的**唯一**铸造点（与 `PluginIdentity::new` 同一约定）。
            label: format!("plugin-{id}"),
            plugin_id: id.as_str().to_string(),
            url,
            title: req.title.clone().unwrap_or_else(|| entry.manifest.name.clone()),
            width: req.width.unwrap_or(DEFAULT_WINDOW_WIDTH),
            height: req.height.unwrap_or(DEFAULT_WINDOW_HEIGHT),
        };

        let created = state.window_sink.create(&spec)?;
        Ok(WindowCreateOutcome {
            label: spec.label,
            plugin_id: spec.plugin_id,
            created,
            reason: (!created).then(|| {
                "宿主未实现窗口创建（非 Tauri 宿主 / 未注入 TauriWindowSink）：\
                 **没有**创建任何窗口"
                    .to_string()
            }),
        })
    })?
}

/// `entry.ui` is consumed by Tauri as an application asset path when no on-disk
/// plugin installation root is configured. Keep it a portable relative path on
/// every host OS; in particular, reject Windows separators and drive prefixes
/// even when validation runs on Unix.
fn validate_plugin_ui_entry(raw: &str) -> HostResult<()> {
    let invalid = raw.trim().is_empty()
        || raw.starts_with('/')
        || raw.starts_with('\\')
        || raw.contains('\\')
        || raw.contains(':')
        || raw.split('/').any(|part| part.is_empty() || part == "." || part == "..");
    if invalid {
        return Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            "插件 entry.ui 必须是安全的相对路径".to_string(),
        ));
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// 壳扩展状态与命令（P2：窗口持久化 / 剪贴板 / 深链接 / 对话框 / 更新）
// ──────────────────────────────────────────────────────────────────────────

/// 壳扩展进程内状态。
///
/// 说明：窗口几何与剪贴板是**真实进程内行为**（可被读取验证）；
/// 对话框在无 Tauri 对话框插件的环境下返回"取消"（None/false）；
/// 更新命令缺省为模拟结果（诚实标注 `simulated: true`），轮 40 起装配
/// [`DistributeUpgradeInstaller`] 后 download/install 为真实效果（`simulated: false`）。
#[derive(Debug, Clone, Default)]
pub struct ShellExtState {
    /// 窗口几何 (x, y, width, height)：由 set_position/set_size 更新。
    ///
    /// ⚠️ **接入状态：今天没有任何生产读取方**（全仓只有本文件单测读它）。
    /// 窗口几何的**持久化与恢复在前端**：`@tauron/host` 的 `window-state.ts`
    /// 把它存在 `localStorage`，恢复时经 `host_window_set_position/_set_size`
    /// 回写平台。因此它是"进程内几何账本"，不是恢复链路的读取来源——不要据它
    /// 推断"窗口状态恢复读宿主"。
    pub window_rect: (i32, i32, u32, u32),
    /// 进程内剪贴板文本。
    pub clipboard: String,
    /// 已注册的深链接协议（None = 未注册）。
    pub deep_link_protocol: Option<String>,
    /// 更新状态机：None → downloaded → installed。
    ///
    /// **接入状态（R9 更正；轮 40 更新）**：已有生产读取方——`host_updater_status`
    /// 把本字段并入返回的 `state` 字段（见 [`cmd_updater_status`]）。写入方是
    /// `cmd_market_download` / `cmd_market_install` 的**分派腿**：缺省装配
    /// （[`NoUpgradeInstaller`]）是模拟桩，轮 40 起装配 [`DistributeUpgradeInstaller`]
    /// 后是真效果。所以本字段的每一个非 `None` 值都必须连同
    /// [`Self::update_state_simulated`] 一起读。
    /// 前端的 `auto-update-client.ts`
    /// 自持 `UpdateStatus`，与本字段并存；本字段是**宿主侧**的进程内账本。
    pub update_state: Option<String>,
    /// `update_state` 现值是否来自**模拟**推进（轮 33）。
    ///
    /// 与 `update_state` **同处更新、同处读取**（见 [`cmd_updater_status`]）：把它做成
    /// 独立字段而不是往字符串里塞前缀，是为了让读侧不必解析字符串——解析一次就会
    /// 漂一次（轮 32 的状态词表就是那样的三面镜像）。模拟腿
    /// （[`cmd_market_download`] / [`cmd_market_install`] 缺省路径）置 `true`；
    /// 轮 40 起装配腿真路径（`_wired` 腿，[`DistributeUpgradeInstaller`]）在真实效果
    /// 成功后置 `false`。
    pub update_state_simulated: bool,
    /// **origin 允许清单（R4-D2）**：由 [`AdapterConfig::origin_allowlist`] 装配，
    /// 由 `tauri::origin_gate` 在命令分发入口读取。空 = 不启用（Development/Test）；
    /// Production 下空清单被 `tauron_host::authz::production_caller_allowed` 直接拒绝。
    pub origin_allowlist: Vec<String>,
    /// 可信主窗 label 集合（V4 轮 10 / F1），由 [`AdapterConfig::main_window_labels`]
    /// 装配；配置为空时装配方展开为 `authz::default_main_window_labels()`。
    pub main_window_labels: Vec<String>,
    /// Local plugin package deployment directory (required to enable installation).
    #[cfg(feature = "plugin-install")]
    pub plugin_install_dir: Option<PathBuf>,
    /// Trusted Ed25519 public keys keyed by package signer id.
    #[cfg(feature = "plugin-install")]
    pub plugin_signing_keys: std::collections::BTreeMap<String, Vec<u8>>,
    /// Host-owned ACL HMAC key. Must be backed by host secure storage.
    #[cfg(feature = "plugin-install")]
    pub acl_signing_key: Option<Vec<u8>>,
}

/// `host_window_set_position`：移动调用方窗口到 `(x, y)`（逻辑像素/DIP）。
///
/// **两件事，都要做**（R8）：
/// 1. 写 `shell_ext.window_rect` —— 进程内的几何账本（**无生产读取方**；
///    持久化与恢复在前端 `window-state.ts`，见该字段的接入状态说明）；
/// 2. 转调 [`SubstrateState::window_sink`] 执行真实移动。
///
/// 只做 1 = 几何只在账本里变了（R8 之前的桩就是这样）；只做 2 = 账本失真。
/// 顺序是**先记账后落平台**：平台调用失败时账本已记下用户意图，而失败的 `Err`
/// 会如实返回，不会假装移动成功。
pub fn cmd_window_set_position(
    state: &SubstrateState,
    label: &str,
    x: i32,
    y: i32,
) -> HostResult<()> {
    guard("window_set_position", || {
        {
            let mut ext = state.shell_ext.lock();
            ext.window_rect.0 = x;
            ext.window_rect.1 = y;
        }
        state.window_sink.set_position(label, x, y)
    })?
}

/// `host_window_set_size`：缩放调用方窗口（逻辑像素/DIP）。记账 + 落平台，同
/// [`cmd_window_set_position`]。
pub fn cmd_window_set_size(
    state: &SubstrateState,
    label: &str,
    width: u32,
    height: u32,
) -> HostResult<()> {
    guard("window_set_size", || {
        {
            let mut ext = state.shell_ext.lock();
            ext.window_rect.2 = width;
            ext.window_rect.3 = height;
        }
        state.window_sink.set_size(label, width, height)
    })?
}

/// 写入进程内缓冲区并明示系统剪贴板不支持。
pub fn cmd_clipboard_write(state: &SubstrateState, text: String) -> HostResult<UnsupportedBody> {
    guard("clipboard_write", || {
        state.shell_ext.lock().clipboard = text;
        Ok(unsupported_body(
            "system clipboard provider is not configured",
            Some("in-process-buffer"),
        ))
    })?
}

/// 读取进程内缓冲区并明示系统剪贴板不支持。
pub fn cmd_clipboard_read(state: &SubstrateState) -> HostResult<DegradedValue<String>> {
    guard("clipboard_read", || {
        Ok(DegradedValue {
            supported: false,
            reason: "system clipboard provider is not configured".to_string(),
            fallback: "in-process-buffer".to_string(),
            value: state.shell_ext.lock().clipboard.clone(),
        })
    })?
}

/// 剪贴板族（读写）的**带身份判定**版本（轮 12）。
///
/// # 为什么这两条是主窗专属
///
/// 现状的"进程内缓冲区"是 `shell_ext.clipboard` 这**一个**全局槽位，不分主体：
/// 任何插件 `host_clipboard_write` 就能覆盖别人刚写的内容（下一位读者拿到的是它的
/// 值），`host_clipboard_read` 就能读到别的插件刚放进去的内容。也就是说它虽然
/// **降级**（`supported: false`），却仍是**跨主体的共享可变状态**——和
/// `host_notifications_*` 在轮 11 的处理理由同型。
///
/// 判定选择"仅主窗"而不是"按插件分槽"：剪贴板的语义是**用户级**的跨应用粘贴面
/// （`@tauron/host` 的 `ClipboardClient` 由主窗外壳使用），插件侧没有"我的剪贴板"
/// 这种合法用法；真要给插件用，正确形态是每主体独立槽 + 显式共享，那是新功能，
/// 不是把越权面留在原地冒充降级实现。
///
/// 接原生 provider 后判定同样够用：那时它变成"读/写用户系统剪贴板"，插件能碰等于
/// 任何插件都能窃听与改写用户剪贴板，比现在更严重。
pub fn cmd_clipboard_read_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<DegradedValue<String>> {
    require_main_window(caller, "host_clipboard_read")?;
    cmd_clipboard_read(state)
}

/// `host_clipboard_write` 的带身份判定版本，理由同 [`cmd_clipboard_read_as`]。
pub fn cmd_clipboard_write_as(
    caller: &Caller,
    state: &SubstrateState,
    text: String,
) -> HostResult<UnsupportedBody> {
    require_main_window(caller, "host_clipboard_write")?;
    cmd_clipboard_write(state, text)
}

/// `host_deep_link_register` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第三批）。
///
/// # 为什么这条是主窗专属（危害原文）
///
/// # R8 曾把它当 self-service——那是错的，这里是修正
///
/// 它写的不是"某个插件自己的东西"，而是**应用级**的两处共享状态：
///
/// 1. `shell_ext.deep_link_protocol`（整个应用当前认哪个协议）——插件换值就是
///    **把整个应用的深链接入口改到自己名下**；
/// 2. `deep-link` 公共 topic 的声明（EventBus 的授权前提）——换值会连带
///    [`cmd_deep_link_register`] 的"先注销旧值"路径，把**上一个协议**从 OS 关联里
///    摘掉（`unregister(old)`）。
///
/// 组合起来的真实攻击面：插件 A 调一次 `register("evil")`，应用的深链接入口就被
/// 改成 `evil://`，用户点原本的 `tauron://` 链接**不再拉起本应用**（旧关联已被注销），
/// 而 A 可以用自己的协议接管后续 URL 投递（含带 token 的跳转链接）。这是跨插件/
/// 应用级的越权面，不是"插件配置自己的协议"。
///
/// 主窗是唯一合法主体。拒绝码复用既有 [`ErrorCode::E_AUTH_DENIED`]，判定在
/// **任何副作用之前**：拒绝时 `shell_ext.deep_link_protocol` 与 topic 声明、
/// 以及 sink 的 `unregister` 一律不动（有测试断言协议值逐字不变）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_deep_link_register_as(
    caller: &Caller,
    state: &SubstrateState,
    protocol: String,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_deep_link_register")?;
    cmd_deep_link_register(state, protocol)
}

/// `host_deep_link_register`：注册深链接协议。
///
/// # 这条命令做两件事，其中**只有一件是真的平台事**
///
/// 1. **应用层（真实）**：记录协议到 `shell_ext`，并声明 `deep-link` 公共 topic——
///    这是前端 `subscribe` 与 [`deep_link_delivered`] 投递的授权前提（EventBus 要求
///    先声明后订阅/发布）。这一步任何时候都会发生。
/// 2. **平台层（走 sink）**：转调 [`SubstrateState::deep_link_sink`]。
///    ⚠️ **本仓的 sink 实现全部是降级路径**（`tauri-plugin-deep-link` 不在依赖闭包
///    内）：`register` 只记账并（Tauri 实现下）向前端发一个"未做 OS 注册"的信号，
///    `native_supported()` 恒 `false`。也就是说：**OS 不会把 `tauron://` 交给本应用**，
///    本命令只是让"URL 到了宿主之后"的那一段可用（谁把 URL 送进来是宿主自己的事，
///    例如命令行/单实例转发）。
///
/// # 为什么换协议时要先注销（`unregister` 的真实消费者）
///
/// 协议名是 OS 关联的键：`tauron` → `other` 的换名如果不注销旧值，系统里会留下
/// 旧协议的关联（用户点旧链接仍会拉起本应用，而应用层已按新协议解析）。
/// 因此核心在写新值之前先对旧值调 `unregister`——这不是预留接口。
///
/// **入口**：wire 层转调 [`cmd_deep_link_register_as`]（仅主窗：注册的是**应用级**
/// 协议，插件改它 = 把整个应用的深链接入口改到自己名下），本函数是**不过身份**的核心。
pub fn cmd_deep_link_register(
    state: &SubstrateState,
    protocol: String,
) -> HostResult<ProviderResult<()>> {
    guard("deep_link_register", || {
        // 空协议不是一个"稍微不对"的协议名，而是一次无意义的 OS 注册请求。
        if protocol.trim().is_empty() {
            return Err(HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                "深链接协议不能为空（调用方可能传了 undefined/null）".to_string(),
            ));
        }
        let previous = state.shell_ext.lock().deep_link_protocol.clone();
        if let Some(old) = previous.filter(|old| old != &protocol) {
            state.deep_link_sink.unregister(&old)?;
        }
        {
            let mut ext = state.shell_ext.lock();
            ext.deep_link_protocol = Some(protocol.clone());
        }
        // 声明 `deep-link` 公共 topic：这是前端 subscribe 与
        // deep_link_delivered 投递的授权前提（EventBus 要求先声明后订阅/发布）。
        {
            let bus = state.bus.lock();
            bus.declare_topics(
                DEEP_LINK_PUBLISHER,
                &[EventDecl { topic: DEEP_LINK_TOPIC.to_string(), public: true }],
            )?;
        }
        // 平台侧注册放在最后：应用层状态与 topic 都到位之后才谈"让 OS 认这个协议"。
        state.deep_link_sink.register(&protocol)?;
        if state.deep_link_sink.native_supported() {
            Ok(ProviderResult::Value(()))
        } else {
            Ok(ProviderResult::Unsupported(unsupported_body(
                "OS deep-link provider is not configured",
                Some("internal-event-routing"),
            )))
        }
    })?
}

/// 深链接事件的 topic 与伪发布者（框架内部源，非插件）。
pub const DEEP_LINK_TOPIC: &str = "deep-link";
pub const DEEP_LINK_PUBLISHER: &str = "core.deep-link";

/// 深链接到达入口（OS 协议回调侧 glue 调用点）。
///
/// 生产接线：`tauri-plugin-deep-link` 的回调里调用本函数，把收到的 URL
/// 发布到 EventBus 的 `deep-link` 主题；前端 `DeepLinkClient.subscribe()`
/// 即在该 URL 上收到（含 parseUrl 结构化解析）。
///
/// 这不是一条新 IPC 命令（JS 不可调用，防伪造）——只供 Rust 侧回调使用。
pub fn deep_link_delivered(state: &SubstrateState, url: &str) -> HostResult<()> {
    guard("deep_link_delivered", || {
        let payload = serde_json::json!({
            "url": url,
            "protocol": state.shell_ext.lock().deep_link_protocol.clone(),
        });
        let bus = state.bus.lock();
        // Event 通道：与前端 backend.listen('deep-link') 的 drain 通道一致。
        let res = bus.publish(DEEP_LINK_PUBLISHER, DEEP_LINK_TOPIC, payload, ChannelKind::Event);
        if res.dropped {
            return Err(HostError::new(
                ErrorCode::E_STATE_INVALID_TRANSITION,
                "deep-link topic 未声明：须先调用 host_deep_link_register",
            ));
        }
        Ok(())
    })?
}

/// `host_market_check` 的返回值（camelCase 线形，**有类型**）。
///
/// R8 之前这里是裸 `serde_json::json!({ "available": false })`：`simulated` 这件事
/// 只写在注释里，前端**没有任何字段**可以据此判断"这是模拟结果"。现在它是线字段。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketCheckResult {
    /// 是否有可用更新。当前恒 `false`（没有更新源）。
    pub available: bool,
    /// **是否模拟结果**：当前恒 `true`——本命令不做任何真实可用性探测
    /// （无 `tauri-plugin-updater`，也不请求任何 endpoint）。
    pub simulated: bool,
    /// 可用版本号；无则 `null`。
    pub version: Option<String>,
    /// 为什么不可用 / 为什么是模拟结果；成功且非模拟时为 `null`。
    pub reason: Option<String>,
}

/// `host_market_download` / `host_market_install` 的返回值（camelCase 线形）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketUpdateResult {
    /// 本次命令是否执行成功（**不是**"更新是否真的落地"——那看 `simulated`）。
    pub ok: bool,
    /// **是否模拟结果**：缺省装配恒 `true`——没有下载任何字节、没有验签、没有
    /// 替换文件；升级装配腿（[`DistributeUpgradeInstaller`]）注入后为 `false`
    /// （真实效果已发生并落账）。
    pub simulated: bool,
    /// 目标版本号。模拟路径缺省入参时为 `null`；真路径恒为**宿主清单**版本。
    pub version: Option<String>,
    /// 为什么是模拟结果；真实效果成功后为 `null`。
    pub reason: Option<String>,
}

/// `host_market_check`：检查更新。
///
/// ⚠️ **这是桩，而且现在会如实说出来**：返回值里 `simulated: true` +
/// `reason` 写明"未接入更新源"，并且**指到真通道去**。
///
/// 说清楚一件事（轮 28 复核、轮 29 收口、轮 40 更新）：本仓的更新能力**不是**没有
/// 真实现——真的检查是 [`cmd_updater_check`]（宿主注入 `EndpointClient` 后直接跑
/// `tauron_distribute::check_for_update`：清单校验 + 灰度 + 签名）；真的下载/安装是
/// [`cmd_market_download`] / [`cmd_market_install`]（宿主装配
/// [`DistributeUpgradeInstaller`] 后为真，缺省如实模拟）。**唯独本命令**（check）刻意
/// 是桩：它**不**去读调用方下发的 `endpoints` / `pubkey`——那两个参数今天
/// 一个都不用，将来也不用——让 webview 指定宿主去哪取更新清单，等于把宿主的更新通道
/// 交给调用方（供应链投毒的第一步，见 [`cmd_market_check_as`] 的危害原文）。
/// 端点的权威来源只能是宿主装配。
///
/// 对外 SDK 的 `AutoUpdateClient.checkUpdate()` 自轮 29 起读的是真通道，不再把本桩
/// 当可用性结论。
///
/// **主窗专属（轮 11 第二批）**：商城命令操作的是**宿主级产物**（更新源 / 安装
/// 包），插件不得触发；wire 层转调 [`cmd_market_check_as`] 判定，本函数是
/// **不过身份**的核心。
pub fn cmd_market_check(_state: &SubstrateState) -> HostResult<MarketCheckResult> {
    guard("market_check", || {
        Ok(MarketCheckResult {
            available: false,
            simulated: true,
            version: None,
            reason: Some(
                "未接入更新源：本命令是宿主本地桩，不做任何真实可用性探测（没有请求任何 \
                 endpoint，调用方下发的 endpoints/pubkey 一律不用）；真检查在 \
                 host_updater_check（宿主注入端点即为真），真下载/替换在 \
                 host_market_download / host_market_install（宿主装配 UpgradeInstaller \
                 后为真，缺省如实模拟）"
                    .to_string(),
            ),
        })
    })?
}

/// `host_market_download`：下载并校验更新包（真实路径在装配腿注入后成立）。
///
/// **两条路，线形同形，`simulated` 如实区分**：
/// - 缺省（[`NoUpgradeInstaller`]）：模拟——只把进程内 `update_state` 推到
///   `downloaded:<version>`（版本取调用方入参），`simulated: true` + `reason`；
/// - 装配腿注入（[`DistributeUpgradeInstaller`]）：真下载——字节落盘 + SHA-256 +
///   验签通过后才推账本（版本取宿主清单，**不读**调用方入参），`simulated: false`。
///
/// 两条路的失败语义一致：**零账本**——失败时 `update_state` /
/// `update_state_simulated` 原样不动（下载失败不得表现为"已下载"）。
///
/// **主窗专属（轮 11 第二批；轮 40 起带审计）**：wire 层转调
/// [`cmd_market_download_as`]——判定 + 结构化审计留痕（供应链入口级特权）；
/// 本函数是**不过身份**的核心。
pub fn cmd_market_download(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    guard("market_download", || {
        if state.upgrade_installer.native_supported() {
            return cmd_market_download_wired(state);
        }
        cmd_market_download_simulated(state, version)
    })?
}

/// 模拟下载腿（缺省装配；原桩实现原样保留，账本如实标注 simulated）。
fn cmd_market_download_simulated(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("downloaded:{}", version.unwrap_or("unknown")));
    ext.update_state_simulated = true;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: true,
        version: version.map(str::to_string),
        reason: Some(
            "模拟下载：**没有**下载任何字节、没有写入任何文件、没有校验签名；\
             只是把进程内 updateState 推进到 downloaded"
                .to_string(),
        ),
    })
}

/// 真下载腿（装配腿已注入）：seam 成功后账本推进为**真实**（`simulated: false`）。
///
/// 失败路径零账本：seam 返回 `Err` 时账本两个字段原样不动。
fn cmd_market_download_wired(state: &SubstrateState) -> HostResult<MarketUpdateResult> {
    let staged = state.upgrade_installer.download()?;
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("downloaded:{}", staged.version));
    ext.update_state_simulated = false;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: false,
        version: Some(staged.version),
        reason: None,
    })
}

/// `host_market_install`：安装已 staged 的更新包（真实路径在装配腿注入后成立）。
///
/// 与 [`cmd_market_download`] 同构的两条路：缺省模拟（推 `installed:<version>`，
/// `simulated: true`）；装配腿注入后经完整 `UpgradeRunner`（备份 → 解压 → 原子
/// 交换 → 健康检查 → 提交日志 → 重启/自动回滚），`simulated: false`。
///
/// 失败路径同样**零账本**：含健康检查失败后的自动回滚——回滚不改账本，账本仍
/// 停在 `downloaded:<v>`（与磁盘真相同步），runner 的错误上浮为类型化拒绝。
///
/// **主窗专属（轮 11 第二批；轮 40 起带审计）**：wire 层转调
/// [`cmd_market_install_as`]（同 [`cmd_market_download`] 的判定与审计口径）；
/// 本函数是**不过身份**的核心。
pub fn cmd_market_install(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    guard("market_install", || {
        if state.upgrade_installer.native_supported() {
            return cmd_market_install_wired(state);
        }
        cmd_market_install_simulated(state, version)
    })?
}

/// 模拟安装腿（缺省装配；原桩实现原样保留）。
fn cmd_market_install_simulated(
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("installed:{}", version.unwrap_or("unknown")));
    ext.update_state_simulated = true;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: true,
        version: version.map(str::to_string),
        reason: Some(
            "模拟安装：**没有**替换任何二进制/文件、没有触发任何重启流程；\
             只是把进程内 updateState 推进到 installed"
                .to_string(),
        ),
    })
}

/// 真安装腿（装配腿已注入）：runner 提交成功后才推账本（`simulated: false`）。
fn cmd_market_install_wired(state: &SubstrateState) -> HostResult<MarketUpdateResult> {
    let installed = state.upgrade_installer.install()?;
    let mut ext = state.shell_ext.lock();
    ext.update_state = Some(format!("installed:{}", installed.version));
    ext.update_state_simulated = false;
    Ok(MarketUpdateResult {
        ok: true,
        simulated: false,
        version: Some(installed.version),
        reason: None,
    })
}

/// `host_market_check` 的**带身份判定**版本（wire 层转调的就是它，轮 11 第二批）。
///
/// # 为什么这三条是主窗专属（危害原文）
///
/// 商城命令操作的是**宿主级产物**，不是某个插件的东西：
///
/// - `check` 会拿调用方下发的**更新源 endpoint / 公钥**去探更新（接入后）：插件能
///   借此把宿主指向自己的更新源（供应链投毒的第一步），或当成内网探测跳板；
/// - `download` / `install` 自轮 40 起是**真供应链操作**（装配腿注入后）：写宿主
///   磁盘、替换应用自身二进制——没有任何"某个插件"能成为这类操作的主体，让插件
///   webview 调它等于把宿主自身的更新通道交给插件；
/// - 三条都不收身份参数，所以判定只能落在**整条命令**上（同码 `E_AUTH_DENIED`）。
///
/// **判定与审计（轮 40 起）**：`check` 仍是桩（不请求网络、不写文件），走
/// [`require_main_window`] 且不进审计表；`download` / `install` 已接线，按
/// `tauron_host::admin_audit` 的口径说明**连同 `authz` 档位登记**一起走
/// [`admin_gate`]（判定 + 结构化审计留痕——被拒的提权尝试同样留痕）。拒绝路径
/// **零副作用**：`updateState` 不被写（有测试断言）。
///
/// 命令**线形不变**（`window` 由 Tauri 注入，前端参数一个字节没改）。
pub fn cmd_market_check_as(
    caller: &Caller,
    state: &SubstrateState,
) -> HostResult<MarketCheckResult> {
    require_main_window(caller, "host_market_check")?;
    cmd_market_check(state)
}

/// `host_market_download` 的**带身份判定 + 审计**版本（见 [`cmd_market_check_as`] 的
/// 危害说明；轮 40 接线后与注册表安装同级，判定与审计同源）。
pub fn cmd_market_download_as(
    caller: &Caller,
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    admin_gate(state, caller, "host_market_download")?;
    cmd_market_download(state, version)
}

/// `host_market_install` 的**带身份判定 + 审计**版本（见 [`cmd_market_check_as`] 的
/// 危害说明；轮 40 接线后与注册表安装同级，判定与审计同源）。
pub fn cmd_market_install_as(
    caller: &Caller,
    state: &SubstrateState,
    version: Option<&str>,
) -> HostResult<MarketUpdateResult> {
    admin_gate(state, caller, "host_market_install")?;
    cmd_market_install(state, version)
}

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

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tauron_host::{
        manifest::{EventDecl, EventsDecl, PermissionIndex, PluginManifest, PluginType},
        ErrorCode,
    };
    // P0-2 测试用具：fake 启动面只实现 trait，不碰真进程。
    use tauron_proc::{ProcResult, ProcessFrameSink, SpawnedProc};

    /// R4-D2 装配链路：`AdapterConfig.origin_allowlist` 必须流到 origin 门读取的
    /// `shell_ext` 上；默认装配 = 不启用（兼容既有宿主）。
    #[test]
    fn origin_allowlist_flows_from_adapter_config_to_shell_ext() {
        let configured = CommandState::with_adapter_config(AdapterConfig {
            origin_allowlist: vec!["http://127.0.0.1:63896".to_string()],
            ..AdapterConfig::default()
        });
        assert_eq!(
            configured.shell_ext.lock().origin_allowlist,
            vec!["http://127.0.0.1:63896".to_string()]
        );
        assert!(
            CommandState::new().shell_ext.lock().origin_allowlist.is_empty(),
            "默认装配必须不启用 origin 门（缺省放行，兼容既有宿主）"
        );
    }

    /// 轮 10 / F1 装配链路：主窗 label 集合必须从配置流到 origin 门读取的 `shell_ext`；
    /// 未显式配置时展开成 Tauri 约定缺省，而不是空集合——空集合在 Production 下会让
    /// 合法主窗被 `MAIN_WINDOW_LABEL_NOT_DECLARED` 全量拒掉。
    #[test]
    fn main_window_labels_flow_from_adapter_config_and_default_to_tauri_convention() {
        let configured = CommandState::with_adapter_config(AdapterConfig {
            main_window_labels: vec!["editor".to_string()],
            ..AdapterConfig::default()
        });
        assert_eq!(configured.shell_ext.lock().main_window_labels, vec!["editor".to_string()]);

        let defaulted = CommandState::new();
        assert_eq!(
            defaulted.shell_ext.lock().main_window_labels,
            tauron_host::authz::default_main_window_labels(),
            "缺省必须展开为约定主窗 label，不得留空"
        );
    }

    /// 轮 10 / F2：`caller_identity_policy_enabled` 是「声明」，origin 允许清单才是
    /// 「装弹」。只声明不装弹的 Production 配置必须 fail closed——否则 origin 门在
    /// 生产宿主上是空转的，而 readiness 却报绿。
    #[test]
    fn production_declared_identity_policy_without_allowlist_is_not_ready() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AdapterConfig::production();
        cfg.caller_identity_policy_enabled = true;
        cfg.recovery_data_dir = Some(temp.path().to_path_buf());
        cfg.admin_audit_dir = Some(temp.path().join("audit"));
        #[cfg(feature = "plugin-install")]
        {
            let mut keys = std::collections::BTreeMap::new();
            keys.insert("fixture-key".to_string(), vec![0x4b; 32]);
            cfg = cfg
                .with_plugin_install(temp.path().join("plugins"), keys, vec![0x5a; 32])
                .with_trusted_time_provider(Arc::new(tauron_host::SystemTimeProvider::new(
                    tauron_host::TimeTrustState::Trusted,
                )));
        }

        assert!(cfg.origin_allowlist.is_empty(), "本用例考察的正是「已声明身份策略但清单为空」");
        assert!(cfg.production_readiness().caller_identity_policy_enabled);
        assert!(!cfg.production_readiness().origin_gate_armed);
        let err = cfg
            .validate_production_readiness()
            .expect_err("未装弹的 origin 门不得通过 production 门");
        assert!(err.contains("ORIGIN_GATE_ARMED_REQUIRED"), "err={err}");

        cfg.origin_allowlist = vec!["tauri://localhost".into()];
        assert!(cfg.production_readiness().origin_gate_armed);
        assert!(cfg.validate_production_readiness().is_ok());
    }

    /// 开发态保持不变：空清单依旧放行（兼容既有宿主），readiness 也不因此报错。
    #[test]
    fn development_keeps_the_origin_gate_disarmed_compatibility_path() {
        let cfg = AdapterConfig::default();
        assert!(!cfg.production_readiness().origin_gate_armed);
        assert!(cfg.validate_production_readiness().is_ok());
    }

    // ══════════════════════════════════════════════════════════════════════
    // 轮 11 / Batch 0-3（F3）：特权操作产出**结构化审计事实**，而不是一个开关位
    //
    // 三条链路各自有例：写入点（`admin_gate`）→ sink；读取点（doctor 的
    // `adminAudit`）→ 真实健康态；跨进程（同一目录重开）→ 序号续接。
    // ══════════════════════════════════════════════════════════════════════

    /// 一份**只差审计目录**就 production-ready 的配置（其余安全事实全部给足）。
    fn production_config_with_audit(temp: &tempfile::TempDir) -> AdapterConfig {
        let mut cfg = AdapterConfig::production();
        cfg.origin_allowlist = vec!["tauri://localhost".into()];
        cfg.recovery_data_dir = Some(temp.path().join("data"));
        cfg.admin_audit_dir = Some(temp.path().join("audit"));
        #[cfg(feature = "plugin-install")]
        {
            let mut keys = std::collections::BTreeMap::new();
            keys.insert("fixture-key".to_string(), vec![0x4b; 32]);
            cfg = cfg
                .with_plugin_install(temp.path().join("plugins"), keys, vec![0x5a; 32])
                .with_trusted_time_provider(Arc::new(tauron_host::SystemTimeProvider::new(
                    tauron_host::TimeTrustState::Trusted,
                )));
        }
        cfg
    }

    /// 有 admin 调用 ⇒ 有审计记录（写侧 E2E）。
    #[test]
    fn an_admin_call_leaves_a_structured_audit_fact() {
        let temp = tempfile::tempdir().unwrap();
        let state = SubstrateState::with_adapter_config(&production_config_with_audit(&temp));
        state
            .bus
            .lock()
            .declare_topics(
                "com.a",
                &[EventDecl { topic: "plugin-com.a.private".into(), public: false }],
            )
            .unwrap();

        cmd_events_approve_as(&Caller::MainWindow, &state, "com.a", "plugin-com.a.private")
            .unwrap();

        let records = state.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].command, "host_events_approve");
        assert_eq!(records[0].caller, "main-window");
        assert_eq!(records[0].outcome, tauron_host::AdminAuditOutcome::Allowed);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[0].prev_hash, "");
        assert!(records[0].hash.starts_with(|c: char| c.is_ascii_hexdigit()));
        assert_eq!(records[0].error_code, None);
        let file = temp.path().join("audit").join(tauron_host::ADMIN_AUDIT_FILE);
        let facts = tauron_host::admin_audit::verify_file(&file).unwrap();
        assert_eq!(facts.records, 1);
        assert_eq!(facts.last_command.as_deref(), Some("host_events_approve"));
    }

    /// 被拒的特权尝试同样留痕（错误码上线），且**判定仍在副作用之前**——审计不改状态。
    ///
    /// 用开发态装配：审计写入点不区分部署模式（留痕与否只取决于有没有 sink），
    /// 提权尝试在生产之外的环境里同样值得记。
    #[test]
    fn a_denied_admin_attempt_is_audited_without_side_effects() {
        let temp = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(
            AdapterConfig::default().with_admin_audit_dir(temp.path().join("audit")),
        );
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let id = PluginId::new("com.a").unwrap();

        let err = cmd_registry_admin_as(
            &plugin_caller("com.a"),
            &state,
            "com.a",
            RegistryAdminOp::Disable,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed,
            "拒绝路径不得改插件状态"
        );

        let records = state.substrate.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].command, "host_registry_admin");
        assert_eq!(records[0].caller, "plugin:com.a");
        assert_eq!(records[0].plugin_id.as_deref(), Some("com.a"));
        assert_eq!(records[0].outcome, tauron_host::AdminAuditOutcome::Denied);
        assert_eq!(records[0].error_code.as_deref(), Some("E_AUTH_DENIED"));

        // 对照：主窗执行同一条命令 ⇒ 追加第二条，链式续接。
        cmd_registry_admin_as(&Caller::MainWindow, &state, "com.a", RegistryAdminOp::Disable)
            .unwrap();
        let records = state.substrate.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].seq, 2);
        assert_eq!(records[1].prev_hash, records[0].hash);
        assert!(tauron_host::admin_audit::verify_records("", &records[..1]).is_ok());
    }

    /// 读取侧：doctor 的 `admin-audit` 检查项由**真实 sink** 推导，快照一并上线。
    #[test]
    fn doctor_derives_the_admin_audit_check_from_the_live_sink() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = SubstrateState::with_adapter_config(&production_config_with_audit(&temp));
        let report = cmd_production_doctor_as(&Caller::MainWindow, &state).unwrap();
        let audit_check = report.checks.iter().find(|c| c.id == "admin-audit").unwrap();
        assert!(audit_check.pass, "durable sink 打开即健康");
        assert!(report.production_safe);
        let facts = report.admin_audit.expect("production 报告必须带审计快照");
        assert!(facts.durable);
        assert_eq!(facts.records, 0);
        assert!(facts.healthy());

        // 有调用 ⇒ 快照里的条数跟着动（读取侧不是静态声明）。
        state
            .bus
            .lock()
            .declare_topics("com.b", &[EventDecl { topic: "plugin-com.b.x".into(), public: false }])
            .unwrap();
        cmd_events_approve_as(&Caller::MainWindow, &state, "com.b", "plugin-com.b.x").unwrap();
        let after = cmd_production_doctor_as(&Caller::MainWindow, &state).unwrap();
        assert_eq!(after.admin_audit.as_ref().unwrap().records, 1);
        assert_eq!(
            after.admin_audit.as_ref().unwrap().last_command.as_deref(),
            Some("host_events_approve")
        );
        assert!(after.production_safe);

        // 换成不落盘的 sink：检查项必须**跟着变红**，production_safe 随之为假。
        // 这条断言是「判定读真实 sink 状态」的反向证明——布尔位时代它无法成立。
        // （就地换 sink：A87 单写锁不允许同一数据目录开出第二份状态。）
        state.admin_audit = Some(Arc::new(tauron_host::AdminAuditSink::in_memory()));
        let report = cmd_production_doctor_as(&Caller::MainWindow, &state).unwrap();
        assert!(!report.checks.iter().find(|c| c.id == "admin-audit").unwrap().pass);
        assert!(!report.production_safe);
        assert!(!report.admin_audit.as_ref().unwrap().healthy());
    }

    /// 没配审计目录的 Production 必须 not-ready——唯一满足方式是给出真实目录。
    #[test]
    fn production_without_an_audit_directory_is_not_ready() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = production_config_with_audit(&temp);
        cfg.admin_audit_dir = None;
        let err = cfg.validate_production_readiness().expect_err("缺审计目录必须拒启");
        assert!(err.contains("ADMIN_AUDIT_REQUIRED"), "err={err}");

        // 反向：把目录换成一个**指向文件**的路径（打不开），同样拒启——
        // 证明门禁看的是"能不能用"，不是"有没有填"。
        let blocker = temp.path().join("blocked");
        std::fs::write(&blocker, b"not a directory").unwrap();
        cfg.admin_audit_dir = Some(blocker);
        let err = cfg.validate_production_readiness().expect_err("打不开的审计目录必须拒启");
        assert!(err.contains("admin audit sink"), "err={err}");
    }

    /// 跨进程：同一目录重开，序号续接、历史不丢（否则审计等于每轮清零）。
    #[test]
    fn admin_audit_survives_a_restart_of_the_host() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = production_config_with_audit(&temp);
        let first = SubstrateState::with_adapter_config(&cfg);
        let decl = [EventDecl { topic: "plugin-com.c.t".into(), public: false }];
        first.bus.lock().declare_topics("com.c", &decl).unwrap();
        cmd_events_approve_as(&Caller::MainWindow, &first, "com.c", "plugin-com.c.t").unwrap();
        assert_eq!(first.admin_audit.as_ref().unwrap().records().len(), 1);
        drop(first);

        let second = SubstrateState::with_adapter_config(&cfg);
        let records = second.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 1, "重启后必须读回既有事实");
        second.bus.lock().declare_topics("com.c", &decl).unwrap();
        cmd_events_approve_as(&Caller::MainWindow, &second, "com.c", "plugin-com.c.t").unwrap();
        let records = second.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].seq, 2, "序号必须续接而不是重新从 1 开始");
    }

    /// 未配置 sink（开发态缺省）：特权判定照常，命令面不因审计而变。
    #[test]
    fn no_sink_means_no_records_but_authorization_still_runs() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());
        assert!(state.admin_audit.is_none());
        let decl = [EventDecl { topic: "plugin-com.d.t".into(), public: false }];
        state.bus.lock().declare_topics("com.d", &decl).unwrap();
        cmd_events_approve_as(&Caller::MainWindow, &state, "com.d", "plugin-com.d.t").unwrap();
        let err = cmd_events_approve_as(&plugin_caller("com.d"), &state, "com.d", "plugin-com.d.t")
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
    }

    /// 审计命令集必须**全部是真命令 + 真特权登记**，且 `ADMIN_COMMANDS` 被
    /// 「审计 / 只读豁免」二分完全覆盖——新增特权写命令忘记归类就在这里红。
    #[test]
    fn audited_admin_commands_are_real_dispatched_privileged_commands() {
        #[allow(unused_mut)]
        let mut dispatched: Vec<&str> =
            SUBSTRATE_COMMANDS.iter().chain(PLUGIN_RUNTIME_COMMANDS.iter()).copied().collect();
        #[cfg(feature = "plugin-install")]
        dispatched.extend(PLUGIN_INSTALL_COMMANDS.iter().copied());
        #[allow(unused_mut)]
        let mut privileged: Vec<&str> =
            tauron_host::authz::ADMIN_COMMANDS.iter().map(|c| c.command).collect();
        #[cfg(feature = "plugin-install")]
        privileged.extend(PLUGIN_INSTALL_AUTH.iter().map(|c| c.command));
        // 只读特权命令：读审批事实 / 资源占用 / 诊断报告，刻意不留审计痕迹。
        let read_only = [
            "host_events_approvals",
            "host_production_doctor",
            "host_resource_stats",
            "host_runtime_health",
        ];

        for command in tauron_host::AUDITED_ADMIN_COMMANDS {
            // plugin-install 关闭时这两条命令**不在命令面上**（编译期裁剪），
            // 只在 feature 打开时断言派发与登记。
            if !cfg!(feature = "plugin-install")
                && ["host_registry_install", "host_registry_install_preview"].contains(command)
            {
                continue;
            }
            assert!(dispatched.contains(command), "{command} 不在任何命令面清单里");
            assert!(privileged.contains(command), "{command} 未登记为特权命令");
            if let Some(entry) = tauron_host::authz::resolve(command) {
                assert_eq!(
                    entry.tier,
                    tauron_host::authz::AuthTier::Privileged,
                    "{command} 被审计但不是 privileged 档"
                );
            }
        }
        for entry in tauron_host::authz::ADMIN_COMMANDS {
            assert!(
                read_only.contains(&entry.command)
                    || tauron_host::admin_audit_required(entry.command),
                "{} 是特权命令，却既不在审计集也不在只读豁免集（§8-17 归类缺口）",
                entry.command
            );
            assert!(
                !(read_only.contains(&entry.command)
                    && tauron_host::admin_audit_required(entry.command)),
                "{} 同时被归为只读与审计，二分失效",
                entry.command
            );
        }
        // 轮 40：download/install 装配腿落地 → 已是特权写命令（真效果可能发生），
        // 必须进审计表；check 仍是桩、无副作用，不得进（见 `tauron_host::admin_audit`
        // 的口径说明）。
        assert!(
            !tauron_host::admin_audit_required("host_market_check"),
            "host_market_check 仍是模拟桩，不得进审计表"
        );
        for wired in ["host_market_download", "host_market_install"] {
            assert!(
                tauron_host::admin_audit_required(wired),
                "{wired} 是装配腿真路径（轮 40），必须审计"
            );
        }
    }

    #[test]
    fn host_capabilities_matches_base_and_plugin_runtime_assemblies() {
        let substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
        let base = cmd_host_capabilities(&substrate).unwrap();
        assert!(!base.plugin_runtime);
        // 命令数量必须由唯一清单推导；新增 substrate 命令时不得再维护第二份魔法数字。
        assert_eq!(base.commands.len(), SUBSTRATE_COMMANDS.len());
        assert!(base.commands.contains(&"host_capabilities".to_string()));

        let plugin_runtime = CommandState::new();
        let full = cmd_host_capabilities(&plugin_runtime).unwrap();
        assert!(full.plugin_runtime);
        #[cfg(feature = "plugin-install")]
        let expected = SUBSTRATE_COMMANDS.len()
            + PLUGIN_RUNTIME_COMMANDS.len()
            + PLUGIN_INSTALL_COMMANDS.len();
        #[cfg(not(feature = "plugin-install"))]
        let expected = SUBSTRATE_COMMANDS.len() + PLUGIN_RUNTIME_COMMANDS.len();
        assert_eq!(full.commands.len(), expected);
        assert_eq!(full.commands.iter().collect::<std::collections::HashSet<_>>().len(), expected);
        for domain in
            ["fs", "http", "dialog", "clipboard", "deep-link-os", "brand", "market-update"]
        {
            assert!(
                full.unsupported.iter().any(|u| u.domain == domain),
                "missing unsupported domain {domain}"
            );
        }

        // P2-4 修正：两列必须**互斥**——同一域既不能同时声称"已装配"又声称"未实现"。
        for family in &base.families {
            assert!(
                !base.unsupported.iter().any(|u| &u.domain == family),
                "域 `{family}` 同时出现在 families 与 unsupported（自相矛盾的过度声明）"
            );
        }
        // 无 provider 依赖的域恒在 families；缺省装配（无 tauri）下 provider 域恒在 unsupported。
        for family in ["shell", "ipc", "settings", "i18n", "notify", "recovery", "theme"] {
            assert!(base.families.contains(&family.to_string()), "缺 families 域 {family}");
        }
        for domain in ["menu", "tray", "fs", "http", "updater", "brand"] {
            assert!(
                base.unsupported.iter().any(|u| u.domain == domain),
                "缺省装配下 `{domain}` 必须如实落在 unsupported，而不是冒充 families"
            );
        }
    }

    fn empty_index() -> PermissionIndex {
        PermissionIndex { version: 1, generated_at: None, entries: vec![] }
    }

    fn test_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: PluginId::new(id).unwrap(),
            name: "Test".to_string(),
            version: semver::Version::parse("1.0.0").unwrap(),
            plugin_type: PluginType::Js,
            entry: tauron_host::manifest::EntrySpec {
                js: Some("index.js".to_string()),
                sidecar: None,
                wasm: None,
                ui: None,
            },
            permissions: vec![],
            scopes: Default::default(),
            platforms: vec![],
            framework: semver::VersionReq::parse(">=1.0.0, <3.0.0").unwrap(),
            abi: None,
            contributes: Default::default(),
            settings_schema: None,
            events: EventsDecl { publish: vec![], subscribe: vec![] },
            host_functions: vec![],
            min_allowed_version: None,
            signature: None,
            publisher: None,
        }
    }

    /// 把一组文件打成**签名合法**的 `.tpkg`（+ 同名 `.sig`）。
    ///
    /// 签名载荷是 **v2 规范化文本**（元数据 + path/size/hash），由
    /// `signing_payload` 单一生成——测试 fixture 必须用同一个函数，
    /// 否则会退回"手搓载荷"的旧形态（那正是 v1 的缺陷来源）。
    #[cfg(feature = "plugin-install")]
    fn signed_tpkg(
        dir: &std::path::Path,
        id: &str,
        files: &[(String, Vec<u8>)],
    ) -> (std::path::PathBuf, ed25519_dalek::VerifyingKey) {
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};
        use zip::write::SimpleFileOptions;

        let signed_files: Vec<tauron_market::package_signature::SignedFile> = files
            .iter()
            .map(|(path, bytes)| tauron_market::package_signature::SignedFile {
                path: path.clone(),
                size: bytes.len() as u64,
                hash: hex::encode(Sha256::digest(bytes)),
            })
            .collect();
        let issued_at = "2026-09-26T00:00:00Z";
        let payload = tauron_market::package_signature::signing_payload(
            "ed25519",
            "fixture-key",
            issued_at,
            &signed_files,
        );
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let signature = signing_key.sign(&payload);
        let sidecar = serde_json::json!({
            "algorithm": "ed25519", "kid": "fixture-key", "issuedAt": issued_at,
            "signature": hex::encode(signature.to_bytes()), "files": signed_files,
        });

        let package = dir.join(format!("{id}.tpkg"));
        let file = std::fs::File::create(&package).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (path, bytes) in files {
            archive.start_file(path.as_str(), SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut archive, bytes).unwrap();
        }
        archive.finish().unwrap();
        std::fs::write(format!("{}.sig", package.display()), sidecar.to_string()).unwrap();
        (package, signing_key.verifying_key())
    }

    /// 标准三文件安装包（manifest + entry js + index.html）。
    #[cfg(feature = "plugin-install")]
    fn signed_install_fixture(
        dir: &std::path::Path,
        id: &str,
    ) -> (std::path::PathBuf, ed25519_dalek::VerifyingKey) {
        let manifest = serde_json::json!({
            "id": id,
            "name": "Install Fixture",
            "version": "1.0.0",
            "type": "js",
            "entry": { "js": "src/index.js", "ui": "index.html" },
            "permissions": ["store:allow-get"],
            "framework": ">=1.0.0, <2.0.0"
        });
        let files = vec![
            ("manifest.json".to_string(), serde_json::to_vec(&manifest).unwrap()),
            ("src/index.js".to_string(), b"export const activate = () => true;".to_vec()),
            ("index.html".to_string(), b"<!doctype html><html><body>plugin</body></html>".to_vec()),
        ];
        signed_tpkg(dir, id, &files)
    }

    /// 给定签名密钥建一个可安装的宿主状态 + 安装根目录。
    #[cfg(feature = "plugin-install")]
    fn install_state(
        install_root: std::path::PathBuf,
        verifying_key: &ed25519_dalek::VerifyingKey,
    ) -> PluginRuntimeState {
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert("fixture-key".to_string(), verifying_key.to_bytes().to_vec());
        PluginRuntimeState::with_adapter_config(AdapterConfig {
            plugin_install_dir: Some(install_root),
            plugin_signing_keys: signing_keys,
            acl_signing_key: Some(vec![0x5a; 32]),
            ..AdapterConfig::default()
        })
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn explicit_suspicious_time_provider_blocks_install_before_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.time");
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert("fixture-key".to_string(), verifying_key.to_bytes().to_vec());
        let state = PluginRuntimeState::with_adapter_config(
            AdapterConfig {
                plugin_install_dir: Some(install_root.clone()),
                plugin_signing_keys: signing_keys,
                acl_signing_key: Some(vec![0x5a; 32]),
                ..AdapterConfig::default()
            }
            .with_trusted_time_provider(Arc::new(
                tauron_host::SystemTimeProvider::new(tauron_host::TimeTrustState::Suspicious),
            )),
        );

        let err =
            cmd_registry_install_preview_as(&Caller::MainWindow, &state, package.to_str().unwrap())
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED);
        assert!(err.message.contains("时间不可受信"), "{}", err.message);
        assert!(!install_root.join("com.install.time").exists());
        assert!(state.registry.find(&PluginId::new("com.install.time").unwrap()).is_none());
    }

    /// 解包防护（端到端回归锁）：条目数超上限的包必须被拒绝。
    ///
    /// **强制点不在适配层**——在 `read_verified_package` →
    /// `package_signature::verify_tpkg_reader[_with_time]`（`zip.len() > MAX_ENTRIES`、
    /// 逐条目 `sanitize_entry_path`、单文件/解压总量上限、逐文件哈希比对）。
    /// 本测试锁的是**安装这条链路整体**的行为：拒绝、且不留任何半截产物。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn install_rejects_package_exceeding_entry_count_limit() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let manifest = serde_json::json!({
            "id": "com.install.bomb",
            "name": "Bomb",
            "version": "1.0.0",
            "type": "js",
            "entry": { "js": "src/index.js", "ui": "index.html" },
            "permissions": [],
            "framework": ">=1.0.0, <2.0.0"
        });
        let mut files = vec![
            ("manifest.json".to_string(), serde_json::to_vec(&manifest).unwrap()),
            ("src/index.js".to_string(), b"export const activate = () => true;".to_vec()),
            ("index.html".to_string(), b"<!doctype html>".to_vec()),
        ];
        // MAX_ENTRIES 条填充 → 总数 MAX_ENTRIES + 3 > 上限。
        for i in 0..tauron_market::MAX_ENTRIES {
            files.push((format!("junk/{i}.txt"), b"x".to_vec()));
        }
        let (package, verifying_key) = signed_tpkg(dir.path(), "com.install.bomb", &files);
        let state = install_state(install_root.clone(), &verifying_key);

        let err =
            cmd_registry_install_as(&Caller::MainWindow, &state, package.to_str().unwrap(), &[])
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED, "{}", err.message);
        assert!(!install_root.join("com.install.bomb").exists(), "拒绝后不得留下安装目录");
        assert!(state.registry.find(&PluginId::new("com.install.bomb").unwrap()).is_none());
        // 临时目录也必须清干净（安装目录里除了刚建的 root 之外不该有残留）。
        let leftovers: Vec<_> = std::fs::read_dir(&install_root)
            .map(|it| it.filter_map(|e| e.ok().map(|e| e.file_name())).collect())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "解包失败后残留了临时目录：{leftovers:?}");
    }

    /// 解包防护（端到端回归锁）：含 `..` 的条目必须被拒绝，且**不得有任何文件
    /// 落到安装根之外**。
    ///
    /// 强制点同上（`package_signature::verify_package_against_zip` 里的
    /// `sanitize_entry_path`）。这里锁的是「逃逸不成立」这个可观测事实。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn install_rejects_path_traversal_entry() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let manifest = serde_json::json!({
            "id": "com.install.traverse",
            "name": "Traverse",
            "version": "1.0.0",
            "type": "js",
            "entry": { "js": "src/index.js", "ui": "index.html" },
            "permissions": [],
            "framework": ">=1.0.0, <2.0.0"
        });
        let files = vec![
            ("manifest.json".to_string(), serde_json::to_vec(&manifest).unwrap()),
            ("src/index.js".to_string(), b"export const activate = () => true;".to_vec()),
            ("index.html".to_string(), b"<!doctype html>".to_vec()),
            ("../escape.txt".to_string(), b"pwned".to_vec()),
        ];
        let (package, verifying_key) = signed_tpkg(dir.path(), "com.install.traverse", &files);
        let state = install_state(install_root.clone(), &verifying_key);

        let err =
            cmd_registry_install_as(&Caller::MainWindow, &state, package.to_str().unwrap(), &[])
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED, "{}", err.message);
        assert!(err.message.contains("恶意路径"), "错误消息应点名恶意路径：{}", err.message);
        assert!(!install_root.join("com.install.traverse").exists());
        assert!(!install_root.join("escape.txt").exists(), "逃逸文件不得落盘");
        assert!(!dir.path().join("escape.txt").exists(), "逃逸文件不得落到安装根之外");
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn preview_approval_rows_come_from_the_acl_builder() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let id = "com.install.approval-rows";
        let manifest = serde_json::json!({
            "id": id,
            "name": "Approval Rows",
            "version": "1.0.0",
            "type": "js",
            "entry": { "js": "src/index.js", "ui": "index.html" },
            "permissions": ["store:allow-get", "autostart:allow-enable", "fs:allow-remove"],
            "scopes": { "fs:allow-remove": ["$APPDATA/sub/**"] },
            "framework": ">=1.0.0, <2.0.0"
        });
        let files = vec![
            ("manifest.json".to_string(), serde_json::to_vec(&manifest).unwrap()),
            ("src/index.js".to_string(), b"export const activate = () => 'ok';".to_vec()),
            ("index.html".to_string(), b"<!doctype html><body>ok</body>".to_vec()),
        ];
        let (package, verifying_key) = signed_tpkg(dir.path(), id, &files);
        let state = install_state(install_root, &verifying_key);

        let preview =
            cmd_registry_install_preview_as(&Caller::MainWindow, &state, package.to_str().unwrap())
                .unwrap();
        let rows = preview.permissions;
        let names: Vec<&str> = rows.iter().map(|r| r.permission.as_str()).collect();
        assert_eq!(
            names,
            vec!["store:allow-get", "autostart:allow-enable", "fs:allow-remove"],
            "审批行顺序必须跟随 manifest 声明顺序"
        );

        let index = tauron_host::manifest::embedded_permission_index();
        for row in &rows {
            assert_eq!(
                row.description,
                index.entry_of(row.permission.as_str()).unwrap().description,
                "审批文案必须取自权限词表（§4.5 单一来源）"
            );
        }

        let high = rows.iter().find(|r| r.permission == "autostart:allow-enable").unwrap();
        assert_eq!(high.risk, "high");
        assert!(!high.default_checked, "高危档一律默认不勾");
        assert_eq!(
            high.confirmation_hint.as_deref(),
            Some("我理解该权限的能力边界并显式批准"),
            "确认词由 acl 审批构造器单源给出，前端不得另写一份"
        );

        let low = rows.iter().find(|r| r.permission == "store:allow-get").unwrap();
        assert!(low.default_checked);
        assert!(low.confirmation_hint.is_none(), "非高危行不该带确认词");
        assert!(low.scope.is_none(), "无 scope 的权限整字段为 None");
        // 载荷形状：缺席的可选字段不得写成 `null` 混进线形（既有 TS 镜像按缺席处理）。
        let json = serde_json::to_value(low).unwrap();
        assert!(json.get("scope").is_none() && json.get("confirmationHint").is_none(), "{json}");

        let scoped = rows.iter().find(|r| r.permission == "fs:allow-remove").unwrap();
        assert_eq!(scoped.scope.as_deref(), Some("$APPDATA/sub/**"));
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn reviewed_install_rejects_package_replaced_after_preview() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let id = "com.install.reviewed";
        let (package, verifying_key) = signed_install_fixture(dir.path(), id);
        let state = install_state(install_root.clone(), &verifying_key);

        let preview =
            cmd_registry_install_preview_as(&Caller::MainWindow, &state, package.to_str().unwrap())
                .unwrap();

        // Replace the exact path with another correctly signed package having the same
        // plugin id/version/permissions but different executable content. A permission-only
        // approval check would accept this; the V4 digest-bound review must not.
        let manifest = serde_json::json!({
            "id": id,
            "name": "Install Fixture",
            "version": "1.0.0",
            "type": "js",
            "entry": { "js": "src/index.js", "ui": "index.html" },
            "permissions": ["store:allow-get"],
            "framework": ">=1.0.0, <2.0.0"
        });
        let replacement = vec![
            ("manifest.json".to_string(), serde_json::to_vec(&manifest).unwrap()),
            ("src/index.js".to_string(), b"export const activate = () => 'replacement';".to_vec()),
            ("index.html".to_string(), b"<!doctype html><body>replacement</body>".to_vec()),
        ];
        let (same_path, _) = signed_tpkg(dir.path(), id, &replacement);
        assert_eq!(same_path, package);

        let err = cmd_registry_install_reviewed_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
            &preview.review_token,
        )
        .expect_err("package replacement after preview must be rejected");
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED);
        assert!(err.message.contains("package changed after approval"), "{}", err.message);
        assert!(!install_root.join(id).exists());
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn reviewed_install_accepts_the_exact_previewed_package_once() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.review-ok");
        let state = install_state(install_root.clone(), &verifying_key);
        let preview =
            cmd_registry_install_preview_as(&Caller::MainWindow, &state, package.to_str().unwrap())
                .unwrap();

        let installed = cmd_registry_install_reviewed_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
            &preview.review_token,
        )
        .unwrap();
        assert_eq!(installed.plugin_id, "com.install.review-ok");

        let reused = cmd_registry_install_reviewed_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
            &preview.review_token,
        )
        .expect_err("review token must be one-time");
        assert_eq!(reused.code, ErrorCode::E_INSTALL_FAILED);
        assert!(
            reused.message.contains("unknown, expired, or already consumed"),
            "{}",
            reused.message
        );
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn signed_package_preview_install_enable_call_and_uninstall_form_one_chain() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.e2e");
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert("fixture-key".to_string(), verifying_key.to_bytes().to_vec());
        let state = PluginRuntimeState::with_adapter_config(AdapterConfig {
            plugin_install_dir: Some(install_root.clone()),
            plugin_signing_keys: signing_keys,
            acl_signing_key: Some(vec![0x5a; 32]),
            ..AdapterConfig::default()
        });

        let preview =
            cmd_registry_install_preview_as(&Caller::MainWindow, &state, package.to_str().unwrap())
                .unwrap();
        assert_eq!(preview.plugin_id, "com.install.e2e");
        assert_eq!(
            preview.permissions.iter().map(|p| p.permission.as_str()).collect::<Vec<_>>(),
            ["store:allow-get"]
        );

        let denied =
            cmd_registry_install_as(&Caller::MainWindow, &state, package.to_str().unwrap(), &[])
                .unwrap_err();
        assert_eq!(denied.code, ErrorCode::E_FORBIDDEN_PERMISSION);
        assert!(!install_root.join("com.install.e2e").exists());
        assert!(state.registry.find(&PluginId::new("com.install.e2e").unwrap()).is_none());

        let installed = cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();
        assert!(std::path::Path::new(&installed.install_path).join("src/index.js").is_file());
        assert!(std::path::Path::new(&installed.install_path).join("index.html").is_file());
        assert!(install_root.join(".acl/com.install.e2e.acl.json").is_file());
        assert!(std::path::Path::new(&installed.install_path)
            .join(PLUGIN_UI_ACTIVATION_FILE)
            .is_file());

        cmd_registry_admin_as(
            &Caller::MainWindow,
            &state,
            "com.install.e2e",
            RegistryAdminOp::Enable,
        )
        .unwrap();
        let ui = installed_plugin_ui(&state, "com.install.e2e").unwrap();
        assert_eq!(ui.entry.file_name().and_then(|name| name.to_str()), Some("index.html"));
        let pending =
            cmd_plugin_call(&state, "plugin-com.install.e2e", None, "hello", serde_json::json!({}))
                .unwrap();
        assert_eq!(pending.plugin_id, "com.install.e2e");
        cmd_call_end(&state, &Caller::Plugin("com.install.e2e".to_string()), &pending.call_id)
            .unwrap();

        let uninstalled = cmd_registry_admin_as(
            &Caller::MainWindow,
            &state,
            "com.install.e2e",
            RegistryAdminOp::Uninstall,
        )
        .unwrap();
        assert!(!uninstalled.illegal);
        assert!(!install_root.join("com.install.e2e").exists());
        assert!(!install_root.join(".acl/com.install.e2e.acl.json").exists());
        assert!(state.registry.find(&PluginId::new("com.install.e2e").unwrap()).is_none());
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn install_config_root_is_canonical_so_window_asset_prefix_matches() {
        // 轮 16 R-C6：`tauri.rs` 的 `window_create` 对 `plugin-*` 标签做的是
        // `installed.entry.strip_prefix(state.install_config_root())`，而 `entry`
        // 在 `installed_plugin_ui` 里已过 `canonicalize`。此前状态里存的是集成方
        // **原样**传入的根，两个串不同形（多一个 `.`、相对路径、符号链接、
        // Windows 8.3 短名都算）时 strip_prefix 失败 → 插件窗口 E_INSTALL_FAILED。
        let dir = tempfile::tempdir().unwrap();
        let real_root = dir.path().join("plugins");
        std::fs::create_dir_all(&real_root).unwrap();
        let raw_root = real_root.parent().unwrap().join(".").join("plugins");
        assert_ne!(
            raw_root,
            real_root.canonicalize().unwrap(),
            "测试前提：原始串与 canonical 串不同形"
        );

        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.canonical");
        let state = install_state(raw_root, &verifying_key);
        let stored = state.install_config_root().cloned().expect("install root 应已装配");
        assert_eq!(
            stored,
            stored.canonicalize().unwrap(),
            "install_config_root 必须是 canonical 权威副本，否则插件窗口路径前缀对不上"
        );

        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .expect("canonical 根下安装应成功");
        cmd_registry_admin_as(
            &Caller::MainWindow,
            &state,
            "com.install.canonical",
            RegistryAdminOp::Enable,
        )
        .unwrap();
        let ui = installed_plugin_ui(&state, "com.install.canonical").unwrap();
        assert!(
            ui.entry.strip_prefix(&stored).is_ok(),
            "window_create 用的前缀剥离必须成立：{:?} vs {:?}",
            ui.entry,
            stored
        );
    }

    /// 轮 17 R-C6 续：装配期「目录当时不存在 → 按原样保留」这条更窄的路径。
    ///
    /// 用 `..` 段构造失配——它在**所有**平台上都让 `Path` 的组件比较不同形（纯 `.` 段
    /// 会被忽略，Windows 的 `\\?\` 前缀只在 Windows 上成立，都不能当跨平台前提）。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn window_asset_relative_strips_a_raw_root_left_by_the_first_install() {
        let dir = tempfile::tempdir().unwrap();
        let real_root = dir.path().join("plugins");
        std::fs::create_dir_all(&real_root).unwrap();
        let raw_root = real_root.join("..").join("plugins");
        let canon_root = real_root.canonicalize().unwrap();
        assert_ne!(raw_root, canon_root, "测试前提：原样串与 canonical 串不同形");

        // 测试前提的另一半：不 canonicalize 就比较，正是轮 16 修掉的那个失败。
        let entry = canon_root.join("p1").join("ui").join("index.html");
        assert!(
            entry.strip_prefix(&raw_root).is_err(),
            "若这条断言红，说明本例没构造出真实失配，测试失效"
        );

        assert_eq!(
            plugin_window_asset_relative(&raw_root, "p1", &entry).unwrap(),
            "ui/index.html",
            "首装留下的原样根不得让已装好的插件窗口报「越出安装目录」"
        );
    }

    /// 同一入口的失效侧：越出安装根、以及根又消失时，都必须**拒绝**而不是放行。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn window_asset_relative_fails_closed_on_escape_and_vanished_root() {
        let dir = tempfile::tempdir().unwrap();
        let real_root = dir.path().join("plugins");
        std::fs::create_dir_all(&real_root).unwrap();
        let canon_root = real_root.canonicalize().unwrap();

        let outside = canon_root.parent().unwrap().join("elsewhere").join("ui.html");
        let escaped = plugin_window_asset_relative(&real_root, "p1", &outside).unwrap_err();
        assert_eq!(escaped.code, ErrorCode::E_INSTALL_FAILED);
        assert!(escaped.message.contains("越出安装目录"));

        // 根在比较时已被删除：canonicalize 失败退回原样串 → 与 canonical 的 entry 不同形
        // → 拒绝。退回原值只是退回旧行为，不是放行。
        let entry = canon_root.join("p1").join("index.html");
        let vanished = dir.path().join("no-such-install-root");
        assert_eq!(
            plugin_window_asset_relative(&vanished, "p1", &entry).unwrap_err().code,
            ErrorCode::E_INSTALL_FAILED,
            "根不可用时宁可开不了窗口，也不能把任意路径当插件资产"
        );
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn installed_plugin_ui_rejects_content_and_activation_metadata_tampering() {
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.integrity");
        let state = install_state(install_root.clone(), &verifying_key);
        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();
        cmd_registry_admin_as(
            &Caller::MainWindow,
            &state,
            "com.install.integrity",
            RegistryAdminOp::Enable,
        )
        .unwrap();

        assert!(installed_plugin_ui(&state, "com.install.integrity").is_ok());
        let plugin_dir = install_root.join("com.install.integrity");

        // Referenced JS is part of the activation set even though the window entry is index.html.
        let js_path = plugin_dir.join("src/index.js");
        let original_js = std::fs::read(&js_path).unwrap();
        std::fs::write(&js_path, b"export const activate = () => 'tampered';").unwrap();
        let js_error = installed_plugin_ui(&state, "com.install.integrity").unwrap_err();
        assert_eq!(js_error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(js_error.message.contains("integrity"));
        std::fs::write(&js_path, original_js).unwrap();
        assert!(installed_plugin_ui(&state, "com.install.integrity").is_ok());

        let ui_path = plugin_dir.join("index.html");
        let tampered = b"<!doctype html><html><body>tampered</body></html>";
        std::fs::write(&ui_path, tampered).unwrap();
        let content_error = installed_plugin_ui(&state, "com.install.integrity").unwrap_err();
        assert_eq!(content_error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(content_error.message.contains("integrity"));

        // Even if an attacker edits the matching digest record, the HMAC covers the complete
        // ordered activation set and cannot be forged without the host secret.
        let activation_path = plugin_dir.join(PLUGIN_UI_ACTIVATION_FILE);
        let mut activation: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&activation_path).unwrap()).unwrap();
        let records = activation["records"].as_array_mut().unwrap();
        let ui_record = records
            .iter_mut()
            .find(|record| {
                record["resource"]
                    .as_str()
                    .is_some_and(|resource| resource.ends_with(":asset:index.html"))
            })
            .expect("index.html activation record");
        ui_record["content"]["sha256"] = serde_json::json!(hex::encode(Sha256::digest(tampered)));
        ui_record["content"]["size"] = serde_json::json!(tampered.len() as u64);
        std::fs::write(&activation_path, serde_json::to_vec_pretty(&activation).unwrap()).unwrap();
        let metadata_error = installed_plugin_ui(&state, "com.install.integrity").unwrap_err();
        assert_eq!(metadata_error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(metadata_error.message.contains("HMAC"));
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn installed_plugin_ui_rejects_injected_files_after_activation() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.injected");
        let state = install_state(install_root.clone(), &verifying_key);
        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();
        cmd_registry_admin_as(
            &Caller::MainWindow,
            &state,
            "com.install.injected",
            RegistryAdminOp::Enable,
        )
        .unwrap();
        assert!(installed_plugin_ui(&state, "com.install.injected").is_ok());

        std::fs::write(
            install_root.join("com.install.injected/injected.js"),
            b"window.pwned = true;",
        )
        .unwrap();
        let error = installed_plugin_ui(&state, "com.install.injected").unwrap_err();
        assert_eq!(error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(error.message.contains("integrity"));
    }

    /// V4 A84：asset 读侧逐字节复核密封摘要——路径合规不等于可以服务。
    ///
    /// 这条链此前断在：入口页在 [`installed_plugin_ui`] 里做过全目录摘要复核，而浏览器
    /// 随后逐个 GET 的 `src/index.js` 等资产走 asset 协议，一条字节都没复核过。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn asset_read_path_serves_only_sealed_and_untampered_content() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.asset");
        let state = install_state(install_root.clone(), &verifying_key);
        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();

        let trust = PluginAssetTrust::new(install_root.clone(), vec![0x5a; 32]).unwrap();
        let plugin_dir = install_root.join("com.install.asset");
        let html = std::fs::read(plugin_dir.join("index.html")).unwrap();
        let js = std::fs::read(plugin_dir.join("src/index.js")).unwrap();
        trust.verify_asset("com.install.asset", "index.html", &html).unwrap();
        trust.verify_asset("com.install.asset", "src/index.js", &js).unwrap();

        let tampered = b"<!doctype html><html><body>tampered</body></html>";
        std::fs::write(plugin_dir.join("index.html"), tampered).unwrap();
        let error = trust
            .verify_asset("com.install.asset", "index.html", tampered)
            .expect_err("tampered asset must not be served");
        assert_eq!(error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(error.message.contains("integrity"), "{}", error.message);

        // 换回原字节即恢复服务：复核是逐次比对内容，不是一次性的开关。
        trust.verify_asset("com.install.asset", "index.html", &html).unwrap();
    }

    /// 攻击者改掉记录里的摘要也没用：记录集本身由宿主 HMAC 认证。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn asset_read_path_refuses_a_forged_activation_record() {
        use sha2::{Digest, Sha256};

        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.forged");
        let state = install_state(install_root.clone(), &verifying_key);
        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();
        let trust = PluginAssetTrust::new(install_root.clone(), vec![0x5a; 32]).unwrap();
        let plugin_dir = install_root.join("com.install.forged");

        let tampered = b"<!doctype html><html><body>evil</body></html>";
        std::fs::write(plugin_dir.join("index.html"), tampered).unwrap();
        let activation_path = plugin_dir.join(PLUGIN_UI_ACTIVATION_FILE);
        let mut activation: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&activation_path).unwrap()).unwrap();
        let records = activation["records"].as_array_mut().unwrap();
        let ui_record = records
            .iter_mut()
            .find(|record| {
                record["resource"]
                    .as_str()
                    .is_some_and(|resource| resource.ends_with(":asset:index.html"))
            })
            .expect("index.html activation record");
        ui_record["content"]["sha256"] = serde_json::json!(hex::encode(Sha256::digest(tampered)));
        ui_record["content"]["size"] = serde_json::json!(tampered.len() as u64);
        std::fs::write(&activation_path, serde_json::to_vec_pretty(&activation).unwrap()).unwrap();

        let error = trust
            .verify_asset("com.install.forged", "index.html", tampered)
            .expect_err("records are only trusted while the HMAC still covers them");
        assert_eq!(error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(error.message.contains("HMAC"), "{}", error.message);
    }

    /// 没有密封记录的内容一律不服务：安装后注入的文件、activation 文件本身、
    /// 以及**另一个插件**合法密封但错位的记录集（同一把宿主密钥也拦不住跨插件重放）。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn asset_read_path_refuses_unsealed_and_foreign_plugin_content() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.sealed");
        let state = install_state(install_root.clone(), &verifying_key);
        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();
        let second_package = signed_install_fixture(dir.path(), "com.install.foreign").0;
        cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            second_package.to_str().unwrap(),
            &["store:allow-get".into()],
        )
        .unwrap();
        let trust = PluginAssetTrust::new(install_root.clone(), vec![0x5a; 32]).unwrap();
        let plugin_dir = install_root.join("com.install.sealed");
        let html = std::fs::read(plugin_dir.join("index.html")).unwrap();
        trust.verify_asset("com.install.sealed", "index.html", &html).unwrap();

        std::fs::write(plugin_dir.join("injected.js"), b"window.pwned = true;").unwrap();
        let injected = trust
            .verify_asset("com.install.sealed", "injected.js", b"window.pwned = true;")
            .expect_err("content without a sealed record must not be served");
        assert!(injected.message.contains("没有密封的 activation 记录"), "{}", injected.message);

        let activation_error = trust
            .verify_asset(
                "com.install.sealed",
                PLUGIN_UI_ACTIVATION_FILE,
                &std::fs::read(plugin_dir.join(PLUGIN_UI_ACTIVATION_FILE)).unwrap(),
            )
            .expect_err("the activation seal is not an asset");
        assert!(
            activation_error.message.contains("没有密封的 activation 记录"),
            "{}",
            activation_error.message
        );

        // 两个 fixture 的 index.html 字节完全相同，所以只有「记录属于哪个插件」这一条
        // 判定能把外来记录集挡住——去掉它，这里就会被服务。
        std::fs::copy(
            install_root.join("com.install.foreign").join(PLUGIN_UI_ACTIVATION_FILE),
            plugin_dir.join(PLUGIN_UI_ACTIVATION_FILE),
        )
        .unwrap();
        let foreign = trust
            .verify_asset("com.install.sealed", "index.html", &html)
            .expect_err("another plugin's sealed records do not authorize this one");
        assert!(foreign.message.contains("没有密封的 activation 记录"), "{}", foreign.message);
    }

    /// 读侧信任根与安装侧同源：密钥不足时构造不出信任根。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn asset_trust_requires_the_host_sized_key() {
        let dir = tempfile::tempdir().unwrap();
        let error = PluginAssetTrust::new(dir.path().join("plugins"), vec![0x5a; 31])
            .expect_err("a 31-byte host key cannot authenticate activation records");
        assert_eq!(error.code, ErrorCode::E_INSTALL_FAILED);
        assert!(error.message.contains("32 字节"), "{}", error.message);
        let trust = PluginAssetTrust::new(dir.path().join("plugins"), vec![0x5a; 32]).unwrap();
        assert!(
            format!("{trust:?}").contains("<32 bytes redacted>"),
            "Debug 输出不得带出宿主密钥字节"
        );
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn invalid_package_signature_leaves_no_install_state() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.invalid");
        let sidecar_path = std::path::PathBuf::from(format!("{}.sig", package.display()));
        let sidecar = std::fs::read_to_string(&sidecar_path)
            .unwrap()
            .replace("\"signature\":\"", "\"signature\":\"00");
        std::fs::write(sidecar_path, sidecar).unwrap();
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert("fixture-key".to_string(), verifying_key.to_bytes().to_vec());
        let state = PluginRuntimeState::with_adapter_config(AdapterConfig {
            plugin_install_dir: Some(install_root.clone()),
            plugin_signing_keys: signing_keys,
            acl_signing_key: Some(vec![0x5a; 32]),
            ..AdapterConfig::default()
        });
        assert!(cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()]
        )
        .is_err());
        assert!(!install_root.join("com.install.invalid").exists());
        assert!(state.registry.find(&PluginId::new("com.install.invalid").unwrap()).is_none());
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn acl_persistence_failure_rolls_back_package_directory_and_registry() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        std::fs::create_dir_all(&install_root).unwrap();
        std::fs::write(install_root.join(".acl"), "not a directory").unwrap();
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.aclfail");
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert("fixture-key".to_string(), verifying_key.to_bytes().to_vec());
        let state = PluginRuntimeState::with_adapter_config(AdapterConfig {
            plugin_install_dir: Some(install_root.clone()),
            plugin_signing_keys: signing_keys,
            acl_signing_key: Some(vec![0x5a; 32]),
            ..AdapterConfig::default()
        });
        assert!(cmd_registry_install_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()]
        )
        .is_err());
        assert!(!install_root.join("com.install.aclfail").exists());
        assert!(state.registry.find(&PluginId::new("com.install.aclfail").unwrap()).is_none());
        assert_eq!(std::fs::read_to_string(install_root.join(".acl")).unwrap(), "not a directory");
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn failed_registry_uninstall_restores_staged_files_and_acl() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("plugins");
        let plugin_path = install_root.join("com.install.orphan");
        let acl_path = install_root.join(".acl/com.install.orphan.acl.json");
        std::fs::create_dir_all(&plugin_path).unwrap();
        std::fs::create_dir_all(acl_path.parent().unwrap()).unwrap();
        std::fs::write(plugin_path.join("marker"), "preserve").unwrap();
        std::fs::write(&acl_path, "grant").unwrap();
        let state = PluginRuntimeState::with_adapter_config(AdapterConfig {
            plugin_install_dir: Some(install_root.clone()),
            ..AdapterConfig::default()
        });
        assert!(
            cmd_registry_admin(&state, "com.install.orphan", RegistryAdminOp::Uninstall).is_err()
        );
        assert_eq!(std::fs::read_to_string(plugin_path.join("marker")).unwrap(), "preserve");
        assert_eq!(std::fs::read_to_string(acl_path).unwrap(), "grant");
        assert_eq!(
            std::fs::read_dir(&install_root).unwrap().count(),
            2,
            "暂存目录恢复后不得留下隐藏残留"
        );
    }

    #[test]
    fn command_state_new() {
        let state = CommandState::new();
        let result = cmd_registry_list_all(&state);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn command_state_with_config() {
        let config = RegistryConfig {
            max_plugins: 4,
            max_active_identities: 4,
            max_pending_calls: 100,
            pending_ttl: std::time::Duration::from_secs(10),
            plugin_filter: None,
        };
        let state = CommandState::with_config(config);
        assert!(cmd_registry_list_all(&state).is_ok());
    }

    #[test]
    fn cmd_registry_list_visible_empty() {
        let state = CommandState::new();
        let result = cmd_registry_list(&state, None, &[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn cmd_registry_list_all_empty() {
        let state = CommandState::new();
        let result = cmd_registry_list_all(&state);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn cmd_lifecycle_report_unknown_label() {
        let state = CommandState::new();
        let result = cmd_lifecycle_report(&state, "plugin-test.nonexistent", None, Event::Attach);
        assert!(result.is_err());
        // label_to_plugin_id 应解析成功，但 registry 中没有该插件
        assert_eq!(result.unwrap_err().code, ErrorCode::E_UNKNOWN_PLUGIN);
    }

    #[test]
    fn cmd_lifecycle_report_forged_identity() {
        let state = CommandState::new();
        let result = cmd_lifecycle_report(&state, "plugin-p.real", Some("p.fake"), Event::Attach);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn cmd_lifecycle_report_rejects_privileged_events_before_lookup() {
        // self 档白名单在适配器边界同样生效。断言错误码是 `E_AUTH_DENIED` 而不是
        // `E_UNKNOWN_PLUGIN`：说明白名单判定发生在注册表查表**之前**——它是权限闸，
        // 不是查表失败。插件不得自报用户放行（Enable）、自己解禁（SafemodeExit）、
        // 伪造安装结果（InstallOk）、或把自己推进终态（Uninstall）。
        let state = CommandState::new();
        for ev in [
            Event::Enable,
            Event::Disable,
            Event::TrialEnable,
            Event::SafemodeEnter,
            Event::SafemodeExit,
            Event::InstallStart,
            Event::InstallOk,
            Event::InstallFail,
            Event::Uninstall,
            Event::Purge,
        ] {
            let err = cmd_lifecycle_report(&state, "plugin-p.any", None, ev).unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{ev} 应被 self 档白名单拒绝");
        }
    }

    #[test]
    fn cmd_events_publish_undeclared_topic_dropped() {
        let state = CommandState::new();
        let result = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.producer.ready",
            serde_json::json!({"ready": true}),
        );
        assert!(result.is_ok(), "未声明的 topic 不应报错，应静默丢弃");
        let pr = result.unwrap();
        assert!(pr.dropped, "未声明的 topic 应被丢弃");
    }

    #[test]
    fn cmd_events_publish_declared_topic() {
        let state = CommandState::new();
        // 先声明 topic
        {
            let bus = state.bus.lock();
            let decls =
                vec![EventDecl { topic: "plugin:p.producer.ready".to_string(), public: true }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 再发布
        let result = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.producer.ready",
            serde_json::json!({"ready": true}),
        );
        assert!(result.is_ok());
        let pr = result.unwrap();
        assert!(pr.delivered == 0, "无订阅者时应 delivered=0");
        assert!(!pr.dropped);
    }

    #[test]
    fn cmd_events_publish_wrong_publisher_dropped() {
        let state = CommandState::new();
        // 声明 topic（声明者 = p.producer）
        {
            let bus = state.bus.lock();
            let decls =
                vec![EventDecl { topic: "plugin:p.producer.ready".to_string(), public: true }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 另一个插件尝试发布 → 被丢弃（不报错，但 dropped=true）
        let result = cmd_events_publish(
            &state,
            "p.forged",
            "plugin:p.producer.ready",
            serde_json::json!({"forged": true}),
        );
        // publish_request 对越界发布不报错（静默丢弃）
        assert!(result.is_ok());
        let pr = result.unwrap();
        assert!(pr.dropped, "越界发布应被丢弃");
    }

    #[test]
    fn cmd_events_subscribe_declared_public_topic() {
        let state = CommandState::new();
        // 声明 public topic
        {
            let bus = state.bus.lock();
            let decls =
                vec![EventDecl { topic: "plugin:p.producer.ready".to_string(), public: true }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 另一个插件订阅 → 成功（public topic 无需审批）
        let result =
            cmd_events_subscribe(&state, "p.consumer", "window-1", "plugin:p.producer.ready");
        assert!(result.is_ok());
        let outcome = result.unwrap();
        assert!(!outcome.duplicate);
        assert!(!outcome.token.is_empty());
    }

    #[test]
    fn cmd_events_subscribe_idempotent() {
        let state = CommandState::new();
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl { topic: "plugin:p.ready".to_string(), public: true }];
            bus.declare_topics("p", &decls).unwrap();
        }
        // 同 subscriber × window × topic 重复订阅 → 幂等返回同一 token
        let r1 = cmd_events_subscribe(&state, "c1", "w1", "plugin:p.ready").unwrap();
        let r2 = cmd_events_subscribe(&state, "c1", "w1", "plugin:p.ready").unwrap();
        assert_eq!(r1.token, r2.token);
        assert!(!r1.duplicate);
        assert!(r2.duplicate);
    }

    #[test]
    fn cmd_events_drain_completes_subscribe_publish_consume_chain() {
        // 断链回归测试：订阅 → 发布 → drain 必须取到帧。
        // 此前 drain 无任何调用者，队列只进不出，前端永远收不到事件。
        let state = CommandState::new();
        {
            let bus = state.bus.lock();
            let decls =
                vec![EventDecl { topic: "plugin:p.producer.ready".to_string(), public: true }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }

        cmd_events_subscribe(&state, "p.consumer", "window-1", "plugin:p.producer.ready")
            .expect("subscribe should succeed");

        let pr = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.producer.ready",
            serde_json::json!({"v": 1}),
        )
        .expect("publish should succeed");
        assert_eq!(pr.delivered, 1, "publish must enqueue for the subscriber");

        // 取件：host_events_publish 走可靠语义（Request 通道）
        let frames =
            cmd_events_drain(&state, "p.consumer", "request").expect("drain should succeed");
        assert_eq!(frames.len(), 1, "drain must return the queued frame");
        assert_eq!(frames[0].topic, "plugin:p.producer.ready");
        assert_eq!(frames[0].payload, serde_json::json!({"v": 1}));

        // event 通道不受影响（各通道队列独立）
        let event_frames = cmd_events_drain(&state, "p.consumer", "event").unwrap();
        assert!(event_frames.is_empty(), "channels are independent");

        // 取完即空（不重复投递）
        let again = cmd_events_drain(&state, "p.consumer", "request").unwrap();
        assert!(again.is_empty(), "frames must not be redelivered");
    }

    #[test]
    fn cmd_events_drain_rejects_unknown_kind() {
        let state = CommandState::new();
        let err = cmd_events_drain(&state, "c1", "bogus").expect_err("must reject");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn channel_kind_parse_matches_serde_kebab_case() {
        use tauron_host::eventbus::ChannelKind;
        assert_eq!(ChannelKind::parse("event"), Some(ChannelKind::Event));
        assert_eq!(ChannelKind::parse("request"), Some(ChannelKind::Request));
        assert_eq!(ChannelKind::parse("state"), Some(ChannelKind::State));
        assert_eq!(ChannelKind::parse("Event"), None, "大小写敏感");
        assert_eq!(ChannelKind::parse(""), None);
        // 与 serde 序列化互逆
        for kind in [ChannelKind::Event, ChannelKind::Request, ChannelKind::State] {
            let s = serde_json::to_value(kind).unwrap();
            assert_eq!(ChannelKind::parse(s.as_str().unwrap()), Some(kind));
        }
    }

    #[test]
    fn cmd_events_subscribe_private_topic_rejected() {
        let state = CommandState::new();
        // 声明私有 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl { topic: "plugin:p.private".to_string(), public: false }];
            bus.declare_topics("p", &decls).unwrap();
        }
        // 另一个插件订阅私有 topic → 拒绝
        let result = cmd_events_subscribe(&state, "c1", "w1", "plugin:p.private");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn cmd_events_unsubscribe_unknown_token_idempotent() {
        let state = CommandState::new();
        let result = cmd_events_unsubscribe(&state, "fake-token");
        assert!(result.is_ok(), "退订不存在的 token 应幂等成功");
    }

    #[test]
    fn cmd_events_full_cycle() {
        let state = CommandState::new();
        // 1. 声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl { topic: "plugin:p.ready".to_string(), public: true }];
            bus.declare_topics("p.producer", &decls).unwrap();
        }
        // 2. 订阅
        let sub = cmd_events_subscribe(&state, "p.consumer", "w1", "plugin:p.ready").unwrap();
        // 3. 发布
        let pub_result = cmd_events_publish(
            &state,
            "p.producer",
            "plugin:p.ready",
            serde_json::json!({"ready": true}),
        )
        .unwrap();
        assert!(pub_result.delivered == 1, "应投递到 1 个订阅者");
        // 4. 退订
        cmd_events_unsubscribe(&state, &sub.token).unwrap();
    }

    #[test]
    fn cmd_cancel_unknown_call() {
        let state = CommandState::new();
        let result =
            cmd_cancel(&state, &Caller::Plugin("com.example.a".to_string()), "unknown-call-id");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn cmd_call_end_unknown_call() {
        let state = CommandState::new();
        let result =
            cmd_call_end(&state, &Caller::Plugin("com.example.a".to_string()), "unknown-call-id");
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    /// 第 4 轮回归：`host_call_end` / `host_cancel` 是 `self` 档（"只能作用于自己"），
    /// 但此前完全不校验归属——任何插件拿到别人的 callId 就能终结对方的调用与流。
    /// 本测试钉住"跨插件一律拒绝，且拒绝时零副作用"。
    #[test]
    fn call_end_and_cancel_reject_cross_plugin() {
        let state = CommandState::new();
        let index = empty_index();
        state.registry.install(&index, test_manifest("p.call")).unwrap();
        cmd_registry_admin(&state, "p.call", RegistryAdminOp::Enable).unwrap();
        cmd_lifecycle_report(&state, "plugin-p.call", None, Event::Attach).unwrap();
        let pending =
            cmd_plugin_call(&state, "plugin-p.call", None, "m", serde_json::json!({})).unwrap();
        let intruder = Caller::Plugin("com.example.other".to_string());

        let denied = cmd_call_end(&state, &intruder, &pending.call_id).unwrap_err();
        assert_eq!(denied.code, ErrorCode::E_AUTH_DENIED);
        assert!(state.registry.peek_call(&pending.call_id).is_ok(), "被拒时调用必须仍在");

        let denied = cmd_cancel(&state, &intruder, &pending.call_id).unwrap_err();
        assert_eq!(denied.code, ErrorCode::E_AUTH_DENIED);
        assert!(state.registry.peek_call(&pending.call_id).is_ok(), "被拒时调用必须仍在");

        // 归属者自己可以做这两件事。
        cmd_call_end(&state, &Caller::Plugin("p.call".to_string()), &pending.call_id).unwrap();
    }

    #[test]
    fn cmd_settings_get_returns_null() {
        let state = CommandState::new();
        let result = cmd_settings_get(&state, "some.key");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn cmd_notify_succeeds() {
        let state = CommandState::new();
        let result = cmd_notify(&state, "p.audio", "Title", "Body");
        assert!(result.is_ok());
    }

    #[test]
    fn notifications_list_and_read_roundtrip() {
        let state = CommandState::new();
        cmd_notify(&state, "com.a", "T1", "B1").unwrap();
        cmd_notify(&state, "com.b", "T2", "B2").unwrap();

        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 2);
        assert_eq!(snap["unread"], 2);
        let items = snap["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        // 时间倒序：最后推入的排最前；线形键必须 camelCase。
        assert_eq!(items[0]["title"], "T2");
        assert_eq!(items[0]["pluginId"], "com.b");

        // 单条已读。
        let id = items[0]["id"].as_str().unwrap().to_string();
        let r = cmd_notifications_read(&state, Some(&id)).unwrap();
        assert_eq!(r["marked"], 1);
        let snap = cmd_notifications_list(&state, Some(1)).unwrap();
        assert_eq!(snap["items"].as_array().unwrap().len(), 1, "limit 生效");
        assert_eq!(snap["items"][0]["read"], true);
        assert_eq!(snap["unread"], 1, "unread 是全量计数，不受 limit 影响");

        // 全部已读。
        let r = cmd_notifications_read(&state, None).unwrap();
        assert_eq!(r["marked"], 1);
        assert_eq!(cmd_notifications_list(&state, None).unwrap()["unread"], 0);

        // 未知 id：marked 0，不 panic。
        assert_eq!(cmd_notifications_read(&state, Some("nope")).unwrap()["marked"], 0);
    }

    #[test]
    fn uninstall_recycles_plugin_notifications() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        cmd_notify(&state, "com.a", "T", "B").unwrap();
        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();
        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 0, "卸载必须回收该插件的通知");
    }

    // ────────────────────────────────────────────────────────────
    // R5 / P0-1：流式帧走完整命令面
    // ────────────────────────────────────────────────────────────

    /// 记录型载体：断言「帧真的派发到了接收方」。
    #[derive(Default)]
    struct StreamRecorder {
        frames: parking_lot::Mutex<Vec<StreamFrame>>,
    }

    impl tauron_host::stream::StreamSink for StreamRecorder {
        fn send(&self, frame: &StreamFrame) -> HostResult<()> {
            self.frames.lock().push(frame.clone());
            Ok(())
        }
    }

    impl StreamRecorder {
        fn frames(&self) -> Vec<StreamFrame> {
            self.frames.lock().clone()
        }
    }

    fn installed_state(id: &str) -> CommandState {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest(id)).unwrap();
        // 发起调用前必须处于 Enabled：`call_begin` 对 INSTALLED 直接拒绝
        // （E_PLUGIN_DISABLED），这是生命周期门，不是本轮的流式逻辑。
        cmd_registry_admin(&state, id, RegistryAdminOp::Enable).unwrap();
        state
    }

    /// 方案 R5「验证」项：**写 3 帧 → 收 3 帧 + end**，且 seq 连续。
    #[test]
    fn stream_roundtrip_three_frames_then_end() {
        let state = installed_state("com.a");
        let call =
            cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "format", serde_json::json!({}))
                .unwrap();

        // wire 层把 channel 转成 sink 后在这里登记（生产路径见 `wire_plugin_call`）。
        let sink = Arc::new(StreamRecorder::default());
        state.registry.stream_bind(&call.call_id, &call.plugin_id, sink.clone()).unwrap();

        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();
        assert_eq!(opened.call_id, call.call_id);

        for i in 1..=3u64 {
            let f = cmd_stream_write(
                &state,
                "com.a",
                &opened.stream_id,
                Some(serde_json::json!({ "i": i })),
                None,
            )
            .unwrap();
            assert_eq!(f.seq, i, "seq 由宿主铸且连续");
            assert_eq!(f.kind, StreamKind::Data);
        }
        let end = cmd_stream_close(&state, "com.a", &opened.stream_id, "end").unwrap();
        assert_eq!(end.seq, 4, "终帧占一个 seq");

        let frames = sink.frames();
        assert_eq!(frames.len(), 4, "接收方必须真的收到 3 帧 + end");
        assert_eq!(frames[3].kind, StreamKind::End);
        assert_eq!(frames[1].args_json.as_ref().unwrap()["i"], 2);

        // 终帧后句柄失效（不静默成功）。
        let err = cmd_stream_write(&state, "com.a", &opened.stream_id, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
    }

    /// `argsRaw` 是二进制载荷的**真实出口**（P0-1 的全部意义）：
    /// 字节到达接收方时仍是字节，不经 base64、也不被丢弃。
    #[test]
    fn stream_carries_binary_payload_without_base64() {
        let state = installed_state("com.a");
        let call =
            cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
                .unwrap();
        let sink = Arc::new(StreamRecorder::default());
        state.registry.stream_bind(&call.call_id, &call.plugin_id, sink.clone()).unwrap();
        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();

        let bytes = vec![0u8, 159, 146, 150, 255];
        cmd_stream_write(&state, "com.a", &opened.stream_id, None, Some(bytes.clone())).unwrap();

        let frames = sink.frames();
        assert_eq!(frames[0].args_raw.as_deref(), Some(bytes.as_slice()));
        assert!(frames[0].args_json.is_none());
    }

    /// 没有载体的调用**不得**开出「帧进虚空」的流：宁可显式失败。
    #[test]
    fn stream_open_without_carrier_is_rejected() {
        let state = installed_state("com.a");
        let call =
            cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
                .unwrap();
        let err = cmd_stream_open(&state, "com.a", &call.call_id).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_CALL_NOT_FOUND);
        assert!(err.message.contains("没有帧载体"), "message={}", err.message);
    }

    /// 跨插件写帧必须被拒（句柄是 UUID，但不靠「猜不到」兜底）。
    #[test]
    fn stream_write_from_another_plugin_is_denied() {
        let state = installed_state("com.a");
        state.registry.install(&empty_index(), test_manifest("com.b")).unwrap();
        let call =
            cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
                .unwrap();
        let sink = Arc::new(StreamRecorder::default());
        state.registry.stream_bind(&call.call_id, &call.plugin_id, sink.clone()).unwrap();
        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();

        let err = cmd_stream_write(&state, "com.b", &opened.stream_id, None, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(sink.frames().is_empty(), "被拒的写不得留下帧");
    }

    /// 关流的 `kind` 是闭集：`data` 不是终帧，`done` 是拼写错误。
    #[test]
    fn stream_close_rejects_non_terminal_or_unknown_kind() {
        let state = installed_state("com.a");
        let call =
            cmd_plugin_call(&state, "plugin-com.a", Some("com.a"), "fmt", serde_json::json!({}))
                .unwrap();
        let sink = Arc::new(StreamRecorder::default());
        state.registry.stream_bind(&call.call_id, &call.plugin_id, sink).unwrap();
        let opened = cmd_stream_open(&state, "com.a", &call.call_id).unwrap();

        let err = cmd_stream_close(&state, "com.a", &opened.stream_id, "data").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        let err = cmd_stream_close(&state, "com.a", &opened.stream_id, "done").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("未知帧种类"), "message={}", err.message);
        // 未终结的句子柄应仍可写。
        assert!(cmd_stream_write(&state, "com.a", &opened.stream_id, None, None).is_ok());
    }

    #[test]
    fn cmd_recover_boot_succeeds() {
        let state = CommandState::new();
        let result = cmd_recover_boot(&state);
        assert!(result.is_ok());
        let json = result.unwrap();
        assert_eq!(json["phase"], "normal");
        // 线名必须与 TS RecoveryBootResult 消费字段一致（camelCase）。
        assert_eq!(json["phaseName"], "正常启动");
        assert_eq!(json["counter"]["consecutiveFailures"], 0);
        assert_eq!(json["counter"]["safemodeFailures"], 0);
    }

    #[test]
    fn cmd_recover_report_drives_phase_and_reconciles_registry() {
        let state = CommandState::new();
        // 注册表里有一个（非必需）插件：安全模式应该把它标记为禁用。
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 表外 outcome 直接拒绝——不静默当成 failure 处理。
        assert_eq!(
            cmd_recover_report(&state, "crashed", None).unwrap_err().code,
            ErrorCode::E_INVALID_MANIFEST
        );

        // 两次失败 → 安全模式，且判定要落到注册表的 disabledBySafemode 上。
        for _ in 0..2 {
            let r = cmd_recover_report(&state, "failure", None).unwrap();
            assert_eq!(r["engineAction"], "bootFailure");
            assert_eq!(r["outcome"], "failure");
        }
        assert_eq!(cmd_recover_boot(&state).unwrap()["phase"], "safemode");
        assert!(
            find_summary(&state, "com.a").unwrap().disabled_by_safemode,
            "安全模式下非必需插件必须被标记"
        );
        // 上报即结论明确：bootInFlight 必须清零，否则下次启动会重复计数。
        assert_eq!(
            cmd_recover_boot(&state).unwrap()["persistence"]["bootInFlight"],
            serde_json::json!(false)
        );

        // 上报成功 → 计数清零 + 阶段回 normal + 注册表禁用标记清除。
        let r = cmd_recover_report(&state, "success", None).unwrap();
        assert_eq!(r["engineAction"], "bootSuccess");
        assert_eq!(r["phase"], "normal");
        assert!(
            !find_summary(&state, "com.a").unwrap().disabled_by_safemode,
            "恢复正常后禁用标记必须清除"
        );
    }

    #[test]
    fn cmd_recover_report_failure_scoped_to_trial_plugin() {
        let state = CommandState::new();

        // 正常阶段不允许试验性启用。
        assert_eq!(
            cmd_recover_trial_enable(&state, "com.a").unwrap_err().code,
            ErrorCode::E_STATE_INVALID_TRANSITION
        );

        // 两次失败 → 安全模式。
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        let r = cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(r["phase"], "safemode");
        assert_eq!(r["engineAction"], "notInRegistry");
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("trial-enable")
        );
        // 试启中的插件**不算禁用**：`disabledPlugins` 按 `!is_enabled()` 过滤，
        // `TrialEnable` 属于启用。TS 侧 `DisabledPlugin['state']` 的取值集合
        // 据此收窄——这里锁住该不变式，改过滤条件时必须同步改类型。
        let ids: Vec<&str> = r["disabledPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v["pluginId"].as_str())
            .collect();
        assert!(!ids.contains(&"com.a"), "试启中的插件不得出现在 disabledPlugins");

        // 试启中失败 → 按试验失败记（只累该插件的试验次数）。
        let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
        assert_eq!(r["engineAction"], "disabled-by-safemode");
        assert_eq!(r["suspectedPlugin"], "com.a");
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("disabled-by-safemode")
        );
        // 全局启动计数**不得**被这次试验失败累加——那会把单个插件的失败误判
        // 成整个应用的崩溃。
        assert_eq!(r["counter"]["consecutiveFailures"], 2);
        assert_eq!(r["phase"], "safemode");

        // 预算耗尽：不可再次试启。
        assert_eq!(
            cmd_recover_trial_enable(&state, "com.a").unwrap_err().code,
            ErrorCode::E_PLUGIN_DISABLED
        );
    }

    /// 试启主链路闭环：安全模式 → 标记在册 → 试启（引擎改判 + 注册表走 D28
    /// `TrialEnable`，清标志）→ 试验失败（引擎回落 + 对账重新标记禁用）。
    ///
    /// 断链回归：若试启仍发 `SafemodeExit`，注册表不记独立试验预算、
    /// `trial_from_safemode` 不置位，该插件自行上报错误时走不到 D28 回落
    /// （与引擎的试验判定各自为政）；若不前置对账，刚装载、尚未被标记的
    /// 插件停在 `Installed`，试启事件会撞非法迁移。
    #[test]
    fn recovery_trial_enable_action_is_deduplicated_in_same_incident() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        let first = cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_ne!(first["engineAction"], "trialEnable:deduplicated");
        let second = cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(second["engineAction"], "trialEnable:deduplicated");
        let engine = state.recovery.lock();
        let effect_id_prefix = "registry:com.a:";
        let serialized = engine.to_json().to_string();
        assert!(
            serialized.contains(effect_id_prefix),
            "effect ledger must be persisted in engine state"
        );
    }

    #[test]
    fn recovery_trial_enable_rolls_back_engine_when_registry_rejects_trial() {
        let state = CommandState::new();
        let id = PluginId::new("com.a").unwrap();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // Exhaust only the registry-side D28 trial budget. Do not report these failures through
        // the recovery adapter; the engine intentionally still has an unused trial budget.
        let entered = state.registry.report_event(&id, Event::SafemodeEnter).unwrap();
        assert!(!entered.illegal);
        for _ in 0..tauron_host::lifecycle::MAX_TRIAL_ATTEMPTS {
            let trial = state.registry.report_event(&id, Event::TrialEnable).unwrap();
            assert!(!trial.illegal);
            let failed = state.registry.report_event(&id, Event::ErrorRetryable).unwrap();
            assert!(!failed.illegal);
        }

        // Independently move the recovery engine into safemode. Reconcile sees the registry
        // already disabled-by-safemode, so the next TrialEnable reaches the registry budget gate.
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        assert_eq!(
            state.recovery.lock().plugin_state("com.a"),
            Some(RecoveryPluginState::DisabledBySafemode)
        );

        let err = cmd_recover_trial_enable(&state, "com.a").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_STATE_INVALID_TRANSITION);
        assert_eq!(
            state.recovery.lock().plugin_state("com.a"),
            Some(RecoveryPluginState::DisabledBySafemode),
            "registry rejection must roll the staged engine TrialEnable back"
        );
        assert!(
            !state.recovery.lock().is_recovery_in_progress(),
            "rejected recovery action must not leave the executor stuck in-progress"
        );
    }

    #[test]
    fn cmd_recover_trial_enable_drives_registry_trial_state() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 两次失败 → 安全模式，对账把 com.a 标进注册表（DISABLED + 标志）。
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Disabled);
        assert!(s.disabled_by_safemode);

        // 试启：引擎改判 trial-enable + 注册表走 D28 TrialEnable（清标志、记预算）。
        let r = cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(r["engineAction"], "trialEnable:ENABLED");
        assert_eq!(r["phaseReconcile"]["scanned"], serde_json::json!(1));
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Enabled);
        assert!(!s.disabled_by_safemode, "试启必须清安全模式标志");
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("trial-enable")
        );

        // 试验失败 → 引擎回落，对账把注册表重新标回禁用（两侧闭环）。
        let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
        assert_eq!(r["engineAction"], "disabled-by-safemode");
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Disabled);
        assert!(s.disabled_by_safemode, "试验失败后注册表必须重新标记");
    }

    /// D28 反向同步边：试启期间插件**自报错误**（`ErrorRetryable`）→ 注册表按
    /// D28 回落并计一次试验失败，引擎必须同步得知。若不同步，引擎仍记它
    /// `TrialEnable`，同一调用尾部的对账会把注册表刚打上的安全模式标志当成
    /// 不一致再补发 `SafemodeExit` 清掉——「插件报错 → 标志被清 → 插件重入」
    /// 往返抖动。
    #[test]
    fn lifecycle_trial_error_syncs_engine_trial_failure() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        cmd_recover_trial_enable(&state, "com.a").unwrap();
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("trial-enable")
        );

        // 插件 webview 自报试启期间的可重试错误。
        let out = cmd_lifecycle_report(
            &state,
            "plugin-com.a",
            Some("com.a"),
            tauron_host::lifecycle::Event::ErrorRetryable,
        )
        .unwrap();
        assert!(!out.illegal, "试启期间报错必须走 D28 回落迁移");

        // 引擎已同步为回落，注册表重新标记禁用——对账静默（不再补发清标志事件）。
        assert_eq!(
            state.recovery.lock().plugin_state("com.a").map(|s| s.as_str()),
            Some("disabled-by-safemode")
        );
        let s = find_summary(&state, "com.a").unwrap();
        assert_eq!(s.state, tauron_host::lifecycle::State::Disabled);
        assert!(s.disabled_by_safemode, "标志必须保留：引擎已知失败，对账不得把它清掉");
        // 再对账一次，证明是稳态而非一次侥幸。
        reconcile_recovery_phase(&state);
        assert!(find_summary(&state, "com.a").unwrap().disabled_by_safemode);
    }

    #[test]
    fn recover_boot_and_report_are_lock_order_safe() {
        // 回归测试：恢复链上有两把锁（recovery / recovery_store），任何一处
        // 「持锁 A 再取锁 B」与「持锁 B 再取锁 A」并存就是 ABBA 死锁；而
        // 「持 recovery_store 锁调用 recovery_boot_payload」是自死锁
        // （`parking_lot` 不重入）。两者都表现为**空转而非报错**，所以必须有
        // 并发用例盯着——单线程用例抓不到。
        //
        // 语义断言放在单线程段：两条线程共享同一个引擎，失败计数会互相干扰
        // （可能已被推到修复模式），断言 trial 结果会变成对竞态的断言。
        {
            let s = CommandState::new();
            assert!(cmd_recover_report(&s, "failure", None).is_ok());
            assert!(cmd_recover_report(&s, "failure", None).is_ok());
            assert!(cmd_recover_trial_enable(&s, "com.t0").is_ok());
            assert!(cmd_recover_report(&s, "failure", Some("com.t0")).is_ok());
            assert!(cmd_recover_report(&s, "success", None).is_ok());
            assert!(cmd_recover_boot(&s).is_ok());
        }

        use std::sync::{Arc, Barrier};
        let state = Arc::new(CommandState::new());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let state = state.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0..200usize {
                    let pid = format!("com.t{i}");
                    // 只关心「都能返回」——死锁会表现为永不返回，而不是 Err。
                    let _ = cmd_recover_boot(&state);
                    let _ = cmd_recover_report(&state, "failure", None);
                    let _ = cmd_recover_report(&state, "failure", Some(&pid));
                    let _ = cmd_recover_trial_enable(&state, &pid);
                    let _ = cmd_recover_report(&state, "success", None);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    // ────────────────────────────────────────────────────────────
    // R7-1：崩溃 → 重启 → boot 回传 lastContext
    // ────────────────────────────────────────────────────────────

    /// 带恢复持久化目录的装配配置（其余取缺省）。
    fn recovery_cfg(dir: &std::path::Path) -> AdapterConfig {
        AdapterConfig { recovery_data_dir: Some(dir.to_path_buf()), ..AdapterConfig::default() }
    }

    #[test]
    fn recover_boot_returns_last_context_after_crash_and_restart() {
        let t = tempfile::tempdir().unwrap();

        // ── 第 1 轮：上报一次带插件归因的失败（干净退出，in_flight 被清零）。
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
            assert_eq!(r["outcome"], "failure");
            let ctx = r["lastContext"].as_array().unwrap();
            assert_eq!(ctx.len(), 1, "本轮失败必须立刻进上下文");
            assert_eq!(ctx[0]["pluginId"], "com.a");
            assert_eq!(ctx[0]["failureKind"], "boot-failure");
        }

        // ── 第 2 轮：重启（干净退出后重开）→ 上下文必须从磁盘回来。
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            let boot = cmd_recover_boot(&state).unwrap();
            assert_eq!(boot["loadSource"], "restored");
            // `load` 一律把本轮标成「进行中」。
            assert_eq!(boot["persistence"]["bootInFlight"], true);
            // 上一轮上报过 → 本次不该再算一次崩溃（计数仍是 1，不是 2）。
            assert_eq!(boot["counter"]["consecutiveFailures"], 1);
            let ctx = boot["lastContext"].as_array().unwrap();
            assert_eq!(ctx.len(), 1, "上一轮的上下文必须跨进程回传");
            assert_eq!(ctx[0]["pluginId"], "com.a");
            assert_eq!(ctx[0]["failureKind"], "boot-failure");
            assert!(ctx[0]["ts"].as_u64().unwrap() > 0, "时间戳不能是 0");
            // 本轮**没有**上报就退出 = 崩溃。第 3 轮应能识别。
        }

        // ── 第 3 轮：识别出上一次崩溃，并把崩溃本身追加成一条摘要。
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["persistence"]["bootInFlight"], true, "本轮尚未上报");
        assert_eq!(boot["counter"]["consecutiveFailures"], 2, "崩溃由 load 自己计数");
        let ctx = boot["lastContext"].as_array().unwrap();
        assert_eq!(ctx.len(), 2, "历史 + 本次崩溃各一条");
        assert_eq!(ctx[0]["pluginId"], "com.a", "最旧的是上一轮那条");
        assert_eq!(ctx[1]["pluginId"], serde_json::Value::Null, "崩溃无法归因到插件");
        assert_eq!(ctx[1]["failureKind"], "boot-failure");
        assert_eq!(ctx[1]["trial"], false);
    }

    #[test]
    fn recover_boot_returns_context_after_a_safemode_restart() {
        // R7 验证项原文：「safemode 重启后 cmd_recover_boot 回传 context」。
        let t = tempfile::tempdir().unwrap();
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            cmd_recover_report(&state, "failure", None).unwrap();
            cmd_recover_report(&state, "failure", None).unwrap();
        }

        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["phase"], "safemode");
        assert_eq!(boot["phaseName"], "安全模式");
        let ctx = boot["lastContext"].as_array().unwrap();
        assert_eq!(ctx.len(), 2, "两次失败都必须回传");
        assert_eq!(ctx[0]["consecutiveFailures"], 1);
        assert_eq!(ctx[1]["consecutiveFailures"], 2);
        assert_eq!(ctx[1]["phase"], "normal", "两次都发生在正常模式阶段");
        assert_eq!(ctx[1]["failureKind"], "boot-failure");
    }

    #[test]
    fn recover_boot_context_keeps_the_attributed_plugin_without_spending_its_trial_budget() {
        // 归因进上下文，但**不得**入账试验预算：安全模式里某插件崩过一次
        // 就永久失去试启用机会，是越权式的连带惩罚。
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        cmd_recover_report(&state, "failure", None).unwrap();
        cmd_recover_report(&state, "failure", Some("com.suspect")).unwrap();

        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["phase"], "safemode");
        let ctx = boot["lastContext"].as_array().unwrap();
        assert_eq!(ctx[1]["pluginId"], "com.suspect", "疑似故障插件要留在上下文里");
        // 该插件仍可试验性启用（没有被记进 trial_failures）。
        let enabled = cmd_recover_trial_enable(&state, "com.suspect");
        assert!(enabled.is_ok(), "疑似归因不该消耗试验预算：{enabled:?}");
    }

    #[test]
    fn recover_boot_keeps_every_pre_existing_field() {
        // 线形回归：既有字段不得改名/删除（前端已消费），R7 只做**增加**。
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        for field in [
            "phase",
            "phaseName",
            "counter",
            "disabledPlugins",
            "requiredPlugins",
            "loadSource",
            "persistence",
        ] {
            assert!(boot.get(field).is_some(), "R7 不得删掉既有字段 {field}");
        }
        for field in ["consecutiveFailures", "safemodeFailures"] {
            assert!(boot["counter"].get(field).is_some(), "counter 缺 {field}");
        }
        for field in ["enabled", "dir", "bootInFlight", "lastError"] {
            assert!(boot["persistence"].get(field).is_some(), "persistence 缺 {field}");
        }
        assert!(boot["lastContext"].is_array(), "R7 新增 lastContext");
    }

    #[test]
    fn recover_boot_context_is_empty_on_a_fresh_boot_not_fabricated() {
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["loadSource"], "fresh");
        assert!(boot["lastContext"].as_array().unwrap().is_empty(), "首次启动没有上下文，不许伪造");
    }

    #[test]
    fn recovery_marker_write_failure_does_not_break_boot_or_report() {
        // 落盘 best-effort：把目标路径用目录占住，让 rename 失败。
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("recovery-state.json")).unwrap();

        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        // 启动查询照常成功，失败原因如实暴露在 persistence.lastError。
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["persistence"]["enabled"], true);
        assert!(
            boot["persistence"]["lastError"].as_str().unwrap().contains("落盘失败"),
            "落盘失败必须可见：{}",
            boot["persistence"]["lastError"]
        );
        // 上报也照常成功——落盘失败绝不能阻断启动流程。
        let r = cmd_recover_report(&state, "failure", Some("com.a")).unwrap();
        assert_eq!(r["outcome"], "failure");
        let ctx = r["lastContext"].as_array().unwrap();
        assert_eq!(ctx.len(), 1);
        assert_eq!(ctx[0]["pluginId"], "com.a", "写失败不影响内存里的上下文");
    }

    // ══════════════════════════════════════════════════════════════════════
    // R7 收口：命令层身份判定（主体解析 / 特权仅主窗 / 设置族键空间）
    //
    // 这些用例都走 `*_as` 系列入口——**就是 wire 层包装器转调的那两个函数**
    // （`window.label()` 那一跳留在 `tauri.rs`，见那里的编译期签名证据）。
    // 每条「插件被拒」的用例都配一条「主窗通过」的对照，证明拒绝来自身份，
    // 而不是命令被整体关死。
    // ══════════════════════════════════════════════════════════════════════

    fn plugin_caller(id: &str) -> Caller {
        Caller::Plugin(id.to_string())
    }

    /// 畸形 label **绝不**降级成主窗（提权），且必须报既有的身份码。
    #[test]
    fn malformed_label_is_rejected_and_never_downgrades_to_main_window() {
        // 都是 `plugin-` 前缀但 id 非法：空 / 单段 / 首段大写 / 结尾点。
        for bad in ["plugin-", "plugin-p", "plugin-P.a", "plugin-p.a."] {
            let err = Caller::from_label(bad)
                .expect_err("畸形 label 必须被拒——降级成主窗即等于伪造 label 提权");
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{bad}");
            assert!(err.message.contains(bad), "错误消息要带原始 label：{}", err.message);
            assert!(err.message.contains("绝不降级"), "消息必须点明不降级：{}", err.message);
        }
        // 正常解析（既有模型）：无前缀 = 主窗；`plugin-<合法 id>` = 插件。
        assert_eq!(Caller::from_label("main").unwrap(), Caller::MainWindow);
        assert_eq!(
            Caller::from_label("plugin-com.example.a").unwrap(),
            Caller::Plugin("com.example.a".to_string())
        );
        assert!(!Caller::from_label("plugin-com.example.a").unwrap().is_main_window());
        assert_eq!(
            Caller::from_label("plugin-com.example.a").unwrap().plugin_id(),
            Some("com.example.a")
        );
    }

    /// 档位表与代码判定**同源**：表里每一条 `privileged` 命令都必须被
    /// `require_main_window` 拒绝插件主体（表增了特权命令却写下别的档位，这里就红）。
    #[test]
    fn every_privileged_command_in_the_authz_table_denies_plugins() {
        let mut checked = 0;
        for c in tauron_host::authz::ADMIN_COMMANDS {
            assert_eq!(
                c.tier,
                tauron_host::authz::AuthTier::Privileged,
                "{} 在 ADMIN_COMMANDS 里必须是 privileged",
                c.command
            );
            let err = require_main_window(&plugin_caller("com.x"), c.command).unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{}", c.command);
            assert!(
                require_main_window(&Caller::MainWindow, c.command).is_ok(),
                "主窗必须是特权命令的合法主体"
            );
            checked += 1;
        }
        assert!(checked >= 3, "档位表里的特权命令不该少于 3 条（校验没跑空）");
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn optional_install_commands_are_main_window_only_and_registered_as_privileged() {
        assert_eq!(PLUGIN_INSTALL_AUTH.len(), PLUGIN_INSTALL_COMMANDS.len());
        for (auth, command) in PLUGIN_INSTALL_AUTH.iter().zip(PLUGIN_INSTALL_COMMANDS) {
            assert_eq!(auth.command, *command);
            assert_eq!(auth.tier, tauron_host::authz::AuthTier::Privileged);
            assert_eq!(
                require_main_window(&plugin_caller("com.x"), command).unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
            assert!(require_main_window(&Caller::MainWindow, command).is_ok());
        }
    }

    /// `host_registry_admin`：插件主体被拒且**零副作用**；主窗能禁用。
    #[test]
    fn plugin_caller_cannot_use_registry_admin_but_main_window_can() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let id = PluginId::new("com.a").unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed
        );

        let err = cmd_registry_admin_as(
            &plugin_caller("com.a"),
            &state,
            "com.a",
            RegistryAdminOp::Disable,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed,
            "拒绝路径不得改插件状态（判定必须在副作用之前）"
        );

        // 对照：主窗能禁用（证明拒绝来自身份而不是命令被整体关死）。
        let out =
            cmd_registry_admin_as(&Caller::MainWindow, &state, "com.a", RegistryAdminOp::Disable)
                .unwrap();
        assert_eq!(out.to, tauron_host::lifecycle::State::Disabled);
    }

    // ══════════════════════════════════════════════════════════════════════
    // 轮 43 / A83：破坏性管理操作的审批令牌（预览 → 确认 → 提交）
    //
    // 与安装域 `InstallReviewToken` 同构的第二个消费者：一次性（nonce）、
    // 有界（TTL + 容量）、绑定注册表事实（id + 操作 + 版本）。预览零副作用；
    // 提交前重核，任一漂移即拒绝且令牌已作废（必须重新预览）。
    // 「更新」没有独立执行路径 = uninstall + install 两条组合腿，两端都走令牌。
    // ══════════════════════════════════════════════════════════════════════

    fn preview_admin_review(
        state: &PluginRuntimeState,
        plugin_id: &str,
        op: RegistryAdminOp,
    ) -> RegistryAdminReview {
        match cmd_registry_admin_reviewed_as(&Caller::MainWindow, state, plugin_id, op, true, None)
            .expect("preview 必须成功")
        {
            RegistryAdminResponse::Review(review) => review,
            RegistryAdminResponse::Executed(_) => panic!("preview 不得执行操作"),
        }
    }

    /// 预览零副作用且绑定事实；提交一次性消费，重放被拒。
    #[test]
    fn admin_review_preview_binds_facts_and_commits_once() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let id = PluginId::new("com.a").unwrap();

        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);
        assert_eq!(review.plugin_id, "com.a");
        assert_eq!(review.op, RegistryAdminOp::Uninstall);
        assert_eq!(review.version, "1.0.0");
        assert_eq!(review.state, tauron_host::lifecycle::State::Installed);
        assert_eq!(
            review.review_token.expires_at - review.review_token.issued_at,
            ADMIN_REVIEW_TTL_SECS,
        );
        // 预览零副作用：条目仍在、状态未变。
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed,
        );

        let executed = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap();
        let RegistryAdminResponse::Executed(outcome) = executed else {
            panic!("提交必须执行迁移");
        };
        assert!(!outcome.illegal);
        assert!(state.registry.find(&id).is_none(), "卸载后条目必须回收");

        // 重放：一次性令牌已消费。
        let replay = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(replay.code, ErrorCode::E_AUTH_DENIED);
        assert!(replay.message.contains("already consumed"), "{}", replay.message);
    }

    /// 令牌绑定「被审阅的操作」：换操作提交拒绝；消费顺序先摘除再比对 → 必须重新预览。
    #[test]
    fn admin_review_rejects_op_mismatch_and_stays_consumed() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);

        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Purge,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("不匹配"), "{}", err.message);
        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some(), "拒绝路径不得改注册表");

        // 换回被审阅的操作也不行：失败提交已把 nonce 摘除（失败关闭）。
        let reused = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert!(reused.message.contains("already consumed"), "{}", reused.message);
    }

    /// 令牌绑定「预览时刻的版本」：预览后换版本 → 拒绝，必须重新预览。
    #[test]
    fn admin_review_version_drift_forces_a_fresh_preview() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);

        // 漂移：卸载旧版后装上新版（同一 id、版本不同）。
        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();
        let mut upgraded = test_manifest("com.a");
        upgraded.version = semver::Version::parse("2.0.0").unwrap();
        state.registry.install(&empty_index(), upgraded).unwrap();

        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("预览后已变更"), "{}", err.message);
        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some(), "拒绝路径不得改注册表");
    }

    /// 协议面收窄：预览只服务 uninstall/purge；令牌只被破坏性路径消费；
    /// 预览与令牌不能同调混用。
    #[test]
    fn admin_review_protocol_rejects_non_destructive_mixing() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // disable/enable 可逆、无破坏面：不提供预览。
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Disable,
            true,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        // 令牌也不能被非破坏性操作借道消费。
        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Purge);
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Enable,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        // preview 只铸发、commit 只消费：同调混用拒绝。
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            true,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
    }

    /// 可推进的测试时间源：`now = 墙钟 + 偏移`。`advance` 模拟「可信时间前进
    /// 而墙钟照常」，用来证明 TTL 判定读的是**时间源**而不是墙钟；`retreat` 让
    /// 时间源**落后于墙钟**——不后退的话两钟同源，「铸发改读墙钟」的变异在这组
    /// 测试里无感（轮 43 变异证明揪出过这一点）。
    #[cfg(feature = "plugin-install")]
    #[derive(Debug, Default)]
    struct OffsetTimeProvider {
        offset_secs: std::sync::atomic::AtomicI64,
    }

    #[cfg(feature = "plugin-install")]
    impl OffsetTimeProvider {
        fn advance(&self, delta: std::time::Duration) {
            self.offset_secs.fetch_add(delta.as_secs() as i64, std::sync::atomic::Ordering::SeqCst);
        }

        fn retreat(&self, delta: std::time::Duration) {
            self.offset_secs.fetch_sub(delta.as_secs() as i64, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[cfg(feature = "plugin-install")]
    impl tauron_host::TrustedTimeProvider for OffsetTimeProvider {
        fn trusted_time(&self) -> tauron_host::TrustedTime {
            let offset = self.offset_secs.load(std::sync::atomic::Ordering::SeqCst);
            let now = std::time::SystemTime::now();
            let now = if offset >= 0 {
                now + std::time::Duration::from_secs(offset as u64)
            } else {
                now - std::time::Duration::from_secs(offset.unsigned_abs())
            };
            tauron_host::TrustedTime { now, state: tauron_host::TimeTrustState::Trusted }
        }
    }

    /// 启动时 Trusted、运行期可降级为 Suspicious 的时间源——证明审批**时刻**的
    /// 失败关闭独立于启动门（真实时间源在运行期丢失可信状态是可能的）。
    #[cfg(feature = "plugin-install")]
    #[derive(Debug)]
    struct DemotableTimeProvider {
        trustworthy: std::sync::atomic::AtomicBool,
    }

    #[cfg(feature = "plugin-install")]
    impl Default for DemotableTimeProvider {
        fn default() -> Self {
            Self { trustworthy: std::sync::atomic::AtomicBool::new(true) }
        }
    }

    #[cfg(feature = "plugin-install")]
    impl DemotableTimeProvider {
        fn demote(&self) {
            self.trustworthy.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[cfg(feature = "plugin-install")]
    impl tauron_host::TrustedTimeProvider for DemotableTimeProvider {
        fn trusted_time(&self) -> tauron_host::TrustedTime {
            tauron_host::TrustedTime {
                now: std::time::SystemTime::now(),
                state: if self.trustworthy.load(std::sync::atomic::Ordering::SeqCst) {
                    tauron_host::TimeTrustState::Trusted
                } else {
                    tauron_host::TimeTrustState::Suspicious
                },
            }
        }
    }

    /// 生产档插件运行时装配（测试）：生产就绪门要求**附着**的运行时报告 hard
    /// 沙箱（`validate_process_runtime_for_start`），默认 `CommandSpawner` 不是，
    /// 因此注入标记为 hard 的 [`FakeSpawner`]——它只通过装配门，不做任何真实隔离。
    fn production_admin_state(cfg: AdapterConfig) -> PluginRuntimeState {
        let substrate = Arc::new(SubstrateState::with_adapter_config(&cfg));
        let spawner = Arc::new(FakeSpawner::default());
        spawner.mark_hard_sandbox();
        PluginRuntimeState::with_substrate_and_spawner(substrate, cfg, spawner)
            .expect("生产档插件运行时装配失败（已附着 hard 描述 fake）")
    }

    /// 生产档（装插件特性开启）：破坏性操作的令牌强制——旧入口与无令牌的
    /// reviewed 入口都拒绝；走完两步即可执行；disable/enable 不受影响。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn production_destructive_admin_op_requires_the_review_path() {
        let temp = tempfile::tempdir().unwrap();
        let state = production_admin_state(production_config_with_audit(&temp));
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 旧入口（无令牌直通）在生产档被拒——不能绕过令牌强制。
        let err =
            cmd_registry_admin_as(&Caller::MainWindow, &state, "com.a", RegistryAdminOp::Uninstall)
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("AdminReviewToken"), "{}", err.message);
        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some(), "拒绝路径不得改注册表");

        // reviewed 入口不带令牌同样被拒。
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("preview"), "{}", err.message);

        // 两步链（preview → commit）在生产档可执行。
        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);
        let executed = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap();
        assert!(matches!(executed, RegistryAdminResponse::Executed(_)));
        assert!(state.registry.find(&id).is_none());

        // 可逆操作（disable）不受令牌强制影响。
        state.registry.install(&empty_index(), test_manifest("com.b")).unwrap();
        cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.b",
            RegistryAdminOp::Disable,
            false,
            None,
        )
        .unwrap();
    }

    /// 生产档缺可信时间源：**启动门**先拒（生产就绪校验的 TRUSTED_TIME_REQUIRED）——
    /// 第一道失败关闭，根本到不了审批路径。
    ///
    /// 审批时刻自身的 None 分支是第二道防线（防运行期配置漂移把判定架空），
    /// 在底座-only 构建下可达，由
    /// `production_preview_fails_closed_without_trusted_time_in_substrate_builds` 钉住。
    #[cfg(feature = "plugin-install")]
    #[test]
    #[should_panic(expected = "TRUSTED_TIME_REQUIRED")]
    fn production_without_trusted_time_is_rejected_at_startup() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = production_config_with_audit(&temp);
        cfg.trusted_time_provider = None;
        let _ = SubstrateState::with_adapter_config(&cfg);
    }

    /// 时钟状态为 Suspicious 的时间源不满足生产就绪（"currently trusted"），
    /// 启动同样被拒——Suspicious 不是在档位表里打个折，而是直接不合格。
    #[cfg(feature = "plugin-install")]
    #[test]
    #[should_panic(expected = "TRUSTED_TIME_REQUIRED")]
    fn production_with_suspicious_clock_is_rejected_at_startup() {
        let temp = tempfile::tempdir().unwrap();
        let mut cfg = production_config_with_audit(&temp);
        cfg.trusted_time_provider = Some(Arc::new(tauron_host::SystemTimeProvider::new(
            tauron_host::TimeTrustState::Suspicious,
        )));
        let _ = SubstrateState::with_adapter_config(&cfg);
    }

    /// 运行期丢失可信状态（启动时 Trusted → 之后 Suspicious）：**审批时刻**
    /// 失败关闭——提交（TTL 走 `require_unexpired`）与重新预览（铸发）都被拒，
    /// 不静默回落墙钟。真实时间源在运行期降级是可能的，这与启动门是两道独立判定。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn demoted_clock_after_startup_fails_closed_for_admin_review() {
        let temp = tempfile::tempdir().unwrap();
        let provider = Arc::new(DemotableTimeProvider::default());
        let mut cfg = production_config_with_audit(&temp);
        cfg.trusted_time_provider = Some(provider.clone());
        let state = production_admin_state(cfg);
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 时钟仍可信时预览成功——证明失败来自降级，而不是链路本来就断。
        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);

        provider.demote();

        // ① 提交：TTL 校验走 require_unexpired，时钟不可信 → 拒。
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("已失效"), "{}", err.message);

        // ② 重新预览：铸发本身也拒绝（不把令牌建立在不可信时钟上）。
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            true,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("不可信"), "{}", err.message);

        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some(), "失败关闭路径不得改注册表");
    }

    /// 管理域 TTL 读可信时间源（A100），铸发与判定两侧都钉住：① 铸发——时间源
    /// 落后墙钟 6h，若铸发改读墙钟，`issued_at` 断言直接红；② 过期判定——时间
    /// 源前进到 TTL 之外 → 提交失败（墙钟只走了毫秒级，若判定读墙钟这里必绿）。
    ///
    /// 落后量取 6h 而非 24h：签名侧 `MAX_CLOCK_SKEW_SECS` 正是 24h，压边界会让
    /// 安装域夹具在钟偏判定上抖。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn admin_review_ttl_follows_the_trusted_provider() {
        let temp = tempfile::tempdir().unwrap();
        let provider = Arc::new(OffsetTimeProvider::default());
        provider.retreat(std::time::Duration::from_secs(6 * 3600));
        let mut cfg = production_config_with_audit(&temp);
        cfg.trusted_time_provider = Some(provider.clone());
        let state = production_admin_state(cfg);
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
        let expected_issued =
            system_time_to_unix_secs(std::time::SystemTime::now()).saturating_sub(6 * 3600);
        let review = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);
        assert!(
            review.review_token.issued_at.abs_diff(expected_issued) <= 2,
            "铸发时间必须取自可信时间源（落后墙钟 6h）：issued_at={}，期望≈{}",
            review.review_token.issued_at,
            expected_issued
        );

        provider.advance(std::time::Duration::from_secs(ADMIN_REVIEW_TTL_SECS + 1));
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&review.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("已失效"), "{}", err.message);
        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some(), "过期拒绝不得改注册表");
    }

    /// 安装域同径（轮 43 retrofit）：install 预览令牌的 TTL 也走可信时间源——
    /// 铸发时间来源与过期判定两侧都钉住（时间源落后墙钟 6h，与签名侧
    /// `MAX_CLOCK_SKEW_SECS` 的 24h 裕量保持距离）。
    #[cfg(feature = "plugin-install")]
    #[test]
    fn install_review_ttl_follows_the_trusted_provider() {
        let dir = tempfile::tempdir().unwrap();
        let (package, verifying_key) = signed_install_fixture(dir.path(), "com.install.ttl");
        let mut signing_keys = std::collections::BTreeMap::new();
        signing_keys.insert("fixture-key".to_string(), verifying_key.to_bytes().to_vec());
        let provider = Arc::new(OffsetTimeProvider::default());
        provider.retreat(std::time::Duration::from_secs(6 * 3600));
        let state = PluginRuntimeState::with_adapter_config(AdapterConfig {
            plugin_install_dir: Some(dir.path().join("plugins")),
            plugin_signing_keys: signing_keys,
            acl_signing_key: Some(vec![0x5a; 32]),
            trusted_time_provider: Some(provider.clone()),
            ..AdapterConfig::default()
        });

        let expected_issued =
            system_time_to_unix_secs(std::time::SystemTime::now()).saturating_sub(6 * 3600);
        let preview =
            cmd_registry_install_preview_as(&Caller::MainWindow, &state, package.to_str().unwrap())
                .unwrap();
        assert!(
            preview.review_token.issued_at.abs_diff(expected_issued) <= 2,
            "install 铸发时间必须取自可信时间源（落后墙钟 6h）：issued_at={}，期望≈{}",
            preview.review_token.issued_at,
            expected_issued
        );
        provider.advance(std::time::Duration::from_secs(INSTALL_REVIEW_TTL_SECS + 1));
        let err = cmd_registry_install_reviewed_as(
            &Caller::MainWindow,
            &state,
            package.to_str().unwrap(),
            &["store:allow-get".into()],
            &preview.review_token,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("已失效"), "{}", err.message);
        assert!(
            !dir.path().join("plugins").join("com.install.ttl").exists(),
            "过期拒绝不得落盘安装"
        );
    }

    /// 底座-only（未开安装特性）的生产档没有 `TRUSTED_TIME_REQUIRED` 启动门
    /// （安装域不存在，时间源事实缺省视为满足）——但 **preview 铸发**一旦被调用
    /// 仍需可信时间，缺源即失败关闭。
    ///
    /// 这也把 `review_now` 的 None+Production 分支钉在**默认构建的可达路径**上：
    /// 不是防御性死代码。
    #[cfg(not(feature = "plugin-install"))]
    #[test]
    fn production_preview_fails_closed_without_trusted_time_in_substrate_builds() {
        let temp = tempfile::tempdir().unwrap();
        let state = production_admin_state(production_config_with_audit(&temp));
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            true,
            None,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(err.message.contains("可信时间源"), "{}", err.message);
        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some(), "预览失败不得改注册表");
    }

    /// `host_registry_list_all`：插件主体被拒（全量列表本身就是 scoped-read 要遮的信息）。
    #[test]
    fn plugin_caller_cannot_list_all_plugins_but_main_window_can() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let err = cmd_registry_list_all_as(&plugin_caller("com.a"), &state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        let all = cmd_registry_list_all_as(&Caller::MainWindow, &state).unwrap();
        assert_eq!(all.len(), 1, "主窗必须拿得到全量");
    }

    /// `host_runtime_spawn`：插件主体不得起**别人的** sidecar（越权执行原语）。
    ///
    /// 判别力设计：攻击者目标选一个**尚未启动**的插件 B——若判定缺失，这次调用会
    /// 真的走到启动面（`call_count` 变 2）。用同一个插件会因幂等返回旧租约而
    /// 「看起来也对」，失去判别力。
    #[test]
    fn plugin_caller_cannot_spawn_another_plugins_sidecar() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        for (id, sidecar) in [("com.proc.a", "a.exe"), ("com.proc.b", "b.exe")] {
            state.registry.install(&empty_index(), process_manifest(id, Some(sidecar))).unwrap();
            enabled_process_plugin(&state, id);
        }

        // 对照：主窗起 A → 真的经过启动面。
        cmd_runtime_spawn_as(&Caller::MainWindow, &state, "com.proc.a", &valid_profile()).unwrap();
        assert_eq!(fake.call_count(), 1);

        // 插件 A 的主体去起 B：拒绝，且启动面**一次都不许新增调用**。
        let err = cmd_runtime_spawn_as(
            &plugin_caller("com.proc.a"),
            &state,
            "com.proc.b",
            &valid_profile(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            fake.call_count(),
            1,
            "拒绝路径一次都不许新增启动调用——否则等于插件越权起了 B 的 sidecar"
        );
        assert_eq!(state.registry.runtime_len(), 1, "拒绝路径不得给 B 留下租约");
    }

    /// `host_runtime_health`：插件主体不得查进程健康（带 pid、且会驱动崩溃检测）。
    #[test]
    fn plugin_caller_cannot_query_process_health_but_main_window_can() {
        let (state, _fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle =
            cmd_runtime_spawn_as(&Caller::MainWindow, &state, "com.proc", &valid_profile())
                .unwrap();

        let err =
            cmd_runtime_health_as(&plugin_caller("com.proc"), &state, &handle.lease).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        // 对照：主窗用**同一个租约**查得到（证明拒绝来自身份，不是租约无效）。
        let health = cmd_runtime_health_as(&Caller::MainWindow, &state, &handle.lease).unwrap();
        assert_eq!(health.pid, handle.pid);
        assert!(health.alive);
    }

    /// 设置族键空间：插件只能碰自己的命名空间；主窗任意键（现状不变）。
    #[test]
    fn plugin_caller_can_only_touch_its_own_settings_namespace() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");

        // 自己的键：读写都通过（含命名空间根与子键）。
        cmd_settings_set_as(&a, &state, "plugin:p.a.theme", serde_json::json!("dark")).unwrap();
        cmd_settings_set_as(&a, &state, "plugin:p.a", serde_json::json!("root")).unwrap();
        assert_eq!(
            cmd_settings_get_as(&a, &state, "plugin:p.a.theme").unwrap(),
            serde_json::json!("dark")
        );

        // 别人的键：读、写都拒；写被拒后 Store 里**没有**任何痕迹。
        for key in ["plugin:p.b.theme", "theme", "host.settings"] {
            let err =
                cmd_settings_set_as(&a, &state, key, serde_json::json!("hijack")).unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "写 {key} 必须被拒");
            assert_eq!(
                cmd_settings_get_as(&Caller::MainWindow, &state, key).unwrap(),
                serde_json::Value::Null,
                "越界写入不得落进 Store（{key}）"
            );
        }
        let err = cmd_settings_get_as(&a, &state, "plugin:p.b.theme").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "读别人的键也必须拒");

        // **前缀包含误伤**：`plugin:p.a` 的插件不得借 `starts_with` 写穿 `plugin:p.ab.x`。
        let err =
            cmd_settings_set_as(&a, &state, "plugin:p.ab.x", serde_json::json!(1)).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::E_AUTH_DENIED,
            "`plugin:p.a` 不是 `plugin:p.ab.x` 的命名空间（前缀包含误伤）"
        );
        // 反向同理：`plugin:p.ab` 碰不到 `plugin:p.a.x`，但自己的键没问题。
        let ab = plugin_caller("p.ab");
        assert!(cmd_settings_set_as(&ab, &state, "plugin:p.a.x", serde_json::json!(1)).is_err());
        cmd_settings_set_as(&ab, &state, "plugin:p.ab.x", serde_json::json!(1)).unwrap();

        // 对照：主窗任意键（含宿主级键）读写照旧。
        cmd_settings_set_as(&Caller::MainWindow, &state, "theme", serde_json::json!("light"))
            .unwrap();
        assert_eq!(
            cmd_settings_get_as(&Caller::MainWindow, &state, "theme").unwrap(),
            serde_json::json!("light")
        );
    }

    /// 迁移入口：**仅主窗**（整份文档级操作无法用「只准动自己的键」表达）。
    #[test]
    fn settings_migration_entries_are_main_window_only() {
        let state = CommandState::new();
        let plugin = plugin_caller("p.a");

        let err =
            cmd_settings_adopt_legacy_as(&plugin, &state, serde_json::json!({"theme": "dark"}))
                .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(host_settings_data_version(&state), None, "拒绝路径不得标注数据版本");
        assert_eq!(
            cmd_settings_get_as(&Caller::MainWindow, &state, "theme").unwrap(),
            serde_json::Value::Null,
            "拒绝路径不得写入任何设置"
        );

        let err = cmd_settings_migrate_as(&plugin, &state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(host_settings_data_version(&state), None, "拒绝路径不得把空数据标成最新版");

        // 对照：主窗走得通（接手 → 迁一步 → 再迁 0 步）。
        cmd_settings_adopt_legacy_as(
            &Caller::MainWindow,
            &state,
            serde_json::json!({"plugin:p.a.theme": "dark"}),
        )
        .unwrap();
        assert_eq!(host_settings_data_version(&state).as_deref(), Some(HOST_SETTINGS_SCHEMA_V1));
        assert_eq!(cmd_settings_migrate_as(&Caller::MainWindow, &state).unwrap(), 1);
        assert_eq!(cmd_settings_migrate_as(&Caller::MainWindow, &state).unwrap(), 0);
    }

    // ──────────────────────────────────────────────────────────────────────
    // 轮 11 第二批：身份绑定参数 / 主窗专属（跨插件副作用与冒名原语）
    //
    // 每条"拒绝"都配了两类对照，否则测试没有判别力：
    // ① 主窗（或插件对自己）**能**通过——证明拒绝来自身份而不是命令被整体关死；
    // ② 拒绝路径**零副作用**——证明判定落在副作用之前（而不是"先做了再报错"）。
    // ──────────────────────────────────────────────────────────────────────

    /// `host_notify`：署名必须是自己。插件以别人/宿主名义发通知一律拒绝且零副作用。
    #[test]
    fn plugin_caller_can_only_notify_under_its_own_name() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");

        // 自己名义 → 入缓冲（也进兼容日志）。
        cmd_notify_as(&a, &state, "p.a", "T", "B").unwrap();
        assert_eq!(state.notify_store.lock().len(), 1);
        assert_eq!(state.notifications.lock().len(), 1);

        // 冒名（别的插件 / 宿主）→ 拒绝，且**一条都不入**：环形缓冲、兼容日志、
        // 分发日志（未注入 sink 时不写日志，这里以两个真实存储为准）都不动。
        for claimed in ["p.b", "host", "tauron"] {
            let err = cmd_notify_as(&a, &state, claimed, "T", "B").unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "以 `{claimed}` 名义必须被拒");
            assert_eq!(
                state.notify_store.lock().len(),
                1,
                "拒绝路径不得入环形缓冲（署名 {claimed}）"
            );
            assert_eq!(
                state.notifications.lock().len(),
                1,
                "拒绝路径不得写兼容日志（署名 {claimed}）"
            );
        }
        assert_eq!(cmd_notifications_list(&state, None).unwrap()["total"], 1);

        // 对照：主窗可发任意署名（宿主自己就是这些通知的来源）。
        cmd_notify_as(&Caller::MainWindow, &state, "p.b", "T", "B").unwrap();
        cmd_notify_as(&Caller::MainWindow, &state, "host", "T", "B").unwrap();
        assert_eq!(state.notify_store.lock().len(), 3);

        // **危害实证**（不是推测）：判定缺席时同一个存储会照收别人的署名——
        // 直接调不过身份的核心 `cmd_notify` 即可复现。
        cmd_notify(&state, "p.b", "T", "B").unwrap();
        assert_eq!(
            state.notify_store.lock().len(),
            4,
            "冒名通知在不过身份的核心上会被照收——这正是 gate 要挡的东西"
        );
    }

    /// `host_i18n_cleanup_plugin`：插件只能清自己的文案（清别人 = 跨插件销毁）。
    #[test]
    fn plugin_caller_can_only_clean_its_own_i18n_bundle() {
        let state = CommandState::new();
        // 两个插件各装一份文案（主窗代装：这是正当路径）。
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "A 的标题")]), Some("p.a")).unwrap();
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "B 的标题")]), Some("p.b")).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        let b_key = I18nEngine::plugin_key("p.b", "title");
        assert_eq!(cmd_i18n_t(&state, &b_key).unwrap(), "B 的标题");

        // 清别人 → 拒绝，且 B 的 bundle **仍在**（逐 key 可读）。
        let err = cmd_i18n_cleanup_plugin_as(&plugin_caller("p.a"), &state, "p.b").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            cmd_i18n_t(&state, &b_key).unwrap(),
            "B 的标题",
            "拒绝路径不得销毁别人的文案（否则 B 的界面会变成一屏原始 key）"
        );

        // 清自己 → 生效（证明"拒绝"不是命令整体不可用）。
        let r = cmd_i18n_cleanup_plugin_as(&plugin_caller("p.a"), &state, "p.a").unwrap();
        assert_eq!(r["removed"], 1);
        let a_key = I18nEngine::plugin_key("p.a", "title");
        assert_eq!(
            cmd_i18n_t(&state, &a_key).unwrap(),
            a_key,
            "清掉之后该 key 落空（按设计返回 key 本身）"
        );

        // 对照：主窗清任意插件（卸载/禁用的回收路径）。
        let r = cmd_i18n_cleanup_plugin_as(&Caller::MainWindow, &state, "p.b").unwrap();
        assert_eq!(r["removed"], 1);
    }

    /// `host_i18n_load`：命名空间归属必须是自己；`None`（宿主文案）插件不得主张。
    #[test]
    fn plugin_caller_can_only_load_into_its_own_i18n_namespace() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");

        // 自己命名空间 → 装载成功。
        cmd_i18n_load_as(&a, &state, "zh-CN", entries(&[("title", "我的")]), Some("p.a")).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        assert_eq!(cmd_i18n_t(&state, &I18nEngine::plugin_key("p.a", "title")).unwrap(), "我的");

        // 别人的命名空间 → 拒绝，且**引擎里没有** `plugin:p.b.oc.*`。
        let err = cmd_i18n_load_as(&a, &state, "zh-CN", entries(&[("title", "投毒")]), Some("p.b"))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(
            state
                .i18n
                .lock()
                .get_bundle("zh-CN")
                .and_then(|b| b.get(&I18nEngine::plugin_key("p.b", "title")))
                .is_none(),
            "命名空间投毒：拒绝路径不得往别人的命名空间写任何 key"
        );

        // 宿主级文案（`None`）→ 插件不得主张。
        let err = cmd_i18n_load_as(&a, &state, "zh-CN", entries(&[("oc.app", "伪造")]), None)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(
            state.i18n.lock().get_bundle("zh-CN").and_then(|b| b.get("oc.app")).is_none(),
            "宿主命名空间不得被插件写入"
        );

        // 对照：主窗带 `None` 装载宿主级文案照旧通过。
        cmd_i18n_load_as(
            &Caller::MainWindow,
            &state,
            "zh-CN",
            entries(&[("oc.app", "应用标题")]),
            None,
        )
        .unwrap();
        assert_eq!(cmd_i18n_t(&state, "oc.app").unwrap(), "应用标题");
        // 主窗也能代插件装载（卸载/重装等宿主流程）。
        cmd_i18n_load_as(
            &Caller::MainWindow,
            &state,
            "zh-CN",
            entries(&[("title", "代装")]),
            Some("p.b"),
        )
        .unwrap();
        assert_eq!(cmd_i18n_t(&state, &I18nEngine::plugin_key("p.b", "title")).unwrap(), "代装");
    }

    /// 把 state 推到「安全模式 + `p.b` 处于试验性启用」，供下面两条测试用。
    fn state_with_b_under_trial() -> CommandState {
        let state = CommandState::new();
        // 两次失败 → 安全模式（BootCounter::SAFEMODE_THRESHOLD）。
        for _ in 0..2 {
            cmd_recover_report(&state, "failure", None).unwrap();
        }
        cmd_recover_trial_enable(&state, "p.b").unwrap();
        assert_eq!(
            state.recovery.lock().plugin_state("p.b").map(|s| s.as_str()),
            Some("trial-enable"),
            "前置状态没搭起来，后面的断言就没有意义"
        );
        state
    }

    /// `host_recover_report`：插件不得替**别人**报 failure，也不得认领应用级 `None`。
    ///
    /// 靶子选「正处于 `TrialEnable` 的 `p.b`」：这是**唯一**会让 `pluginId` 真的
    /// 伤到别人的路径（`record_trial_failure_at` 烧掉它 1 次的试验预算 → 永久
    /// `disabled-by-safemode`）。选一个"没启动过的插件"当靶子会因为该路径不入账而
    /// 看不出差别。
    #[test]
    fn plugin_caller_cannot_report_failure_for_another_plugin() {
        let state = state_with_b_under_trial();
        let a = plugin_caller("p.a");

        // 前置：诊断上下文里此刻有两条（两次启动失败，`pluginId` 均为 `None`）。
        let ctx_before: Vec<Option<String>> =
            state.recovery.lock().context().iter().map(|e| e.plugin_id.clone()).collect();
        assert_eq!(ctx_before.len(), 2);
        assert!(ctx_before.iter().all(|p| p.is_none()), "前置上下文不该已经指向 B");

        // 替 B 报 failure → 拒绝。
        let err = cmd_recover_report_as(&a, &state, "failure", Some("p.b")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);

        // **零副作用**：B 仍在试启、试验预算为 0、阶段与全局计数一动不动、
        // 诊断上下文里没有多出"B 崩了"的纪录。
        assert_eq!(
            state.recovery.lock().plugin_state("p.b").map(|s| s.as_str()),
            Some("trial-enable"),
            "拒绝路径不得消耗别人的试验预算"
        );
        assert_eq!(
            state.recovery.lock().counter().trial_failures.get("p.b").copied(),
            None,
            "别人的试验失败计数必须还是空的"
        );
        assert_eq!(state.recovery.lock().counter().consecutive_failures, 2);
        assert_eq!(state.recovery.lock().counter().safemode_failures, 0);
        assert_eq!(state.recovery.lock().decide_boot_phase().as_str(), "safemode");
        let ctx_after: Vec<Option<String>> =
            state.recovery.lock().context().iter().map(|e| e.plugin_id.clone()).collect();
        assert_eq!(
            ctx_after, ctx_before,
            "拒绝路径不得把故障栽赃进诊断上下文（长度与归因都必须原样）"
        );
        assert!(
            !ctx_after.iter().any(|p| p.as_deref() == Some("p.b")),
            "上下文里不得出现被冒名的插件"
        );

        // 应用级结果（`None`）同样不归插件主张。
        let err = cmd_recover_report_as(&a, &state, "failure", None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(state.recovery.lock().counter().consecutive_failures, 2, "全局计数不得动");

        // 对照一：插件报**自己**照旧通过（self 档，不是"插件不能上报"）。
        let ok = cmd_recover_report_as(&a, &state, "failure", Some("p.a")).unwrap();
        assert_eq!(ok["outcome"], "failure");
        assert_eq!(ok["suspectedPlugin"], "p.a");

        // 对照二：主窗带任意 pluginId / `None` 照旧通过。
        let fresh = state_with_b_under_trial();
        let ok =
            cmd_recover_report_as(&Caller::MainWindow, &fresh, "failure", Some("p.b")).unwrap();
        assert_eq!(ok["suspectedPlugin"], "p.b");
        let ok = cmd_recover_report_as(&Caller::MainWindow, &fresh, "success", None).unwrap();
        assert_eq!(ok["engineAction"], "bootSuccess");
    }

    /// **危害实证**：判定缺席时，替别人报一次 `failure` 就能永久关掉它的试启用机会。
    ///
    /// 这条刻意调用**不过身份**的核心 `cmd_recover_report`，把"gate 挡住了什么"变成
    /// 可执行证据（而不是文档里的一句形容词）。它同时锁住"一旦有人把 `_as` 里的
    /// 判定摘掉，会失去什么"。
    #[test]
    fn recover_report_without_the_identity_check_burns_another_plugins_trial_budget() {
        let state = state_with_b_under_trial();
        cmd_recover_report(&state, "failure", Some("p.b")).unwrap();

        assert_eq!(
            state.recovery.lock().plugin_state("p.b").map(|s| s.as_str()),
            Some("disabled-by-safemode"),
            "未过身份的核心：B 被替报一次就回落（这就是 gate 要挡的伤害）"
        );
        assert_eq!(
            state.recovery.lock().counter().trial_failures.get("p.b").copied(),
            Some(1),
            "别人的试验预算被消耗"
        );
        // 不可逆：B 连合法试启都做不了了（TRIAL_MAX_ATTEMPTS = 1）。
        assert_eq!(
            cmd_recover_trial_enable_as(&Caller::MainWindow, &state, "p.b").unwrap_err().code,
            ErrorCode::E_PLUGIN_DISABLED
        );
    }

    /// `host_recover_trial_enable`：仅主窗，且拒绝路径不消耗任何试启预算。
    #[test]
    fn plugin_caller_cannot_trial_enable_a_plugin_but_main_window_can() {
        let state = state_with_b_under_trial();
        let state2 = CommandState::new();
        for _ in 0..2 {
            cmd_recover_report(&state2, "failure", None).unwrap();
        }

        // 插件（哪怕是"自己的" id）→ 拒绝，且引擎侧毫无动静。
        let err = cmd_recover_trial_enable_as(&plugin_caller("p.a"), &state2, "p.a").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state2.recovery.lock().plugin_state("p.a"),
            None,
            "拒绝路径不得把插件登记进恢复引擎"
        );
        assert_eq!(state2.recovery.lock().counter().trial_failures.get("p.a").copied(), None);

        // 对照：主窗试启同一个插件 → 真的进了 TrialEnable（拒绝不是命令被关死）。
        let r = cmd_recover_trial_enable_as(&Caller::MainWindow, &state2, "p.a").unwrap();
        assert_eq!(r["phase"], "safemode");
        assert_eq!(
            state2.recovery.lock().plugin_state("p.a").map(|s| s.as_str()),
            Some("trial-enable")
        );
        // 已经试启中的 state 上再让插件调一次，也必须被拒（不能"借"主窗的试启）。
        let err = cmd_recover_trial_enable_as(&plugin_caller("p.b"), &state, "p.b").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
    }

    /// `host_market_check/download/install`：仅主窗，且拒绝路径不写 `updateState`。
    #[test]
    fn plugin_caller_cannot_use_market_commands_but_main_window_can() {
        let state = CommandState::new();
        let a = plugin_caller("p.a");
        // 哨兵值：拒绝路径不得覆盖它（"没写"与"写成别的东西"要能区分）。
        state.shell_ext.lock().update_state = Some("sentinel".to_string());

        let err = cmd_market_check_as(&a, &state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        let err = cmd_market_download_as(&a, &state, Some("2.0.0")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        let err = cmd_market_install_as(&a, &state, Some("2.0.0")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.shell_ext.lock().update_state.as_deref(),
            Some("sentinel"),
            "拒绝路径不得推进 updateState"
        );
        assert!(
            !state.shell_ext.lock().update_state_simulated,
            "拒绝路径不得连 provenance 一起写（哨兵是直接写的，不是桩推进的）"
        );

        // 对照：主窗三条全通（返回线形不变，仍是模拟结果的诚实口径）。
        let check = cmd_market_check_as(&Caller::MainWindow, &state).unwrap();
        assert!(check.simulated && !check.available);
        let dl = cmd_market_download_as(&Caller::MainWindow, &state, Some("2.0.0")).unwrap();
        assert!(dl.ok && dl.simulated);
        assert_eq!(state.shell_ext.lock().update_state.as_deref(), Some("downloaded:2.0.0"));
        let ins = cmd_market_install_as(&Caller::MainWindow, &state, Some("2.0.0")).unwrap();
        assert!(ins.ok && ins.simulated);
        assert_eq!(state.shell_ext.lock().update_state.as_deref(), Some("installed:2.0.0"));
    }

    /// 畸形 label（`plugin-` 前缀但 id 非法）走既有拒绝路径，**不得**降级成主窗。
    ///
    /// 这是第二批所有判定共用的一道前提：`Caller::from_label` 只可能给出
    /// `MainWindow` / `Plugin`，畸形输入在解析处就被拒（`Caller` 类型刻意没有
    /// `Invalid` 变体）——所以下面的核心判定拿到的 caller 一定是可信主体。
    /// 验收标准 1（方案 §9-1）：harness 类宿主**只有底座**也要"功能完整"。
    ///
    /// 本用例只构造 [`SubstrateState`]——**不建** `PluginRuntimeState`、不碰注册表、
    /// 不碰 contributes/执行模型，逐域各打一次真实调用：shell（窗口 → Sink）、
    /// settings、i18n、notify、recovery、事件总线、brand。
    ///
    /// 为什么需要它：`cargo check` 只能证明"能编译"，不能证明"底座不是空壳"。
    /// 可编译形态的独立证据在 `examples/minimal-app` 的 `--features substrate-only`
    /// （那里连 `PluginRuntimeState` 都不 `manage`）。
    #[test]
    fn substrate_only_host_is_functionally_complete() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());
        let main = Caller::MainWindow;

        // ① shell：窗口命令经 `WindowSink`（缺省 `MemoryWindowSink` 留痕、不假装原生）。
        cmd_window_minimize(&state, "main").expect("shell 域：窗口命令必须可用");
        cmd_window_set_size(&state, "main", 800, 600).expect("shell 域：改尺寸必须可用");

        // ② settings：主窗读写（插件档的键空间判定另有专测）。
        cmd_settings_set_as(&main, &state, "app.theme", serde_json::json!("dark"))
            .expect("settings 域：主窗必须可写");
        assert_eq!(
            cmd_settings_get_as(&main, &state, "app.theme").unwrap(),
            serde_json::json!("dark"),
            "settings 域：写进去的要能原样读出来"
        );

        // ③ i18n：装载 → 切语言 → 取词。
        let mut entries = serde_json::Map::new();
        entries.insert("app.title".to_string(), serde_json::json!("标题"));
        cmd_i18n_load_as(&main, &state, "zh-CN", entries, None).expect("i18n 域：装载必须可用");
        cmd_i18n_set_locale_as(&main, &state, "zh-CN").expect("i18n 域：切语言必须可用");
        assert_eq!(
            cmd_i18n_t(&state, "app.title").unwrap(),
            "标题",
            "i18n 域：装载后必须真能取到该语言的词条"
        );

        // ④ notify：写入 → 读回（同一底座状态，不依赖插件运行时）。
        cmd_notify_as(&main, &state, "host", "标题", "正文").expect("notify 域：发送必须可用");
        let list = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(
            list.get("total").and_then(|v| v.as_u64()),
            Some(1),
            "notify 域：写入后必须能读回一条（{list}）"
        );

        // ⑤ recovery：启动载荷 + 成功上报（持久化目录为 None = 仅进程内，属有意降级）。
        let boot = cmd_recover_boot(&state).expect("recovery 域：启动载荷必须可用");
        assert!(
            boot.as_object().is_some_and(|o| !o.is_empty()),
            "recovery 域：启动载荷不能是空对象（{boot}）"
        );
        cmd_recover_report_as(&main, &state, "success", None).expect("recovery 域：上报必须可用");

        // ⑥ 事件总线（底座 IPC 面）：发布必须成功。
        cmd_events_publish(&state, "host", "app.ready", serde_json::json!({ "n": 1 }))
            .expect("ipc 域：发布必须可用");

        // ⑦ brand provider 未接入时必须明确报告 unsupported。
        match cmd_brand_info(&state).unwrap() {
            ProviderResult::Unsupported(body) => assert!(!body.supported),
            ProviderResult::Value(_) => panic!("未配置品牌来源时必须如实报告 unsupported"),
        }
    }

    #[test]
    fn malformed_labels_cannot_reach_the_new_identity_checks_as_main_window() {
        let state = CommandState::new();
        for bad in ["plugin-", "plugin-..", "plugin-a b"] {
            let err = Caller::from_label(bad).unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{bad}");
        }
        // 判定的第二个入口同样只认主窗：`require_self_plugin_scope` 对主窗放行任意
        // 署名、对插件只放行自己——两块判定不重叠，也不是"二选一"。
        assert!(require_self_plugin_scope(&Caller::MainWindow, "host_notify", Some("x")).is_ok());
        assert!(require_self_plugin_scope(&Caller::MainWindow, "host_notify", None).is_ok());
        assert!(
            require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", Some("p.a")).is_ok()
        );
        for claimed in [Some("p.b"), None] {
            assert_eq!(
                require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", claimed)
                    .unwrap_err()
                    .code,
                ErrorCode::E_AUTH_DENIED
            );
        }
        // 两个拒绝分支的消息必须说清"为什么"（诊断时能区分冒名与宿主级主张）。
        let impersonate =
            require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", Some("p.b"))
                .unwrap_err();
        assert!(impersonate.message.contains("p.b") && impersonate.message.contains("p.a"));
        let host_ns =
            require_self_plugin_scope(&plugin_caller("p.a"), "host_notify", None).unwrap_err();
        assert!(host_ns.message.contains("宿主级") || host_ns.message.contains("应用级"));
        let _ = state;
    }

    // ──────────────────────────────────────────────────────────────────────
    // 轮 11 第三批：通知读端（过滤）/ 通知写端 / 全局语言 / 应用级深链接
    //
    // 读端是**过滤**（插件必须仍能读自己的），写端与后两条是**判定**。
    // 每条都配：① 主窗对照 ② 越权零副作用 ③ "不过判定的内核会造成什么"的实证。
    // ──────────────────────────────────────────────────────────────────────

    /// 通知读端：插件只看得到署名是自己的条目，且 `total`/`unread` **同源**。
    #[test]
    fn plugin_caller_only_lists_its_own_notifications() {
        let state = CommandState::new();
        cmd_notify(&state, "p.a", "A1", "body-a1").unwrap();
        cmd_notify(&state, "p.b", "B1", "body-b1").unwrap();
        cmd_notify(&state, "p.a", "A2", "body-a2").unwrap();
        assert_eq!(state.notify_store.lock().len(), 3, "前置：三条真的都写进去了");

        let a = plugin_caller("p.a");
        let snap = cmd_notifications_list_as(&a, &state, None).unwrap();
        assert_eq!(snap["total"], 2, "total 必须是**可见**集合的真值（不是全局 3）");
        assert_eq!(snap["unread"], 2, "unread 同理——只裁数组会留下未读数泄露");
        assert_eq!(snap["pluginCapacity"], 64);
        assert_eq!(snap["pluginUsage"]["p.a"], 2);
        assert!(snap["pluginUsage"].get("p.b").is_none(), "配额观测不能泄漏邻居活动");
        let items = snap["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        for it in items {
            assert_eq!(it["pluginId"], "p.a", "不得出现别人的条目");
        }
        assert_eq!(items[0]["title"], "A2", "过滤之后仍按时间倒序");
        let wire = serde_json::to_string(&snap).unwrap();
        assert!(
            !wire.contains("body-b1") && !wire.contains("B1"),
            "别人的标题/正文一个字都不许出现在返回体里（含任何字段）"
        );

        // `limit` 作用在**过滤之后**：全局最新那条是**别人的**，插件仍能拿到自己最新那条。
        cmd_notify(&state, "p.b", "B2", "body-b2").unwrap();
        let page = cmd_notifications_list_as(&a, &state, Some(1)).unwrap();
        assert_eq!(
            page["items"].as_array().unwrap().len(),
            1,
            "先分页再过滤会让插件自己的通知被别人的挤掉（这里会变成 0 条）"
        );
        assert_eq!(page["items"][0]["title"], "A2");
        assert_eq!(page["total"], 2, "total 不受 limit 影响（可见集合总数）");
        assert_eq!(page["unread"], 2);

        // 对照：主窗照旧看全部（现状不变）。
        let all = cmd_notifications_list_as(&Caller::MainWindow, &state, None).unwrap();
        assert_eq!(all["total"], 4);
        assert_eq!(all["items"].as_array().unwrap().len(), 4);
        assert_eq!(all["capacity"], 512);
        assert_eq!(all["pluginUsage"]["p.a"], 2);
        assert_eq!(all["pluginUsage"]["p.b"], 2);
        assert_eq!(
            cmd_notifications_list_as(&Caller::MainWindow, &state, Some(1)).unwrap()["items"][0]
                ["title"],
            "B2"
        );
    }

    /// **危害实证**：不过身份的内核把别人的通知正文（可能是敏感内容）整份交出去。
    #[test]
    fn notifications_list_without_scoping_leaks_other_plugins_content() {
        let state = CommandState::new();
        cmd_notify(&state, "p.b", "B 的故障详情", "reset-token=secret-b").unwrap();

        let leak = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(leak["total"], 1);
        assert!(
            serde_json::to_string(&leak).unwrap().contains("secret-b"),
            "未过滤时别人的通知正文（含 data 里的跳转载荷）完整可读——这就是 gate 要挡的东西"
        );

        // 同一个存储、同一个"插件 A"视角，走 gate 后什么都看不到。
        let scoped = cmd_notifications_list_as(&plugin_caller("p.a"), &state, None).unwrap();
        assert_eq!(scoped["total"], 0);
        assert!(!serde_json::to_string(&scoped).unwrap().contains("secret-b"));
    }

    /// 通知读端的 `dispatchLog` 同样按身份过滤（记录里只有 `entryId`，要反查归属）。
    #[test]
    fn plugin_caller_only_lists_its_own_dispatch_log() {
        let state = CommandState::new();
        let sink = AdapterMockSink::new(true, false);
        assert!(state.notify_sink.set(sink.clone()).is_ok());
        cmd_notify(&state, "p.a", "A", "a").unwrap();
        cmd_notify(&state, "p.b", "B", "b").unwrap();

        // 前置：主窗视角下日志确实有两条（否则下面的断言没有意义）。
        let all = cmd_notifications_list_as(&Caller::MainWindow, &state, None).unwrap();
        assert_eq!(all["dispatchLog"].as_array().unwrap().len(), 2);

        let scoped = cmd_notifications_list_as(&plugin_caller("p.a"), &state, None).unwrap();
        let log = scoped["dispatchLog"].as_array().unwrap();
        assert_eq!(log.len(), 1, "只留能归属到自己的分发记录");
        assert_eq!(log[0]["entryId"], scoped["items"][0]["id"], "留下的那条必须指向自己的条目");
    }

    /// 通知写端：插件只能标记自己的通知；`None`（全部已读）与未知 id 一律拒绝。
    #[test]
    fn plugin_caller_can_only_mark_its_own_notifications_read() {
        let state = CommandState::new();
        cmd_notify(&state, "p.a", "A", "a").unwrap();
        cmd_notify(&state, "p.b", "B", "b").unwrap();
        let (a_id, b_id) = {
            let store = state.notify_store.lock();
            let ids: Vec<(String, String)> = store
                .recent(store.len())
                .iter()
                .map(|e| (e.plugin_id.clone(), e.id.clone()))
                .collect();
            let pick = |who: &str| ids.iter().find(|(p, _)| p == who).unwrap().1.clone();
            (pick("p.a"), pick("p.b"))
        };
        let a = plugin_caller("p.a");

        // 标记**别人的** → 拒绝，且 B 仍是未读、全局未读计数不动。
        let err = cmd_notifications_read_as(&a, &state, Some(&b_id)).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert!(
            !state.notify_store.lock().get(&b_id).unwrap().read,
            "别人的已读状态不得被篡改（那等于替别人把消息吞掉）"
        );
        assert_eq!(state.notify_store.lock().unread_count(), 2);

        // 未知 id → 拒绝：放行会让 `marked` 变成"这个 id 存不存在"的预言机。
        let err = cmd_notifications_read_as(&a, &state, Some("nope")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(state.notify_store.lock().unread_count(), 2);

        // `None` = 标记全部（全局状态）→ 拒绝，且一条都不许被清。
        let err = cmd_notifications_read_as(&a, &state, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.notify_store.lock().unread_count(),
            2,
            "拒绝路径不得清全局未读（否则宿主的角标会静默归零）"
        );

        // 标记**自己的** → 生效。
        let r = cmd_notifications_read_as(&a, &state, Some(&a_id)).unwrap();
        assert_eq!(r["marked"], 1);
        assert_eq!(state.notify_store.lock().unread_count(), 1);

        // 对照：主窗任意 id / `None` 照旧；未知 id 仍是 `marked: 0`（既有行为不变）。
        cmd_notifications_read_as(&Caller::MainWindow, &state, Some(&b_id)).unwrap();
        assert!(state.notify_store.lock().get(&b_id).unwrap().read);
        assert_eq!(
            cmd_notifications_read_as(&Caller::MainWindow, &state, None).unwrap()["marked"],
            0
        );
        assert_eq!(
            cmd_notifications_read_as(&Caller::MainWindow, &state, Some("nope")).unwrap()["marked"],
            0
        );
    }

    /// **危害实证**：不过身份的内核一次调用就能清掉**所有人**的未读。
    #[test]
    fn notifications_read_without_the_identity_check_can_wipe_the_unread_state_of_others() {
        let state = CommandState::new();
        cmd_notify(&state, "host", "宿主重要告警", "磁盘快满了").unwrap();
        cmd_notify(&state, "p.b", "B", "b").unwrap();
        assert_eq!(state.notify_store.lock().unread_count(), 2);

        // 不过身份的核心：`None` = 全部已读。
        assert_eq!(cmd_notifications_read(&state, None).unwrap()["marked"], 2);
        assert_eq!(
            state.notify_store.lock().unread_count(),
            0,
            "未过身份的核心：一次调用把宿主与所有插件的未读清零（用户再也不会被提醒）"
        );
        // 单条路径同理：替别人标记已读。
        let state2 = CommandState::new();
        cmd_notify(&state2, "p.b", "B", "b").unwrap();
        let b_id = state2.notify_store.lock().recent(1)[0].id.clone();
        assert_eq!(cmd_notifications_read(&state2, Some(&b_id)).unwrap()["marked"], 1);
        assert!(state2.notify_store.lock().get(&b_id).unwrap().read);
    }

    /// `host_i18n_set_locale`：仅主窗（切的是全局语言：宿主 UI + 所有插件）。
    #[test]
    fn plugin_caller_cannot_change_the_global_locale_but_main_window_can() {
        let state = CommandState::new();
        let before = cmd_i18n_stats(&state).unwrap();
        let current = before["locale"].as_str().unwrap().to_string();
        let other = if current == "ja-JP" { "ko-KR" } else { "ja-JP" };

        let err = cmd_i18n_set_locale_as(&plugin_caller("p.a"), &state, other).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            cmd_i18n_stats(&state).unwrap(),
            before,
            "拒绝路径不得改语言（引擎状态与已注册语言包逐字不变）"
        );
        // 也确认没有"悄悄改了一半"：locale 与回退链都还是原值。
        assert_eq!(cmd_i18n_stats(&state).unwrap()["locale"], current.as_str());
        assert_eq!(state.i18n.lock().locale().as_str(), current.as_str());

        // 对照：主窗切换生效。
        let out = cmd_i18n_set_locale_as(&Caller::MainWindow, &state, other).unwrap();
        assert_eq!(out["locale"], other);
        assert_eq!(state.i18n.lock().locale().as_str(), other);
    }

    /// **危害实证**：不过身份的内核让一个插件改掉**所有人**界面的语言。
    #[test]
    fn i18n_set_locale_without_the_identity_check_changes_the_language_for_everyone() {
        let state = CommandState::new();
        let before = cmd_i18n_stats(&state).unwrap()["locale"].as_str().unwrap().to_string();
        let other = if before == "ja-JP" { "ko-KR" } else { "ja-JP" };

        cmd_i18n_set_locale(&state, other).unwrap();
        assert_eq!(state.i18n.lock().locale().as_str(), other);
        assert_ne!(before, other);
        // 受影响的是**同一个引擎**：插件自己与宿主的每一次 host_i18n_t 都改了口径，
        // 也就是"用户明明在中文环境里看到日文界面，而根因在另一个插件的一次调用里"。
        assert_eq!(
            cmd_i18n_stats(&state).unwrap()["locale"],
            other,
            "全局语言被改：宿主 UI 与所有插件的界面同时换语种"
        );
    }

    /// `host_deep_link_register`：仅主窗（注册的是**应用级**协议，且会注销上一个协议）。
    #[test]
    fn plugin_caller_cannot_register_an_app_level_deep_link_but_main_window_can() {
        let state = CommandState::new();
        // 正当路径：主窗先注册 `tauron`。
        cmd_deep_link_register_as(&Caller::MainWindow, &state, "tauron".to_string()).unwrap();
        assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("tauron"));

        // 插件接管 → 拒绝，且应用级协议**逐字不动**（也没有触发注销旧值）。
        let err = cmd_deep_link_register_as(&plugin_caller("p.a"), &state, "evil".to_string())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            state.shell_ext.lock().deep_link_protocol.as_deref(),
            Some("tauron"),
            "拒绝路径不得改应用级协议（也不得把上一个协议从 OS 关联里摘掉）"
        );

        // 对照：主窗换协议生效。
        cmd_deep_link_register_as(&Caller::MainWindow, &state, "other".to_string()).unwrap();
        assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("other"));
    }

    /// **危害实证**：不过身份的内核让插件把整个应用的深链接入口改到自己名下。
    #[test]
    fn deep_link_register_without_the_identity_check_lets_a_plugin_take_over_the_protocol() {
        let state = CommandState::new();
        cmd_deep_link_register_as(&Caller::MainWindow, &state, "tauron".to_string()).unwrap();
        // 不过身份的核心（插件能直接调到的就是它）。
        cmd_deep_link_register(&state, "evil".to_string()).unwrap();
        assert_eq!(
            state.shell_ext.lock().deep_link_protocol.as_deref(),
            Some("evil"),
            "未过身份的核心：应用认的协议被改成插件给的值（用户的 tauron:// 链接不再拉起本应用）"
        );
    }

    #[test]
    fn cmd_market_check_returns_consumable_stub() {
        let state = CommandState::new();
        let result = cmd_market_check(&state);
        assert!(result.is_ok());
        let check = result.unwrap();
        // 必须是 TS 可直接消费的形状：Null 会让前端 result.available 崩溃。
        assert!(!check.available);
        // R8：**模拟**这件事必须是线字段，而不是只写在注释里。
        assert!(check.simulated, "本地桩结果必须如实标 simulated");
        assert!(
            check.reason.as_deref().is_some_and(|r| r.contains("未接入更新源")),
            "必须写明为什么不可用：{:?}",
            check.reason
        );
        assert_eq!(
            serde_json::to_value(&check).unwrap(),
            serde_json::json!({
                "available": false,
                "simulated": true,
                "version": null,
                "reason": check.reason.clone(),
            }),
            "线形字段名/数量必须与 TS 同步（camelCase、无额外字段）"
        );
    }

    #[test]
    fn cmd_brand_info_reports_missing_provider() {
        let state = CommandState::new();
        // 未设置 TAURON_BRAND_CONFIG(_JSON) 时必须如实降级（不伪造默认品牌）。
        let result = cmd_brand_info(&state).unwrap();
        match result {
            ProviderResult::Unsupported(body) => {
                assert!(!body.supported);
                assert!(body.reason.contains("brand provider"));
            }
            ProviderResult::Value(_) => panic!("未配置品牌来源时必须返回 UnsupportedBody"),
        }
    }

    #[test]
    fn cmd_brand_info_projects_and_validates_a_configured_brand() {
        // 直接走投影函数（不碰环境变量，避免进程级竞态），验证 tauron-brand 真校验。
        let cfg = tauron_brand::BrandConfig {
            identifier: "com.example.app".into(),
            protocol_scheme: "example".into(),
            autostart_name: "example-autostart".into(),
            data_dir: "example".into(),
            ..Default::default()
        };
        let info = brand_info_from_raw(&cfg).unwrap();
        assert_eq!(info.identifier, "com.example.app");
        assert_eq!(info.protocol_scheme, "example");

        // 必填字段为空 → 走 tauron-brand 校验并返回 E_INVALID_MANIFEST。
        let bad = tauron_brand::BrandConfig::default();
        let err = brand_info_from_raw(&bad).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
    }

    fn find_summary(state: &PluginRuntimeState, plugin_id: &str) -> Option<PluginSummary> {
        state.registry.list_all().into_iter().find(|p| p.id == plugin_id)
    }

    fn entries(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        let mut m = serde_json::Map::new();
        for (k, v) in pairs {
            m.insert(k.to_string(), serde_json::json!(v));
        }
        m
    }

    #[test]
    fn cmd_i18n_t_missing_key_returns_key_and_counts_miss() {
        let state = CommandState::new();
        // 全部缺失时返回 key 本身（**不是空串**）：让用户看到 `oc.x` 比看到空
        // 按钮更能暴露缺失文案，空串会让问题彻底隐形。
        assert_eq!(cmd_i18n_t(&state, "oc.settings.title").unwrap(), "oc.settings.title");
        assert_eq!(cmd_i18n_stats(&state).unwrap()["missingTotal"], 1);
    }

    #[test]
    fn cmd_i18n_t_rejects_empty_key() {
        let state = CommandState::new();
        assert_eq!(cmd_i18n_t(&state, "   ").unwrap_err().code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn cmd_i18n_load_set_locale_and_translate() {
        let state = CommandState::new();
        cmd_i18n_load(
            &state,
            "zh-CN",
            entries(&[("oc.hello", "你好"), ("oc.by.name", "你好，{{name}}")]),
            None,
        )
        .unwrap();

        let switched = cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        assert_eq!(switched["locale"], "zh-CN");
        assert_eq!(switched["rtl"], serde_json::json!(false));
        assert_eq!(switched["fallbackChain"], serde_json::json!(["zh-CN", "zh", "en-US"]));

        assert_eq!(cmd_i18n_t(&state, "oc.hello").unwrap(), "你好");
        assert_eq!(
            cmd_i18n_t_params(&state, "oc.by.name", entries(&[("name", "世界")])).unwrap(),
            "你好，世界"
        );
    }

    #[test]
    fn cmd_i18n_t_falls_back_through_chain_without_counting_miss() {
        let state = CommandState::new();
        cmd_i18n_load(&state, "en-US", entries(&[("oc.menu.file", "File")]), None).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        // zh-CN 缺 → 回退链（zh → en-US）命中。回退链命中不算缺失：它成功翻译了，
        // 缺失计数只统计回退链耗尽的情况（那才是需要补文案的）。
        assert_eq!(cmd_i18n_t(&state, "oc.menu.file").unwrap(), "File");
        assert_eq!(cmd_i18n_stats(&state).unwrap()["missingTotal"], 0);

        // 回退链耗尽才计数。
        assert_eq!(cmd_i18n_t(&state, "oc.nowhere").unwrap(), "oc.nowhere");
        assert_eq!(cmd_i18n_stats(&state).unwrap()["missingTotal"], 1);
    }

    #[test]
    fn cmd_i18n_load_merges_instead_of_replacing() {
        let state = CommandState::new();
        cmd_i18n_load(&state, "zh-CN", entries(&[("oc.a", "a1")]), None).unwrap();
        let result = cmd_i18n_load(&state, "zh-CN", entries(&[("oc.b", "b1")]), None).unwrap();
        assert_eq!(result["loadedKeys"], 1);
        assert_eq!(result["newKeys"], 1);
        // 合并而不是替换：两次装载的 key 都必须还在。
        assert_eq!(result["bundleKeys"]["zh-CN"], 2);
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        assert_eq!(cmd_i18n_t(&state, "oc.a").unwrap(), "a1");
        assert_eq!(cmd_i18n_t(&state, "oc.b").unwrap(), "b1");
    }

    #[test]
    fn cmd_i18n_load_namespaces_plugin_keys_and_cleanup_removes_only_prefix() {
        let state = CommandState::new();

        let loaded = cmd_i18n_load(
            &state,
            "zh-CN",
            entries(&[("title", "我的插件"), ("plugin:p.my-plugin.oc.owned", "已带前缀")]),
            Some("p.my-plugin"),
        )
        .unwrap();
        assert_eq!(loaded["pluginId"], "p.my-plugin");
        assert_eq!(loaded["loadedKeys"], 2);
        assert_eq!(loaded["newKeys"], 2);

        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        // 带前缀的 key 可翻译；未带前缀的原始 key 不存在（不会双写）。
        assert_eq!(cmd_i18n_t(&state, "plugin:p.my-plugin.oc.title").unwrap(), "我的插件");
        assert_eq!(cmd_i18n_t(&state, "plugin:p.my-plugin.oc.owned").unwrap(), "已带前缀");
        assert_eq!(cmd_i18n_t(&state, "title").unwrap(), "title");
        // 已经是完整前缀的 key 原样保留，不会被二次前缀。
        assert_eq!(loaded["bundleKeys"]["zh-CN"], serde_json::json!(2));

        // 另一个插件的文案必须不受影响。
        cmd_i18n_load(&state, "zh-CN", entries(&[("x", "X")]), Some("p.other")).unwrap();

        let cleaned = cmd_i18n_cleanup_plugin(&state, "p.my-plugin").unwrap();
        assert_eq!(cleaned["pluginId"], "p.my-plugin");
        assert_eq!(cleaned["removed"], 2);
        assert_eq!(cmd_i18n_t(&state, "plugin:p.other.oc.x").unwrap(), "X");
        assert_eq!(
            cmd_i18n_t(&state, "plugin:p.my-plugin.oc.title").unwrap(),
            "plugin:p.my-plugin.oc.title"
        );
    }

    #[test]
    fn cmd_i18n_set_locale_rejects_invalid_locale() {
        let state = CommandState::new();
        // 空串与超长串都会污染回退链，必须在适配层拦下（引擎自身不校验）。
        for bad in ["", &"a".repeat(36)] {
            assert_eq!(
                cmd_i18n_set_locale(&state, bad).unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );
        }
        assert!(cmd_i18n_set_locale(&state, "he-IL").is_ok());
        // RTL 语言必须反映在状态里（前端布局要依赖它）。
        assert_eq!(cmd_i18n_stats(&state).unwrap()["rtl"], serde_json::json!(true));
    }

    #[test]
    fn cmd_i18n_load_rejects_non_string_value_and_bad_plugin_id() {
        let state = CommandState::new();
        let mut bad = serde_json::Map::new();
        bad.insert("oc.x".to_string(), serde_json::json!(42));
        assert_eq!(
            cmd_i18n_load(&state, "zh-CN", bad, None).unwrap_err().code,
            ErrorCode::E_INVALID_MANIFEST
        );
        // 插件 id 走同一个校验规则（反域名格式）。
        assert_eq!(
            cmd_i18n_load(&state, "zh-CN", entries(&[("a", "b")]), Some("not-valid"))
                .unwrap_err()
                .code,
            ErrorCode::E_INVALID_MANIFEST
        );
    }

    #[test]
    fn cmd_i18n_stats_reports_consumable_shape() {
        let state = CommandState::new();
        cmd_i18n_load(&state, "zh-CN", entries(&[("oc.a", "a")]), None).unwrap();
        cmd_i18n_t(&state, "oc.missing").unwrap();

        let stats = cmd_i18n_stats(&state).unwrap();
        // 线名必须是 camelCase，且字段齐全（前端按此渲染）。
        for field in
            ["locale", "rtl", "fallbackChain", "registeredLocales", "bundleKeys", "missingTotal"]
        {
            assert!(stats.get(field).is_some(), "缺少字段 {field}");
        }
        assert_eq!(stats["locale"], "en-US");
        assert!(stats["registeredLocales"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("zh-CN")));
        assert_eq!(stats["bundleKeys"]["zh-CN"], 1);
        assert_eq!(stats["missingTotal"], 1);
    }

    #[test]
    fn cmd_settings_set_get_roundtrip() {
        let state = CommandState::new();
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("dark")).unwrap();
        let result = cmd_settings_get(&state, "plugin:p.theme").unwrap();
        assert_eq!(result, serde_json::json!("dark"));
    }

    #[test]
    fn settings_commit_mirrors_to_message_plane_through_approval_chain() {
        // §33 R2-4（W6）Gate：settings set → 订阅者收帧（E2E）；未审批的跨插件
        // 观察被拒；revoke 后闭环回 fail-closed——与 R2-5 五步链同一套授权事实。
        use tauron_host::eventbus::ChannelKind;
        let state = CommandState::new();

        // 1) 未审批：订阅被拒，且镜像发布对它零投递。
        let err = cmd_events_subscribe(&state, "com.b", "w1", HOST_SETTINGS_CHANGED_TOPIC)
            .expect_err("host settings mirror is not openly subscribable");
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("dark")).unwrap();
        assert!(
            state.bus.lock().drain("com.b", ChannelKind::Event).unwrap().is_empty(),
            "被拒订阅不得收到任何帧"
        );

        // 2) 主窗审批 → 订阅成功 → 下一次提交可见。
        cmd_events_approve_as(&Caller::MainWindow, &state, "com.b", HOST_SETTINGS_CHANGED_TOPIC)
            .unwrap();
        cmd_events_subscribe(&state, "com.b", "w1", HOST_SETTINGS_CHANGED_TOPIC).unwrap();
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("cobalt")).unwrap();
        let frames = state.bus.lock().drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 1, "提交必须镜像一帧");
        assert_eq!(frames[0].topic, HOST_SETTINGS_CHANGED_TOPIC);
        assert_eq!(frames[0].payload["key"], "plugin:p.theme");
        assert_eq!(frames[0].payload["value"], "cobalt");
        assert_eq!(frames[0].payload["source"], "user");

        // 3) 连续提交逐帧镜像（Event 通道，revision 单调）。
        cmd_settings_set(&state, "plugin:p.lang", serde_json::json!("zh")).unwrap();
        let frames = state.bus.lock().drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 1);
        assert!(
            frames[0].payload["revision"].as_u64().unwrap() > 0,
            "镜像帧必须携带提交后的 revision"
        );

        // 4) revoke → 既有订阅即刻失效，新订阅回到被拒（A81 撤销效力）。
        cmd_events_revoke_as(&Caller::MainWindow, &state, "com.b", HOST_SETTINGS_CHANGED_TOPIC)
            .unwrap();
        assert!(
            state.bus.lock().subscribed_topics_of("com.b", "w1").is_empty(),
            "撤销后观察窗口的订阅必须消失"
        );
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("crimson")).unwrap();
        assert!(
            state.bus.lock().drain("com.b", ChannelKind::Event).unwrap().is_empty(),
            "撤销后的提交不得再镜像给已撤销的观察方"
        );
        assert!(cmd_events_subscribe(&state, "com.b", "w2", HOST_SETTINGS_CHANGED_TOPIC).is_err());
    }

    #[test]
    fn settings_bulk_ops_fan_out_no_frames_and_advance_no_revision() {
        // 文档口径（app-layer-wire：「产帧范围只有按键写路径」）的行为化门禁。
        // adopt/migrate 走 `set_layer` / `migrate_transactional`，压根不产生
        // `ChangeEvent` → 既不镜像也不推进 revision。订阅方在这两个操作后必须
        // 重读 `host_settings_get`，别指望收到"某个键变了"的帧。
        use tauron_host::eventbus::ChannelKind;
        let state = CommandState::new();
        cmd_events_approve_as(&Caller::MainWindow, &state, "com.b", HOST_SETTINGS_CHANGED_TOPIC)
            .unwrap();
        cmd_events_subscribe(&state, "com.b", "w1", HOST_SETTINGS_CHANGED_TOPIC).unwrap();
        let before = state.settings.lock().revision();

        cmd_settings_adopt_legacy(&state, serde_json::json!({ "plugin:p.theme": "v1-dark" }))
            .unwrap();
        assert_eq!(
            state.settings.lock().revision(),
            before,
            "整份接手不是按键提交，不得推进 revision"
        );
        assert!(
            state.bus.lock().drain("com.b", ChannelKind::Event).unwrap().is_empty(),
            "接手不得扇出镜像帧"
        );

        assert_eq!(cmd_settings_migrate(&state).unwrap(), 1, "v1 → v2 是一步");
        assert_eq!(state.settings.lock().revision(), before, "迁移同样不是按键提交");
        assert!(
            state.bus.lock().drain("com.b", ChannelKind::Event).unwrap().is_empty(),
            "迁移不得扇出镜像帧"
        );
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::json!("v1-dark"),
            "迁移后旧键必须按 v2 线形读得回来（否则不扇出就是丢数据）"
        );

        // 对照组：按键写两样都做。缺了这组，上面所有断言只是「总线本来就静默」的假绿。
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("cobalt")).unwrap();
        assert!(state.settings.lock().revision() > before, "按键提交必须推进 revision");
        let frames = state.bus.lock().drain("com.b", ChannelKind::Event).unwrap();
        assert_eq!(frames.len(), 1, "按键提交必须镜像一帧");
        assert_eq!(frames[0].payload["key"], "plugin:p.theme");
    }

    // ────────────────────────────────────────────────────────────
    // R7-2：settings 走 tauron-settings 的 Store
    // ────────────────────────────────────────────────────────────

    #[test]
    fn settings_fault_boundary_blocks_work_and_migrate_reconciles_from_durable_state() {
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("dark")).unwrap();

        // 走**生产闸门**注入 panic：`run_settings_boundary` 自己做 catch_unwind +
        // record_panic（不再让边界锁扣在闭包上）。借道 `FaultBoundary::run` 只能测到
        // 那个类型本身，测不到设置族真正走的那条路。
        let fault = run_settings_boundary(&state, "forced-test-panic", || -> HostResult<()> {
            panic!("boom")
        });
        assert_eq!(fault.unwrap_err().code, ErrorCode::E_HOST_PANIC);
        assert_eq!(state.settings_fault.lock().state(), tauron_host::FaultState::Faulted);
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap_err().code,
            ErrorCode::E_HOST_PANIC
        );

        // Existing main-window migrate command is the repair/reconcile surface; it rebuilds the
        // settings engine from the durable envelope before allowing ordinary work again.
        assert_eq!(cmd_settings_migrate_as(&Caller::MainWindow, &state).unwrap(), 0);
        assert_eq!(state.settings_fault.lock().state(), tauron_host::FaultState::Ready);
        assert_eq!(cmd_settings_get(&state, "plugin:p.theme").unwrap(), serde_json::json!("dark"));
    }

    #[test]
    fn settings_fault_without_durable_state_is_quarantined_not_faked_ready() {
        let state = CommandState::new();
        let _ = run_settings_boundary(&state, "forced-test-panic", || -> HostResult<()> {
            panic!("boom")
        });
        let err = cmd_settings_migrate(&state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_HOST_PANIC);
        assert_eq!(state.settings_fault.lock().state(), tauron_host::FaultState::Quarantined);
    }

    // ────────────────────────────────────────────────────────────
    // A91（轮 47）：事件 / 审批 / 注册表三个子系统的故障边界
    // ────────────────────────────────────────────────────────────

    #[test]
    fn events_fault_boundary_rejects_bus_work_and_recover_boot_resets_session_state() {
        use tauron_host::eventbus::ChannelKind;
        let state = CommandState::new();
        state
            .bus
            .lock()
            .declare_topics("p.a", &[EventDecl { topic: "plugin:p.a.tick".into(), public: true }])
            .unwrap();
        cmd_events_subscribe(&state, "p.a", "w1", "plugin:p.a.tick").unwrap();
        cmd_events_publish(&state, "p.a", "plugin:p.a.tick", serde_json::json!({"n": 1})).unwrap();
        // `host_events_publish` 走可靠发布（`publish_request`）→ 落在订阅者的
        // Request 通道（`host_events_drain(kind = "request")`）。
        assert_eq!(state.bus.lock().drain("p.a", ChannelKind::Request).unwrap().len(), 1);

        // 走**生产闸门**注入 panic（与 settings 边界同一套三段式）。
        let fault = run_events_boundary(&state, "forced-test-panic", || -> HostResult<()> {
            panic!("bus boom")
        });
        assert_eq!(fault.unwrap_err().code, ErrorCode::E_HOST_PANIC);
        assert_eq!(state.events_fault.lock().state(), tauron_host::FaultState::Faulted);

        // 故障后事件面整体拒绝服务，错误文案必须具名修复出口。
        let err = cmd_events_publish(&state, "p.a", "plugin:p.a.tick", serde_json::json!({"n": 2}))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_HOST_PANIC);
        assert!(err.message.contains("events fault boundary rejected work"), "{}", err.message);
        assert!(err.message.contains("host_recover_boot"), "{}", err.message);
        let err = cmd_events_subscribe(&state, "p.a", "w2", "plugin:p.a.tick").unwrap_err();
        assert!(err.message.contains("events fault boundary rejected work"), "{}", err.message);

        // 修复出口 = host_recover_boot：会话态清零——边界归 Ready、订阅/队列清光。
        cmd_recover_boot(&state).unwrap();
        assert_eq!(state.events_fault.lock().state(), tauron_host::FaultState::Ready);
        assert!(
            state.bus.lock().subscribed_topics_of("p.a", "w1").is_empty(),
            "会话态清零：故障前的订阅登记不得残留"
        );
        // 结构声明保留：连声明一起清掉的话，发布端对未声明 topic 只静默丢弃
        // ——修复动作会把可工作的消息面悄悄打哑（宿主镜像/深链同属这类发布路径）。
        assert!(state.bus.lock().topic_meta("plugin:p.a.tick").is_some());
        assert!(
            state.bus.lock().topic_meta(HOST_SETTINGS_CHANGED_TOPIC).is_some(),
            "宿主镜像 topic 的装配期声明必须挺过会话重置"
        );

        // 工作恢复：不重新声明，直接重新订阅 + 发布 → 帧照常投递。
        cmd_events_subscribe(&state, "p.a", "w1", "plugin:p.a.tick").unwrap();
        cmd_events_publish(&state, "p.a", "plugin:p.a.tick", serde_json::json!({"n": 3})).unwrap();
        assert_eq!(state.bus.lock().drain("p.a", ChannelKind::Request).unwrap().len(), 1);
    }

    #[test]
    fn approval_fault_boundary_blocks_tokens_and_preview_reconcile_clears_them() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 基线：预览铸发成功——证明后续拒绝来自故障，而不是链路本来就断。
        let stale = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);

        let fault = run_review_boundary(&state, "forced-test-panic", || -> HostResult<()> {
            panic!("approval boom")
        });
        assert_eq!(fault.unwrap_err().code, ErrorCode::E_HOST_PANIC);
        assert_eq!(state.review_fault.lock().state(), tauron_host::FaultState::Faulted);

        // 故障后令牌面拒绝服务：铸发与消费都在闸门处被拒，文案具名修复出口。
        let err =
            mint_admin_review(&state, "com.a", RegistryAdminOp::Uninstall, "1.0.0").unwrap_err();
        assert!(err.message.contains("approval fault boundary rejected work"), "{}", err.message);
        assert!(err.message.contains("preview"), "{}", err.message);
        let err = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&stale.review_token),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_HOST_PANIC);
        assert!(err.message.contains("approval fault boundary rejected work"), "{}", err.message);
        // 拒绝路径零副作用：注册表条目仍在。
        let id = PluginId::new("com.a").unwrap();
        assert!(state.registry.find(&id).is_some());

        // 修复出口 = 重新预览：确定性清空旧令牌、重新铸发；正常时是幂等空操作。
        let fresh = preview_admin_review(&state, "com.a", RegistryAdminOp::Uninstall);
        assert_eq!(state.review_fault.lock().state(), tauron_host::FaultState::Ready);

        // 旧令牌已被清空 → 不是"已消费"，是彻底不认识（两种拒绝语义要能区分）。
        let replay = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&stale.review_token),
        )
        .unwrap_err();
        assert_eq!(replay.code, ErrorCode::E_AUTH_DENIED);
        assert!(replay.message.contains("unknown"), "{}", replay.message);

        // 新令牌照常一次性消费。
        let executed = cmd_registry_admin_reviewed_as(
            &Caller::MainWindow,
            &state,
            "com.a",
            RegistryAdminOp::Uninstall,
            false,
            Some(&fresh.review_token),
        )
        .unwrap();
        assert!(matches!(executed, RegistryAdminResponse::Executed(_)));
        assert!(state.registry.find(&id).is_none());
    }

    #[test]
    fn registry_fault_boundary_quarantines_until_reassembly_not_faked_ready() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let fault = run_registry_boundary(&state, "forced-test-panic", || -> HostResult<()> {
            panic!("registry boom")
        });
        assert_eq!(fault.unwrap_err().code, ErrorCode::E_HOST_PANIC);
        assert_eq!(state.registry_fault.lock().state(), tauron_host::FaultState::Faulted);

        // 故障后注册表面拒绝服务，错误文案给出**重启重装**这条真修复路径。
        let err = cmd_registry_list(&state, None, &[]).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_HOST_PANIC);
        assert!(err.message.contains("registry fault boundary rejected work"), "{}", err.message);
        assert!(err.message.contains("restart the host to re-assemble"), "{}", err.message);
        assert!(cmd_registry_list_all(&state).is_err());

        // 修复尝试：条目没有持久镜像、会话内无法重建——宁可如实隔离（Quarantined），
        // 也不把边界推回 Ready 伪造"修好了"。
        cmd_recover_boot(&state).unwrap();
        assert_eq!(state.registry_fault.lock().state(), tauron_host::FaultState::Quarantined);
        assert!(cmd_registry_list(&state, None, &[]).is_err(), "隔离后必须继续拒绝服务");

        // 可观测面如实上报四个边界（不是只藏在内部状态里，主窗 UI 读得到）。
        let stats = cmd_resource_stats(&state).unwrap();
        assert_eq!(stats["faults"]["registry"]["state"], serde_json::json!("quarantined"));
        assert_eq!(stats["faults"]["events"]["state"], serde_json::json!("ready"));
        assert_eq!(
            stats["faults"]["registry"]["lastFault"]["operation"],
            serde_json::json!("forced-test-panic")
        );

        // 边界随 SubstrateState 隔离：新实例从 Ready 起步（不是进程级黏性开关）。
        let fresh = CommandState::new();
        assert_eq!(fresh.registry_fault.lock().state(), tauron_host::FaultState::Ready);
    }

    #[test]
    fn settings_are_backed_by_a_store_with_a_registered_schema() {
        // 裸 HashMap 过不了这三条：没有注册表、没有数据版本、没有迁移步。
        let state = CommandState::new();
        cmd_settings_set(&state, "k", serde_json::json!(1)).unwrap();
        let store = state.settings.lock();
        let entry = store
            .registry()
            .get(HOST_SETTINGS_NAMESPACE)
            .expect("宿主设置命名空间必须已注册 schema");
        assert_eq!(entry.schema_version, HOST_SETTINGS_SCHEMA_V2, "当前版本是 v2");
        assert_eq!(store.migration_count(HOST_SETTINGS_NAMESPACE), 1, "v1→v2 迁移已注册");
        assert_eq!(
            store.data_version(HOST_SETTINGS_NAMESPACE),
            Some(HOST_SETTINGS_SCHEMA_V2),
            "写入必须标注数据版本"
        );
    }

    #[test]
    fn settings_keys_are_single_segment_paths_so_flat_semantics_hold() {
        // 点路径不做转义的话：`a` 先写成标量，再写 `a.b` 会撞上
        // `merge::write_path` 的中间节点断言（panic → E_HOST_PANIC），
        // 而且 `get("a")` 会从标量变成对象。转义后这两条都成立。
        let state = CommandState::new();
        cmd_settings_set(&state, "a", serde_json::json!(1)).unwrap();
        cmd_settings_set(&state, "a.b", serde_json::json!(2)).unwrap();
        assert_eq!(cmd_settings_get(&state, "a").unwrap(), serde_json::json!(1));
        assert_eq!(cmd_settings_get(&state, "a.b").unwrap(), serde_json::json!(2));
        // 编码是注入式的：字面键 `a%2Eb` 与 `a.b` 不碰撞。
        cmd_settings_set(&state, "a%2Eb", serde_json::json!(3)).unwrap();
        assert_eq!(cmd_settings_get(&state, "a.b").unwrap(), serde_json::json!(2));
        assert_eq!(cmd_settings_get(&state, "a%2Eb").unwrap(), serde_json::json!(3));
    }

    #[test]
    fn settings_path_encoding_is_injective() {
        assert_eq!(settings_path("plugin:p.theme"), "plugin:p%2Etheme");
        assert_eq!(settings_path("%"), "%25");
        assert_eq!(settings_path("$unset"), "%24unset");
        assert_ne!(settings_path("a.b"), settings_path("a%2Eb"));
        assert_eq!(settings_path("plain"), "plain");
    }

    #[test]
    fn settings_wire_key_inverts_encoding() {
        // 消息面给订阅方的必须是线形键：编码/解码互为逆，且对**非编码**的 `%`
        // 序列不做改写（否则一个本来就含 `%` 的键会被解码改坏）。
        for key in [
            "plugin:p.theme",
            "host$dollar",
            "raw%percent",
            "plain",
            "a.b.c",
            "%",
            "$unset",
            "plugin:com.example.formatter.width",
        ] {
            assert_eq!(settings_wire_key(&settings_path(key)), key, "roundtrip {key}");
        }
        assert_eq!(settings_wire_key("50%"), "50%");
        assert_eq!(settings_wire_key("%zz"), "%zz");
        assert_eq!(settings_wire_key("%2"), "%2");
    }

    #[test]
    fn settings_commit_has_single_mirror_site() {
        // §33 R2-4 的结构不变量：进程内 watcher 与消息面镜像**必须同处一地发生**。
        // 新增设置写路径若绕过 `commit_settings_change`，就会出现「主窗知道、
        // 插件不知道」的半接线——这条门禁把它钉成文本事实（数一下就行）。
        //
        // 探针**拼出来**而不是字面量：本测试文件自身就含这些串，直接写字面量
        // 会让计数把自己也算进去（假绿/假红都_possible）。
        let src = include_str!("lib.rs");
        let commit = format!("{}(", "publish_committed_change");
        let mirror_field = format!("\"key\": settings_{}_key(&committed.key)", "wire");
        assert_eq!(
            src.matches(commit.as_str()).count(),
            1,
            "publish_committed_change 只允许出现在 commit_settings_change 内"
        );
        assert_eq!(src.matches(mirror_field.as_str()).count(), 1, "消息面镜像帧只允许有一个产生点");
        assert!(src.contains("fn commit_settings_change("), "提交口必须收成一个函数");
    }

    #[test]
    fn installation_identity_is_durable_when_data_dir_configured() {
        // R2-8 / §9.1：身份的持久化归**宿主装配**，不是装配方的额外功课。
        // 配了数据目录 → `installation.id` 落盘、跨重开同桶；纯内存装配才临时。
        let t = tempfile::tempdir().unwrap();
        let cfg = recovery_cfg(t.path());

        let first_id = {
            let state = SubstrateState::with_adapter_config(&cfg);
            assert!(state.installation_identity.is_durable(), "有数据目录时身份必须是持久化的");
            state.installation_identity.installation_id().to_string()
        };
        assert!(t.path().join("installation.id").exists());

        let second = SubstrateState::with_adapter_config(&cfg);
        assert_eq!(
            second.installation_identity.installation_id(),
            first_id,
            "同一数据目录重开必须拿回同一身份（灰度桶不跨重启漂移）"
        );

        let memory = SubstrateState::with_adapter_config(&AdapterConfig::default());
        assert!(!memory.installation_identity.is_durable());
    }

    #[test]
    fn settings_set_rejects_empty_key() {
        let state = CommandState::new();
        let e = cmd_settings_set(&state, "  ", serde_json::json!(1)).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn settings_survive_a_restart_when_a_data_dir_is_configured() {
        // 本仓此前的行为：`cmd_settings_set` 只改内存态，Store 从不落盘，
        // 于是「保存成功 → 重启 → 设置没了」。这条用例把「落盘 → 重装配读回」
        // 钉死：没有 `persist_settings_doc` 与装配期 `load_settings_doc` 就过不了。
        let t = tempfile::tempdir().unwrap();

        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("dark")).unwrap();
            cmd_settings_set(&state, "a.b", serde_json::json!(7)).unwrap();
        }

        // 新进程：同一数据目录重新装配，设置必须从磁盘回来。
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        assert_eq!(cmd_settings_get(&state, "plugin:p.theme").unwrap(), serde_json::json!("dark"));
        assert_eq!(cmd_settings_get(&state, "a.b").unwrap(), serde_json::json!(7));
        // 落盘文件名固定，方便宿主/运维定位。
        assert!(t.path().join(HOST_SETTINGS_FILE).is_file());
    }

    #[test]
    fn settings_adopt_legacy_and_migrate_also_persist() {
        // 迁移只改内存态的话，重启后磁盘上的旧文档又盖回来——"迁移成功"却永不生效。
        let t = tempfile::tempdir().unwrap();

        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            cmd_settings_adopt_legacy(&state, serde_json::json!({"plugin:p.theme": "dark"}))
                .unwrap();
            assert_eq!(cmd_settings_migrate(&state).unwrap(), 1);
        }

        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V2),
            "迁移后的版本标注必须跨进程"
        );
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::json!("dark"),
            "迁移后的值必须跨进程可读"
        );
    }

    #[test]
    fn host_settings_migration_declares_snapshot_required_contract() {
        let state = CommandState::new();
        host_settings_adopt_legacy(&state, serde_json::json!({"plugin:p.theme": "dark"})).unwrap();
        let contract = state
            .settings
            .lock()
            .migration_contract(HOST_SETTINGS_NAMESPACE, HOST_SETTINGS_SCHEMA_V2)
            .unwrap();
        assert!(!contract.reversible);
        assert!(!contract.forward_compatible);
        assert!(contract.requires_snapshot);
    }

    #[test]
    fn settings_migration_persist_failure_restores_v1_data_and_version() {
        let t = tempfile::tempdir().unwrap();
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        // A87 now validates/locks the data directory at construction. Inject the persistence
        // failure at the settings temp-file boundary instead, so this test still targets A101
        // rollback rather than failing earlier in storage ownership setup.
        std::fs::create_dir(t.path().join("host-settings.json.tmp")).unwrap();

        // Use the non-persisting core adoption path so the failure is injected specifically at
        // migration commit, not while staging the legacy document.
        host_settings_adopt_legacy(
            &state,
            serde_json::json!({"plugin:p.theme": "dark", "lang": "zh-CN"}),
        )
        .unwrap();
        let before = state.settings.lock().snapshot(HOST_SETTINGS_NAMESPACE).unwrap();

        let err = cmd_settings_migrate(&state).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert_eq!(
            state.settings.lock().snapshot(HOST_SETTINGS_NAMESPACE).unwrap(),
            before,
            "durable commit failure must restore exact pre-migration state"
        );
        assert_eq!(host_settings_data_version(&state).as_deref(), Some(HOST_SETTINGS_SCHEMA_V1));
        assert!(
            !t.path().join(HOST_SETTINGS_ROLLBACK_FILE).exists(),
            "迁移没提交成功 → 镜像必须一起作废（内存已 rewind，留着等于伪造一次没发生的迁移）"
        );
    }

    /// V4 A101：迁移回滚镜像落盘 → 进程重启后由装配消费 → 带回迁移前状态。
    #[test]
    fn a101_rollback_image_restores_pre_migration_state_after_a_restart() {
        let t = tempfile::tempdir().unwrap();
        let doc = t.path().join(HOST_SETTINGS_FILE);
        let image = t.path().join(HOST_SETTINGS_ROLLBACK_FILE);

        // ── 第 1 轮：接手 v1 文档 → 迁移。合同是 requiresSnapshot，镜像必须先于新文档出现。
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            cmd_settings_adopt_legacy(
                &state,
                serde_json::json!({ "plugin:p.theme": "navy", "volume": 11 }),
            )
            .unwrap();
            assert_eq!(
                host_settings_data_version(&state).as_deref(),
                Some(HOST_SETTINGS_SCHEMA_V1)
            );
            assert!(!image.exists(), "迁移前不该有回滚镜像");
            assert_eq!(cmd_settings_migrate(&state).unwrap(), 1);
            assert_eq!(
                host_settings_data_version(&state).as_deref(),
                Some(HOST_SETTINGS_SCHEMA_V2)
            );
            assert!(image.exists(), "requiresSnapshot 的迁移必须留下磁盘回滚镜像");
        }

        // ── 第 2 轮：模拟「迁移之后第一次落盘把文档写坏了」→ 重启必须回到迁移前，
        //    而不是把用户设置清空（非生产档）或拒绝启动。这正是镜像守护的窗口。
        std::fs::write(&doc, b"{ not a durable envelope").unwrap();
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            assert_eq!(
                host_settings_data_version(&state).as_deref(),
                Some(HOST_SETTINGS_SCHEMA_V1),
                "坏文档 + 有效镜像 = 回到迁移前，不是空文档"
            );
            assert_eq!(
                cmd_settings_get(&state, "volume").unwrap(),
                serde_json::json!(11),
                "回滚带的是迁移前的**值**，不只是版本号"
            );
            assert!(!image.exists(), "镜像是一次性的：消费即作废");
            // 回滚带回的是迁移前的**编码层**：v1 文档用裸键，而 `cmd_settings_get` 会先把
            // 键转义再查，所以带点键在这一层读不出来（这正是 v1→v2 迁移存在的原因，不是
            // 回滚把数据弄丢了）。据此再迁一次，链路必须回到可用状态。
            assert_eq!(
                cmd_settings_get(&state, "plugin:p.theme").unwrap(),
                serde_json::Value::Null
            );
            assert_eq!(cmd_settings_migrate(&state).unwrap(), 1, "回滚后必须能重新迁移");
            assert_eq!(
                cmd_settings_get(&state, "plugin:p.theme").unwrap(),
                serde_json::json!("navy"),
                "重新迁移后带点键回到可读"
            );
            assert!(image.exists(), "第二次 requiresSnapshot 迁移必须重新留下镜像");
            // 恢复出来的引擎必须还能继续写、继续落盘。
            cmd_settings_set(&state, "plugin:p.theme", serde_json::json!("crimson")).unwrap();
            assert!(doc.exists(), "恢复后第一次写必须重建正式文档");
        }

        // ── 第 3 轮（轮 16 R3 改判）：健康重启 = **回执**。第 2 轮的文档已重建
        //    并被证明读得动，第二份镜像必须退场——旧断言「健康重启不得消费镜像」
        //    保不住任何东西：镜像自此相对不断前进的正式文档永远陈旧，未来任何一次
        //    无关损坏都会把它当救命稻草，把用户带回任意久远的过去。
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            assert_eq!(
                host_settings_data_version(&state).as_deref(),
                Some(HOST_SETTINGS_SCHEMA_V2)
            );
            assert_eq!(
                cmd_settings_get(&state, "plugin:p.theme").unwrap(),
                serde_json::json!("crimson"),
                "健康重启读到的必须是最新值"
            );
            assert!(!image.exists(), "健康启动必须回执掉镜像");
        }

        // ── 第 4 轮：回执之后再损坏 → 走「无镜像」路径：降级（开发档空文档），
        //    而不是拿一份旧镜像静默穿越回迁移前。这就是改判买到的确定性。
        std::fs::write(&doc, b"{ not a durable envelope").unwrap();
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            assert_eq!(
                host_settings_data_version(&state).as_deref(),
                None,
                "无镜像可兜时如实降级，绝不消费陈旧镜像"
            );
            assert_eq!(
                cmd_settings_get(&state, "plugin:p.theme").unwrap(),
                serde_json::Value::Null
            );
        }
    }

    /// 轮 17 R-A101 续：删不掉的回滚镜像必须**挪出**读取路径。
    ///
    /// 旧实现是 `let _ = remove_file(path)`，把权限/占用/目标是目录这类错误原样咽下，
    /// 结果正是镜像要防的那件事：一份陈旧镜像留在固定文件名上，后续任何一次偶然的
    /// 读损坏都会把它当救命稻草消费掉。这里用**目录**冒充「存在但删不掉」——
    /// `remove_file` 在两个平台上都对目录报错且报的不是 `NotFound`，跨平台稳定。
    #[test]
    fn undeletable_rollback_image_is_moved_out_of_the_load_path() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join(HOST_SETTINGS_ROLLBACK_FILE);
        std::fs::create_dir(&image).unwrap();
        let discarded_prefix = format!("{HOST_SETTINGS_ROLLBACK_FILE}.discarded-");
        let discarded = || -> Vec<String> {
            std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(&discarded_prefix))
                .collect()
        };

        clear_settings_rollback_image(&image);

        assert!(!image.exists(), "作废后固定文件名不得留在读取路径上");
        assert!(
            matches!(load_settings_rollback_image(&image), Ok(None)),
            "「没有镜像」必须是 Ok(None)，不是报错也不是可消费"
        );
        assert_eq!(discarded().len(), 1, "删不掉时留下可查的改名副本，且只有一份");

        // 幂等：第二次作废不得把已挪走的副本再搬一次。
        clear_settings_rollback_image(&image);
        assert_eq!(discarded().len(), 1, "作废是幂等的");
    }

    #[test]
    fn settings_durable_envelope_detects_tamper_and_quarantines_file() {
        let t = tempfile::tempdir().unwrap();
        {
            let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
            cmd_settings_set(&state, "secure", serde_json::json!("value")).unwrap();
        }
        let path = t.path().join(HOST_SETTINGS_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["payload"][0][1]["user"] = serde_json::json!({"secure": "tampered"});
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        // Development degrades to an empty in-memory document but must quarantine the corrupt
        // persistent copy instead of treating it as a valid first-run state.
        let state = CommandState::with_adapter_config(recovery_cfg(t.path()));
        assert_eq!(cmd_settings_get(&state, "secure").unwrap(), serde_json::Value::Null);
        assert!(std::fs::read_dir(t.path()).unwrap().filter_map(Result::ok).any(|entry| entry
            .file_name()
            .to_string_lossy()
            .contains("host-settings.json.corrupt-")));
    }

    #[test]
    fn settings_persistence_is_off_when_no_data_dir_is_configured() {
        // 无数据目录 = 纯内存（测试 / 底座-only 宿主）：`persist` 是 no-op，
        // 不得凭空在进程 CWD 落文件。
        let state = CommandState::new();
        cmd_settings_set(&state, "k", serde_json::json!(1)).unwrap();
        assert!(state.settings_path.is_none());
    }

    #[test]
    fn settings_bridge_migrates_v1_documents_to_v2() {
        let state = CommandState::new();
        // 旧版宿主配置：裸键平铺（R7 之前的裸 HashMap 形态）。
        host_settings_adopt_legacy(
            &state,
            serde_json::json!({"plugin:p.theme": "dark", "other.key": 7}),
        )
        .unwrap();
        assert_eq!(host_settings_data_version(&state).as_deref(), Some(HOST_SETTINGS_SCHEMA_V1));

        // v1 的裸键在 v2 编码下**读不到**——这正是迁移必须存在的原因，
        // 而不是「迁不迁都一样」。
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::Value::Null,
            "未迁移前不该假装读得出来"
        );

        let steps = host_settings_migrate(&state).unwrap();
        assert_eq!(steps, 1, "v1 → v2 恰好一步");
        assert_eq!(host_settings_data_version(&state).as_deref(), Some(HOST_SETTINGS_SCHEMA_V2));
        assert_eq!(cmd_settings_get(&state, "plugin:p.theme").unwrap(), serde_json::json!("dark"));
        assert_eq!(cmd_settings_get(&state, "other.key").unwrap(), serde_json::json!(7));

        // 幂等：再迁一次是 0 步。
        assert_eq!(host_settings_migrate(&state).unwrap(), 0);
    }

    #[test]
    fn settings_migration_commands_round_trip_through_the_command_layer() {
        // 本用例刻意走**命令层**入口（`cmd_*`，即 `host_settings_adopt_legacy` /
        // `host_settings_migrate` 两个 Tauri 包装器转调的那两个函数），而不是底层的
        // `host_settings_*` pub fn——只测底层等于没验证「宿主有渠道调得到」。
        let state = CommandState::new();
        // 旧版文档：裸键平铺（R7 之前的裸 HashMap 形态），含一个带 `.` 的键。
        cmd_settings_adopt_legacy(
            &state,
            serde_json::json!({"plugin:p.theme": "dark", "lang": "zh-CN"}),
        )
        .unwrap();
        assert_eq!(
            host_settings_data_version(&state).as_deref(),
            Some(HOST_SETTINGS_SCHEMA_V1),
            "接手旧文档必须标注起点版本，否则 migrate 只能拒绝"
        );
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::Value::Null,
            "v1 裸键在 v2 编码下读不到——迁移不是可选项"
        );

        // 迁移：v1 → v2 恰好一步。
        assert_eq!(cmd_settings_migrate(&state).unwrap(), 1, "v1 → v2 恰好一步");
        assert_eq!(host_settings_data_version(&state).as_deref(), Some(HOST_SETTINGS_SCHEMA_V2));
        assert_eq!(
            cmd_settings_get(&state, "plugin:p.theme").unwrap(),
            serde_json::json!("dark"),
            "带 `.` 的键迁移后必须经命令层读得出来"
        );
        assert_eq!(cmd_settings_get(&state, "lang").unwrap(), serde_json::json!("zh-CN"));

        // 幂等：再迁一次是 0 步，数据不动。
        assert_eq!(cmd_settings_migrate(&state).unwrap(), 0);
        assert_eq!(cmd_settings_get(&state, "plugin:p.theme").unwrap(), serde_json::json!("dark"));
    }

    #[test]
    fn settings_adopt_legacy_command_rejects_non_object_documents() {
        let state = CommandState::new();
        let e = cmd_settings_adopt_legacy(&state, serde_json::json!(["a", "b"])).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        // 被拒**不留痕**：既没写用户层，也没标版本。留下一个「有数据无版本」的
        // 半态会让后续 migrate 永远拒绝，比不写更糟。
        assert_eq!(host_settings_data_version(&state), None, "被拒的文档不得留版本标注");
        assert_eq!(cmd_settings_get(&state, "a").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn settings_migration_command_refuses_unlabelled_existing_data() {
        // 命令层的错误口径必须与底层一致（不能把拒绝悄悄吞成 0 步）。
        let state = CommandState::new();
        state.settings.lock().set_layer(
            HOST_SETTINGS_NAMESPACE,
            tauron_settings::LayerKind::User,
            serde_json::json!({"k": 1}),
        );
        let e = cmd_settings_migrate(&state).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("未标注"), "{}", e.message);
    }

    #[test]
    fn settings_adopt_legacy_rejects_non_object_documents() {
        let state = CommandState::new();
        let e = host_settings_adopt_legacy(&state, serde_json::json!("not-an-object")).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
    }

    #[test]
    fn settings_migration_refuses_unlabelled_existing_data() {
        let state = CommandState::new();
        // 直接塞用户层（模拟磁盘上的数据）却不标版本 → 不猜起点，拒绝迁移。
        state.settings.lock().set_layer(
            HOST_SETTINGS_NAMESPACE,
            tauron_settings::LayerKind::User,
            serde_json::json!({"k": 1}),
        );
        let e = host_settings_migrate(&state).unwrap_err();
        assert_eq!(e.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(e.message.contains("未标注"), "{}", e.message);
    }

    #[test]
    fn settings_new_store_has_no_data_version_before_any_write() {
        let state = CommandState::new();
        assert_eq!(host_settings_data_version(&state), None);
    }

    // ────────────────────────────────────────────────────────────
    // R7-3：notify 接 DispatchSink
    // ────────────────────────────────────────────────────────────

    /// 记录型 mock sink：证明 `host_notify` **真的**调了 dispatch。
    struct AdapterMockSink {
        supported: bool,
        fail: bool,
        calls: parking_lot::Mutex<Vec<String>>,
    }

    impl AdapterMockSink {
        fn new(supported: bool, fail: bool) -> Arc<Self> {
            Arc::new(Self { supported, fail, calls: parking_lot::Mutex::new(Vec::new()) })
        }

        fn call_count(&self) -> usize {
            self.calls.lock().len()
        }
    }

    impl tauron_notify::DispatchSink for AdapterMockSink {
        fn send(&self, entry: &NotifyEntry) -> Result<bool, String> {
            self.calls.lock().push(entry.id.clone());
            if self.fail {
                return Err("system 通道炸了".into());
            }
            Ok(self.supported)
        }
    }

    #[test]
    fn cmd_notify_dispatches_through_the_injected_sink() {
        let state = CommandState::new();
        let sink = AdapterMockSink::new(true, false);
        assert!(state.notify_sink.set(sink.clone()).is_ok());

        cmd_notify(&state, "p.audio", "T", "B").unwrap();

        assert_eq!(sink.call_count(), 1, "必须真的调了 sink.send");
        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 1, "通知必须入库");
        let log = snap["dispatchLog"].as_array().unwrap();
        assert_eq!(log.len(), 1, "dispatchLog 必须有记录");
        assert_eq!(log[0]["outcome"], "system");
        assert_eq!(log[0]["entryId"], snap["items"][0]["id"], "日志指的就是这条通知");
        assert_eq!(sink.calls.lock()[0], snap["items"][0]["id"].as_str().unwrap());
    }

    #[test]
    fn cmd_notify_degrades_without_losing_the_notification() {
        let state = CommandState::new();
        let sink = AdapterMockSink::new(true, true); // 致命错误
        assert!(state.notify_sink.set(sink.clone()).is_ok());

        // 关键：系统通知失败时 `host_notify` **不返回错误**。
        cmd_notify(&state, "p.audio", "T", "B").expect("系统通知失败不得让 host_notify 报错");

        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 1, "降级后通知仍须在环形缓冲里");
        assert_eq!(snap["unread"], 1);
        let log = snap["dispatchLog"].as_array().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0]["outcome"], "degraded");
        assert_eq!(sink.call_count(), 1, "降级不重试");
    }

    #[test]
    fn cmd_notify_without_a_sink_logs_nothing() {
        // 没有系统通道 = 没有 dispatch 尝试 = 没有日志（不伪造一条 degraded）。
        let state = CommandState::new();
        assert!(state.notify_sink.get().is_none());
        cmd_notify(&state, "p.audio", "T", "B").unwrap();
        let snap = cmd_notifications_list(&state, None).unwrap();
        assert_eq!(snap["total"], 1);
        assert!(snap["dispatchLog"].as_array().unwrap().is_empty());
    }

    #[test]
    fn notifications_list_keeps_existing_fields_and_adds_dispatch_log() {
        // 既有字段不得改名/删除：前端已消费它们。
        let state = CommandState::new();
        cmd_notify(&state, "p.audio", "T", "B").unwrap();
        let snap = cmd_notifications_list(&state, None).unwrap();
        for field in ["unread", "total", "items"] {
            assert!(snap.get(field).is_some(), "缺少既有字段 {field}");
        }
        assert!(snap.get("dispatchLog").is_some(), "R7-3 新增 dispatchLog");
        for field in ["id", "pluginId", "kind", "title", "message", "ts", "read", "data"] {
            assert!(snap["items"][0].get(field).is_some(), "items 缺字段 {field}");
        }
    }

    #[test]
    fn cmd_contributes_register_and_list() {
        let state = CommandState::new();
        let entry = ContributeEntry {
            plugin_id: "p.example".to_string(),
            kind: "command".to_string(),
            id: "hello".to_string(),
            label: "Hello Command".to_string(),
        };
        cmd_contributes_register(&state, "p.example", entry.clone()).unwrap();
        let list = cmd_contributes_list(&state, None).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "hello");
    }

    #[test]
    fn cmd_contributes_register_duplicate_rejected() {
        let state = CommandState::new();
        let entry = ContributeEntry {
            plugin_id: "p.example".to_string(),
            kind: "command".to_string(),
            id: "hello".to_string(),
            label: "Hello".to_string(),
        };
        cmd_contributes_register(&state, "p.example", entry.clone()).unwrap();
        let result = cmd_contributes_register(&state, "p.example", entry);
        assert!(result.is_err());
    }

    #[test]
    fn cmd_contributes_list_by_kind() {
        let state = CommandState::new();
        cmd_contributes_register(
            &state,
            "p1",
            ContributeEntry {
                plugin_id: "p1".to_string(),
                kind: "command".to_string(),
                id: "cmd1".to_string(),
                label: "Cmd 1".to_string(),
            },
        )
        .unwrap();
        cmd_contributes_register(
            &state,
            "p2",
            ContributeEntry {
                plugin_id: "p2".to_string(),
                kind: "panel".to_string(),
                id: "panel1".to_string(),
                label: "Panel 1".to_string(),
            },
        )
        .unwrap();
        let commands = cmd_contributes_list(&state, Some("command")).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, "cmd1");
    }

    // ── 0.4-W3：贡献对账（声明 vs 注册）───────────────────────────

    /// 造一个「声明了 N 条贡献」的 manifest（`kind:id` 与 `createPlugin` 同口径）。
    fn manifest_with_contributes(
        id: &str,
        contributes: tauron_host::manifest::Contributes,
    ) -> PluginManifest {
        let mut m = test_manifest(id);
        m.contributes = contributes;
        m
    }

    fn declared_commands(ids: &[&str]) -> tauron_host::manifest::Contributes {
        tauron_host::manifest::Contributes {
            commands: ids
                .iter()
                .map(|id| tauron_host::manifest::CommandContribute {
                    id: (*id).to_string(),
                    title: format!("T {id}"),
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn declared_contribute_keys_maps_all_five_kinds() {
        // kind 词汇必须与 `@tauron/app-plugin-sdk` 的 createPlugin 一致，
        // 否则对账会把每一对都报成分叉。
        let c = tauron_host::manifest::Contributes {
            commands: vec![tauron_host::manifest::CommandContribute {
                id: "a.cmd".into(),
                title: "A".into(),
            }],
            menus: vec![tauron_host::manifest::MenuContribute {
                id: "a.menu".into(),
                command: "a.cmd".into(),
            }],
            panels: vec![tauron_host::manifest::PanelContribute {
                id: "a.panel".into(),
                title: "P".into(),
                icon: "assets/p.svg".into(),
            }],
            settings_tabs: vec![tauron_host::manifest::SettingsTabContribute {
                id: "a.tab".into(),
                title: "S".into(),
            }],
            shortcuts: vec![tauron_host::manifest::ShortcutContribute {
                accelerator: "Ctrl+Shift+P".into(),
                command: "a.cmd".into(),
            }],
        };
        let keys = declared_contribute_keys(&c);
        // shortcut 的 id 用 command（与 createPlugin 同口径）。
        assert_eq!(
            keys.into_iter().collect::<Vec<_>>(),
            vec![
                "command:a.cmd",
                "menu:a.menu",
                "panel:a.panel",
                "settings:a.tab",
                "shortcut:a.cmd"
            ]
        );
    }

    #[test]
    fn contributes_reconcile_ok_when_declared_matches_registered() {
        let state = CommandState::new();
        state
            .registry
            .install(
                &empty_index(),
                manifest_with_contributes("p.ok", declared_commands(&["a.cmd"])),
            )
            .unwrap();
        cmd_contributes_register(
            &state,
            "p.ok",
            ContributeEntry {
                plugin_id: "p.ok".into(),
                kind: "command".into(),
                id: "a.cmd".into(),
                label: "A".into(),
            },
        )
        .unwrap();

        let report = cmd_contributes_reconcile(&state, "p.ok").expect("无分叉应成功");
        assert_eq!(report.plugin_id, "p.ok");
        assert_eq!(report.declared, 1);
        assert_eq!(report.registered, 1);
        assert!(report.missing.is_empty());
        assert!(report.extra.is_empty());
    }

    #[test]
    fn contributes_reconcile_detects_missing_registration() {
        // 声明了却没注册 = 用户能看到入口但点了没反应。这是 P0-6 的核心落差。
        let state = CommandState::new();
        state
            .registry
            .install(
                &empty_index(),
                manifest_with_contributes("p.drift", declared_commands(&["a.cmd"])),
            )
            .unwrap();

        let err = cmd_contributes_reconcile(&state, "p.drift").expect_err("应报分叉");
        assert_eq!(err.code, ErrorCode::E_CONTRIBUTES_DRIFT);
        assert!(err.message.contains("command:a.cmd"), "诊断必须点名缺哪条：{}", err.message);
        // 对账是只读的：失败不改变任何状态。
        assert!(cmd_contributes_list(&state, None).unwrap().is_empty());
    }

    #[test]
    fn contributes_reconcile_detects_extra_registration() {
        // 注册了却没声明 = 来源不明的入口。
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("p.extra")).unwrap();
        cmd_contributes_register(
            &state,
            "p.extra",
            ContributeEntry {
                plugin_id: "p.extra".into(),
                kind: "panel".into(),
                id: "sneaky".into(),
                label: "S".into(),
            },
        )
        .unwrap();

        let err = cmd_contributes_reconcile(&state, "p.extra").expect_err("应报分叉");
        assert_eq!(err.code, ErrorCode::E_CONTRIBUTES_DRIFT);
        assert!(err.message.contains("panel:sneaky"));
    }

    #[test]
    fn contributes_reconcile_unknown_plugin_is_not_a_silent_pass() {
        // 不在注册表 = 无从对账。返回空报告会让调用方以为"对过账了，没问题"。
        let state = CommandState::new();
        let err = cmd_contributes_reconcile(&state, "p.missing").expect_err("应拒绝");
        assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
    }

    #[test]
    fn contributes_reconcile_ignores_other_plugins_entries() {
        // 事实面必须只取**该插件自己**的条目：否则别人的贡献会把自己判成分叉。
        let state = CommandState::new();
        state
            .registry
            .install(&empty_index(), manifest_with_contributes("p.mine", declared_commands(&["x"])))
            .unwrap();
        state.registry.install(&empty_index(), test_manifest("p.other")).unwrap();
        cmd_contributes_register(
            &state,
            "p.mine",
            ContributeEntry {
                plugin_id: "p.mine".into(),
                kind: "command".into(),
                id: "x".into(),
                label: "X".into(),
            },
        )
        .unwrap();
        cmd_contributes_register(
            &state,
            "p.other",
            ContributeEntry {
                plugin_id: "p.other".into(),
                kind: "command".into(),
                id: "y".into(),
                label: "Y".into(),
            },
        )
        .unwrap();

        assert!(cmd_contributes_reconcile(&state, "p.mine").is_ok());
    }

    #[test]
    fn contributes_register_is_bounded_and_fails_loudly_at_the_cap() {
        // `host_contributes_register` 是 self 档：插件换个 `id` 就能再插一条，
        // 只按 (plugin_id, id) 去重等于没有上限。到顶后必须**如实拒绝**。
        let mut reg = ContributesRegistry::default();
        for i in 0..MAX_CONTRIBUTES {
            reg.register(ContributeEntry {
                plugin_id: "p.greedy".to_string(),
                kind: "command".to_string(),
                id: format!("c{i}"),
                label: "x".to_string(),
            })
            .expect("未到上限前必须成功");
        }
        assert_eq!(reg.count(), MAX_CONTRIBUTES);

        let err = reg
            .register(ContributeEntry {
                plugin_id: "p.greedy".to_string(),
                kind: "command".to_string(),
                id: "overflow".to_string(),
                label: "x".to_string(),
            })
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::E_REGISTRY_FULL);
        assert_eq!(reg.count(), MAX_CONTRIBUTES, "拒绝路径不得改变容量");
    }

    #[test]
    fn memory_window_sink_traces_are_a_ring_not_an_unbounded_log() {
        // 未注入平台 sink 的宿主用的就是这个实现：每次窗口操作永久追加一条，
        // 长跑客户端内存只增不减。环形后必须稳定在 MAX_WINDOW_OPS。
        let sink = MemoryWindowSink::new();
        for _ in 0..(MAX_WINDOW_OPS + 50) {
            sink.minimize("plugin-p").unwrap();
        }
        let ops = sink.ops();
        assert_eq!(ops.len(), MAX_WINDOW_OPS, "到顶后丢最旧，不是无限增长");
        assert!(sink.recorded("minimize"));
    }

    #[test]
    fn cmd_notify_records_notification() {
        let state = CommandState::new();
        cmd_notify(&state, "p.audio", "Title", "Body").unwrap();
        let notifications = state.notifications.lock();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].plugin_id, "p.audio");
        assert_eq!(notifications[0].title, "Title");
    }

    #[test]
    fn notify_text_is_byte_bounded_with_zero_side_effects() {
        // §33 R3-5：环形缓冲的条数上限挡不住"一条巨型正文"，必须另有字节预算。
        let state = CommandState::new();
        let err = cmd_notify(&state, "p.audio", "T", &"x".repeat(MAX_NOTIFY_BODY_BYTES + 1))
            .expect_err("oversized body must be rejected");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("正文"), "{}", err.message);

        let err = cmd_notify(&state, "p.audio", &"t".repeat(MAX_NOTIFY_TITLE_BYTES + 1), "B")
            .expect_err("oversized title must be rejected");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("标题"), "{}", err.message);

        assert!(state.notifications.lock().is_empty(), "拒绝路径必须零副作用");
        assert_eq!(state.notify_store.lock().len(), 0);

        cmd_notify(&state, "p.audio", "T", &"x".repeat(MAX_NOTIFY_BODY_BYTES)).unwrap();
        assert_eq!(state.notifications.lock().len(), 1, "预算内边界值必须可写");
    }

    #[test]
    fn notify_legacy_log_is_bounded_and_keeps_newest() {
        // 兼容日志只写不读：必须不能随通知次数无限增长。
        let state = CommandState::new();
        for i in 0..(MAX_NOTIFICATION_LOG + 5) {
            cmd_notify(&state, "p.audio", &format!("T{i}"), "Body").unwrap();
        }
        let notifications = state.notifications.lock();
        assert_eq!(notifications.len(), MAX_NOTIFICATION_LOG, "日志应被裁剪到上限");
        // 保留的是**最新**的一批（丢掉最早 5 条）。
        assert_eq!(notifications[0].title, "T5");
        assert_eq!(
            notifications[MAX_NOTIFICATION_LOG - 1].title,
            format!("T{}", MAX_NOTIFICATION_LOG + 4)
        );
    }

    #[test]
    fn notify_record_serializes_as_camel_case() {
        // 潜在线上形态必须与 TS `NotificationRecord`（pluginId…）一致：
        // 该结构当前无线上读取方，但字段名不一致会让未来的接线静默读到 undefined。
        let rec = NotificationRecord {
            plugin_id: "p.audio".to_string(),
            title: "T".to_string(),
            body: "B".to_string(),
            timestamp: 7,
        };
        let v = serde_json::to_value(&rec).unwrap();
        assert!(v.get("pluginId").is_some(), "应为 camelCase：{v}");
        assert!(v.get("plugin_id").is_none(), "不应保留蛇形键：{v}");
    }

    #[test]
    fn guard_catches_panic() {
        // 验证所有命令入口都经过 guard()。
        // 模拟一个会导致 panic 的调用（registry 内部 panic 应被捕获）。
        // 这里用 lifecycle_report 测试：对一个不存在的插件，
        // registry 应返回 E_UNKNOWN_PLUGIN 而非 panic。
        let state = CommandState::new();
        let result = cmd_lifecycle_report(&state, "plugin-test.nonexistent", None, Event::Attach);
        assert!(result.is_err());
        // 错误码不应是 E_HOST_PANIC（因为逻辑正确处理了未知插件）
        assert_ne!(result.unwrap_err().code, ErrorCode::E_HOST_PANIC);
    }

    #[test]
    fn command_state_default() {
        let state = CommandState::default();
        assert!(cmd_registry_list_all(&state).is_ok());
    }

    #[test]
    fn concurrent_registry_access() {
        let state = Arc::new(CommandState::new());
        let mut handles = Vec::new();

        for _ in 0..10 {
            let state = Arc::clone(&state);
            handles.push(std::thread::spawn(move || cmd_registry_list_all(&state).is_ok()));
        }

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|&r| r), "所有并发读取应成功");
    }

    #[test]
    fn concurrent_bus_access() {
        let state = Arc::new(CommandState::new());
        let mut handles = Vec::new();

        // 先声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl { topic: "plugin:p.ready".to_string(), public: true }];
            bus.declare_topics("p", &decls).unwrap();
        }

        for i in 0..10 {
            let state = Arc::clone(&state);
            handles.push(std::thread::spawn(move || {
                cmd_events_publish(&state, "p", "plugin:p.ready", serde_json::json!({"seq": i}))
                    .is_ok()
            }));
        }

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|&r| r), "所有并发发布应成功");
    }

    #[test]
    fn concurrent_subscribe_publish() {
        let state = Arc::new(CommandState::new());

        // 声明 topic
        {
            let bus = state.bus.lock();
            let decls = vec![EventDecl { topic: "plugin:p.ready".to_string(), public: true }];
            bus.declare_topics("p", &decls).unwrap();
        }

        let mut handles = Vec::new();
        for i in 0..5 {
            let state = Arc::clone(&state);
            let id = i;
            handles.push(std::thread::spawn(move || {
                let _sub = cmd_events_subscribe(&state, &format!("c{id}"), "w1", "plugin:p.ready");
                cmd_events_publish(&state, "p", "plugin:p.ready", serde_json::json!({"seq": id}))
            }));
        }

        let results: Vec<Result<_, _>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(results.iter().all(|r| r.is_ok()), "所有并发操作应成功");
    }

    #[test]
    fn registry_install_and_list() {
        let state = CommandState::new();
        // 安装一个插件
        let manifest = test_manifest("p.test");
        let index = empty_index();
        let id = state.registry.install(&index, manifest).unwrap();
        // 列表应包含该插件
        let list = cmd_registry_list_all(&state).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id.as_str());
    }

    #[test]
    fn registry_install_duplicate_rejected() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.dup");
        state.registry.install(&index, manifest.clone()).unwrap();
        let result = state.registry.install(&index, manifest);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, ErrorCode::E_PLUGIN_EXISTS);
    }

    // ── 装配期安装入口（`install_plugin_from_json`）─────────────────────────
    //
    // 这组用例的意义：`Registry::install` 此前**只有测试调用**——注册表在生产上
    // 永远是空的，`host_plugin_call` / 流式 / 生命周期 / `host_runtime_spawn`
    // 整条链在真机上没有起点。`install_plugin_from_json` 是它的生产入口。

    #[test]
    fn install_plugin_from_json_installs_and_becomes_visible() {
        let state = CommandState::new();
        let json = serde_json::to_string(&test_manifest("p.assemble")).unwrap();

        let id = install_plugin_from_json(&state, &json).expect("合法 manifest 应装得上");
        assert_eq!(id.as_str(), "p.assemble");

        // 可见性：进列表（UI 看得到）。
        let list = cmd_registry_list_all(&state).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "p.assemble");

        // 生命周期：落 INSTALLED（不是 INSTALL_FAILED）。
        let entry = state.registry.find(&id).unwrap();
        assert_eq!(entry.state.state, tauron_host::lifecycle::State::Installed);

        // 装完之后管理面（enable）可用——证明这条链真的有下游。
        let out = cmd_registry_admin(&state, "p.assemble", RegistryAdminOp::Enable).unwrap();
        assert_eq!(out.to, tauron_host::lifecycle::State::Enabled);
    }

    #[test]
    fn resource_stats_report_live_usage_and_are_main_window_only() {
        let state = CommandState::new();
        let index = empty_index();
        let id = state.registry.install(&index, test_manifest("p.stats")).unwrap();
        state.registry.admin_op(&id, RegistryAdminOp::Enable).unwrap();
        let call = state.registry.call_begin(&id, "stats.probe", serde_json::json!({})).unwrap();
        state
            .registry
            .stream_bind(&call.call_id, "p.stats", Arc::new(tauron_host::stream::NullSink))
            .unwrap();
        state.registry.stream_open(&call.call_id, "p.stats").unwrap();
        state
            .bus
            .lock()
            .declare_topics(
                "p.stats",
                &[tauron_host::manifest::EventDecl {
                    topic: "p.stats.events".into(),
                    public: true,
                }],
            )
            .unwrap();
        state.bus.lock().subscribe("p.stats", "main", "p.stats.events").unwrap();
        cmd_notify(&state, "p.stats", "stats", "visible use").unwrap();

        let snapshot = cmd_resource_stats_as(&Caller::MainWindow, &state).unwrap();
        assert_eq!(snapshot["global"]["pendingCalls"]["used"], 1);
        assert_eq!(snapshot["global"]["streams"]["used"], 1);
        assert_eq!(snapshot["global"]["subscriptions"]["used"], 1);
        assert_eq!(snapshot["global"]["notifications"]["used"], 1);
        assert_eq!(snapshot["global"]["notifications"]["evictedTotal"], 0);
        assert_eq!(snapshot["faults"]["settings"]["state"], "ready");
        assert!(snapshot["faults"]["settings"]["generation"].as_u64().unwrap() >= 1);
        assert_eq!(snapshot["perPlugin"]["plugins"][0]["pluginId"], "p.stats");
        assert_eq!(snapshot["perPlugin"]["plugins"][0]["pendingCalls"], 1);
        assert_eq!(snapshot["perPlugin"]["plugins"][0]["streams"], 1);
        assert_eq!(snapshot["perPlugin"]["plugins"][0]["subscriptions"], 1);
        assert_eq!(snapshot["perPlugin"]["plugins"][0]["notifications"], 1);
        assert_eq!(snapshot["perPlugin"]["plugins"][0]["notificationEvictions"], 0);

        // V7 §9 leak gate：代际台账必须**跨线可见**，且读的就是注册表里那一份。
        let generations = &snapshot["global"]["generations"];
        assert!(generations.is_object(), "缺 generations 读数");
        assert_eq!(generations["trackedResources"], 0);
        assert_eq!(generations["liveLeases"], 0);

        // 起一条运行期租约 → 读数跟着涨；回收 → 读数必须回到零（abandoned generation 不许累积）。
        let (handle, _started) = state.registry.runtime_ensure_lease(&id, || Ok(9100)).unwrap();
        let issued = snapshot["global"]["generations"]["generationsIssued"].as_u64().unwrap();
        let while_live = cmd_resource_stats(&state).unwrap();
        assert_eq!(while_live["global"]["generations"]["trackedResources"], 1);
        assert_eq!(while_live["global"]["generations"]["liveLeases"], 1);
        assert_eq!(
            while_live["global"]["generations"]["generationsIssued"],
            issued + 1,
            "号源只增：回收留痕与代际发号不得互相抵消"
        );

        assert!(state.registry.runtime_remove(&id).is_some());
        let after_reclaim = cmd_resource_stats(&state).unwrap();
        assert_eq!(after_reclaim["global"]["generations"]["trackedResources"], 0);
        assert_eq!(after_reclaim["global"]["generations"]["liveLeases"], 0);
        assert!(
            state.registry.runtime_lease(&handle.lease).is_err(),
            "回收后旧租约必须失效（跟踪摘除不得把旧句柄放回场）"
        );

        let denied = cmd_resource_stats_as(&plugin_caller("p.stats"), &state).unwrap_err();
        assert_eq!(denied.code, ErrorCode::E_AUTH_DENIED);
    }

    /// V7 §7 Process 行的 retry 腿必须**真的接在命令上**：`host_resource_stats` 一次
    /// 读取推进一轮有界重试，并把重试留痕与终态证据带回线上（只留日志 = 查不到）。
    #[test]
    fn resource_stats_drives_reap_retry_and_publishes_the_evidence() {
        /// 永远杀不掉的终止器：逼着回收走完「重试到上限 → 固化证据」这条链。
        #[derive(Default)]
        struct AlwaysFailing(parking_lot::Mutex<Vec<u32>>);
        impl LeaseReaper for AlwaysFailing {
            fn kill(&self, pid: u32) -> Result<ReapOutcome, String> {
                self.0.lock().push(pid);
                Err("拒绝访问".into())
            }
        }

        let state = CommandState::new();
        let index = empty_index();
        let id = state.registry.install(&index, test_manifest("p.reap")).unwrap();
        state.registry.admin_op(&id, RegistryAdminOp::Enable).unwrap();
        let reaper = Arc::new(AlwaysFailing::default());
        state.registry.set_lease_reaper(reaper.clone());
        state.registry.runtime_ensure_lease(&id, || Ok(4321)).expect("租约必须挂得上");
        assert!(state.registry.runtime_remove(&id).is_some(), "回收本身必须成功");

        // 第一次读取：首试失败已记账，这一轮再试一次仍未杀掉。
        let snapshot = cmd_resource_stats_as(&Caller::MainWindow, &state).unwrap();
        let reap = &snapshot["global"]["reap"];
        assert_eq!(reap["attempts"], 2, "诊断读取本身就是重试驱动");
        assert_eq!(reap["retries"], 1);
        assert_eq!(reap["failures"], 2);
        assert_eq!(reap["pending"], 1, "未达上限前必须仍在队列里");
        assert_eq!(reap["terminal"], 0);

        // 第二次读取：到尝试上限 ⇒ 出队并固化成查得动的证据（pid / 插件 / 原因）。
        let snapshot = cmd_resource_stats_as(&Caller::MainWindow, &state).unwrap();
        let reap = &snapshot["global"]["reap"];
        assert_eq!(reap["pending"], 0);
        assert_eq!(reap["terminal"], 1);
        assert_eq!(reap["terminalRecords"][0]["pid"], 4321);
        assert_eq!(reap["terminalRecords"][0]["pluginId"], "p.reap");
        assert_eq!(reap["terminalRecords"][0]["attempts"], 3, "含首试共三次");
        assert!(reap["terminalRecords"][0]["reason"].as_str().unwrap_or("").contains("拒绝访问"));

        // 有界性：队列已空，之后再怎么读都不该再打进程。
        let calls = reaper.0.lock().len();
        assert_eq!(calls, 3);
        for _ in 0..5 {
            cmd_resource_stats_as(&Caller::MainWindow, &state).unwrap();
        }
        assert_eq!(reaper.0.lock().len(), calls, "空队列不得被读命令反复驱动");
        assert_eq!(snapshot["global"]["generations"]["trackedResources"], 0);

        let denied = cmd_resource_stats_as(&plugin_caller("p.reap"), &state).unwrap_err();
        assert_eq!(denied.code, ErrorCode::E_AUTH_DENIED);
    }

    #[test]
    fn production_doctor_is_main_window_only_and_mirrors_readiness() {
        let dev = SubstrateState::with_adapter_config(&AdapterConfig::default());
        let report = cmd_production_doctor_as(&Caller::MainWindow, &dev).unwrap();
        assert_eq!(report.deployment_mode, tauron_host::DeploymentMode::Development);
        assert!(!report.production_safe, "Development 永远不得声称 production-safe");
        assert!(report.checks.iter().any(|c| c.id == "caller-identity" && !c.pass));

        let denied = cmd_production_doctor_as(&plugin_caller("p.doctor"), &dev).unwrap_err();
        assert_eq!(denied.code, ErrorCode::E_AUTH_DENIED);

        let temp = tempfile::tempdir().unwrap();
        let mut cfg = AdapterConfig::production();
        cfg.origin_allowlist = vec!["tauri://localhost".into()];
        cfg.recovery_data_dir = Some(temp.path().to_path_buf());
        cfg.admin_audit_dir = Some(temp.path().join("audit"));
        #[cfg(feature = "plugin-install")]
        {
            // 与 production_readiness 同源：plugin-install 打开时启动门还要求
            // 安装信任材料与可信时间，否则 with_adapter_config 直接 panic。
            let mut keys = std::collections::BTreeMap::new();
            keys.insert("fixture-key".to_string(), vec![0x4b; 32]);
            cfg = cfg
                .with_plugin_install(temp.path().join("plugins"), keys, vec![0x5a; 32])
                .with_trusted_time_provider(Arc::new(tauron_host::SystemTimeProvider::new(
                    tauron_host::TimeTrustState::Trusted,
                )));
        }
        let prod = SubstrateState::with_adapter_config(&cfg);
        let report = cmd_production_doctor_as(&Caller::MainWindow, &prod).unwrap();
        assert!(report.production_safe, "checks={:?}", report.checks);
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["deploymentMode"], "production");
        assert_eq!(wire["productionSafe"], true);
        assert_eq!(wire["checks"][0]["requiredInProduction"], true);
    }

    #[test]
    fn install_plugin_from_json_records_install_failed_when_manifest_malformed() {
        let state = CommandState::new();
        // id 合法、但 `version` 不是 semver → 解析失败。
        let bad = r#"{"id":"p.broken","name":"Broken","version":"not-a-semver"}"#;

        let err = install_plugin_from_json(&state, bad).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(
            err.message.contains("INSTALL_FAILED"),
            "错误信息应告知已落 INSTALL_FAILED，实际：{}",
            err.message
        );

        // 关键：插件**没有凭空消失**——UI 能看到一条失败记录并可卸载。
        let list = cmd_registry_list_all(&state).unwrap();
        assert_eq!(list.len(), 1, "解析失败也必须留痕");
        let entry = state.registry.find(&PluginId::new("p.broken").unwrap()).unwrap();
        assert_eq!(entry.state.state, tauron_host::lifecycle::State::InstallFailed);
        assert!(
            entry.state.last_reason.as_deref().unwrap_or("").contains("解析失败"),
            "失败原因要落在 last_reason 上"
        );
    }

    #[test]
    fn install_plugin_from_json_without_recoverable_id_does_not_register() {
        let state = CommandState::new();
        let err = install_plugin_from_json(&state, r#"{"nope":1}"#).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("无法从 JSON 中辨认插件 id"));
        assert!(
            cmd_registry_list_all(&state).unwrap().is_empty(),
            "连 id 都认不出来时不该留下无名条目"
        );
    }

    #[test]
    fn install_plugin_from_json_rejects_unknown_field() {
        // `deny_unknown_fields`：拼写漂移必须失败，而不是静默忽略。
        let state = CommandState::new();
        let mut value = serde_json::to_value(test_manifest("p.typo")).unwrap();
        value.as_object_mut().unwrap().insert("pluginType".to_string(), serde_json::json!("js"));
        let err = install_plugin_from_json(&state, &value.to_string()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
    }

    // ── 配置化装配（`ClientConfig` 的生产消费点）───────────────────────────

    /// `ClientConfig` → `AdapterConfig` 的映射必须真的生效。
    ///
    /// 回归保护：此前 `ClientConfig` 只被 `pub use` 再导出、没有生产消费方——
    /// 配置里写 `plugin_filter` 也不会有任何效果（"配置化选择加载"是空的）。
    #[test]
    fn adapter_config_from_client_config_maps_registry_and_data_dir() {
        let cfg = ClientConfig::from_json(
            r#"{
                "registry": {
                    "plugin_filter": { "allow": ["com.example.formatter"] },
                    "max_plugins": 3
                },
                "data_dir": "/tmp/tauron-data",
                "env_overrides": { "OC_PROFILE": "prod" }
            }"#,
        )
        .unwrap();

        let adapter = AdapterConfig::from_client_config(&cfg, None);

        assert_eq!(
            adapter.plugin_env_overrides.get("OC_PROFILE").map(String::as_str),
            Some("prod"),
            "`env_overrides` 必须映射进装配配置——它是 `SpawnConfig::env` 的唯一来源，\
             漏在这里就等于配置无效"
        );
        let registry = adapter.registry.expect("registry 必须被映射");
        assert_eq!(registry.max_plugins, 3, "max_plugins 覆盖必须生效");
        let filter = registry.plugin_filter.expect("过滤器必须被映射");
        assert_eq!(filter.allow, vec!["com.example.formatter".to_string()]);
        assert_eq!(
            adapter.recovery_data_dir.as_deref(),
            Some(std::path::Path::new("/tmp/tauron-data")),
            "data_dir 必须映射到恢复数据目录（否则配置无效）"
        );

        // 空配置 → 全部回落默认值，且不 panic（"缺省即安全"）。
        let fallback = AdapterConfig::from_client_config(
            &ClientConfig::default(),
            Some(std::path::PathBuf::from("/fallback")),
        );
        assert_eq!(
            fallback.recovery_data_dir.as_deref(),
            Some(std::path::Path::new("/fallback")),
            "未配置 data_dir 时用调用方给的兜底目录"
        );
        assert_eq!(
            fallback.registry.expect("registry 恒有值").max_plugins,
            tauron_host::registry::default_config().max_plugins
        );
    }

    /// 配置里的 `plugin_filter` 真的会拦住被排除的插件（端到端：配置 → 安装）。
    #[test]
    fn client_config_filter_actually_blocks_install() {
        let cfg = ClientConfig::from_json(
            r#"{ "registry": { "plugin_filter": { "deny": ["com.blocked"] } } }"#,
        )
        .unwrap();
        let state =
            CommandState::with_adapter_config(AdapterConfig::from_client_config(&cfg, None));

        // 被 deny 的插件：`install` 应返回 E_PLUGIN_FILTERED，且不入库。
        let mut blocked = test_manifest("com.blocked");
        blocked.id = PluginId::new("com.blocked").unwrap();
        let json = serde_json::to_string(&blocked).unwrap();
        let err = install_plugin_from_json(&state, &json).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_FILTERED);
        assert!(cmd_registry_list_all(&state).unwrap().is_empty());

        // 未被过滤的插件照常装上（证明拦住它的是过滤器，不是别的原因）。
        let ok = serde_json::to_string(&test_manifest("com.allowed")).unwrap();
        assert!(install_plugin_from_json(&state, &ok).is_ok());
    }

    #[test]
    fn durable_data_dir_has_exactly_one_process_writer() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = AdapterConfig {
            recovery_data_dir: Some(dir.path().to_path_buf()),
            ..AdapterConfig::default()
        };

        let first = SubstrateState::with_adapter_config(&cfg);
        assert!(first.storage_writer_lease.is_some());

        let second = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            SubstrateState::with_adapter_config(&cfg)
        }));
        assert!(second.is_err(), "same durable data dir must reject a second writer");

        drop(first);
        let third = SubstrateState::with_adapter_config(&cfg);
        assert!(third.storage_writer_lease.is_some(), "lease must recover after owner drop");
    }

    #[test]
    fn embedded_permission_index_parses_and_is_non_empty() {
        // 内嵌词表是 `install` 的必填参数；它坏了整条安装链就断了。
        let idx = tauron_host::manifest::embedded_permission_index();
        assert_eq!(idx.version, 1);
        assert!(!idx.entries.is_empty(), "词表不该为空");
    }

    #[test]
    fn registry_admin_enable_disable() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.admin");
        state.registry.install(&index, manifest).unwrap();

        // 启用
        let enable = cmd_registry_admin(&state, "p.admin", RegistryAdminOp::Enable);
        assert!(enable.is_ok());

        // 禁用
        let disable = cmd_registry_admin(&state, "p.admin", RegistryAdminOp::Disable);
        assert!(disable.is_ok());
    }

    #[test]
    fn registry_install_then_lifecycle() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.lifecycle");
        state.registry.install(&index, manifest).unwrap();

        // 启用
        let enable = cmd_registry_admin(&state, "p.lifecycle", RegistryAdminOp::Enable);
        assert!(enable.is_ok());
        let outcome = enable.unwrap();
        assert_eq!(outcome.to, tauron_host::lifecycle::State::Enabled);

        // 通过 lifecycle_report 上报 Attach
        let attach = cmd_lifecycle_report(&state, "plugin-p.lifecycle", None, Event::Attach);
        assert!(attach.is_ok());
        let outcome = attach.unwrap();
        assert_eq!(outcome.to, tauron_host::lifecycle::State::Running);
    }

    #[test]
    fn registry_call_begin_and_end() {
        let state = CommandState::new();
        let index = empty_index();
        let manifest = test_manifest("p.call");
        state.registry.install(&index, manifest).unwrap();
        cmd_registry_admin(&state, "p.call", RegistryAdminOp::Enable).unwrap();
        cmd_lifecycle_report(&state, "plugin-p.call", None, Event::Attach).unwrap();

        // 发起调用
        let call = cmd_plugin_call(
            &state,
            "plugin-p.call",
            None,
            "test_method",
            serde_json::json!({"x": 1}),
        );
        assert!(call.is_ok());
        let pending = call.unwrap();
        assert!(!pending.call_id.is_empty());
        assert_eq!(pending.seq, 1);

        // 结束调用
        let end = cmd_call_end(&state, &Caller::Plugin("p.call".to_string()), &pending.call_id);
        assert!(end.is_ok());

        // 再次结束 → 应报错
        let end2 = cmd_call_end(&state, &Caller::Plugin("p.call".to_string()), &pending.call_id);
        assert!(end2.is_err());
        assert_eq!(end2.unwrap_err().code, ErrorCode::E_CALL_NOT_FOUND);
    }

    #[test]
    fn registry_gc_expired() {
        let config = RegistryConfig {
            max_plugins: 8,
            max_active_identities: 8,
            max_pending_calls: 10,
            pending_ttl: std::time::Duration::from_nanos(1),
            plugin_filter: None,
        };
        let state = CommandState::with_config(config);
        let index = empty_index();
        let manifest = test_manifest("p.gc");
        state.registry.install(&index, manifest).unwrap();
        cmd_registry_admin(&state, "p.gc", RegistryAdminOp::Enable).unwrap();
        cmd_lifecycle_report(&state, "plugin-p.gc", None, Event::Attach).unwrap();

        // 发起多个调用
        for _ in 0..5 {
            cmd_plugin_call(&state, "plugin-p.gc", None, "gc_test", serde_json::Value::Null)
                .unwrap();
        }

        // 等待 TTL 过期
        std::thread::sleep(std::time::Duration::from_millis(5));

        // GC 应回收全部过期条目
        let gc_count = state.registry.gc_expired();
        assert_eq!(gc_count, 5, "应回收 5 个过期条目");
    }

    // ── 窗口管理命令测试（P1-1）──

    /// 主窗 label（与 `tauri.conf.json` 的窗口 label 约定一致：缺省主窗 = `main`）。
    const MAIN: &str = "main";

    #[test]
    fn cmd_window_minimize_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_minimize(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_maximize_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_maximize(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_restore_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_restore(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_close_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_close(&state, MAIN).is_ok());
    }

    #[test]
    fn cmd_window_quit_succeeds() {
        let state = CommandState::new();
        assert!(cmd_window_quit(&state).is_ok());
    }

    // ── 壳扩展命令测试（P2 断链修复）──

    #[test]
    fn cmd_window_set_position_records_rect() {
        let state = CommandState::new();
        cmd_window_set_position(&state, MAIN, 120, 80).unwrap();
        let rect = state.shell_ext.lock().window_rect;
        assert_eq!((rect.0, rect.1), (120, 80));
    }

    #[test]
    fn cmd_window_set_size_records_rect() {
        let state = CommandState::new();
        cmd_window_set_position(&state, MAIN, 10, 20).unwrap();
        cmd_window_set_size(&state, MAIN, 1280, 720).unwrap();
        let rect = state.shell_ext.lock().window_rect;
        assert_eq!(rect, (10, 20, 1280, 720));
    }

    #[test]
    fn cmd_clipboard_roundtrip() {
        let state = CommandState::new();
        assert_eq!(cmd_clipboard_read(&state).unwrap().value, "");
        let write = cmd_clipboard_write(&state, "hello tauron".to_string()).unwrap();
        assert!(!write.supported);
        assert_eq!(write.fallback.as_deref(), Some("in-process-buffer"));
        let read = cmd_clipboard_read(&state).unwrap();
        assert!(!read.supported);
        assert_eq!(read.fallback, "in-process-buffer");
        assert_eq!(read.value, "hello tauron");
    }

    #[test]
    fn cmd_deep_link_register_records_protocol() {
        let state = CommandState::new();
        assert!(state.shell_ext.lock().deep_link_protocol.is_none());
        let result = cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
        assert!(matches!(result, ProviderResult::Unsupported(_)));
        assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("tauron"));
    }

    #[test]
    fn deep_link_delivered_reaches_subscriber_queue() {
        // 全链路：注册协议 → 前端订阅 topic → OS 回调投递 → drain 取帧
        let state = CommandState::new();
        cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
        cmd_events_subscribe(&state, "p1", "w1", "deep-link").unwrap();

        deep_link_delivered(&state, "tauron://plugin/open?x=1").unwrap();

        let frames = cmd_events_drain(&state, "p1", "event").unwrap();
        assert_eq!(frames.len(), 1, "deep-link 帧必须抵达订阅者队列");
        let payload = &frames[0].payload;
        assert_eq!(payload["url"], serde_json::json!("tauron://plugin/open?x=1"));
        assert_eq!(payload["protocol"], serde_json::json!("tauron"));
    }

    #[test]
    fn cmd_market_download_and_install_track_state() {
        let state = CommandState::new();
        let dl = cmd_market_download(&state, Some("2.0.0")).unwrap();
        assert!(dl.ok);
        // R8：模拟这件事是**线字段**，前端据此判断"这不是真实下载"。
        assert!(dl.simulated);
        assert_eq!(dl.version.as_deref(), Some("2.0.0"));
        assert!(dl.reason.as_deref().is_some_and(|r| r.contains("没有")));
        assert_eq!(
            serde_json::to_value(&dl).unwrap(),
            serde_json::json!({
                "ok": true,
                "simulated": true,
                "version": "2.0.0",
                "reason": dl.reason.clone(),
            }),
            "线形字段名/数量必须与 TS 同步（camelCase、无额外字段）"
        );
        assert_eq!(state.shell_ext.lock().update_state.as_deref(), Some("downloaded:2.0.0"));
        let inst = cmd_market_install(&state, Some("2.0.0")).unwrap();
        assert!(inst.ok && inst.simulated);
        assert_eq!(serde_json::to_value(&inst).unwrap()["simulated"], serde_json::json!(true));
        assert_eq!(state.shell_ext.lock().update_state.as_deref(), Some("installed:2.0.0"));
        // 缺省 version 时线形仍是同一形状（version = null），不是换一种形状。
        let none = cmd_market_download(&state, None).unwrap();
        assert_eq!(none.version, None);
        assert_eq!(serde_json::to_value(&none).unwrap()["version"], serde_json::Value::Null);
    }

    #[test]
    fn cmd_dialog_commands_return_cancelled_defaults() {
        let state = CommandState::new();
        // 无原生 UI 必须报告 Unsupported，不能伪装成取消或成功。
        for result in [
            serde_json::to_value(cmd_dialog_open(&state, false, false).unwrap()).unwrap(),
            serde_json::to_value(cmd_dialog_save(&state, None).unwrap()).unwrap(),
            serde_json::to_value(cmd_dialog_confirm(&state, "t", "m").unwrap()).unwrap(),
            serde_json::to_value(cmd_dialog_message(&state, "t", "m", None).unwrap()).unwrap(),
        ] {
            assert_eq!(result["supported"], false);
            assert!(result["reason"].as_str().is_some_and(|s| !s.is_empty()));
        }
    }

    #[test]
    fn cmd_dialog_message_rejects_kind_outside_the_closed_vocabulary() {
        let state = CommandState::new();
        let err = cmd_dialog_message(&state, "t", "m", Some("question"))
            .expect_err("表外 kind 必须拒绝（R8 之前它被整个忽略）");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        // 闭集内的三个值 + 缺省都必须通过。
        for kind in [None, Some("info"), Some("warning"), Some("error")] {
            assert!(cmd_dialog_message(&state, "t", "m", kind).is_ok(), "{kind:?}");
        }
    }

    #[test]
    fn cmd_deep_link_register_rejects_empty_protocol() {
        let state = CommandState::new();
        let err =
            cmd_deep_link_register(&state, "  ".to_string()).expect_err("空协议不是可注册的协议名");
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(state.shell_ext.lock().deep_link_protocol.is_none());
    }

    /// 造出「已声明 topic + 已订阅 + 有队列」的插件状态。
    fn seed_bus(state: &CommandState, id: &str, topic: &str) {
        state
            .bus
            .lock()
            .declare_topics(id, &[EventDecl { topic: topic.to_string(), public: true }])
            .unwrap();
        state.bus.lock().subscribe(id, &format!("plugin-{id}"), topic).unwrap();
        state.bus.lock().publish(
            id,
            topic,
            serde_json::Value::Null,
            tauron_host::eventbus::ChannelKind::Event,
        );
    }

    /// 卸载必须回收事件总线残留（§8-3 零悬挂）。
    ///
    /// 订阅/反向索引/队列/声明都以插件 id 为键，注册表条目移除后再无引用者：
    /// 不回收就会留到进程结束，卸载→重装循环持续累积。
    #[test]
    fn uninstall_disposes_event_bus_residue() {
        let state = CommandState::new();
        let topic = "plugin:com.a:x";
        seed_bus(&state, "com.a", topic);
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 前置：残留确实存在（否则本用例没有判别力）。
        {
            let bus = state.bus.lock();
            assert!(bus.has_hanging_subscriptions("com.a"), "前置：应有订阅");
            assert!(bus.has_hanging_queues("com.a"), "前置：应有队列");
            assert!(bus.topic_meta(topic).is_some(), "前置：应有声明");
        }

        let out = cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();
        assert!(!out.illegal, "Installed → Uninstalled 应为合法迁移");

        let bus = state.bus.lock();
        assert!(!bus.has_hanging_subscriptions("com.a"), "卸载后不得残留订阅");
        assert!(!bus.has_hanging_queues("com.a"), "卸载后不得残留队列");
        assert!(bus.topic_meta(topic).is_none(), "卸载后 topic 声明应被回收");
    }

    #[test]
    fn uninstall_recycles_every_per_plugin_side_state() {
        let state = CommandState::new();
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        // 三个旁路状态先种上数据（否则本用例没有判别力）。
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "A")]), Some("com.a")).unwrap();
        cmd_i18n_load(&state, "zh-CN", entries(&[("title", "B")]), Some("com.b")).unwrap();
        cmd_i18n_set_locale(&state, "zh-CN").unwrap();
        cmd_contributes_register(
            &state,
            "com.a",
            ContributeEntry {
                plugin_id: "com.a".to_string(),
                kind: "menu".to_string(),
                id: "a.menu".to_string(),
                label: "A".to_string(),
            },
        )
        .unwrap();
        state.recovery.lock().register_plugin("com.a");
        // 进程侧崩溃窗口（同样以插件 id 为键）：种满 4 次（缺省上限 3 次/5min），
        // 让 `is_crash_exceeded` 真的成立——否则本断言没有判别力。
        for _ in 0..4 {
            state.proc_runtime.record_crash("com.a");
        }
        assert!(state.proc_runtime.is_crash_exceeded("com.a"), "前置：崩溃预算必须已超限");
        // 多选择器分组登记（以订阅者为键）——卸载必须连它一起回收。
        state.subscription_groups.lock().insert(
            "grp:a".to_string(),
            GroupSubscription {
                subscriber: "com.a".into(),
                tokens: vec!["t.a1".into(), "t.a2".into()],
            },
        );
        state.subscription_groups.lock().insert(
            "grp:b".to_string(),
            GroupSubscription { subscriber: "com.b".into(), tokens: vec!["t.b1".into()] },
        );

        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Uninstall).unwrap();

        // 分组登记：被卸载插件的回收，其它订阅者的保留。
        {
            let groups = state.subscription_groups.lock();
            assert!(
                !groups.contains_key("grp:a"),
                "卸载必须回收该插件的分组登记（否则死条目累积）"
            );
            assert!(groups.contains_key("grp:b"), "不得连坐回收其它订阅者");
        }

        // i18n：被卸载插件的命名空间清空，其他插件的文案不受影响。
        assert_eq!(cmd_i18n_t(&state, "plugin:com.a.oc.title").unwrap(), "plugin:com.a.oc.title");
        assert_eq!(cmd_i18n_t(&state, "plugin:com.b.oc.title").unwrap(), "B");

        // 贡献：被卸载插件的入口全部移除。
        assert!(cmd_contributes_list(&state, Some("menu"))
            .unwrap()
            .iter()
            .all(|e| e.plugin_id != "com.a"));

        // 恢复引擎：不再被登记（它的状态表是持久化的，残留会跨重启）。
        assert!(state.recovery.lock().plugin_state("com.a").is_none());

        // 崩溃窗口：注册表条目已被删掉（重装同名插件是允许的），旧版本的崩溃预算
        // 不得留给新版本——否则"刚装的插件永远起不来"。
        assert_eq!(state.proc_runtime.crash_count("com.a"), 0, "卸载必须清掉崩溃窗口");
        assert!(
            !state.proc_runtime.is_crash_exceeded("com.a"),
            "卸载后重装同名插件不得继承旧版本的崩溃预算"
        );
    }

    /// 禁用**不得**回收总线残留：只有卸载/清除才回收。
    ///
    /// 禁用后插件仍可重新启用，订阅与声明必须原样保留。
    #[test]
    fn disable_keeps_event_bus_residue() {
        let state = CommandState::new();
        let topic = "plugin:com.a:y";
        seed_bus(&state, "com.a", topic);
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        cmd_registry_admin(&state, "com.a", RegistryAdminOp::Disable).unwrap();

        let bus = state.bus.lock();
        assert!(bus.has_hanging_subscriptions("com.a"), "禁用不应回收订阅");
        assert!(bus.topic_meta(topic).is_some(), "禁用不应回收声明");
    }

    // ────────────────────────────────────────────────────────────
    // V7-P1-01：插件运行时**单例装配**
    // ────────────────────────────────────────────────────────────

    /// 同一底座的第二次装配**不得返回 Runtime**，且第一次的写回口 / 注册表原样保留。
    ///
    /// 盯的是此前那条「打日志但仍返回第二个 Registry」的实现：调用方手里是第二份
    /// 注册表，恢复对账却写向第一份——故障表现为「清了标志、重启后没生效」，
    /// 而日志里那句警告没人看。现在冲突是 `Err`，第二份 Registry 根本不会被建出来。
    #[test]
    fn second_assembly_on_same_substrate_returns_no_runtime() {
        let substrate = Arc::new(SubstrateState::with_adapter_config(&AdapterConfig::default()));
        let first = PluginRuntimeState::with_substrate_and_spawner(
            substrate.clone(),
            AdapterConfig::default(),
            Arc::new(FakeSpawner::default()),
        )
        .expect("第一次装配必须成功");
        let claimed = *substrate.plugin_runtime_assembly.get().expect("装配成功必须落下凭证");

        // 让第一份注册表带上可观察事实（一个插件），写回口据此就能被认出指向谁。
        first.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let second = PluginRuntimeState::with_substrate_and_spawner(
            substrate.clone(),
            AdapterConfig::default(),
            Arc::new(FakeSpawner::default()),
        );
        let error = match second {
            Ok(_) => panic!("同一底座的第二次装配必须失败，不能返回 Runtime"),
            Err(error) => error,
        };
        let AssemblyError::AlreadyAssembled { existing, attempted } = error else {
            panic!("冲突必须是 AlreadyAssembled，实际：{error:?}");
        };
        assert_eq!(existing, claimed, "错误里必须指名持有凭证的那次装配");
        assert_ne!(existing, attempted, "被拒的凭证与既有凭证必须可区分");
        assert!(
            substrate.plugin_runtime_assembly.get().is_some_and(|t| *t == claimed),
            "凭证不得被替换"
        );

        // 写回口仍是**第一份**注册表：往里再装一个插件，快照必须跟着变。
        // 若第二次的空注册表接管了 sink，这里只会看见 `com.a` 一条（或全空）。
        let sink = substrate.plugin_flags.get().expect("装配后必须注入插件侧写回口");
        assert_eq!(sink.flag_snapshots(), vec![("com.a".to_string(), false)]);
        first.registry.install(&empty_index(), test_manifest("com.b")).unwrap();
        assert_eq!(
            sink.flag_snapshots(),
            vec![("com.a".to_string(), false), ("com.b".to_string(), false)],
            "恢复对账写回口必须始终读第一份注册表"
        );
        // 沙箱事实也仍由第一次装配持有（不得被清空或改写）。
        assert!(substrate.process_sandbox.get().is_some());
    }

    /// 轮 41：装配即开台账——`recovery_data_dir` 存在时，装配输出一份可校验的
    /// 启动扫描快照（首启 = 空账也落盘，"扫描过"是可查事实）。
    #[test]
    fn assembly_opens_the_reap_ledger_on_the_recovery_dir() {
        let temp = tempfile::tempdir().unwrap();
        let cfg = AdapterConfig {
            recovery_data_dir: Some(temp.path().to_path_buf()),
            ..AdapterConfig::default()
        };
        let substrate = Arc::new(SubstrateState::with_adapter_config(&cfg));
        let runtime = PluginRuntimeState::with_substrate_and_spawner(
            substrate,
            cfg,
            Arc::new(FakeSpawner::default()),
        )
        .expect("装配必须成功");

        let path = temp.path().join(tauron_host::runtime::REAP_LEDGER_FILE);
        let raw = std::fs::read_to_string(&path).expect("装配就该把'已扫描'快照落盘");
        assert!(raw.contains("tauron.reap-ledger/1"), "schema 必须进文件：{raw}");
        assert!(raw.contains("\"pending\":[]"), "首启没有上一轮条目：{raw}");
        let stats = runtime.registry.runtime_reap_stats();
        assert_eq!(
            (stats.sweep_resolved, stats.sweep_survivors, stats.sweep_unknown),
            (0, 0, 0),
            "首启没有上一轮条目，不该凭空产出扫描结论"
        );
    }

    /// 轮 41：台账读不开 = 拒绝装配（不静默重置成空账），错误必须带上原文。
    #[test]
    fn assembly_rejects_a_torn_reap_ledger() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(tauron_host::runtime::REAP_LEDGER_FILE), b"{torn").unwrap();
        let cfg = AdapterConfig {
            recovery_data_dir: Some(temp.path().to_path_buf()),
            ..AdapterConfig::default()
        };
        let substrate = Arc::new(SubstrateState::with_adapter_config(&cfg));
        let result = PluginRuntimeState::with_substrate_and_spawner(
            substrate,
            cfg,
            Arc::new(FakeSpawner::default()),
        );
        // 不写 `expect_err`：Ok 侧 `PluginRuntimeState` 没有 Debug（也不该只为一条
        // 测试给它加），换等价的 let-else 就不会把 Debug 约束带进来。
        let Err(error) = result else {
            panic!("撕裂台账必须拒绝装配（却拿到了 Runtime）");
        };
        let AssemblyError::ReapLedgerRejected(reason) = error else {
            panic!("必须是 ReapLedgerRejected，实际：{error:?}");
        };
        assert!(reason.contains("integrity"), "失败原文必须可查：{reason}");
    }

    /// 并发装配：同一底座的 N 个线程里**恰好一个**拿到 Runtime，其余全部
    /// `AlreadyAssembled`，且都指名同一个既有凭证。
    ///
    /// 这条用例盯的是「先判断、后写入」的实现漏洞：`get().is_none()` 再 `set()`
    /// 之间有窗口，两个线程能同时判断通过并各建一份 Registry。领取凭证只用
    /// `set` 的返回值，因此窗口不存在——本用例把它钉住。
    #[test]
    fn concurrent_assembly_on_same_substrate_yields_exactly_one_runtime() {
        let substrate = Arc::new(SubstrateState::with_adapter_config(&AdapterConfig::default()));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let substrate = substrate.clone();
            handles.push(std::thread::spawn(move || {
                match PluginRuntimeState::with_substrate_and_spawner(
                    substrate,
                    AdapterConfig::default(),
                    Arc::new(FakeSpawner::default()),
                ) {
                    Ok(_runtime) => Ok(()),
                    Err(AssemblyError::AlreadyAssembled { existing, attempted }) => {
                        Err((existing.assembly_id, attempted.assembly_id))
                    }
                    Err(other) => panic!("并发装配不该出现别的失败：{other}"),
                }
            }));
        }
        let results: Vec<Result<(), (u64, u64)>> =
            handles.into_iter().map(|h| h.join().expect("装配线程不得 panic")).collect();

        let winners = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(winners, 1, "同一底座只能有一份插件运行时：{results:?}");
        let rejected: Vec<(u64, u64)> =
            results.iter().filter_map(|r| r.as_ref().err().copied()).collect();
        let claimed = substrate.plugin_runtime_assembly.get().expect("必须有人领到凭证");
        assert!(
            rejected.iter().all(|(existing, _)| *existing == claimed.assembly_id),
            "每次拒绝都要指名同一个持有凭证的装配：{rejected:?}"
        );
        let attempted_ids: std::collections::BTreeSet<u64> =
            rejected.iter().map(|(_, attempted)| *attempted).collect();
        assert_eq!(
            attempted_ids.len(),
            rejected.len(),
            "每次尝试的凭证必须互不相同（凭证计数器不得重复发放）"
        );
        // 唯一赢家写下的写回口也没有被并发者改写。
        assert!(substrate.plugin_flags.get().is_some());
    }

    // ────────────────────────────────────────────────────────────
    // P0-2：进程插件运行时（tauron-proc 激活）
    //
    // 这些用例一律走 **fake 启动面**：测试里绝不真起 sidecar 进程（CI 上没有
    // sidecar 二进制；真起进程还会带进时序、残留进程与平台差异，把用例变 flaky）。
    // 被验证的是执行器**语义**——拒绝路径、租约绑定、幂等、崩溃判定与投递；
    // "真进程真的能起来"由 `tauron_proc::CommandSpawner` 自己的用例覆盖。
    // ────────────────────────────────────────────────────────────

    /// 记录型 + 可控存活的 fake 启动面。
    ///
    /// 同时提供测试需要的三件能力：记录「启动面被调用几次 / 用什么配置」、
    /// 记录「终止面按哪个 pid 被调用」，以及凭空制造崩溃（把 pid 标记为已死）
    /// 与启动失败。
    #[derive(Default)]
    struct FakeSpawner {
        calls: Mutex<Vec<SpawnConfig>>,
        killed: Mutex<Vec<u32>>,
        alive: Mutex<std::collections::HashSet<u32>>,
        next_pid: std::sync::atomic::AtomicU32,
        fail_with: Mutex<Option<ProcError>>,
        fail_kill_with: Mutex<Option<String>>,
        /// pid → 已登记的 stdout 帧接收器（0.4-A1 帧回路）。
        sinks: Mutex<HashMap<u32, Arc<dyn ProcessFrameSink>>>,
        /// pid → 写入 stdin 的帧序列（0.4-A1 帧回路）。
        written: Mutex<HashMap<u32, Vec<Vec<u8>>>>,
        /// 下一次 `write_frame` 的失败种（轮 30：投递写侧的错误码分流）。
        fail_write_with: Mutex<Option<std::io::ErrorKind>>,
        /// 生产装配门（`validate_process_runtime_for_start`）要求附着的运行时
        /// 报告 hard 沙箱；FakeSpawner 默认如实上报 unsupported，置位后上报
        /// hard 描述（仅通过装配门，不做任何真实隔离）。
        hard_sandbox: std::sync::atomic::AtomicBool,
    }

    impl FakeSpawner {
        fn call_count(&self) -> usize {
            self.calls.lock().len()
        }

        fn last_cfg(&self) -> SpawnConfig {
            self.calls.lock().last().cloned().expect("启动面未被调用（本用例要求它必须被调用过）")
        }

        /// 终止面收到的 pid 序列（按调用顺序）。
        fn killed(&self) -> Vec<u32> {
            self.killed.lock().clone()
        }

        /// 制造一次崩溃：此后 `is_alive(pid)` 恒为 false。
        fn crash(&self, pid: u32) {
            self.alive.lock().remove(&pid);
        }

        /// 让下一次 `spawn` 失败。
        fn fail_next_spawn_with(&self, e: ProcError) {
            *self.fail_with.lock() = Some(e);
        }

        /// 让所有 `kill` 都失败（模拟权限不足 / 杀不掉）。
        fn fail_kill_with(&self, msg: &str) {
            *self.fail_kill_with.lock() = Some(msg.to_string());
        }

        /// 某进程写入 stdin 的帧序列（0.4-A1 帧回路验证用）。
        fn written_frames(&self, pid: u32) -> Vec<Vec<u8>> {
            self.written.lock().get(&pid).cloned().unwrap_or_default()
        }

        /// 某进程登记的 stdout 帧接收器（测试据此模拟 sidecar 回帧）。
        fn sink_of(&self, pid: u32) -> Option<Arc<dyn ProcessFrameSink>> {
            self.sinks.lock().get(&pid).cloned()
        }

        /// 让下一次 `write_frame` 以指定 `ErrorKind` 失败（轮 30 写侧分流用）。
        fn fail_next_write_with(&self, kind: std::io::ErrorKind) {
            *self.fail_write_with.lock() = Some(kind);
        }

        /// 描述为 hard 沙箱（仅供生产档装配门通过；不做任何真实隔离）。
        fn mark_hard_sandbox(&self) {
            self.hard_sandbox.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl ProcSpawner for FakeSpawner {
        /// 默认如实上报 unsupported（与 `CommandSpawner` 缺省一致）；
        /// [`Self::mark_hard_sandbox`] 后上报 hard 描述。
        fn sandbox_descriptor(&self) -> tauron_proc::ProcessSandboxDescriptor {
            if self.hard_sandbox.load(std::sync::atomic::Ordering::SeqCst) {
                tauron_proc::ProcessSandboxDescriptor {
                    enforcement: tauron_proc::ProcessSandboxEnforcement::Hard,
                    process_tree_containment: true,
                    filesystem_isolation: true,
                    network_isolation: true,
                    syscall_isolation: true,
                    detail: "FakeSpawner hard 描述（测试标记，非真实隔离）".into(),
                }
            } else {
                tauron_proc::ProcessSandboxDescriptor::unsupported(
                    "FakeSpawner 未标记 hard_sandbox",
                )
            }
        }

        fn spawn(&self, cfg: &SpawnConfig) -> ProcResult<SpawnedProc> {
            self.calls.lock().push(cfg.clone());
            if let Some(e) = self.fail_with.lock().take() {
                return Err(e);
            }
            let pid = self.next_pid.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1000;
            self.alive.lock().insert(pid);
            Ok(SpawnedProc { pid })
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.lock().contains(&pid)
        }

        fn kill(&self, pid: u32) -> ProcResult<tauron_proc::KillOutcome> {
            self.killed.lock().push(pid);
            if let Some(msg) = self.fail_kill_with.lock().clone() {
                return Err(ProcError::ProcessTerminated(msg));
            }
            // 已崩溃的 pid 如实按"早就没了"上报（与 `CommandSpawner` 同语义）；
            // 存活的 pid 按"真杀了"上报。
            let was_alive = self.alive.lock().remove(&pid);
            Ok(if was_alive {
                tauron_proc::KillOutcome::Terminated
            } else {
                tauron_proc::KillOutcome::AlreadyGone
            })
        }

        /// 记录写往 sidecar stdin 的帧（0.4-A1：帧回路投递侧）。
        fn write_frame(&self, pid: u32, frame: &[u8]) -> std::io::Result<()> {
            if let Some(kind) = self.fail_write_with.lock().take() {
                return Err(std::io::Error::new(kind, format!("注入的写失败（kind {kind:?}）")));
            }
            self.written.lock().entry(pid).or_default().push(frame.to_vec());
            Ok(())
        }

        /// 登记 stdout 帧接收器（0.4-A1：帧回路回帧侧；测试据此模拟 sidecar）。
        fn register_frame_sink(&self, pid: u32, sink: Arc<dyn ProcessFrameSink>) -> bool {
            self.sinks.lock().insert(pid, sink);
            true
        }
    }

    fn process_manifest(id: &str, sidecar: Option<&str>) -> PluginManifest {
        let mut m = test_manifest(id);
        m.plugin_type = PluginType::Process;
        m.entry = tauron_host::manifest::EntrySpec {
            js: None,
            sidecar: sidecar.map(|s| s.to_string()),
            wasm: None,
            ui: None,
        };
        m
    }

    /// 验签材料齐全的 profile（缺任何一项都会被 §4.7 的 spawn 前校验挡住）。
    fn valid_profile() -> RuntimeSpawnProfile {
        RuntimeSpawnProfile {
            binary_path: None,
            args: vec!["--serve".to_string()],
            env: std::collections::HashMap::new(),
            signature: RuntimeBinarySignature {
                algorithm: "ed25519".to_string(),
                signature: "sig".to_string(),
                signer_id: "signer-001".to_string(),
            },
            binary_hash: "a".repeat(64),
            // 必须是**宿主当前 ABI 契约**（`cmd_runtime_spawn` 会比对，见
            // `runtime_spawn_rejects_abi_mismatch_before_starting_anything`）——
            // 不是随便填的占位串，填错会被 `E_ABI_MISMATCH` 挡在启动面之前。
            abi: RuntimeAbiFingerprint {
                rust_version: tauron_proc::SIDECAR_ABI_RUST_VERSION.to_string(),
                interface_hash: tauron_proc::SIDECAR_ABI_INTERFACE_HASH.to_string(),
            },
        }
    }

    /// 装好一个 process 插件，并返回带 fake 启动面的状态。
    fn process_state(id: &str, sidecar: Option<&str>) -> (CommandState, Arc<FakeSpawner>) {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        state.registry.install(&empty_index(), process_manifest(id, sidecar)).unwrap();
        (state, fake)
    }

    /// [`process_state`] 的带配置变体：装配配置由调用方给出，启动面仍是 fake。
    ///
    /// 存在的唯一理由是要验证**装配配置 → 子进程启动参数**这条链
    /// （`ClientConfig.env_overrides` 的落点用例），默认装配的 `with_spawner` 到不了它。
    fn process_state_with_config(
        id: &str,
        adapter: AdapterConfig,
    ) -> (CommandState, Arc<FakeSpawner>) {
        let fake = Arc::new(FakeSpawner::default());
        let substrate = Arc::new(SubstrateState::with_adapter_config(&adapter));
        let state = CommandState::with_substrate_and_spawner(substrate, adapter, fake.clone())
            .expect("自建底座首次装配不可能冲突");
        state.registry.install(&empty_index(), process_manifest(id, Some("svc"))).unwrap();
        (state, fake)
    }

    /// `ClientConfig.env_overrides` 必须真的落到 `SpawnConfig::env`（轮 2 / F-1）。
    ///
    /// 三条断言各挡一种失真：
    /// - 运维独有键出现在启动参数里 → 这条链没断，`env_overrides` 不再是
    ///   "写了不生效"的那批键之一（只有落点、没有消费者，等于没有）；
    /// - 同名键上运维**覆盖**调用方 → 优先级没写反：若调用方能改运维注入的键，
    ///   运维就没有任何确定杠杆；
    /// - 调用方独有键保留 → 注入不是白名单，不会把 `profile.env` 整体清掉。
    #[test]
    fn host_env_overrides_reach_spawn_config_and_win_over_caller() {
        let mut adapter = AdapterConfig::default();
        adapter.plugin_env_overrides.insert("TAURON_PROFILE".to_string(), "prod".to_string());
        adapter.plugin_env_overrides.insert("OC_PROFILE".to_string(), "from-host".to_string());
        let (state, fake) = process_state_with_config("com.env", adapter);
        enabled_process_plugin(&state, "com.env");

        let mut profile = valid_profile();
        profile.env.insert("OC_PROFILE".to_string(), "from-caller".to_string());
        profile.env.insert("CALLER_ONLY".to_string(), "kept".to_string());
        cmd_runtime_spawn(&state, "com.env", &profile).unwrap();

        let env = &fake.last_cfg().env;
        assert_eq!(
            env.get("TAURON_PROFILE").map(String::as_str),
            Some("prod"),
            "运维注入的键没进启动参数：`env_overrides` 又变回一条假配置"
        );
        assert_eq!(
            env.get("OC_PROFILE").map(String::as_str),
            Some("from-host"),
            "同名键必须运维优先（调用方能覆盖运维配置 = 没有确定杠杆）"
        );
        assert_eq!(
            env.get("CALLER_ONLY").map(String::as_str),
            Some("kept"),
            "注入只做叠加，不得顺手清空调用方自己的 env"
        );
    }

    /// 缺省装配（没有运维注入）下 `profile.env` 原样透传，零影响。
    #[test]
    fn spawn_env_passes_through_without_host_overrides() {
        let (state, fake) = process_state("com.env0", Some("svc"));
        enabled_process_plugin(&state, "com.env0");
        let mut profile = valid_profile();
        profile.env.insert("CALLER_ONLY".to_string(), "kept".to_string());
        cmd_runtime_spawn(&state, "com.env0", &profile).unwrap();
        let cfg = fake.last_cfg();
        assert_eq!(cfg.env.len(), 1, "默认装配不该往子进程环境里塞东西：{:?}", cfg.env);
        assert_eq!(cfg.env.get("CALLER_ONLY").map(String::as_str), Some("kept"));
    }

    /// `runtime-wasm-broker` 这条 feature **有没有作用**（轮 2 / F-2）。
    ///
    /// 参考宿主默认开着它（`examples/minimal-app/src-tauri/Cargo.toml`），但
    /// `cargo test --workspace` 跑的是 adapter 的**默认 feature 集**：`wasm_delivery`
    /// 整个模块——连同它自己的用例——在 CI 里一次都没被编译过。于是"feature 编译不过"
    /// 或"Wasm 投递表接错"只有 `cargo check --all-features` 挡得住，测试挡不住。
    /// CI 现在专门跑 `cargo test -p tauron-adapter --features runtime-wasm-broker`，
    /// 本用例是那条命令要证明的第一件事：Wasm 投递**确实登记进了投递表**。
    #[cfg(feature = "runtime-wasm-broker")]
    #[test]
    fn wasm_broker_feature_registers_wasm_delivery() {
        let state = CommandState::new();
        assert!(
            state.deliveries.contains_key(&DeliveryKind::Wasm),
            "开了 runtime-wasm-broker 却没有登记 Wasm 投递：feature 是空的，\
             而文档会据它声称 WASM 形态可装配"
        );
    }

    /// 反向配对：不开 feature 时 Wasm 形态**必须不在表里**，从而落
    /// `UnwiredDelivery` 的诚实 `Unsupported`。两条用例合起来才说明这条 feature
    /// 真的改变了装配结果，而不是一个改名占位。
    #[cfg(not(feature = "runtime-wasm-broker"))]
    #[test]
    fn without_wasm_broker_feature_wasm_delivery_is_absent() {
        let state = CommandState::new();
        assert!(
            !state.deliveries.contains_key(&DeliveryKind::Wasm),
            "没开 feature 却登记了 Wasm 投递：最小底座的 unwired 语义被绕过"
        );
    }

    /// 0.4-A1 进程投递闭环（Rust 层，不真起 sidecar）：
    /// 宿主发起 → JSON-RPC 帧写入 sidecar stdin（fake 记录）→ 模拟 sidecar 回帧
    /// （含 `callId`）→ `ProcessFrameSinkImpl` 调 `settle_call` → pending 表结算。
    #[test]
    fn process_call_delivers_frame_to_stdin_and_settles_via_sink() {
        let (state, fake) = process_state("com.example.svcrec", Some("svc"));
        enabled_process_plugin(&state, "com.example.svcrec");
        let handle = cmd_runtime_spawn(&state, "com.example.svcrec", &valid_profile()).unwrap();

        // 宿主 → 插件：跨主体调用应投递成功（待应答）。
        let res = cmd_call_plugin(
            &state,
            "main",
            "com.example.svcrec",
            "doThing",
            serde_json::json!({ "x": 1 }),
        )
        .expect("跨主体调用命令面应成功");
        let call = match res {
            ProviderResult::Value(c) => c,
            ProviderResult::Unsupported(u) => {
                panic!("进程插件应有投递通路，却返回 Unsupported：{}", u.reason)
            }
        };

        // 投递侧：应恰好向 sidecar stdin 写一帧，且帧携带权威字段。
        let frames = fake.written_frames(handle.pid);
        assert_eq!(frames.len(), 1, "应恰好向 sidecar stdin 写一帧");
        let frame: serde_json::Value = serde_json::from_slice(&frames[0]).unwrap();
        assert_eq!(frame["method"], "doThing");
        assert_eq!(frame["target"], "com.example.svcrec");
        assert_eq!(frame["params"], serde_json::json!({ "x": 1 }));
        assert_eq!(
            frame["callId"], call.call_id,
            "帧上的 callId 必须与 pending 表一致（回帧据此关联）"
        );
        assert_eq!(call.runtime_generation, Some(handle.generation));
        assert_eq!(frame["runtimeGeneration"], serde_json::json!(handle.generation.0));

        // 回帧侧：模拟 sidecar 写回结果帧 → sink 结算。
        let sink = fake.sink_of(handle.pid).expect("spawn 后必须已登记 stdout 帧接收器");
        let reply = serde_json::json!({
            "jsonrpc": "2.0",
            "id": call.seq,
            "callId": call.call_id,
            "result": { "done": true },
        });
        sink.on_frame(handle.pid, serde_json::to_vec(&reply).unwrap().as_slice());

        let settled = state.registry.peek_call(&call.call_id).expect("调用应仍在表里");
        assert_eq!(settled.state, tauron_host::registry::CallState::Settled);
        assert_eq!(settled.result, Some(serde_json::json!({ "done": true })));
    }

    /// 轮 30：写侧失败必须**按事实分码**，不再一律塌成"非法状态迁移"。
    ///
    /// 为什么这条用例在 Rust 层而不是只在 E2E：E2E 里写队列满不满**取决于本机负载**
    /// （轮 30 就是靠它在并行门禁跑下抓到这个塌缩的），而错误码分流是可判定语义，
    /// 必须有一种确定性注入方式。这里直接注入 `ErrorKind`。
    #[test]
    fn process_delivery_write_failures_map_to_their_own_codes() {
        // (注入的 io 失败种, 期望码, 期望 retry class)
        let cases = [
            (
                std::io::ErrorKind::WouldBlock,
                ErrorCode::E_CALL_PENDING_FULL,
                // 额度满：不自动重放（V4 幂等证明前不重试），但绝不是说"状态机非法"。
                tauron_host::error::RetryClass::Never,
            ),
            (
                std::io::ErrorKind::BrokenPipe,
                ErrorCode::E_LEASE_EXPIRED,
                // 通路没了：机器可读的"先重连（重新 spawn）再谈重试"。
                tauron_host::error::RetryClass::AfterReconnect,
            ),
            (
                std::io::ErrorKind::NotFound,
                ErrorCode::E_LEASE_EXPIRED,
                tauron_host::error::RetryClass::AfterReconnect,
            ),
            (
                std::io::ErrorKind::InvalidInput,
                ErrorCode::E_INVALID_MANIFEST,
                tauron_host::error::RetryClass::Never,
            ),
        ];
        for (kind, want_code, want_class) in cases {
            let (state, fake) = process_state("com.example.wrfail", Some("svc"));
            enabled_process_plugin(&state, "com.example.wrfail");
            let handle = cmd_runtime_spawn(&state, "com.example.wrfail", &valid_profile()).unwrap();

            fake.fail_next_write_with(kind);
            let err = cmd_call_plugin(
                &state,
                "main",
                "com.example.wrfail",
                "doThing",
                serde_json::json!({}),
            )
            .expect_err(&format!("{kind:?} 的写失败必须上抛错误"));
            assert_eq!(err.code, want_code, "{kind:?} 分流错了：{}", err.message);
            assert_eq!(err.code.retry_class(), want_class, "{kind:?} 的码必须带正确的重试语义");
            // 零泄漏：投递失败的调用不得留在 pending 表上占额度。
            assert_eq!(state.registry.pending_for("main"), 0, "{kind:?} 后 pending 应归零");
            assert!(fake.written_frames(handle.pid).is_empty(), "失败的帧没有被记成已投递");
        }
    }

    /// 轮 30：启动器没有写 stdin 这条通路 = **未装配投递**，不是错误码。
    ///
    /// 走 `ProviderResult::Unsupported`（与"没有 runtime 不投递"同一个形状）：调用方
    /// 据此知道"这条链路压根没接"，而不是收到一个暗示插件状态坏了的失败。
    #[test]
    fn process_delivery_without_write_channel_reports_unsupported() {
        let (state, fake) = process_state("com.example.nowrite", Some("svc"));
        enabled_process_plugin(&state, "com.example.nowrite");
        cmd_runtime_spawn(&state, "com.example.nowrite", &valid_profile()).unwrap();

        fake.fail_next_write_with(std::io::ErrorKind::Unsupported);
        let res = cmd_call_plugin(
            &state,
            "main",
            "com.example.nowrite",
            "doThing",
            serde_json::json!({}),
        )
        .expect("没有写通路应报告 Unsupported，而不是错误");
        match res {
            ProviderResult::Unsupported(body) => {
                assert!(
                    body.reason.contains("不支持写入 sidecar stdin"),
                    "原因要指出没接的是哪条通路：{}",
                    body.reason
                );
            }
            ProviderResult::Value(_) => panic!("没有写通路却报告投递成功"),
        }
        assert_eq!(state.registry.pending_for("main"), 0, "未投递的调用不得留在 pending 表");
    }

    #[test]
    fn process_reply_from_old_generation_cannot_settle_call_after_runtime_replacement() {
        let (state, fake) = process_state("com.example.gen", Some("svc"));
        enabled_process_plugin(&state, "com.example.gen");
        let first = cmd_runtime_spawn(&state, "com.example.gen", &valid_profile()).unwrap();

        let res = cmd_call_plugin(
            &state,
            "main",
            "com.example.gen",
            "doThing",
            serde_json::json!({ "x": 1 }),
        )
        .unwrap();
        let call = match res {
            ProviderResult::Value(call) => call,
            ProviderResult::Unsupported(body) => panic!("unexpected unsupported: {}", body.reason),
        };
        assert_eq!(call.runtime_generation, Some(first.generation));
        let old_sink = fake.sink_of(first.pid).expect("first runtime sink");

        // Replace the runtime generation while the old call is still pending.
        state.registry.runtime_mark_crashed(&first.lease).unwrap();
        let second = cmd_runtime_spawn(&state, "com.example.gen", &valid_profile()).unwrap();
        assert!(second.generation > first.generation);

        let reply = serde_json::json!({
            "jsonrpc": "2.0",
            "id": call.seq,
            "callId": call.call_id,
            "result": { "stale": true },
        });
        let bytes = serde_json::to_vec(&reply).unwrap();

        // A late frame from the old pid is rejected.
        old_sink.on_frame(first.pid, &bytes);
        assert_eq!(
            state.registry.peek_call(&call.call_id).unwrap().state,
            tauron_host::registry::CallState::Pending
        );

        // The new runtime cannot steal the old call either: generation mismatch still rejects it.
        let new_sink = fake.sink_of(second.pid).expect("second runtime sink");
        new_sink.on_frame(second.pid, &bytes);
        assert_eq!(
            state.registry.peek_call(&call.call_id).unwrap().state,
            tauron_host::registry::CallState::Pending
        );
        state.registry.call_cancel(&call.call_id).unwrap();
    }

    /// 「非 Process 插件被拒」：结构化失败码 + **启动面一次都不许被调用** + 无租约。
    #[test]
    fn runtime_spawn_rejects_non_process_plugin_without_starting_anything() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        // test_manifest 是 js 型。
        state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

        let err = cmd_runtime_spawn(&state, "com.a", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_TYPE_NO_RUNTIME);
        assert!(err.message.contains("process"), "message 必须指出正确类型：{}", err.message);
        assert_eq!(fake.call_count(), 0, "类型不匹配时绝不调用启动面");
        assert_eq!(state.registry.runtime_len(), 0, "拒绝路径不得留下租约");
    }

    /// ABI 契约不匹配 → `E_ABI_MISMATCH`（**独立于** `E_INSTALL_FAILED`），且
    /// **启动面一次都不许被调用**：不合格的载荷连"是否已有租约"都不该被它探测到。
    ///
    /// 本用例是 `tauron_proc::validate_abi` 的**生产调用点**的证据——它此前只有
    /// 测试调用，导致 `E_ABI_MISMATCH` 这个线协议码**永不产生**（孤儿码），
    /// 而 TS 侧 `shell-client.ts` 的文档却在承诺它。
    #[test]
    fn runtime_spawn_rejects_abi_mismatch_before_starting_anything() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");

        // 1) 接口哈希维度不符 → E_ABI_MISMATCH，启动面零调用、无租约。
        let mut bad_iface = valid_profile();
        bad_iface.abi.interface_hash = "tauron-sidecar-rpc/0".to_string();
        let err = cmd_runtime_spawn(&state, "com.proc", &bad_iface).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_ABI_MISMATCH);
        assert!(!err.retryable, "ABI 不匹配不可重试——重试还是同一个不匹配");
        assert_eq!(fake.call_count(), 0, "ABI 不合格绝不调用启动面");
        assert_eq!(state.registry.runtime_len(), 0, "拒绝路径不得留下租约");

        // 2) 协议版本维度同样受校验（两个维度都查，不是只查一个）。
        let mut bad_ver = valid_profile();
        bad_ver.abi.rust_version = "tauron-proc-abi/0".to_string();
        assert_eq!(
            cmd_runtime_spawn(&state, "com.proc", &bad_ver).unwrap_err().code,
            ErrorCode::E_ABI_MISMATCH
        );
        assert_eq!(fake.call_count(), 0, "第二个维度被拒时启动面仍为零调用");

        // 3) 契约值正确（`valid_profile()` 即契约值）→ 放行，证明拒因确为 ABI。
        assert!(cmd_runtime_spawn(&state, "com.proc", &valid_profile()).is_ok());
        assert_eq!(fake.call_count(), 1, "契约正确才真正拉起 sidecar");
    }

    #[test]
    fn runtime_spawn_unknown_plugin_is_e_unknown_plugin() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        let err = cmd_runtime_spawn(&state, "com.ghost", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
        assert_eq!(fake.call_count(), 0);
        assert_eq!(state.registry.runtime_len(), 0);
    }

    /// 正路：真经过启动面、缺省二进制取 manifest 的 `entry.sidecar`、lease 绑 pid。
    #[test]
    fn runtime_spawn_starts_sidecar_and_binds_lease_to_pid() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        // 可用性门（P0-2）：只有 ENABLED / RUNNING 才允许有 sidecar。
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();

        assert_eq!(fake.call_count(), 1, "必须真的经过注入的启动面");
        let cfg = fake.last_cfg();
        assert_eq!(cfg.binary_path, "sidecar.exe", "缺省二进制 = manifest 的 entry.sidecar");
        assert_eq!(cfg.args, vec!["--serve".to_string()]);
        assert_eq!(handle.pid, 1000);

        let id = PluginId::new("com.proc").unwrap();
        assert_eq!(
            state.registry.runtime_handle_of(&id),
            Some(handle.clone()),
            "lease ↔ pid 必须登记进注册表"
        );

        let health = cmd_runtime_health(&state, &handle.lease).unwrap();
        assert_eq!(health.pid, handle.pid);
        assert!(health.alive, "fake 未标记死亡 → 必须报存活");
        assert_eq!(health.health, tauron_host::HealthReport::ready());
        assert!(health.health.can_accept_work());
        assert_eq!(health.crashes, 0);
        assert_eq!(health.consecutive_failures, 0);
        assert_eq!(
            health.reap,
            ReapStats::default(),
            "health 里的 reap 是全局回收留痕，此刻还没有任何回收动作"
        );
    }

    /// **可用性门**：装好但没启用的插件不许起进程（`INSTALLED` 不是可用状态）。
    ///
    /// 这条门同时是 `ERRORED_USER_CONFIRM` 的人工闸门（"不 active"是同一条规则）。
    /// 顺序也在这里钉住：**先 enable，再 spawn**——不是为了让老用例过而放宽门。
    #[test]
    fn installed_plugin_cannot_spawn_until_it_is_enabled() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Installed
        );

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(!err.retryable, "不可用状态需要人去启用，不是可重试错误");
        assert!(
            err.message.contains("INSTALLED") && err.message.contains("不可用"),
            "message 必须说清当前状态与下一步：{}",
            err.message
        );
        assert_eq!(fake.call_count(), 0, "被门拦下时一次都不许起进程");
        assert_eq!(state.registry.runtime_len(), 0, "拒绝路径不得留下租约");

        // 既有启用路径（不新造入口）之后必须放行。
        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Enable).unwrap();
        assert_eq!(out.to, tauron_host::lifecycle::State::Enabled);
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(fake.call_count(), 1);
        assert_eq!(handle.pid, 1000);
    }

    #[test]
    fn runtime_spawn_profile_can_override_binary_path() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let mut p = valid_profile();
        p.binary_path = Some("C:\\plugins\\other.exe".to_string());
        cmd_runtime_spawn(&state, "com.proc", &p).unwrap();
        assert_eq!(fake.last_cfg().binary_path, "C:\\plugins\\other.exe");
    }

    /// 重复 spawn 只有**一种**行为：返回既有 lease，且绝不启动第二个进程。
    #[test]
    fn runtime_spawn_is_idempotent_per_plugin() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let first = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(second, first, "重复 spawn 必须原样返回既有 lease");
        assert_eq!(fake.call_count(), 1, "绝不启动第二个进程（否则会产生孤儿 PID）");
        assert_eq!(state.registry.runtime_len(), 1, "同一插件至多一条租约");
    }

    /// 未验签的载荷不得到达启动面（§4.7：spawn 前校验签名与哈希）。
    #[test]
    fn runtime_spawn_rejects_unverified_profile_before_touching_spawner() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let mut p = valid_profile();
        p.binary_hash = "too-short".to_string();
        let err = cmd_runtime_spawn(&state, "com.proc", &p).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert_eq!(fake.call_count(), 0, "未验签的载荷不得到达启动面");
        assert_eq!(state.registry.runtime_len(), 0);
    }

    /// process 插件**装不进来**：manifest 校验要求非空 `entry.sidecar`。
    ///
    /// 这正是 `cmd_runtime_spawn` 里"profile 与 manifest 都没给二进制路径"那条兜底
    /// 分支在实践中**不可达**的原因。它保留为防御性检查（不 panic、不猜路径），
    /// 本用例锁住它的前提——哪天 manifest 放宽了，这条分支就会变成真路径。
    #[test]
    fn process_plugin_without_sidecar_cannot_even_be_installed() {
        let fake = Arc::new(FakeSpawner::default());
        let state = CommandState::with_spawner(fake.clone());
        let err =
            state.registry.install(&empty_index(), process_manifest("com.proc", None)).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("entry.sidecar"), "message={}", err.message);
        assert_eq!(fake.call_count(), 0, "装不进来的插件不可能走到启动面");
        assert_eq!(state.registry.runtime_len(), 0);
    }

    /// 「fake spawner 未被调用时不得产生 lease」的另一半：启动**失败**同样不得产生租约。
    #[test]
    fn runtime_spawn_failure_never_creates_a_lease() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        fake.fail_next_spawn_with(ProcError::SpawnFailed("exec failed".to_string()));
        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INSTALL_FAILED);
        assert!(!err.retryable, "确定性的启动失败不得被标成可重试");
        assert_eq!(state.registry.runtime_len(), 0, "启动失败绝不能铸出租约");
        assert!(state.registry.runtime_handle_of(&PluginId::new("com.proc").unwrap()).is_none());
    }

    /// 未知 / 失效 lease → `E_LEASE_EXPIRED`（**不是** `E_CALL_NOT_FOUND`）。
    #[test]
    fn runtime_health_unknown_lease_is_lease_expired_not_call_not_found() {
        let (state, _fake) = process_state("com.proc", Some("sidecar.exe"));
        let err = cmd_runtime_health(&state, "no-such-lease").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_LEASE_EXPIRED);
        assert_ne!(err.code, ErrorCode::E_CALL_NOT_FOUND, "租约边界必须与 pending call 边界分开");
    }

    #[test]
    fn alive_process_is_not_ready_when_plugin_lifecycle_is_not_running() {
        // RuntimeCrash intentionally revokes the runtime lease, so it cannot model the distinct
        // A103 state "process probe is still Alive while control-plane lifecycle is not Running".
        // Lock that mapping directly here; command-level ready/dead tests below still prove
        // cmd_runtime_health routes real probe + lifecycle facts through this helper.
        let health = runtime_health_report(
            tauron_proc::ProcessStatus::Alive,
            Some(tauron_host::lifecycle::State::Enabled),
            0,
        );
        assert_eq!(health.liveness, tauron_host::Liveness::Alive);
        assert_eq!(health.readiness, tauron_host::Readiness::NotReady);
        assert_eq!(health.degradation, tauron_host::Degradation::Degraded);
        assert!(!health.can_accept_work());
        assert!(
            health.diagnostics.iter().any(|line| line.contains("ENABLED")),
            "diagnostics must expose the lifecycle fact that blocks readiness"
        );
    }

    /// 崩溃：投递 `RuntimeCrash`（状态机真的吃了它）+ `consecutiveFailures` +1
    /// + `CrashTracker` 记一次，且**重复轮询不得重复计数**。
    #[test]
    fn runtime_crash_delivers_event_and_bumps_consecutive_failures_once() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        // 先让状态可用（可用性门），`RUNNING` 由 spawn 自己推上去。
        enabled_process_plugin(&state, "com.proc");
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Enabled);

        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        // `RuntimeCrash` 的合法源态是 ENABLED/RUNNING；spawn 真起了进程 → 状态必须已到 RUNNING。
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Running);
        fake.crash(handle.pid);

        let health = cmd_runtime_health(&state, &handle.lease).unwrap();
        assert!(!health.alive, "被标记死亡的 pid 必须报不存活");
        assert_eq!(health.health.liveness, tauron_host::Liveness::Dead);
        assert_eq!(health.health.readiness, tauron_host::Readiness::NotReady);
        assert!(!health.health.can_accept_work());
        assert_eq!(health.crashes, 1, "崩溃必须记进 CrashTracker");
        assert_eq!(
            health.consecutive_failures, 1,
            "consecutiveFailures 必须 +1（复用恢复引擎的既有计数）"
        );

        // 事件不是投进虚空：RUNNING → ERRORED_RETRYABLE，并消耗既有重试预算。
        let entry = state.registry.find(&id).unwrap();
        assert_eq!(
            entry.state.state,
            LifecycleState::ErroredRetryable,
            "RuntimeCrash 必须真的驱动状态机"
        );
        assert_eq!(entry.state.retry_count, 1);

        // 轮询式探测的关键不变量：同一次死亡只记一次。
        let again = cmd_runtime_health(&state, &handle.lease).unwrap();
        assert!(!again.alive);
        assert_eq!(again.crashes, 1, "重复 health 不得把一次死亡放大成两次");
        assert_eq!(again.consecutive_failures, 1);
        assert_eq!(state.registry.find(&id).unwrap().state.retry_count, 1);

        // 跨通道一致：恢复引擎的线形视图同样看得见这次失败。
        let boot = cmd_recover_boot(&state).unwrap();
        assert_eq!(boot["counter"]["consecutiveFailures"], 1);
    }

    /// 卸载必须回收租约：否则同名插件重装时旧 lease 会变成孤儿条目。
    #[test]
    fn runtime_lease_is_recycled_with_the_plugin_entry() {
        let (state, _fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();

        assert_eq!(state.registry.runtime_len(), 0, "卸载必须回收租约");
        assert_eq!(
            cmd_runtime_health(&state, &handle.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED
        );
    }

    /// 崩溃预算耗尽 → `E_PLUGIN_DISABLED` 而非 panic 码；启动失败类错误都不是可重试。
    #[test]
    fn proc_errors_map_to_non_retryable_structured_codes() {
        assert_eq!(
            proc_error_to_host(ProcError::InvalidSpawnConfig("x".to_string())).code,
            ErrorCode::E_INVALID_MANIFEST
        );
        // ABI 不匹配**独立成码**（不并进 E_INSTALL_FAILED）：它是版本兼容问题，
        // 前端据此提示"升级插件/宿主"而不是"重装"。
        assert_eq!(
            proc_error_to_host(ProcError::AbiMismatch {
                expected: "tauron-proc-abi/1".to_string(),
                actual: "tauron-proc-abi/0".to_string(),
            })
            .code,
            ErrorCode::E_ABI_MISMATCH
        );
        for e in
            [ProcError::SpawnFailed("x".to_string()), ProcError::ProcessTerminated("x".to_string())]
        {
            let mapped = proc_error_to_host(e);
            assert_ne!(
                mapped.code,
                ErrorCode::E_HOST_PANIC,
                "启动失败不是 panic——谎报 panic 会被前端当可重试错误无限重试"
            );
            assert!(!mapped.retryable);
        }
    }

    /// 线形契约：TS 侧 camelCase 必须能反序列化；必填项缺失必须报错（不得静默成空串）。
    #[test]
    fn runtime_spawn_profile_uses_camel_case_wire_shape() {
        let v = serde_json::json!({
            "binaryPath": "sidecar.exe",
            "args": ["--serve"],
            "env": { "RUST_LOG": "info" },
            "signature": { "algorithm": "ed25519", "signature": "sig", "signerId": "signer-001" },
            "binaryHash": "a".repeat(64),
            "abi": { "rustVersion": "1.98.0", "interfaceHash": "iface" }
        });
        let p: RuntimeSpawnProfile = serde_json::from_value(v).expect("TS 线形必须可反序列化");
        assert_eq!(p.binary_path.as_deref(), Some("sidecar.exe"));
        assert_eq!(p.signature.signer_id, "signer-001");
        assert_eq!(p.abi.rust_version, "1.98.0");
        assert_eq!(p.env.get("RUST_LOG").map(String::as_str), Some("info"));

        // 缺 signature/abi → 反序列化必须失败（不能悄悄用空串过校验以外的路）。
        let missing = serde_json::json!({ "binaryHash": "a".repeat(64) });
        assert!(serde_json::from_value::<RuntimeSpawnProfile>(missing).is_err());
    }

    #[test]
    fn runtime_health_serializes_camel_case() {
        let h = RuntimeHealth {
            alive: false,
            status: tauron_proc::ProcessStatus::Exited,
            pid: 7,
            crashes: 2,
            consecutive_failures: 1,
            reap: ReapStats {
                // 一条 pid：首试失败 → 再试两次仍失败 → 固化为终态证据；
                // 另一条租约回收时进程早已退出；还有一条待重试的因为 pid 被在册租约
                // 占用而**让位**（没打第二下，所以 attempts/retries 都不因此增长）。
                attempts: 4,
                terminated: 0,
                already_gone: 1,
                failures: 3,
                last_error: Some("拒绝访问".to_string()),
                pending: 0,
                retries: 2,
                recovered: 0,
                terminal: 3,
                overflow: 0,
                skipped_live_pid: 1,
                // 轮 41：上一轮宿主留下的台账里一条 pid 经重启扫描确认已不存在（销账），
                // 另一条探测不可用（留证不盲杀）；台账有两次写盘失败（计数不静默）。
                sweep_resolved: 1,
                sweep_survivors: 0,
                sweep_unknown: 1,
                ledger_write_failures: 2,
                terminal_records: vec![
                    tauron_host::TerminalReapRecord {
                        plugin_id: "com.crash.proc".to_string(),
                        pid: 4242,
                        attempts: 3,
                        reason: "终止 pid 4242 失败：拒绝访问".to_string(),
                    },
                    tauron_host::TerminalReapRecord {
                        plugin_id: "com.reused.proc".to_string(),
                        pid: 4243,
                        attempts: 1,
                        reason: "pid 4243 现由在册租约 lease-9 持有：重试会误杀活进程，故让位出队（旧进程按定义已退出）"
                            .to_string(),
                    },
                    tauron_host::TerminalReapRecord {
                        plugin_id: "com.restart.proc".to_string(),
                        pid: 4244,
                        attempts: 2,
                        reason: "重启扫描销账：pid 4244 经平台探测已不存在（上一轮宿主终止失败，现已无对象）"
                            .to_string(),
                    },
                ],
            },
            health: tauron_host::HealthReport::dead("sidecar process has exited"),
        };
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "alive": false,
                "status": "exited",
                "pid": 7,
                "crashes": 2,
                "consecutiveFailures": 1,
                "reap": {
                    "attempts": 4,
                    "terminated": 0,
                    "alreadyGone": 1,
                    "failures": 3,
                    "lastError": "拒绝访问",
                    "pending": 0,
                    "retries": 2,
                    "recovered": 0,
                    "terminal": 3,
                    "overflow": 0,
                    "skippedLivePid": 1,
                    "sweepResolved": 1,
                    "sweepSurvivors": 0,
                    "sweepUnknown": 1,
                    "ledgerWriteFailures": 2,
                    "terminalRecords": [
                        {
                            "pluginId": "com.crash.proc",
                            "pid": 4242,
                            "attempts": 3,
                            "reason": "终止 pid 4242 失败：拒绝访问"
                        },
                        {
                            "pluginId": "com.reused.proc",
                            "pid": 4243,
                            "attempts": 1,
                            "reason": "pid 4243 现由在册租约 lease-9 持有：重试会误杀活进程，故让位出队（旧进程按定义已退出）"
                        },
                        {
                            "pluginId": "com.restart.proc",
                            "pid": 4244,
                            "attempts": 2,
                            "reason": "重启扫描销账：pid 4244 经平台探测已不存在（上一轮宿主终止失败，现已无对象）"
                        }
                    ]
                },
                "health": {
                    "liveness": "dead",
                    "readiness": "not-ready",
                    "degradation": "degraded",
                    "diagnostics": ["sidecar process has exited"]
                }
            })
        );
        // 嵌套字段名也必须是既有 camelCase 约定（否则 TS 侧 `alreadyGone` 读成 undefined，
        // 留痕又变回"只有日志"）。
        assert!(v["reap"].get("alreadyGone").is_some());
        assert!(v["reap"].get("lastError").is_some());
        // 重试腿同样得上线：证据必须带得出 pid / 插件 / 原因。
        assert_eq!(v["reap"]["terminalRecords"][0]["pluginId"], "com.crash.proc");
        assert_eq!(v["reap"]["terminalRecords"][0]["pid"], 4242);
        // 让位（同号活进程）也是必须查得到的事实，否则"没误杀"与"没重试"在读数上同形。
        assert_eq!(v["reap"]["skippedLivePid"], 1);
        // 轮 41：重启扫描腿同样上线——销账/存活/不可判定/写盘失败都必须读得到，
        // 且销账证据带得出插件与原因（跨重启孤儿的事实出口）。
        assert_eq!(v["reap"]["sweepResolved"], 1);
        assert_eq!(v["reap"]["sweepSurvivors"], 0);
        assert_eq!(v["reap"]["sweepUnknown"], 1);
        assert_eq!(v["reap"]["ledgerWriteFailures"], 2);
        assert_eq!(v["reap"]["terminalRecords"][2]["pluginId"], "com.restart.proc");
        assert!(v["reap"]["terminalRecords"][2]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("重启扫描销账"));
        assert_eq!(v["reap"]["terminalRecords"][1]["pluginId"], "com.reused.proc");
        assert!(v["reap"]["terminalRecords"][1]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("误杀"));
    }

    // ────────────────────────────────────────────────────────────
    // 缺口二：崩溃预算超限必须真的拒绝启动（`is_crash_exceeded` 不得是孤儿逻辑）
    // ────────────────────────────────────────────────────────────

    /// 把插件推进到**可用**状态（`ENABLED`）——既有启用路径，不新造入口。
    ///
    /// 刻意**不**顺手投 `ATTACH`：进程插件的 `RUNNING` 必须由"真的起了新进程"
    /// 这一步推上去（见 `attach_after_spawn`），测试里伪造一步就测不出那条断链。
    fn enabled_process_plugin(state: &CommandState, id: &str) {
        cmd_registry_admin(state, id, RegistryAdminOp::Enable).unwrap();
    }

    /// 启动 → 崩溃 → 查健康，返回本次崩溃前铸造的句柄。
    fn spawn_then_crash(state: &CommandState, fake: &FakeSpawner, id: &str) -> RuntimeHandle {
        let handle = cmd_runtime_spawn(state, id, &valid_profile()).unwrap();
        fake.crash(handle.pid);
        let health = cmd_runtime_health(state, &handle.lease).unwrap();
        assert!(!health.alive);
        handle
    }

    /// 把崩溃窗口推过上限（`> max_crashes`），同时让插件**保持可用**。
    ///
    /// 第一次崩溃走真实投递链（health → `RuntimeCrash` → 恢复引擎 → 状态机），
    /// 证明窗口计数确实由 health 驱动；其余直接推 `CrashTracker`（**唯一**计数来源，
    /// 不是自造数字）。这么分两步是为了绕开一条无关交互：恢复引擎的
    /// `consecutiveFailures ≥ SAFEMODE_THRESHOLD` 会把插件打成
    /// `DISABLED + disabledBySafemode`，那样测到的就变成"状态不可用"而不是"窗口超限"。
    /// 崩溃后先让插件自报恢复（既有迁移 `ERRORED_RETRYABLE + RETRY_OK → ENABLED`），
    /// 于是调用本函数后插件**可用**且窗口超限——正是预算门唯一该管的局面。
    fn exhaust_crash_window(state: &CommandState, fake: &FakeSpawner, id: &str) {
        let pid = PluginId::new(id).unwrap();
        let handle = spawn_then_crash(state, fake, id);
        assert_eq!(handle.pid, 1000);
        state.registry.report_event(&pid, Event::RetryOk).unwrap();
        // 有界推进到窗口超限（轮 2）：谓词一旦不再被满足，必须**断言失败**而不是
        // 空转——写 `while !is_crash_exceeded { record_crash }` 时，只要
        // `CrashTracker` 的语义变了（上限调高 / 计数饱和），这个 helper 就把
        // 整条 CI 挂成 100% CPU 的死循环。
        for _ in 0..64 {
            if state.proc_runtime.is_crash_exceeded(id) {
                break;
            }
            state.proc_runtime.record_crash(id);
        }
        assert!(
            state.proc_runtime.is_crash_exceeded(id),
            "记了 64 次崩溃仍未超限：崩溃预算语义已变，本 helper 的假设不再成立"
        );
        assert!(
            state.registry.find(&pid).unwrap().is_active(),
            "插件必须仍可用，否则本用例测到的是可用性门而不是预算门"
        );
    }

    /// **边界**：窗口内崩溃数**未**超过 `max_crashes` 时，重试必须被放行
    /// （`tauron_proc::CrashTracker` 的定义：`max_crashes` 是"允许的崩溃次数"，
    /// 超限的判定是 `> max_crashes`）。这条用例锁住"我们没有另造一套更严的阈值"。
    #[test]
    fn crash_window_below_the_limit_does_not_block_a_retry() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");

        // 真实崩溃一次：窗口 1，且状态机离开可用态（崩溃的既有后果）。
        let first = spawn_then_crash(&state, &fake, "com.proc");
        assert_eq!(state.proc_runtime.crash_count("com.proc"), 1);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::ErroredRetryable,
            "崩溃后不再可用——重试前必须先让状态重新可用（P0-2 可用性门）"
        );
        // 插件自报恢复（既有迁移，带 ResetCounters），窗口不动。
        state.registry.report_event(&id, Event::RetryOk).unwrap();

        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile())
            .expect("窗口未超限 + 状态可用 → 重试必须放行");
        assert_ne!(second.lease, first.lease);
        assert_eq!(second.pid, 1001, "重试必须起**新**进程");
        assert_eq!(fake.call_count(), 2);

        // 窗口推到上限（3 = `CrashLimit::default().max_crashes`）：仍未超限。
        // 同样**有界**推进（轮 2，见 `exhaust_crash_window` 的理由）。
        for _ in 0..64 {
            if state.proc_runtime.crash_count("com.proc") >= 3 {
                break;
            }
            state.proc_runtime.record_crash("com.proc");
        }
        assert_eq!(state.proc_runtime.crash_count("com.proc"), 3, "64 次内没推到 3：计数语义已变");
        assert!(
            !state.proc_runtime.is_crash_exceeded("com.proc"),
            "3 次 == 上限：上限的判定是 > max_crashes，第 4 次才算超"
        );
    }

    /// **超限**：窗口内第 4 次崩溃后，即使插件**可用**也必须拒绝启动——
    /// 且**不调用启动面**、落到 `ERRORED_USER_CONFIRM`、错误码为既有的
    /// `E_PLUGIN_DISABLED`（不可重试）。
    ///
    /// 这条用例专门锁"预算门不依赖状态门"：状态是 ENABLED（可用性门放行），
    /// 拒绝只能来自 `CrashTracker::is_exceeded`——它是本仓唯一允许判定"崩溃超限"
    /// 的地方（`tauron-proc` 定稿的 3 次 / 5min），本命令不另造阈值。
    #[test]
    fn crash_window_exceeded_refuses_a_fresh_start_even_when_the_plugin_is_available() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        exhaust_crash_window(&state, &fake, "com.proc");

        assert!(state.proc_runtime.is_crash_exceeded("com.proc"));
        assert_eq!(state.proc_runtime.crash_count("com.proc"), 4, "窗口内 4 次（> 上限 3）");
        assert_eq!(fake.call_count(), 1, "只有第一次崩溃前起过进程");

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::E_PLUGIN_DISABLED,
            "复用既有码：预算耗尽 = 插件当前不可用、需宿主显式放行"
        );
        assert!(!err.retryable, "要求人工介入的失败绝不能被标成可重试");
        assert_eq!(fake.call_count(), 1, "拒绝路径绝不调用启动面");
        assert!(
            state.registry.runtime_needs_restart(&id),
            "拒绝后不得凭空留下活租约（旧租约是崩溃租约，仍然存在）"
        );
        assert!(err.message.contains('4'), "message 必须带上真实崩溃次数：{}", err.message);
        assert!(
            err.message.contains("ERRORED_USER_CONFIRM"),
            "落点必须写进错误信息：{}",
            err.message
        );

        // 落点：状态机必须真的落到"需用户确认"（不是只改了错误信息）。
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredUserConfirm,
            "预算耗尽必须落到 ERRORED_USER_CONFIRM"
        );

        // 再点一次也不放行（幂等拒绝，不会因为重试而漂移）。
        let again = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(again.code, ErrorCode::E_PLUGIN_DISABLED);
        assert_eq!(fake.call_count(), 1);
    }

    /// **人工确认出口**：`host_registry_admin` 的 `Enable` 是既有的"用户显式放行"
    /// 入口（不新造）。它必须同时清零状态机预算与**进程侧崩溃窗口**——只清前者会
    /// 出现"用户已确认、状态机 Enabled、spawn 仍被 5 分钟窗口拒绝"的死结。
    ///
    /// 本用例同时钉住两条**既有交互**（都不是本命令引入的）：
    /// 1. 崩溃把恢复引擎的 `consecutiveFailures` 推过 `SAFEMODE_THRESHOLD`，于是
    ///    `Enable` 之后的对账又用 `SafemodeEnter` 把插件打成
    ///    `DISABLED + disabledBySafemode`（"安全模式优先于手动启用"）——因此这里
    ///    断言的是 `out.to == ENABLED`（本次迁移）与最终 `DISABLED`，而不是"最终
    ///    一定是 ENABLED"；
    /// 2. 因此"确认后能再起进程"的完整顺序是 **enable → 安全模式试验性启用
    ///    （`host_recover_trial_enable`）→ spawn**，这也正是 P0-2 可用性门要求的顺序。
    ///    本用例把三步都走一遍，证明它真的走得通（不是只写在文档里）。
    #[test]
    fn admin_enable_clears_the_crash_window_and_trial_enable_allows_spawn_again() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        exhaust_crash_window(&state, &fake, "com.proc");

        // 预算门先把插件落到"需用户确认"。
        assert_eq!(
            cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err().code,
            ErrorCode::E_PLUGIN_DISABLED
        );
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredUserConfirm
        );
        assert!(state.proc_runtime.is_crash_exceeded("com.proc"));

        // 用户在主窗显式启用（人工确认）。
        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Enable).unwrap();
        assert!(!out.illegal);
        assert_eq!(
            out.to,
            LifecycleState::Enabled,
            "ERRORED_USER_CONFIRM + ENABLE 必须回到 ENABLED（既有迁移）"
        );
        assert_eq!(
            state.proc_runtime.crash_count("com.proc"),
            0,
            "人工确认必须同时清零崩溃窗口（否则 spawn 仍被拒，用户点了没用）"
        );
        assert!(!state.proc_runtime.is_crash_exceeded("com.proc"));

        // 既有交互：安全模式优先于手动启用——但只在连续失败计数已过阈值时发生。
        // 本用例只走了 **1 次真实崩溃**（其余是直接推窗口计数，见 `exhaust_crash_window`），
        // 因此确认后不触发安全模式；安全模式那条腿由
        // `spawn_is_refused_while_disabled_by_safemode` 覆盖（那里真的崩了两次）。
        assert_eq!(
            state.recovery.lock().counter().consecutive_failures,
            1,
            "1 次真实崩溃 → 连续失败计数 1（< SAFEMODE_THRESHOLD）"
        );
        let after = state.registry.find(&id).unwrap();
        assert!(
            after.is_active(),
            "确认后插件必须处于可用状态（安全模式未介入）——这是「确认后能再 spawn」的前提"
        );
        assert!(!after.state.disabled_by_safemode);

        // 确认后必须真的能再起一个进程（窗口已清，不再被预算门拦下）。
        let handle =
            cmd_runtime_spawn(&state, "com.proc", &valid_profile()).expect("确认后必须能再次启动");
        assert_eq!(fake.call_count(), 2, "确认后必须能再次启动");
        assert_eq!(handle.pid, 1001, "必须是新的 pid");
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "新进程起好后状态必须到 RUNNING"
        );
    }

    /// 可用性门单独生效：状态机落到 `ERRORED_USER_CONFIRM`（人工闸门）而崩溃窗口
    /// **未**超限时，也必须拒绝启动——因为 `ERRORED_USER_CONFIRM` 不是可用状态。
    /// （人工确认闸门由可用性门覆盖，不再是独立分支。）
    #[test]
    fn user_confirm_state_refuses_spawn_via_the_availability_gate() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");

        // 一次崩溃 → ERRORED_RETRYABLE（崩溃窗口只有 1 次，远未超限）。
        spawn_then_crash(&state, &fake, "com.proc");
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::ErroredRetryable);
        // 走**既有**迁移落到"需用户确认"：ErroredRetryable + RETRY_EXHAUSTED。
        let out = state.registry.report_event(&id, Event::RetryExhausted).unwrap();
        assert!(!out.illegal);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::ErroredUserConfirm
        );
        assert!(
            !state.proc_runtime.is_crash_exceeded("com.proc"),
            "本用例的前提：崩溃窗口**没有**超限（拒绝只能来自可用性门）"
        );

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(!err.retryable);
        assert!(
            err.message.contains("ERRORED_USER_CONFIRM"),
            "message 必须点明当前状态：{}",
            err.message
        );
        assert_eq!(fake.call_count(), 1, "被门拦下时一次都不许再起进程");
    }

    /// 中间态同样被拒：崩溃后 `ERRORED_RETRYABLE` 不是可用状态（"还有重试预算"
    /// 不等于"现在可以起进程"）。
    #[test]
    fn errored_retryable_state_refuses_spawn_until_re_enabled() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        spawn_then_crash(&state, &fake, "com.proc");

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(err.message.contains("ERRORED_RETRYABLE"), "message={}", err.message);
        assert_eq!(fake.call_count(), 1);
        assert!(!state.proc_runtime.is_crash_exceeded("com.proc"), "窗口只有 1 次");

        // 既有路径重新可用后，重试被放行（这正是"有预算的自动重试"）。
        state.registry.report_event(&id, Event::RetryOk).unwrap();
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(handle.pid, 1001);
    }

    /// **不可用即无进程**（P0-2 单一规则）：运行中的插件被禁用时，它的 sidecar 必须
    /// 被终止——不能出现"注册表说禁用、进程还在跑"。
    #[test]
    fn disabling_a_running_plugin_terminates_its_sidecar() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "spawn 成功 → RUNNING；只有禁用一个**在运行**的插件才测得到「禁用即回收进程」"
        );
        assert!(fake.killed().is_empty(), "禁用之前不得终止");

        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Disable).unwrap();
        assert_eq!(out.to, LifecycleState::Disabled);

        assert_eq!(fake.killed(), vec![handle.pid], "禁用必须终止该插件仍活着的 sidecar");
        assert_eq!(state.registry.runtime_len(), 0, "租约必须一起回收");
        let s = state.registry.runtime_reap_stats();
        assert_eq!((s.attempts, s.terminated, s.failures), (1, 1, 0), "{s:?}");
        assert_eq!(
            cmd_runtime_health(&state, &handle.lease).unwrap_err().code,
            ErrorCode::E_LEASE_EXPIRED,
            "租约已随禁用回收"
        );

        // 禁用期间不能再起进程（可用性门）。
        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert_eq!(fake.call_count(), 1);
    }

    /// 安全模式禁用（`DISABLED + disabledBySafemode`）期间不得起进程：要让插件重新
    /// 可用，得走 `host_recover_trial_enable`（或用户确认后的 enable），**不是**放宽
    /// 这道门。
    #[test]
    fn spawn_is_refused_while_disabled_by_safemode() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        // 两次崩溃把恢复引擎推过 SAFEMODE_THRESHOLD。
        spawn_then_crash(&state, &fake, "com.proc");
        state.registry.report_event(&id, Event::RetryOk).unwrap();
        spawn_then_crash(&state, &fake, "com.proc");
        assert!(state.recovery.lock().counter().consecutive_failures >= 2);

        // 人工启用 → 对账（安全模式优先）→ DISABLED + 安全模式标志。
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Enable).unwrap();
        let entry = state.registry.find(&id).unwrap();
        assert_eq!(entry.state.state, tauron_host::lifecycle::State::Disabled);
        assert!(entry.state.disabled_by_safemode);

        let err = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_PLUGIN_DISABLED);
        assert!(
            err.message.contains("DISABLED") && err.message.contains("host_recover_trial_enable"),
            "message 必须给出**正确**的下一步动作：{}",
            err.message
        );
        assert_eq!(fake.call_count(), 2, "安全模式期间一次都不许起进程");
        assert!(
            state.registry.runtime_needs_restart(&id),
            "崩溃租约仍在表里（留给 health 报账），但语义是「没有活进程」——这正是 needs_restart"
        );

        // 既有出口：安全模式内的**试验性启用**（`host_recover_trial_enable`，不新造入口）
        // → 状态回到可用 → 这时才允许 spawn。这就是"先 enable/trial-enable，再 spawn"。
        cmd_recover_trial_enable(&state, "com.proc").unwrap();
        assert!(
            state.registry.find(&id).unwrap().is_active(),
            "trial-enable 后插件必须处于可用状态（ENABLED/RUNNING）"
        );
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile())
            .expect("trial-enable 后必须能起进程");
        assert_eq!(fake.call_count(), 3);
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            tauron_host::lifecycle::State::Running,
            "新进程起好后状态必须到 RUNNING（进程插件没有 webview 可上报 ATTACH）"
        );
        assert!(handle.pid > 0);
    }

    // ────────────────────────────────────────────────────────────
    // 缺口一：租约回收必须真的终止 sidecar（否则卸载留下孤儿进程）
    // ────────────────────────────────────────────────────────────

    /// 卸载必须按租约里的 pid 调终止面，并留痕（`terminated` 计数）。
    #[test]
    fn uninstall_terminates_the_sidecar_and_traces_it() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert!(fake.killed().is_empty(), "未卸载前不得调终止面");

        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();

        assert_eq!(fake.killed(), vec![handle.pid], "必须终止租约绑定的那个 pid");
        assert_eq!(state.registry.runtime_len(), 0, "租约必须回收");
        let s = state.registry.runtime_reap_stats();
        assert_eq!(
            (s.attempts, s.terminated, s.failures),
            (1, 1, 0),
            "真杀必须计入 terminated：{s:?}"
        );
        assert_eq!(s.last_error, None);
    }

    /// 清除（Purge）走同一条回收路径。
    #[test]
    fn purge_also_terminates_the_sidecar() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Purge).unwrap();
        assert_eq!(fake.killed(), vec![handle.pid]);
        assert_eq!(state.registry.runtime_reap_stats().terminated, 1);
    }

    /// 终止失败（权限不足等）：**卸载必须成功**，但必须留痕（不得静默吞）——
    /// 而且留痕必须能从 IPC 看到（`host_runtime_health` 的 `reap` 字段），
    /// 否则"不静默吞"只等于写日志。
    #[test]
    fn kill_failure_does_not_fail_uninstall_but_is_traced() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        fake.fail_kill_with("拒绝访问（测试模拟权限不足）");

        let out = cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();
        assert!(!out.illegal, "卸载本身必须成功");
        assert_eq!(fake.killed(), vec![handle.pid], "终止面仍必须被调用（不能因为怕失败就不调）");
        assert_eq!(state.registry.runtime_len(), 0, "租约必须消失，否则重装会撞上旧租约");
        assert!(state.registry.find(&PluginId::new("com.proc").unwrap()).is_none());

        let s = state.registry.runtime_reap_stats();
        assert_eq!((s.attempts, s.terminated, s.failures), (1, 0, 1), "{s:?}");
        assert!(
            s.last_error.as_deref().unwrap_or("").contains("权限不足"),
            "失败原因必须可查：{:?}",
            s.last_error
        );

        // 同一宿主内、**另一条**租约的 health 也必须看到这次失败（`reap` 是全局
        // 留痕）：跨 IPC 可见 = 宿主 UI 能把它显示出来，而不是只写日志。
        state
            .registry
            .install(&empty_index(), process_manifest("com.other", Some("sidecar2.exe")))
            .unwrap();
        enabled_process_plugin(&state, "com.other");
        let other = cmd_runtime_spawn(&state, "com.other", &valid_profile()).unwrap();
        let health = cmd_runtime_health(&state, &other.lease).unwrap();
        assert_eq!((health.reap.attempts, health.reap.failures), (1, 1));
        assert!(
            health.reap.last_error.as_deref().unwrap_or("").contains("权限不足"),
            "跨 IPC 的留痕必须带原因：{:?}",
            health.reap.last_error
        );
        assert_ne!(health.reap, ReapStats::default(), "留痕必须真的跨 IPC 可见");
    }

    /// 崩溃后重试（换新租约）时，旧 pid 也必须先被终止——不允许"直接覆盖表项"。
    #[test]
    fn retry_after_crash_terminates_the_previous_pid_before_replacing_the_lease() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        let first = spawn_then_crash(&state, &fake, "com.proc");
        assert!(fake.killed().is_empty(), "换新之前不得终止");

        // 崩溃后状态离开可用态：重试前必须先让它重新可用（P0-2 可用性门）。
        state.registry.report_event(&id, Event::RetryOk).unwrap();

        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(fake.killed(), vec![first.pid], "旧 pid 必须先终止再换新");
        assert_ne!(second.lease, first.lease);
        assert_eq!(state.registry.runtime_len(), 1);
        // 崩溃租约的进程早已退出 → 如实记 `already_gone`，不混进失败计数。
        let s = state.registry.runtime_reap_stats();
        assert_eq!((s.attempts, s.terminated, s.already_gone, s.failures), (1, 0, 1, 0), "{s:?}");
    }

    /// 崩溃租约是"需要重启"，活租约不是——`host_runtime_health` 的幂等语义全靠它。
    #[test]
    fn live_lease_does_not_need_restart_but_crashed_lease_does() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        assert!(state.registry.runtime_needs_restart(&id), "没租约 = 需要启动");

        enabled_process_plugin(&state, "com.proc");
        let handle = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert!(!state.registry.runtime_needs_restart(&id), "活租约不需要重启");
        fake.crash(handle.pid);
        cmd_runtime_health(&state, &handle.lease).unwrap();
        assert!(state.registry.runtime_needs_restart(&id), "崩溃后需要重启");
    }

    // ────────────────────────────────────────────────────────────
    // 状态机断链：spawn 成功必须把状态推到 RUNNING（进程插件没有 webview 可上报）
    // ────────────────────────────────────────────────────────────

    /// 全新 spawn 成功后状态必须 `ENABLED → RUNNING`（断言**注册表状态**，
    /// 不是"投过事件"）；幂等重复 spawn **不**投第二次 `ATTACH`。
    ///
    /// 判别力来自 `illegal_transitions`：`RUNNING + ATTACH` 在迁移表里没有规则，
    /// 所以"多投一次"一定会被状态机计成非法迁移——计数为 0 就证明真的没投。
    #[test]
    fn a_new_process_moves_the_plugin_from_enabled_to_running() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Enabled);

        let first = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "起了新进程就必须把状态推到 RUNNING（进程插件没有 webview 上报 ATTACH）"
        );
        assert_eq!(
            state.registry.find(&id).unwrap().state.illegal_transitions,
            0,
            "ENABLED + ATTACH 是合法迁移，不得产生非法迁移"
        );

        // 幂等重复 spawn：不得再投 ATTACH（否则会是一次非法迁移，计数会 +1）。
        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_eq!(second, first, "幂等返回既有租约");
        assert_eq!(fake.call_count(), 1, "绝不启动第二个进程");
        assert_eq!(
            state.registry.find(&id).unwrap().state.state,
            LifecycleState::Running,
            "状态不得因为重复调用而漂移"
        );
        assert_eq!(
            state.registry.find(&id).unwrap().state.illegal_transitions,
            0,
            "幂等返回不得再投 ATTACH（多投会是一条非法迁移）"
        );
    }

    /// 并发卸载窗口：进程起好后条目才消失 → **回收刚铸的租约**，绝不把孤儿句柄交出去。
    ///
    /// 真并发没法确定性复现，这里手工构造那个中间态（租约已铸、条目已消失），直接验证
    /// 补偿分支本身——它存在的唯一意义就是"孤儿进程不允许被交回调用方"。
    #[test]
    fn a_spawn_that_loses_its_entry_reclaims_the_fresh_lease() {
        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        cmd_registry_admin(&state, "com.proc", RegistryAdminOp::Uninstall).unwrap();
        assert!(state.registry.find(&id).is_none(), "条目已消失 = 并发卸载窗口");

        // 造出"启动发生在卸载之后"的中间态：租约在表里，条目却已不存在。
        let (orphan, started) = state.registry.runtime_ensure_lease(&id, || Ok(9999)).unwrap();
        assert!(started);
        assert_eq!(state.registry.runtime_len(), 1);

        let err = attach_after_spawn(&state, &id).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
        assert!(err.message.contains("已回收刚铸的租约"), "message={}", err.message);
        assert_eq!(state.registry.runtime_len(), 0, "孤儿租约必须被回收");
        assert_eq!(
            *fake.killed().last().unwrap(),
            orphan.pid,
            "回收必须真的终止那个刚起的进程（killed={:?}）",
            fake.killed()
        );
    }

    /// 无可迁移路径时不伪造事件：状态已在 `RUNNING` 时 `ATTACH` 没有规则——
    /// 保持原状（目标态已达成），非法迁移由状态机计数（可查，不静默）。
    #[test]
    fn a_redundant_attach_is_counted_but_never_faked() {
        use tauron_host::lifecycle::State as LifecycleState;

        let (state, fake) = process_state("com.proc", Some("sidecar.exe"));
        let id = PluginId::new("com.proc").unwrap();
        enabled_process_plugin(&state, "com.proc");
        let first = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();

        // 造出「状态 RUNNING + 租约已崩」这个组合：插件自报恢复并自己 ATTACH 过，
        // 随后宿主才发现旧租约已崩、于是换了新进程——此时 ATTACH 无规则可走。
        fake.crash(first.pid);
        cmd_runtime_health(&state, &first.lease).unwrap();
        state.registry.report_event(&id, Event::RetryOk).unwrap();
        state.registry.report_event(&id, Event::Attach).unwrap();
        assert_eq!(state.registry.find(&id).unwrap().state.state, LifecycleState::Running);
        assert!(state.registry.runtime_needs_restart(&id), "旧租约已崩 → 需要新进程");

        let second = cmd_runtime_spawn(&state, "com.proc", &valid_profile()).unwrap();
        assert_ne!(second.pid, first.pid, "必须真的起了新进程");
        assert_eq!(fake.call_count(), 2);

        let entry = state.registry.find(&id).unwrap();
        assert_eq!(
            entry.state.state,
            LifecycleState::Running,
            "RUNNING + ATTACH 无规则 → 保持原状（已在目标态），不伪造别的状态"
        );
        assert_eq!(
            entry.state.illegal_transitions, 1,
            "非法迁移必须被状态机计数（可查），而不是被静默吞掉"
        );
    }

    // ──────────────────────────────────────────────────────────────────────
    // R8 §1：Sink 抽象的验收标准——「换一个 sink 实现，命令结果就变」
    //
    // 这一节回答的是本次抽象**唯一**要回答的问题：平台部分能不能被替换，
    // 替换之后命令的行为是不是真的跟着变。凡是"注入假实现后命令仍然给出旧结果"
    // 的地方，都是抽象没接上。
    // ──────────────────────────────────────────────────────────────────────
    mod sink_tests {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// 用假 sink 装配一份状态（**这就是"可在测试里注入"的落地点**）。
        ///
        /// 三个字段都是 `SubstrateState` 的 `pub` 字段，在 `Arc` 化之前替换；
        /// 生产侧的同一个位置在 `tauri.rs::command_state_with_dir_and_config`。
        fn state_with_sinks(
            window: Arc<dyn WindowSink>,
            dialog: Arc<dyn DialogSink>,
            deep_link: Arc<dyn DeepLinkSink>,
        ) -> PluginRuntimeState {
            let mut substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
            substrate.window_sink = window;
            substrate.dialog_sink = dialog;
            substrate.deep_link_sink = deep_link;
            PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
                .unwrap()
        }

        fn noop_only(window: Arc<dyn WindowSink>) -> PluginRuntimeState {
            state_with_sinks(window, Arc::new(NoopDialogSink), Arc::new(NoopDeepLinkSink))
        }

        /// 记录型窗口假实现：证明命令**真的**转调了 sink（而不是自己吞掉）。
        #[derive(Default)]
        struct RecordingWindowSink {
            ops: Mutex<Vec<WindowOpRecord>>,
            /// 构造出的窗口规格（[`WindowSink::create`] 的入参逐字段留痕）。
            specs: Mutex<Vec<WindowCreateSpec>>,
            /// `relaunch` / `create` 的返回值（真实宿主由平台结果决定）。
            relaunch_ok: bool,
            create_ok: bool,
        }

        impl RecordingWindowSink {
            fn with_results(relaunch_ok: bool, create_ok: bool) -> Self {
                Self { relaunch_ok, create_ok, ..Default::default() }
            }

            fn push(&self, op: &'static str, label: Option<&str>, detail: String) {
                self.ops.lock().push(WindowOpRecord {
                    op,
                    label: label.map(str::to_string),
                    detail,
                });
            }

            fn op_names(&self) -> Vec<&'static str> {
                self.ops.lock().iter().map(|r| r.op).collect()
            }

            fn specs(&self) -> Vec<WindowCreateSpec> {
                self.specs.lock().clone()
            }
        }

        impl WindowSink for RecordingWindowSink {
            fn minimize(&self, label: &str) -> HostResult<()> {
                self.push("minimize", Some(label), String::new());
                Ok(())
            }

            fn maximize(&self, label: &str) -> HostResult<()> {
                self.push("maximize", Some(label), String::new());
                Ok(())
            }

            fn restore(&self, label: &str) -> HostResult<()> {
                self.push("restore", Some(label), String::new());
                Ok(())
            }

            fn close(&self, label: &str) -> HostResult<()> {
                self.push("close", Some(label), String::new());
                Ok(())
            }

            fn set_position(&self, label: &str, x: i32, y: i32) -> HostResult<()> {
                self.push("set_position", Some(label), format!("x={x},y={y}"));
                Ok(())
            }

            fn set_size(&self, label: &str, width: u32, height: u32) -> HostResult<()> {
                self.push("set_size", Some(label), format!("width={width},height={height}"));
                Ok(())
            }

            fn quit(&self) -> HostResult<()> {
                self.push("quit", None, String::new());
                Ok(())
            }

            fn relaunch(&self) -> HostResult<bool> {
                self.push("relaunch", None, String::new());
                Ok(self.relaunch_ok)
            }

            fn create(&self, spec: &WindowCreateSpec) -> HostResult<bool> {
                self.push("create", Some(&spec.label), spec.url.clone());
                self.specs.lock().push(spec.clone());
                Ok(self.create_ok)
            }
        }

        /// 记录型对话框假实现。
        struct RecordingDialogSink {
            open: Option<String>,
            save: Option<String>,
            confirm: bool,
            messages: Mutex<Vec<(String, String, String)>>,
        }

        impl DialogSink for RecordingDialogSink {
            fn native_supported(&self) -> bool {
                true
            }

            fn open_file(&self, _multiple: bool, _directory: bool) -> HostResult<Option<String>> {
                Ok(self.open.clone())
            }

            fn save_file(&self, _default_name: Option<&str>) -> HostResult<Option<String>> {
                Ok(self.save.clone())
            }

            fn message(&self, kind: &str, title: &str, body: &str) -> HostResult<()> {
                self.messages.lock().push((kind.to_string(), title.to_string(), body.to_string()));
                Ok(())
            }

            fn confirm(&self, _title: &str, _body: &str) -> HostResult<bool> {
                Ok(self.confirm)
            }
        }

        /// 记录型深链接假实现。
        #[derive(Default)]
        struct RecordingDeepLinkSink {
            calls: Mutex<Vec<String>>,
            native: bool,
        }

        impl DeepLinkSink for RecordingDeepLinkSink {
            fn register(&self, protocol: &str) -> HostResult<()> {
                self.calls.lock().push(format!("register:{protocol}"));
                Ok(())
            }

            fn unregister(&self, protocol: &str) -> HostResult<()> {
                self.calls.lock().push(format!("unregister:{protocol}"));
                Ok(())
            }

            fn native_supported(&self) -> bool {
                self.native
            }
        }

        /// 顺序观察型窗口假实现：在 `relaunch` 被调用的**那一刻**读注册表标志。
        ///
        /// 注册表句柄在装配后注入（`OnceLock`）：sink 先于 `PluginRuntimeState`
        /// 存在，而 `with_substrate` 会自建注册表——只有事后注入才能观察到"命令真的
        /// 用的那份注册表"。
        #[derive(Default)]
        struct RelaunchOrderingSink {
            registry: std::sync::OnceLock<Arc<Registry>>,
            observed: Mutex<Option<bool>>,
            calls: AtomicUsize,
        }

        impl RelaunchOrderingSink {
            fn bind_registry(&self, registry: Arc<Registry>) {
                let _ = self.registry.set(registry);
            }

            fn observed_flag(&self) -> Option<bool> {
                *self.observed.lock()
            }

            fn calls(&self) -> usize {
                self.calls.load(Ordering::SeqCst)
            }
        }

        impl WindowSink for RelaunchOrderingSink {
            fn minimize(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn maximize(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn restore(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn close(&self, _label: &str) -> HostResult<()> {
                Ok(())
            }
            fn set_position(&self, _label: &str, _x: i32, _y: i32) -> HostResult<()> {
                Ok(())
            }
            fn set_size(&self, _label: &str, _w: u32, _h: u32) -> HostResult<()> {
                Ok(())
            }
            fn quit(&self) -> HostResult<()> {
                Ok(())
            }

            fn relaunch(&self) -> HostResult<bool> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                // 观察点：此刻注册表里 `com.a` 的安全模式标志是什么？
                // true = 对账已经在 relaunch **之前**跑完了。
                let flag = self.registry.get().and_then(|r| {
                    r.list_all()
                        .into_iter()
                        .find(|p| p.id == "com.a")
                        .map(|p| p.disabled_by_safemode)
                });
                *self.observed.lock() = flag;
                Ok(true)
            }

            fn create(&self, _spec: &WindowCreateSpec) -> HostResult<bool> {
                Ok(true)
            }
        }

        /// 轮 11 第三批：深链接注册是**应用级**操作（写 `shell_ext` + 声明 topic，且会
        /// 注销上一个协议），所以仅主窗；越权时 **sink 一个字节都不许被碰**——否则
        /// "拒绝"仍然会留下副作用（旧协议的 OS 关联被摘掉）。
        #[test]
        fn deep_link_registration_is_main_window_only_and_never_touches_the_sink_when_denied() {
            let sink = Arc::new(RecordingDeepLinkSink::default());
            let state = state_with_sinks(
                Arc::new(MemoryWindowSink::new()),
                Arc::new(NoopDialogSink),
                sink.clone(),
            );

            // 对照基线：主窗注册 → sink 真的被调用。
            cmd_deep_link_register_as(&Caller::MainWindow, &state, "tauron".to_string()).unwrap();
            assert_eq!(sink.calls.lock().clone(), vec!["register:tauron"]);

            // 插件接管 → 拒绝；sink 停在上一行（没有 register:evil，也没有 unregister:tauron）。
            let err = cmd_deep_link_register_as(&plugin_caller("p.a"), &state, "evil".to_string())
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
            assert_eq!(
                sink.calls.lock().clone(),
                vec!["register:tauron"],
                "拒绝路径不得碰 sink（连旧值的 unregister 都不许发生）"
            );
            assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("tauron"));

            // 主窗换协议 → 恰好是"先注销旧值、再注册新值"（证明拒绝不是把命令整体关死）。
            cmd_deep_link_register_as(&Caller::MainWindow, &state, "other".to_string()).unwrap();
            assert_eq!(
                sink.calls.lock().clone(),
                vec!["register:tauron", "unregister:tauron", "register:other"]
            );
        }

        /// `tauron.rs` 的平台操作搬到 sink 之后，命令包装器必须真的调它。
        #[test]
        fn swapping_the_window_sink_changes_the_command_result() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            let label = "plugin-com.a";

            cmd_window_minimize(&state, label).unwrap();
            cmd_window_maximize(&state, label).unwrap();
            cmd_window_restore(&state, label).unwrap();
            cmd_window_close(&state, label).unwrap();
            cmd_window_set_position(&state, label, 7, 9).unwrap();
            cmd_window_set_size(&state, label, 800, 600).unwrap();
            cmd_window_quit(&state).unwrap();
            // 真实宿主才会返回 true；假实现返回 true 时命令必须如实转达。
            let relaunch = cmd_window_relaunch_as(&Caller::MainWindow, &state).unwrap();

            assert_eq!(
                sink.op_names(),
                vec![
                    "minimize",
                    "maximize",
                    "restore",
                    "close",
                    "set_position",
                    "set_size",
                    "quit",
                    "relaunch",
                ],
                "窗口命令必须逐条转调 sink（顺序 = 调用顺序）"
            );
            assert_eq!(
                sink.ops.lock().iter().filter(|r| r.label.as_deref() == Some(label)).count(),
                6,
                "带目标的六条操作必须把宿主侧的 label 原样传给 sink"
            );
            assert!(relaunch.relaunch_requested, "sink 说重启已请求 → 命令必须如实回传 true");

            // 反向对照：**换回缺省（进程内）sink，同一命令给出不同结果**。
            let degraded = CommandState::new();
            let out = cmd_window_relaunch_as(&Caller::MainWindow, &degraded).unwrap();
            assert!(
                !out.relaunch_requested,
                "缺省 MemoryWindowSink 没有重启原语，必须如实回 false"
            );
            assert!(out.reason.is_some(), "降级必须带原因，而不是一个无解释的 false");
        }

        /// 注入返回 `Some("x")` 的假对话框实现 → `cmd_dialog_open` 必须回 `Some("x")`，
        /// 而不是恒 `None`。这条就是"换 sink 即换结果"在对话框面的形态。
        #[test]
        fn swapping_the_dialog_sink_changes_the_command_result() {
            let dialog = Arc::new(RecordingDialogSink {
                open: Some("/tmp/picked.png".to_string()),
                save: Some("/tmp/out.txt".to_string()),
                confirm: true,
                messages: Mutex::new(Vec::new()),
            });
            let state = state_with_sinks(
                Arc::new(MemoryWindowSink::new()),
                dialog.clone(),
                Arc::new(NoopDeepLinkSink),
            );

            assert_eq!(
                match cmd_dialog_open(&state, false, false).unwrap() {
                    ProviderResult::Value(value) => value,
                    ProviderResult::Unsupported(_) =>
                        panic!("recording provider reports supported"),
                }
                .as_deref(),
                Some("/tmp/picked.png")
            );
            assert_eq!(
                match cmd_dialog_save(&state, Some("a.txt")).unwrap() {
                    ProviderResult::Value(value) => value,
                    ProviderResult::Unsupported(_) =>
                        panic!("recording provider reports supported"),
                }
                .as_deref(),
                Some("/tmp/out.txt")
            );
            assert_eq!(cmd_dialog_confirm(&state, "t", "m").unwrap(), ProviderResult::Value(true));
            assert_eq!(
                cmd_dialog_message(&state, "t", "m", Some("warning")).unwrap(),
                ProviderResult::Value(())
            );
            assert_eq!(
                dialog.messages.lock().clone(),
                vec![("warning".to_string(), "t".to_string(), "m".to_string())],
                "kind/title/body 必须原样到达 sink（kind 已过闭集校验）"
            );

            // 反向对照：缺省 NoopDialogSink 对同一入参返回取消。
            let degraded = CommandState::new();
            assert!(matches!(
                cmd_dialog_open(&degraded, false, false).unwrap(),
                ProviderResult::Unsupported(_)
            ));
            assert!(matches!(
                cmd_dialog_confirm(&degraded, "t", "m").unwrap(),
                ProviderResult::Unsupported(_)
            ));
        }

        // ── R9：五域 + 品牌/主题 的「换 sink 即换结果」与诚实降级证据 ──

        /// 记录型菜单假实现（`native_supported = true`）。
        #[derive(Default)]
        struct RecordingMenuSink {
            ops: Mutex<Vec<&'static str>>,
        }

        impl MenuSink for RecordingMenuSink {
            fn native_supported(&self) -> bool {
                true
            }
            fn set_menu(&self, _spec: &MenuSpec) -> HostResult<bool> {
                self.ops.lock().push("set_menu");
                Ok(true)
            }
            fn popup(&self, _spec: &MenuSpec) -> HostResult<bool> {
                self.ops.lock().push("popup");
                Ok(true)
            }
            fn reset(&self) -> HostResult<bool> {
                self.ops.lock().push("reset");
                Ok(true)
            }
        }

        fn two_item_menu() -> MenuSpec {
            MenuSpec {
                items: vec![
                    MenuItemSpec {
                        id: "file.open".into(),
                        label: "打开".into(),
                        event: Some("app.menu.open".into()),
                        enabled: true,
                    },
                    MenuItemSpec {
                        id: "file.exit".into(),
                        label: "退出".into(),
                        event: None,
                        enabled: true,
                    },
                ],
            }
        }

        #[test]
        fn swapping_the_menu_sink_changes_the_command_result() {
            let sink = Arc::new(RecordingMenuSink::default());
            let mut substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
            substrate.menu_sink = sink.clone();
            let state =
                PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
                    .unwrap();

            let spec = two_item_menu();
            match cmd_menu_set(&state, &spec).unwrap() {
                ProviderResult::Value(o) => {
                    assert!(o.applied);
                    assert_eq!(o.item_count, 2);
                }
                ProviderResult::Unsupported(_) => panic!("recording menu sink reports supported"),
            }
            cmd_menu_popup(&state, &spec).unwrap();
            cmd_menu_reset(&state).unwrap();
            assert_eq!(*sink.ops.lock(), vec!["set_menu", "popup", "reset"]);

            // 反向对照：缺省 MemoryMenuSink 恒不支持 → Unsupported。
            let degraded = CommandState::new();
            assert!(matches!(
                cmd_menu_set(&degraded, &spec).unwrap(),
                ProviderResult::Unsupported(_)
            ));
            // 空菜单 / 重复 id 被闭集校验拒绝（参数错用参数错码）。
            let empty = MenuSpec { items: vec![] };
            assert_eq!(
                cmd_menu_set(&degraded, &empty).unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );
            let dup = MenuSpec {
                items: vec![
                    MenuItemSpec { id: "a".into(), label: "A".into(), event: None, enabled: true },
                    MenuItemSpec { id: "a".into(), label: "B".into(), event: None, enabled: true },
                ],
            };
            assert_eq!(
                cmd_menu_set(&degraded, &dup).unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );
            // 主窗专属：插件主体被代码层拒绝。
            assert_eq!(
                cmd_menu_set_as(&Caller::Plugin("com.p".into()), &degraded, &spec)
                    .unwrap_err()
                    .code,
                ErrorCode::E_AUTH_DENIED
            );
            assert_eq!(
                cmd_tray_remove_as(&Caller::Plugin("com.p".into()), &degraded).unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
        }

        /// P2-4 修正的**正向证据**：`host_capabilities` 的两列由 sink 实际可用性推导——
        /// 注入可用 sink 后，同一域必须**从 unsupported 移入 families**（换装配即换结论）。
        #[test]
        fn host_capabilities_derives_domains_from_injected_sinks() {
            // 缺省：menu 在 unsupported（MemoryMenuSink 不建菜单）。
            let degraded = CommandState::new();
            let before = cmd_host_capabilities(&degraded).unwrap();
            assert!(before.unsupported.iter().any(|u| u.domain == "menu"));
            assert!(!before.families.contains(&"menu".to_string()));

            // 注入可用菜单 sink + 配置 fs 允许根 → 两域都移入 families。
            let dir = tempfile::tempdir().unwrap();
            let mut substrate = SubstrateState::with_adapter_config(
                &AdapterConfig::default().with_fs_roots(vec![dir.path().to_path_buf()]),
            );
            substrate.menu_sink = Arc::new(RecordingMenuSink::default());
            let state =
                PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
                    .unwrap();

            let after = cmd_host_capabilities(&state).unwrap();
            for domain in ["menu", "fs"] {
                assert!(
                    after.families.contains(&domain.to_string()),
                    "注入可用 sink 后 `{domain}` 必须移入 families"
                );
                assert!(
                    !after.unsupported.iter().any(|u| u.domain == domain),
                    "`{domain}` 已可用，不得再留在 unsupported"
                );
            }
        }

        #[test]
        fn fs_commands_are_confined_to_allowed_roots() {
            let t = tempfile::tempdir().unwrap();
            let root = t.path().to_path_buf();
            let cfg = AdapterConfig::default().with_fs_roots(vec![root.clone()]);
            let state = CommandState::with_adapter_config(cfg);

            let file = root.join("hello.txt");
            match cmd_fs_write(&state, file.to_str().unwrap(), "你好").unwrap() {
                ProviderResult::Value(o) => assert_eq!(o.bytes, 6),
                ProviderResult::Unsupported(_) => panic!("fs 真实现（std::fs）必须可用"),
            }
            match cmd_fs_read(&state, file.to_str().unwrap(), None).unwrap() {
                ProviderResult::Value(o) => {
                    assert_eq!(o.text, "你好");
                    assert!(!o.truncated);
                }
                ProviderResult::Unsupported(_) => panic!("fs 真实现必须可用"),
            }
            match cmd_fs_list(&state, root.to_str().unwrap()).unwrap() {
                ProviderResult::Value(o) => assert!(o.iter().any(|e| e.name == "hello.txt")),
                ProviderResult::Unsupported(_) => panic!("fs 真实现必须可用"),
            }
            match cmd_fs_stat(&state, file.to_str().unwrap()).unwrap() {
                ProviderResult::Value(o) => {
                    assert!(o.is_file);
                    assert_eq!(o.size, 6);
                }
                ProviderResult::Unsupported(_) => panic!("fs 真实现必须可用"),
            }

            // 路径穿越：`../evil.txt` 逃出根目录 → E_AUTH_DENIED（不落盘）。
            let escape = root.join("..").join("evil.txt");
            assert_eq!(
                cmd_fs_write(&state, escape.to_str().unwrap(), "x").unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
            assert!(!t.path().parent().unwrap().join("evil.txt").exists(), "穿越写入不得发生");

            // 空允许根 = 该域不可用 → 如实 Unsupported（不伪造成功）。
            let disabled = CommandState::new();
            assert!(matches!(
                cmd_fs_read(&disabled, file.to_str().unwrap(), None).unwrap(),
                ProviderResult::Unsupported(_)
            ));
        }

        #[test]
        fn http_request_is_scope_checked_then_degrades_honestly() {
            let ok = HttpRequestSpec {
                method: "get".into(),
                url: "https://example.com".into(),
                headers: Default::default(),
                body: None,
                timeout_ms: None,
                max_bytes: None,
            };

            // V4 默认 fail-closed：即使 URL 语法合法，没有 domain scope 也不能发请求。
            let denied = CommandState::new();
            assert_eq!(cmd_http_request(&denied, &ok).unwrap_err().code, ErrorCode::E_AUTH_DENIED);

            let cfg = AdapterConfig::default().with_http_policy(
                tauron_host::NetworkPolicy::public_https(vec![tauron_host::DomainRule::exact(
                    "example.com",
                )]),
            );
            let scoped = CommandState::with_adapter_config(cfg);
            // scope 合法，但缺省 UnavailableHttpSink 仍如实 Unsupported（不伪造响应）。
            assert!(matches!(
                cmd_http_request(&scoped, &ok).unwrap(),
                ProviderResult::Unsupported(_)
            ));

            let bad_method = HttpRequestSpec { method: "DELETE".into(), ..ok.clone() };
            assert_eq!(
                cmd_http_request(&scoped, &bad_method).unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );
            let bad_url = HttpRequestSpec { url: "file:///etc/passwd".into(), ..ok.clone() };
            assert_eq!(
                cmd_http_request(&scoped, &bad_url).unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );
            let outside = HttpRequestSpec { url: "https://other.example.net".into(), ..ok.clone() };
            assert_eq!(
                cmd_http_request(&scoped, &outside).unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
        }

        /// 单跳契约假 sink（轮 48 / A96）：按脚本逐跳回放响应，记录每次收到的 spec。
        /// 脚本耗尽后重复最后一项（hop 超限场景要一直 302）。
        #[derive(Debug)]
        struct ScriptedHttpSink {
            script: Vec<HttpResponseSpec>,
            cursor: std::sync::Mutex<usize>,
            hops: std::sync::Mutex<Vec<HttpRequestSpec>>,
        }

        impl ScriptedHttpSink {
            fn new(script: Vec<HttpResponseSpec>) -> Arc<Self> {
                Arc::new(Self {
                    script,
                    cursor: std::sync::Mutex::new(0),
                    hops: std::sync::Mutex::new(Vec::new()),
                })
            }
        }

        impl HttpSink for ScriptedHttpSink {
            fn native_supported(&self) -> bool {
                true
            }

            fn network_enforcement(&self) -> tauron_host::NetworkEnforcement {
                tauron_host::NetworkEnforcement::RedirectAndDns
            }

            fn request(
                &self,
                spec: &HttpRequestSpec,
                _policy: &tauron_host::NetworkPolicy,
            ) -> HostResult<ProviderResult<HttpResponseSpec>> {
                self.hops.lock().unwrap().push(spec.clone());
                let mut cursor = self.cursor.lock().unwrap();
                let index = (*cursor).min(self.script.len() - 1);
                *cursor += 1;
                Ok(ProviderResult::Value(self.script[index].clone()))
            }
        }

        fn http_response(
            status: u16,
            location: Option<&str>,
            resolved: &[&str],
        ) -> HttpResponseSpec {
            let mut headers = std::collections::BTreeMap::new();
            if let Some(location) = location {
                headers.insert("location".to_string(), location.to_string());
            }
            HttpResponseSpec {
                status,
                headers,
                body: String::new(),
                truncated: false,
                resolved_addrs: resolved.iter().map(|s| s.to_string()).collect(),
            }
        }

        fn http_state(
            sink: Arc<ScriptedHttpSink>,
            domains: &[&str],
            max_redirects: u8,
        ) -> CommandState {
            let mut policy = tauron_host::NetworkPolicy::public_https(
                domains.iter().map(|d| tauron_host::DomainRule::exact(*d)).collect(),
            );
            policy.max_redirects = max_redirects;
            let cfg = AdapterConfig::default().with_http_policy(policy).with_http_sink(sink);
            CommandState::with_adapter_config(cfg)
        }

        fn http_spec(url: &str, headers: &[(&str, &str)]) -> HttpRequestSpec {
            HttpRequestSpec {
                method: "GET".into(),
                url: url.into(),
                headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                body: None,
                timeout_ms: None,
                max_bytes: None,
            }
        }

        #[test]
        fn http_redirect_flow_follows_hops_with_rfc_3986_resolution() {
            let sink = ScriptedHttpSink::new(vec![
                http_response(302, Some("/next"), &["93.184.216.34"]),
                http_response(200, None, &["93.184.216.34"]),
            ]);
            let state = http_state(sink.clone(), &["example.com"], 8);
            let spec = http_spec("https://example.com/start", &[("Authorization", "Bearer t")]);
            let ProviderResult::Value(response) = cmd_http_request(&state, &spec).unwrap() else {
                panic!("脚本以 200 结束，必须是 Supported");
            };
            assert_eq!(response.status, 200);
            let hops = sink.hops.lock().unwrap();
            assert_eq!(hops.len(), 2, "宿主必须逐跳发请求（sink 契约：不跟 redirect）");
            assert_eq!(
                hops[1].url, "https://example.com/next",
                "相对 Location 必须按 RFC 3986 解析成绝对 URL 再发"
            );
            assert_eq!(
                hops[1].headers.get("Authorization").map(String::as_str),
                Some("Bearer t"),
                "同源 redirect 允许转凭据"
            );
        }

        #[test]
        fn http_redirect_cross_origin_strips_credentials_and_denied_target_never_sent() {
            let sink = ScriptedHttpSink::new(vec![
                http_response(302, Some("https://cdn.example.com/asset"), &["93.184.216.34"]),
                http_response(200, None, &["93.184.216.34"]),
            ]);
            let state = http_state(sink.clone(), &["example.com", "cdn.example.com"], 8);
            let spec = http_spec(
                "https://example.com/start",
                &[("Authorization", "Bearer t"), ("Cookie", "sid=1")],
            );
            let _ = cmd_http_request(&state, &spec).unwrap();
            let hops = sink.hops.lock().unwrap();
            assert_eq!(hops.len(), 2);
            assert!(!hops[1].headers.contains_key("Authorization"), "跨源不得转 Authorization");
            assert!(!hops[1].headers.contains_key("Cookie"), "跨源不得转 Cookie");

            // 策略外目标 → 拒绝，且第二跳从未发出。
            let denied_sink = ScriptedHttpSink::new(vec![http_response(
                302,
                Some("https://evil.example.net/"),
                &["93.184.216.34"],
            )]);
            let denied_state = http_state(denied_sink.clone(), &["example.com"], 8);
            let error = cmd_http_request(&denied_state, &spec).unwrap_err();
            assert_eq!(error.code, ErrorCode::E_AUTH_DENIED);
            assert!(error.message.contains("outside scope"), "{}", error.message);
            assert_eq!(denied_sink.hops.lock().unwrap().len(), 1, "被拒的下一跳不得发出");
        }

        #[test]
        fn http_redirect_loop_is_bounded_by_policy_max_redirects() {
            let sink =
                ScriptedHttpSink::new(vec![http_response(302, Some("/loop"), &["93.184.216.34"])]);
            let state = http_state(sink.clone(), &["example.com"], 2);
            let spec = http_spec("https://example.com/start", &[]);
            let error = cmd_http_request(&state, &spec).unwrap_err();
            assert_eq!(error.code, ErrorCode::E_AUTH_DENIED);
            assert!(error.message.contains("redirect hop"), "{}", error.message);
            assert_eq!(
                sink.hops.lock().unwrap().len(),
                3,
                "初始跳 + 两次授权跳后第 3 次越限，不再发出"
            );
        }

        #[test]
        fn http_provider_resolution_is_rechecked_against_private_network_policy() {
            // 域名公开、解析地址私有 → deny（DNS rebinding 面）。
            let sink = ScriptedHttpSink::new(vec![http_response(200, None, &["10.0.0.5"])]);
            let state = http_state(sink, &["example.com"], 8);
            let spec = http_spec("https://example.com/start", &[]);
            let error = cmd_http_request(&state, &spec).unwrap_err();
            assert_eq!(error.code, ErrorCode::E_AUTH_DENIED);
            assert!(error.message.contains("private"), "{}", error.message);

            // provider 回报不可解析地址 → fail-closed。
            let bad = ScriptedHttpSink::new(vec![http_response(200, None, &["not-an-ip"])]);
            let bad_state = http_state(bad, &["example.com"], 8);
            let error = cmd_http_request(&bad_state, &spec).unwrap_err();
            assert_eq!(error.code, ErrorCode::E_AUTH_DENIED);
            assert!(error.message.contains("unparseable"), "{}", error.message);
        }

        #[test]
        fn http_provider_without_redirect_enforcement_is_rejected_before_any_request() {
            // 声称 native 支持但只做 UrlOnly：逐跳/DNS 复检无从执行 → 拒发（不是裸发）。
            #[derive(Debug)]
            struct UrlOnlySink;
            impl HttpSink for UrlOnlySink {
                fn native_supported(&self) -> bool {
                    true
                }

                fn request(
                    &self,
                    _spec: &HttpRequestSpec,
                    _policy: &tauron_host::NetworkPolicy,
                ) -> HostResult<ProviderResult<HttpResponseSpec>> {
                    panic!("UrlOnly provider 不得收到请求");
                }
            }
            let cfg = AdapterConfig::default()
                .with_http_policy(tauron_host::NetworkPolicy::public_https(vec![
                    tauron_host::DomainRule::exact("example.com"),
                ]))
                .with_http_sink(Arc::new(UrlOnlySink));
            let state = CommandState::with_adapter_config(cfg);
            let spec = http_spec("https://example.com/start", &[]);
            assert!(matches!(
                cmd_http_request(&state, &spec).unwrap(),
                ProviderResult::Unsupported(_)
            ));
        }

        /// 假更新端点：直接返回给定清单（不起网络）。
        struct FakeEndpoint {
            manifest: Option<tauron_distribute::UpdateManifest>,
        }

        impl tauron_distribute::EndpointClient for FakeEndpoint {
            fn fetch_manifest(
                &self,
                _current_version: &str,
            ) -> tauron_distribute::DistributeResult<Option<tauron_distribute::UpdateManifest>>
            {
                Ok(self.manifest.clone())
            }
        }

        #[test]
        fn updater_status_must_say_whether_the_ledger_came_from_a_stub() {
            // 轮 33：`state` 字符串本身分不出"模拟推进"与"真实推进"。桩把 `update_state`
            // 推到 `downloaded:<v>` / `installed:<v>` 是**如实**的进程内账本行为（写侧的
            // `reason` 说清楚了），但读侧只看到 `state` 就会把"点了一下模拟安装"显示成
            // "已安装 2.0.0"。所以 provenance 位必须与 `state` 成对回吐。
            let state = CommandState::new();
            let fresh = cmd_updater_status(&state).unwrap();
            assert_eq!(fresh.state, None);
            assert!(!fresh.state_simulated, "没有状态时不得凭空声称它是模拟来的");

            cmd_market_download(&state, Some("2.0.0")).unwrap();
            let after_download = cmd_updater_status(&state).unwrap();
            assert_eq!(after_download.state.as_deref(), Some("downloaded:2.0.0"));
            assert!(
                after_download.state_simulated,
                "桩推进的 state 必须带 provenance，否则读侧无从分辨"
            );

            cmd_market_install(&state, Some("2.0.0")).unwrap();
            let after_install = cmd_updater_status(&state).unwrap();
            assert_eq!(after_install.state.as_deref(), Some("installed:2.0.0"));
            assert!(after_install.state_simulated);
        }

        #[test]
        fn updater_check_runs_tauron_distribute_and_status_reads_the_ledger() {
            // 未注入端点 → 如实 Unsupported；`update_state` 仍可读（真实读取方）。
            let degraded = CommandState::new();
            degraded.shell_ext.lock().update_state = Some("downloaded:2.0.0".to_string());
            assert!(matches!(
                cmd_updater_check(&degraded, "1.0.0").unwrap(),
                ProviderResult::Unsupported(_)
            ));
            let s = cmd_updater_status(&degraded).unwrap();
            assert!(!s.available);
            assert_eq!(s.state.as_deref(), Some("downloaded:2.0.0"));
            // 绕过桩、直接写账本 → provenance 保持 false（真实装配腿落地后就是这么写的）。
            assert!(!s.state_simulated, "未经桩的账本写入不得被标成模拟");
            assert!(s.reason.is_some(), "未配置必须带原因");
            assert_eq!(
                cmd_updater_check(&degraded, "  ").unwrap_err().code,
                ErrorCode::E_INVALID_MANIFEST
            );

            // 注入端点 → 真跑 `tauron-distribute::check_for_update`；
            // 安装身份走 §9.1 正式路径（数据目录 load_or_create，持久化）。
            let install_dir = tempfile::tempdir().unwrap();
            let identity = Arc::new(
                tauron_distribute::InstallationIdentity::load_or_create(install_dir.path())
                    .unwrap(),
            );
            assert!(identity.is_durable());
            let mut substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
            substrate.updater_sink = Arc::new(DistributeUpdaterSink::with_endpoint(
                Arc::new(FakeEndpoint {
                    manifest: Some(tauron_distribute::UpdateManifest {
                        version: "2.0.0".into(),
                        url: "https://example.com/app.zip".into(),
                        signature: "abc123def456".into(),
                        release_date: "2026-09-27T00:00:00Z".into(),
                        platform_notes: Default::default(),
                        // 检查侧不解释这些字段（执行侧才要求 `sha256`），留空即
                        // 「清单只声明了版本/URL/签名」这一条被测路径。
                        sha256: None,
                        signature_algorithm: None,
                        public_key_id: None,
                        min_host_version: None,
                        abi: None,
                        rollback_policy: None,
                    }),
                }),
                identity.clone(),
            ));
            let state =
                PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
                    .unwrap();
            match cmd_updater_check(&state, "1.0.0").unwrap() {
                ProviderResult::Value(o) => {
                    assert!(o.available);
                    assert_eq!(o.version.as_deref(), Some("2.0.0"));
                    assert!(!o.degraded);
                }
                ProviderResult::Unsupported(_) => panic!("注入端点后必须可用"),
            }
            assert!(cmd_updater_status(&state).unwrap().available);
            // 端点清单签名为空 → 如实 `degraded`（SignatureInvalid），不谎报有更新。
            let mut substrate2 = SubstrateState::with_adapter_config(&AdapterConfig::default());
            substrate2.updater_sink = Arc::new(DistributeUpdaterSink::with_endpoint(
                Arc::new(FakeEndpoint {
                    manifest: Some(tauron_distribute::UpdateManifest {
                        version: "2.0.0".into(),
                        url: "https://example.com/app.zip".into(),
                        signature: String::new(),
                        release_date: "2026-09-27T00:00:00Z".into(),
                        platform_notes: Default::default(),
                        sha256: None,
                        signature_algorithm: None,
                        public_key_id: None,
                        min_host_version: None,
                        abi: None,
                        rollback_policy: None,
                    }),
                }),
                identity,
            ));
            let state2 =
                PluginRuntimeState::with_substrate(Arc::new(substrate2), AdapterConfig::default())
                    .unwrap();
            match cmd_updater_check(&state2, "1.0.0").unwrap() {
                ProviderResult::Value(o) => {
                    assert!(!o.available);
                    assert!(o.degraded);
                }
                ProviderResult::Unsupported(_) => panic!("注入端点后必须可用"),
            }
        }

        /// R2-8 的分桶必须能在**装配主路径**上被证明：同一份端点清单，`Batch1`
        /// 下桶内安装看得见更新、桶外安装看不见，且 `host_updater_status` 如实
        /// 回吐当前百分比。
        ///
        /// 为什么要单独写这条：`with_endpoint` 便捷口把批次钉在 `Batch100`，而
        /// `check_for_update` 在 100% 覆盖下**跳过**分桶判定（`percentage < 100`
        /// 才比模）——只测那条等于没测灰度，桶外安装照样拿到更新也发现不了。
        #[test]
        fn grayscale_batch_decides_update_visibility_per_installation() {
            let manifest = tauron_distribute::UpdateManifest {
                version: "2.0.0".into(),
                url: "https://example.com/app.zip".into(),
                signature: "abc123def456".into(),
                release_date: "2026-09-27T00:00:00Z".into(),
                platform_notes: Default::default(),
                sha256: None,
                signature_algorithm: None,
                public_key_id: None,
                min_host_version: None,
                abi: None,
                rollback_policy: None,
            };
            let state_for = |id: &str| {
                let mut substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
                substrate.updater_sink =
                    Arc::new(DistributeUpdaterSink::with_endpoint_in_grayscale(
                        Arc::new(FakeEndpoint { manifest: Some(manifest.clone()) }),
                        Arc::new(
                            tauron_distribute::InstallationIdentity::from_id(id)
                                .expect("测试安装身份必须合法"),
                        ),
                        tauron_distribute::GrayscalePolicy {
                            current: tauron_distribute::GrayscaleBatch::Batch1,
                            ..Default::default()
                        },
                    ));
                PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
                    .unwrap()
            };

            // Batch1 只覆盖 `user_hash % 100 == 0`：在前 200 个稳定 id 里取出桶内/桶外各一。
            let inside = (0..200u32)
                .map(|seq| format!("tauron-bucket-test-{seq}"))
                .find(|id| tauron_distribute::InstallationIdentity::hash_id(id).is_multiple_of(100))
                .expect("200 个稳定 id 里必须有桶内的");
            let outside = (0..200u32)
                .map(|seq| format!("tauron-bucket-test-{seq}"))
                .find(|id| {
                    !tauron_distribute::InstallationIdentity::hash_id(id).is_multiple_of(100)
                })
                .expect("200 个稳定 id 里必须有桶外的");

            match cmd_updater_check(&state_for(&inside), "1.0.0").unwrap() {
                ProviderResult::Value(o) => {
                    assert!(o.available, "桶内安装必须看见更新：{o:?}");
                    assert!(!o.degraded);
                }
                ProviderResult::Unsupported(_) => panic!("注入端点后必须可用"),
            }
            match cmd_updater_check(&state_for(&outside), "1.0.0").unwrap() {
                ProviderResult::Value(o) => {
                    assert!(!o.available, "桶外安装不得看见更新：{o:?}");
                    assert!(!o.degraded, "灰度未覆盖是运维事实，不是端点故障，不得标成 degraded");
                    assert_eq!(o.reason.as_deref(), Some("灰度批次未覆盖该用户"));
                }
                ProviderResult::Unsupported(_) => panic!("注入端点后必须可用"),
            }
            let status = cmd_updater_status(&state_for(&inside)).unwrap();
            assert_eq!(status.grayscale_percent, 1, "批次百分比必须如实回吐给状态面");
            assert!(!status.crash_gate_stopped);
        }

        /// 装配方控制面：`advance_grayscale` / `update_crash_gate` 不是装饰性方法，
        /// 它们改的就是 `host_updater_status` 回吐的两列事实。没有这条测试，
        /// `grayscale_percent` 恒 1、`crash_gate_stopped` 恒 `false` 就成了
        /// 「字段宣称有门禁、门禁永不被拨动」的孤儿逻辑。
        #[test]
        fn grayscale_and_crash_gate_mutators_move_the_observable_status() {
            let mut substrate = SubstrateState::with_adapter_config(&AdapterConfig::default());
            let sink = Arc::new(DistributeUpdaterSink::with_endpoint_in_grayscale(
                Arc::new(FakeEndpoint { manifest: None }),
                Arc::new(tauron_distribute::InstallationIdentity::ephemeral()),
                tauron_distribute::GrayscalePolicy::default(),
            ));
            substrate.updater_sink = sink.clone();
            let state =
                PluginRuntimeState::with_substrate(Arc::new(substrate), AdapterConfig::default())
                    .unwrap();

            assert_eq!(cmd_updater_status(&state).unwrap().grayscale_percent, 1);
            // 停留时间未满 → 推进被拒（批次不是想推就能推）。
            assert!(sink.advance_grayscale(0).is_err());
            // 满 1 小时（`DEFAULT_MIN_DWELL_SECONDS`）→ 推进到 5%。
            sink.advance_grayscale(3_600).expect("达到停留时间必须可推进");
            assert_eq!(cmd_updater_status(&state).unwrap().grayscale_percent, 5);

            assert!(!cmd_updater_status(&state).unwrap().crash_gate_stopped);
            assert!(!sink.update_crash_gate(1, 100), "1% 崩溃率低于 5% 阈值，不得触发停发");
            assert!(sink.update_crash_gate(10, 100), "10% 崩溃率越过阈值 → 门禁必须触发停发");
            assert!(
                cmd_updater_status(&state).unwrap().crash_gate_stopped,
                "停发事实必须回吐给 host_updater_status"
            );
        }

        #[test]
        fn theme_commands_use_the_theme_registry() {
            let state = CommandState::new();
            let themes = cmd_theme_list(&state).unwrap();
            assert!(themes.len() >= 2, "内置 light/dark 必须可见：{themes:?}");
            assert!(matches!(cmd_theme_get(&state, "dark").unwrap(), ProviderResult::Value(_)));
            match cmd_theme_set(&state, "dark").unwrap() {
                ProviderResult::Value(v) => assert_eq!(v["activeId"], "dark"),
                ProviderResult::Unsupported(_) => panic!("内置主题必须可激活"),
            }
            // 未知主题 → Unsupported（不是静默成功）。
            assert!(matches!(
                cmd_theme_get(&state, "nope").unwrap(),
                ProviderResult::Unsupported(_)
            ));
            assert!(matches!(
                cmd_theme_set(&state, "nope").unwrap(),
                ProviderResult::Unsupported(_)
            ));
            // 主窗专属。
            assert_eq!(
                cmd_theme_list_as(&Caller::Plugin("com.p".into()), &state).unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
        }

        /// **顺序不变量**：`host_window_relaunch` 必须先对账、后重启。
        ///
        /// 反序会把「注册表标志未与引擎判定对齐」的状态带进下一次启动，而阶段计数是
        /// 跨进程持久化的 → 下一轮仍按旧判定走 → 重启循环。这里用"relaunch 被调用
        /// 那一刻的注册表标志"作为顺序证据。
        #[test]
        fn window_relaunch_reconciles_before_requesting_restart() {
            let sink = Arc::new(RelaunchOrderingSink::default());
            let state = noop_only(sink.clone());
            sink.bind_registry(state.registry.clone());
            state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();

            // 直接在恢复引擎上造出「安全模式 + 非必需插件应被禁用」的判定，
            // 但**不**经过 `cmd_recover_report`（它会自己对账）——否则注册表标志
            // 早已写回，本用例就没有判别力了。
            {
                let mut engine = state.recovery.lock();
                engine.register_plugin("com.a");
                engine.record_boot_failure(None);
                engine.record_boot_failure(None);
            }
            assert_eq!(
                cmd_recover_boot(&state).unwrap()["phase"],
                "safemode",
                "前置：引擎判定必须是安全模式"
            );
            assert!(
                !find_summary(&state, "com.a").unwrap().disabled_by_safemode,
                "前置：注册表标志此时尚未对账 —— 这正是「重启前必须对账」的场景"
            );

            let out = cmd_window_relaunch_as(&Caller::MainWindow, &state).unwrap();
            assert_eq!(out.reconcile.scanned, 1);
            assert_eq!(
                out.reconcile.entered, 1,
                "对账必须真的把安全模式判定补发到注册表（不是空跑）"
            );
            assert!(out.relaunch_requested);
            assert_eq!(
                sink.observed_flag(),
                Some(true),
                "relaunch 被调用时注册表标志必须已经写上 —— 顺序反了这里会是 false"
            );
            assert_eq!(sink.calls(), 1, "重启只请求一次");

            // 判别力对照：同样的前置状态，只调 sink（跳过对账）→ 观察点读到 false。
            // 没有这一条，上面的断言无法区分「顺序正确」与「观察点恒为 true」。
            let control_sink = Arc::new(RelaunchOrderingSink::default());
            let control = noop_only(control_sink.clone());
            control_sink.bind_registry(control.registry.clone());
            control.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
            {
                let mut engine = control.recovery.lock();
                engine.register_plugin("com.a");
                engine.record_boot_failure(None);
                engine.record_boot_failure(None);
            }
            assert_eq!(
                cmd_recover_boot(&control).unwrap()["phase"],
                "safemode",
                "对照前置：同一份安全模式判定"
            );
            // 只重启、不对账：这正是"顺序写反"会造成的局面。
            control.window_sink.relaunch().unwrap();
            assert_eq!(
                control_sink.observed_flag(),
                Some(false),
                "对照组必须读到未对账的 false —— 证明观察点真的有判别力"
            );
        }

        /// 重启是应用级动作：插件主体一律拒绝，且**拒绝路径不产生任何副作用**。
        #[test]
        fn window_relaunch_is_main_window_only_and_leaves_no_side_effect() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());

            let err = cmd_window_relaunch_as(&Caller::Plugin("com.a".to_string()), &state)
                .expect_err("插件主体不得触发应用重启");
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
            assert!(sink.op_names().is_empty(), "拒绝路径不得调用 sink（判定必须在转调之前）");

            // 畸形 label 构造不出主体（`Caller::from_label` 就拒了）——这是同一道门。
            assert_eq!(
                Caller::from_label("plugin-not a valid id").unwrap_err().code,
                ErrorCode::E_AUTH_DENIED
            );
        }

        /// 缺省装配（非 Tauri 宿主）必须如实说"没有重启"，而不是假装重启成功。
        #[test]
        fn builtin_sinks_are_honest_about_what_they_do_not_do() {
            let state = CommandState::new();
            let out = cmd_window_relaunch_as(&Caller::MainWindow, &state).unwrap();
            assert!(!out.relaunch_requested);
            assert!(out.reason.as_deref().is_some_and(|r| r.contains("没有")));
            assert_eq!(out.reconcile, PhaseReconcileOutcome::default());

            // 三个内建降级实现的自我描述逐条钉住。
            let window = MemoryWindowSink::new();
            let spec = WindowCreateSpec {
                label: "plugin-com.a".to_string(),
                plugin_id: "com.a".to_string(),
                url: "ui/panel.html".to_string(),
                title: "A".to_string(),
                width: 100,
                height: 100,
            };
            assert!(!window.relaunch().unwrap(), "进程内 sink 没有重启原语");
            assert!(!window.create(&spec).unwrap(), "进程内 sink 不创建窗口");
            assert!(window.recorded("create") && window.recorded("relaunch"), "但要留痕");
            assert!(!NoopDeepLinkSink.native_supported(), "无 OS 级深链接注册");
            assert!(!NoopDialogSink.confirm("t", "m").unwrap());
        }

        /// `host_window_create`：label 由核心铸造、URL 来自 manifest、缺省值固定。
        #[test]
        fn window_create_mints_plugin_label_and_resolves_the_manifest() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            state
                .registry
                .install(&empty_index(), manifest_with_ui("com.a", "面板 A", "ui/panel.html"))
                .unwrap();

            let out = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .unwrap();

            assert_eq!(out.label, "plugin-com.a", "label 恒为 plugin-<id>（身份载体）");
            assert_eq!(out.plugin_id, "com.a");
            assert!(out.created, "sink 说创建成功 → 如实回传");
            assert!(out.reason.is_none());

            let specs = sink.specs();
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].label, "plugin-com.a");
            assert_eq!(specs[0].url, "ui/panel.html", "URL 只能来自 manifest.entry.ui");
            assert_eq!(specs[0].title, "面板 A", "标题缺省 = manifest 的 name");
            assert_eq!(specs[0].width, DEFAULT_WINDOW_WIDTH);
            assert_eq!(specs[0].height, DEFAULT_WINDOW_HEIGHT);

            // 入参可覆盖标题与尺寸（线参数的最小面）。
            cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    title: Some("自定义".to_string()),
                    width: Some(640),
                    height: Some(480),
                },
            )
            .unwrap();
            let specs = sink.specs();
            assert_eq!(specs[1].title, "自定义");
            assert_eq!((specs[1].width, specs[1].height), (640, 480));
        }

        /// 不存在的插件 id：拒绝，且**不触碰 sink**（不创建半态窗口）。
        #[test]
        fn window_create_rejects_unknown_plugin_before_touching_the_sink() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            let err = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.ghost".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .expect_err("未注册插件不得开窗");
            assert_eq!(err.code, ErrorCode::E_UNKNOWN_PLUGIN);
            assert!(sink.specs().is_empty(), "拒绝路径不得创建任何窗口");
        }

        /// 未声明 `entry.ui` 的插件：拒绝（不编造默认 URL），同样不触碰 sink。
        #[test]
        fn window_create_refuses_plugins_without_a_ui_entry() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            state.registry.install(&empty_index(), test_manifest("com.a")).unwrap();
            let err = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .expect_err("没有 UI 入口就没有可加载的页面");
            assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
            assert!(sink.specs().is_empty());
        }

        #[test]
        fn window_create_rejects_unsafe_ui_paths_before_touching_the_sink() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            for (i, ui) in
                ["../outside.html", "/absolute.html", "C:/outside.html", "ui\\panel.html"]
                    .into_iter()
                    .enumerate()
            {
                let id = format!("com.example.path{i}");
                state
                    .registry
                    .install(&empty_index(), manifest_with_ui(&id, "Path test", ui))
                    .unwrap();
                let err = cmd_window_create_as(
                    &Caller::MainWindow,
                    &state,
                    &WindowCreateRequest { plugin_id: id, ..WindowCreateRequest::default() },
                )
                .expect_err("unsafe entry.ui must be rejected");
                assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
            }
            assert!(sink.specs().is_empty(), "拒绝路径不得创建任何窗口");
        }

        /// 开窗是宿主管理面动作：插件主体一律拒绝。
        #[test]
        fn window_create_is_main_window_only() {
            let sink = Arc::new(RecordingWindowSink::with_results(true, true));
            let state = noop_only(sink.clone());
            state
                .registry
                .install(&empty_index(), manifest_with_ui("com.a", "A", "ui/a.html"))
                .unwrap();
            let err = cmd_window_create_as(
                &Caller::Plugin("com.a".to_string()),
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .expect_err("插件不得为自己/别人开窗");
            assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
            assert!(sink.specs().is_empty());
        }

        /// 缺省装配下 `create` 返回降级结果（`created: false` + 原因），不假装成功。
        #[test]
        fn window_create_degrades_honestly_without_a_native_sink() {
            let state = CommandState::new();
            state
                .registry
                .install(&empty_index(), manifest_with_ui("com.a", "A", "ui/a.html"))
                .unwrap();
            let out = cmd_window_create_as(
                &Caller::MainWindow,
                &state,
                &WindowCreateRequest {
                    plugin_id: "com.a".to_string(),
                    ..WindowCreateRequest::default()
                },
            )
            .unwrap();
            assert!(!out.created);
            assert_eq!(out.label, "plugin-com.a", "label 仍然铸造出来（清理钩子按它回收）");
            assert!(
                out.reason
                    .as_deref()
                    .is_some_and(|r| r.contains("没有") && r.contains("创建任何窗口")),
                "降级必须写明没有创建窗口：{:?}",
                out.reason
            );
        }

        /// 深链接：sink 被真实驱动；**换协议时先注销旧值**。
        #[test]
        fn deep_link_register_drives_the_sink_and_releases_the_previous_protocol() {
            let sink = Arc::new(RecordingDeepLinkSink { native: false, ..Default::default() });
            let state = state_with_sinks(
                Arc::new(MemoryWindowSink::new()),
                Arc::new(NoopDialogSink),
                sink.clone(),
            );

            cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
            cmd_deep_link_register(&state, "tauron".to_string()).unwrap();
            cmd_deep_link_register(&state, "other".to_string()).unwrap();

            assert_eq!(
                sink.calls.lock().clone(),
                vec![
                    "register:tauron".to_string(),
                    "register:tauron".to_string(),
                    // 换协议：先注销旧值，再注册新值（旧关联不得留在系统里）。
                    "unregister:tauron".to_string(),
                    "register:other".to_string(),
                ],
                "同值重复注册不注销；换值必须先注销"
            );
            assert_eq!(state.shell_ext.lock().deep_link_protocol.as_deref(), Some("other"));
            // 降级事实必须可机读：本仓没有说话算数的 OS 级注册。
            assert!(!sink.native_supported());
        }

        /// 带 UI 入口的 manifest（窗口创建需要 `entry.ui`）。
        fn manifest_with_ui(id: &str, name: &str, ui: &str) -> PluginManifest {
            let mut manifest = test_manifest(id);
            manifest.name = name.to_string();
            manifest.entry.ui = Some(ui.to_string());
            manifest
        }
    }
}

#[cfg(test)]
mod v4_production_config_tests {
    use super::*;

    #[test]
    fn development_defaults_remain_test_friendly() {
        assert!(AdapterConfig::default().validate_for_start().is_ok());
    }

    #[test]
    fn production_default_is_fail_closed() {
        let cfg =
            AdapterConfig::default().with_deployment_mode(tauron_host::DeploymentMode::Production);
        let error = cfg.validate_for_start().expect_err("empty production config must fail");
        assert_eq!(error.code, ErrorCode::E_STATE_INVALID_TRANSITION);
        assert!(error.message.contains("CALLER_IDENTITY_POLICY_REQUIRED"));
        assert!(error.message.contains("ADMIN_AUDIT_REQUIRED"));
        assert!(error.message.contains("DATA_DIR_REQUIRED"));
        #[cfg(feature = "plugin-install")]
        assert!(error.message.contains("TRUSTED_TIME_REQUIRED"));
    }

    #[test]
    fn production_process_runtime_rejects_the_default_unsupported_sandbox() {
        let cfg =
            AdapterConfig::default().with_deployment_mode(tauron_host::DeploymentMode::Production);
        let descriptor = tauron_proc::ProcessSandboxDescriptor::unsupported("test");
        let error = cfg
            .validate_process_runtime_for_start(&descriptor)
            .expect_err("production process runtime must require hard sandbox enforcement");
        assert!(error.message.contains("PROCESS_SANDBOX_HARD_REQUIRED"));
    }

    #[test]
    fn development_process_runtime_keeps_compatibility_with_unsupported_sandbox() {
        let cfg = AdapterConfig::default();
        let descriptor = tauron_proc::ProcessSandboxDescriptor::unsupported("test");
        assert!(cfg.validate_process_runtime_for_start(&descriptor).is_ok());
    }

    #[cfg(feature = "plugin-install")]
    #[test]
    fn production_install_time_trust_is_explicit_and_current() {
        let cfg = AdapterConfig::default();
        assert!(!cfg.production_readiness().trusted_time_available);

        let trusted = cfg.clone().with_trusted_time_provider(Arc::new(
            tauron_host::SystemTimeProvider::new(tauron_host::TimeTrustState::Trusted),
        ));
        assert!(trusted.production_readiness().trusted_time_available);

        let suspicious = cfg.with_trusted_time_provider(Arc::new(
            tauron_host::SystemTimeProvider::new(tauron_host::TimeTrustState::Suspicious),
        ));
        assert!(!suspicious.production_readiness().trusted_time_available);
    }

    #[test]
    fn origin_allowlist_is_real_identity_policy_evidence() {
        let mut cfg = AdapterConfig::default();
        cfg.origin_allowlist.push("tauri://localhost".to_string());
        assert!(cfg.production_readiness().caller_identity_policy_enabled);
    }
}

#[cfg(test)]
mod v4_service_graph_wiring_tests {
    use super::*;

    /// A75（轮 11 收口）：装配期消费的是图的**有效性**，不是它抄出来的两个序。
    ///
    /// 这条测试存在的理由是反向的：一旦有人把 `startup_order()`/`shutdown_order()`
    /// 重新存成没人执行的字段（"看着像按拓扑序装配"），就得先在这里改回真实消费点。
    #[test]
    fn substrate_assembly_validates_the_canonical_service_graph() {
        // 构造本身即断言：图非法时装配会 panic，测试当场失败。
        SubstrateState::with_adapter_config(&AdapterConfig::default());
        let order = canonical_substrate_service_graph()
            .startup_order()
            .expect("canonical Tauron service graph must stay acyclic and complete");
        assert_eq!(order.first().map(String::as_str), Some("contract"));
        assert!(order.iter().any(|id| id == "message"));
        assert!(order.iter().any(|id| id == "provider"));
        // 缺依赖/成环 = 构建缺陷，装配当场拒绝（不是运行期重试）。
        let mut broken = canonical_substrate_service_graph();
        broken
            .insert(tauron_host::ServiceNode { id: "orphan".into(), requires: vec!["nope".into()] })
            .unwrap();
        assert!(matches!(
            broken.startup_order(),
            Err(tauron_host::ServiceGraphError::MissingDependency { .. })
        ));
    }
}

#[cfg(test)]
mod v4_settings_transaction_tests {
    use super::*;

    #[test]
    fn in_memory_settings_commit_advances_revision_after_set() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());
        assert_eq!(host_settings_revision(&state), 0);
        cmd_settings_set(&state, "theme", serde_json::json!("dark")).unwrap();
        assert_eq!(host_settings_revision(&state), 1);
        assert_eq!(cmd_settings_get(&state, "theme").unwrap(), serde_json::json!("dark"));
    }

    #[test]
    fn settings_write_lock_is_shared_by_cloned_substrate_state() {
        let state = SubstrateState::with_adapter_config(&AdapterConfig::default());
        let cloned = state.clone();
        assert!(Arc::ptr_eq(&state.settings_write_lock, &cloned.settings_write_lock));
    }

    /// 单写者事务的**行为**证明（不只是 `Arc::ptr_eq`）。
    ///
    /// 缺了写租约会坏两件事，两件都在这里钉住：
    /// 1. **丢键**——「改内存 → 落盘」不再是一个整体，后写者可以按一份**不含前者**
    ///    的快照落盘（全量文档，后写者赢），前者的键就此消失；回滚路径更狠，
    ///    `restore(&before)`  rewind 的是整份快照，会把别人已成功的写入一起吃掉。
    /// 2. **generation 撞号**——两个线程读到同一个 `settings_generation`，各自封印
    ///    `G+1`，于是两次成功 rename 只推进一代，durable 契约里的「每提交一代」失效。
    ///
    /// 红→绿实测：把 `cmd_settings_set` 的 `_write` 换成 `None`（去掉租约）后本测试连跑
    /// 3 次全部失败——写线程先在共享的 `.json.tmp` 上互相踩（Windows sharing violation，
    /// `unwrap()` 当场 panic），主线程再报 generation 少推进。故障闸门**不**替它兜底：
    /// 见 `run_settings_boundary`，闸门只判就绪，不代持事务。
    #[test]
    fn concurrent_settings_writes_never_lose_a_key_or_share_a_generation() {
        const WRITERS: usize = 4;
        const PER_WRITER: usize = 10;
        let t = tempfile::tempdir().unwrap();
        let cfg = AdapterConfig {
            recovery_data_dir: Some(t.path().to_path_buf()),
            ..AdapterConfig::default()
        };
        let state = SubstrateState::with_adapter_config(&cfg);
        let generation_before = *state.settings_generation.lock();

        let barrier = Arc::new(std::sync::Barrier::new(WRITERS));
        let mut handles = Vec::new();
        for w in 0..WRITERS {
            let state = state.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for i in 0..PER_WRITER {
                    cmd_settings_set(&state, &format!("w{w}k{i}"), serde_json::json!(i)).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(
            *state.settings_generation.lock(),
            generation_before + (WRITERS * PER_WRITER) as u64,
            "每次成功落盘必须恰好推进一代（并发下不得撞号）"
        );
        for w in 0..WRITERS {
            for i in 0..PER_WRITER {
                assert_eq!(
                    cmd_settings_get(&state, &format!("w{w}k{i}")).unwrap(),
                    serde_json::json!(i),
                    "并发写不得吃掉别人的键"
                );
            }
        }

        // 磁盘与内存同一事实：重启（新装配读回同一目录）后一个键都不少。
        // 先出让：A87 的 durable 数据目录有 OS 级单写者租约，同一 namespace 的第二次
        // 装配会在构造期 panic——这正是「两个进程抢同一份设置文档」被挡住的机制。
        drop(barrier);
        drop(state);
        let reopened = SubstrateState::with_adapter_config(&cfg);
        for w in 0..WRITERS {
            assert_eq!(
                cmd_settings_get(&reopened, &format!("w{w}k{}", PER_WRITER - 1)).unwrap(),
                serde_json::json!(PER_WRITER - 1),
                "落盘文档必须是全量提交的那一份"
            );
        }
    }
}

/// 轮 12：主窗专属面上**缺判定**的那九条。
///
/// 审计入口是生成的命令面参考：它把「既不在 authz 档位表、沿委托链又找不到
/// `require_*` 判定」的命令逐条列出来，而这九条恰好全在那份清单里——文档口径一直
/// 称它们为「主窗专属」，可示例应用的 capability 文件里 `host_*` 走 root 注册
/// （不按命令名授 ACL）且 `windows` 同时覆盖 `plugin-*`，所以那句口径在代码里没有
/// 任何对应判定。这里把判定补成真判定，并钉住「判定先于副作用」。
#[cfg(test)]
mod main_window_surface_guard_tests {
    use super::*;

    fn state() -> SubstrateState {
        SubstrateState::with_adapter_config(&AdapterConfig::default())
    }

    fn plugin() -> Caller {
        Caller::Plugin("com.a".to_string())
    }

    /// 九条命令逐条：插件主体被拒，且拒绝码是既有的 `E_AUTH_DENIED`（不新增错误码）。
    #[test]
    fn nine_main_window_surface_commands_reject_plugin_callers() {
        let state = state();
        let caller = plugin();

        macro_rules! denied {
            ($cmd:expr, $call:expr) => {{
                let err = ($call).expect_err(concat!("插件主体不得调用 ", $cmd));
                assert_eq!(err.code, ErrorCode::E_AUTH_DENIED, "{} 的拒绝码", $cmd);
                assert!(
                    err.message.contains($cmd),
                    "拒绝原因必须点明是哪条命令被拒：{}",
                    err.message
                );
            }};
        }

        denied!("host_window_quit", cmd_window_quit_as(&caller, &state));
        denied!("host_clipboard_read", cmd_clipboard_read_as(&caller, &state));
        denied!("host_clipboard_write", cmd_clipboard_write_as(&caller, &state, "x".to_string()));
        denied!("host_dialog_open", cmd_dialog_open_as(&caller, &state, false, false));
        denied!("host_dialog_save", cmd_dialog_save_as(&caller, &state, Some("a.txt")));
        denied!(
            "host_dialog_message",
            cmd_dialog_message_as(&caller, &state, "t", "m", Some("info"))
        );
        denied!("host_dialog_confirm", cmd_dialog_confirm_as(&caller, &state, "t", "m"));
        denied!("host_recover_boot", cmd_recover_boot_as(&caller, &state));
        denied!("host_i18n_stats", cmd_i18n_stats_as(&caller, &state));
    }

    /// 判别力对照：拒绝路径**零副作用**。剪贴板是唯一可直接观察的全局槽位——
    /// 若判定写在转调之后，插件就会先把主窗写进去的内容覆盖掉。
    #[test]
    fn denial_happens_before_any_mutation() {
        let state = state();
        cmd_clipboard_write_as(&Caller::MainWindow, &state, "from-main".to_string()).unwrap();

        let err = cmd_clipboard_write_as(&plugin(), &state, "from-plugin".to_string())
            .expect_err("插件写剪贴板必须被拒");
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
        assert_eq!(
            cmd_clipboard_read(&state).unwrap().value,
            "from-main",
            "被拒的写入不得落到共享缓冲区"
        );

        // 对话框族的 `kind` 闭集校验在判定**之后**：表外值 + 插件主体 → 先报越权。
        let err = cmd_dialog_message_as(&plugin(), &state, "t", "m", Some("nonsense"))
            .expect_err("判定先于参数校验");
        assert_eq!(err.code, ErrorCode::E_AUTH_DENIED);
    }

    /// 主窗主体不受影响：九条全部照常放行（线形与返回值形状不变，只多一道判定）。
    #[test]
    fn main_window_callers_keep_working() {
        let state = state();
        assert!(cmd_window_quit_as(&Caller::MainWindow, &state).is_ok());
        assert!(cmd_clipboard_read_as(&Caller::MainWindow, &state).is_ok());
        assert!(cmd_clipboard_write_as(&Caller::MainWindow, &state, "v".to_string()).is_ok());
        assert!(cmd_dialog_open_as(&Caller::MainWindow, &state, false, false).is_ok());
        assert!(cmd_dialog_save_as(&Caller::MainWindow, &state, None).is_ok());
        assert!(cmd_dialog_message_as(&Caller::MainWindow, &state, "t", "m", None).is_ok());
        assert!(cmd_dialog_confirm_as(&Caller::MainWindow, &state, "t", "m").is_ok());
        assert!(cmd_recover_boot_as(&Caller::MainWindow, &state).is_ok());
        assert!(cmd_i18n_stats_as(&Caller::MainWindow, &state).is_ok());
    }

    /// 畸形 label 仍然构造不出主体（同一道身份门，绝不降级成主窗）。
    #[test]
    fn malformed_label_never_becomes_main_window() {
        assert_eq!(
            Caller::from_label("plugin-not a valid id").expect_err("畸形 label 必须拒绝").code,
            ErrorCode::E_AUTH_DENIED
        );
    }
}

/// 轮 49：A71 变体过滤——`resolve_best` 经 [`tauron_host::ArtifactVariantResolver`]
/// 接进安装/更新装配腿。
///
/// 钉住三件事：无兼容变体在**任何下载动作之前**硬拒（失败关闭，不动网络）；
/// 有兼容变体时按本机目标选择（不是列表首位、也不是 options 单清单默认）；
/// `install()` 用同一份被选中的清单重验（download/install 口径一致）。
#[cfg(test)]
mod round49_variant_resolution_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    struct UrlRecordingDownloader {
        bytes: Vec<u8>,
        urls: Arc<Mutex<Vec<String>>>,
    }

    impl tauron_distribute::Downloader for UrlRecordingDownloader {
        fn download(
            &self,
            url: &str,
            dest_path: &Path,
            _control: &tauron_distribute::PhaseControl,
            progress_callback: &mut dyn FnMut(u64, u64),
        ) -> tauron_distribute::DistributeResult<PathBuf> {
            self.urls.lock().unwrap().push(url.to_string());
            std::fs::write(dest_path, &self.bytes).map_err(|e| {
                tauron_distribute::DistributeError::FileOperationFailed(format!(
                    "mock 下载写盘失败：{e}"
                ))
            })?;
            progress_callback(self.bytes.len() as u64, self.bytes.len() as u64);
            Ok(dest_path.to_path_buf())
        }
    }

    struct AcceptVerifier;

    impl tauron_distribute::SignatureVerifier for AcceptVerifier {
        fn verify(
            &self,
            _file_path: &Path,
            _signature: &str,
            _context: &tauron_distribute::VerificationContext<'_>,
        ) -> tauron_distribute::DistributeResult<bool> {
            Ok(true)
        }
    }

    struct StagingExtractor {
        contents: Vec<u8>,
    }

    impl tauron_distribute::ArchiveExtractor for StagingExtractor {
        fn extract(
            &self,
            _archive_path: &Path,
            staging_dir: &Path,
            _limits: &tauron_distribute::ArchiveLimits,
            _control: &tauron_distribute::PhaseControl,
        ) -> tauron_distribute::DistributeResult<tauron_distribute::ExtractedArchive> {
            std::fs::create_dir_all(staging_dir).map_err(|e| {
                tauron_distribute::DistributeError::FileOperationFailed(format!(
                    "mock 建 staging 失败：{e}"
                ))
            })?;
            std::fs::write(staging_dir.join("app.txt"), &self.contents).map_err(|e| {
                tauron_distribute::DistributeError::FileOperationFailed(format!(
                    "mock 写 staging 失败：{e}"
                ))
            })?;
            Ok(tauron_distribute::ExtractedArchive {
                file_count: 1,
                total_bytes: self.contents.len() as u64,
            })
        }
    }

    struct AlwaysHealthy;

    impl tauron_distribute::UpgradeHealthCheck for AlwaysHealthy {
        fn check(
            &self,
            _current_dir: &Path,
            _control: &tauron_distribute::PhaseControl,
        ) -> tauron_distribute::DistributeResult<bool> {
            Ok(true)
        }
    }

    const OPTIONS_URL: &str = "https://updates.example/options.zip";

    fn manifest(version: &str, url: &str, payload: &[u8]) -> tauron_distribute::UpdateManifest {
        tauron_distribute::UpdateManifest {
            version: version.into(),
            url: url.into(),
            signature: "c0ffee".into(),
            release_date: "2026-10-04T00:00:00Z".into(),
            platform_notes: Default::default(),
            sha256: Some(sha256_hex(payload)),
            signature_algorithm: Some("ed25519".into()),
            public_key_id: Some("variant-key".into()),
            min_host_version: None,
            abi: None,
            rollback_policy: None,
        }
    }

    /// 与任何真实主机都不同的目标（os 不同即不兼容）。
    fn foreign_target() -> tauron_host::TargetSpec {
        let mut target = tauron_host::current_target_spec();
        target.os = tauron_host::TargetOs::Other;
        target
    }

    fn installer(
        root: &std::path::Path,
        payload: Vec<u8>,
        urls: Arc<Mutex<Vec<String>>>,
    ) -> DistributeUpgradeInstaller {
        let options = tauron_distribute::UpgradeOptions {
            manifest: manifest("9.9.9", OPTIONS_URL, &payload),
            download_dir: root.join("downloads"),
            install_dir: root.join("install"),
            backup_dir: root.join("backups"),
            auto_restart: false,
            download_timeout_secs: 30,
            extract_timeout_secs: 30,
            swap_timeout_secs: 30,
            health_check_timeout_secs: 30,
            archive: tauron_distribute::ArchiveLimits::default(),
            installed_version: Some("1.0.0".into()),
        };
        DistributeUpgradeInstaller::new(
            options,
            Arc::new(UrlRecordingDownloader { bytes: payload, urls }),
            Arc::new(AcceptVerifier),
            Arc::new(StagingExtractor { contents: b"v3-new-bytes".to_vec() }),
            Arc::new(AlwaysHealthy),
            None,
        )
    }

    /// 降级门禁（轮 54）：清单目标版本低于 `installed_version`（装配腿缺省 1.0.0）
    /// 必须在**任何下载动作之前**硬拒，且不留 staged 残留。
    #[test]
    fn downgrade_manifest_is_rejected_before_any_download() {
        let root = tempfile::tempdir().unwrap();
        let urls = Arc::new(Mutex::new(Vec::new()));
        let lower = vec![UpdateArtifactVariant {
            target: tauron_host::current_target_spec(),
            manifest: manifest("0.5.0", "https://updates.example/tauron-0.5.0.zip", b"x"),
        }];
        let installer =
            installer(root.path(), b"payload".to_vec(), urls.clone()).with_variants(lower);
        let err = installer.download().unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST, "降级属清单类硬拒");
        assert!(
            err.message.contains("拒绝降级") && err.message.contains("1.0.0"),
            "拒绝理由必须点名降级与基线版本：{}",
            err.message
        );
        assert!(urls.lock().unwrap().is_empty(), "降级拒绝不得下载任何字节");
        assert!(!installer.staged_path().exists(), "降级拒绝不得落 staged 残留");
    }

    #[test]
    fn variant_list_without_compatible_target_is_rejected_before_any_download() {
        let root = tempfile::tempdir().unwrap();
        let urls = Arc::new(Mutex::new(Vec::new()));
        let only_foreign = vec![UpdateArtifactVariant {
            target: foreign_target(),
            manifest: manifest("3.1.0", "https://updates.example/tauron-3.1.0-foreign.zip", b"x"),
        }];
        let installer =
            installer(root.path(), b"payload".to_vec(), urls.clone()).with_variants(only_foreign);
        let err = installer.download().unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST, "无兼容变体必须走清单类硬拒");
        assert!(
            err.message.contains("无一兼容本机目标"),
            "拒绝理由必须点名「无一兼容」而不是别的清单错误：{}",
            err.message
        );
        assert!(urls.lock().unwrap().is_empty(), "硬拒必须发生在任何下载动作之前");
        assert!(!installer.staged_path().exists(), "硬拒不得落任何 staged 残留");
    }

    #[test]
    fn download_and_install_both_use_the_variant_compatible_with_this_host() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("install").join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("app.txt"), b"v1-old-bytes").unwrap();

        let urls = Arc::new(Mutex::new(Vec::new()));
        let payload = b"variant-payload".to_vec();
        let variants = vec![
            UpdateArtifactVariant {
                target: foreign_target(),
                manifest: manifest(
                    "3.1.0",
                    "https://updates.example/tauron-3.1.0-foreign.zip",
                    &payload,
                ),
            },
            UpdateArtifactVariant {
                target: tauron_host::current_target_spec(),
                manifest: manifest(
                    "3.1.0",
                    "https://updates.example/tauron-3.1.0-host.zip",
                    &payload,
                ),
            },
        ];
        let installer =
            installer(root.path(), payload.clone(), urls.clone()).with_variants(variants);

        let staged = installer.download().unwrap();
        assert_eq!(staged.version, "3.1.0", "版本必须来自被选中的变体清单");
        assert_eq!(staged.sha256, sha256_hex(&payload));
        assert_eq!(
            urls.lock().unwrap().as_slice(),
            &["https://updates.example/tauron-3.1.0-host.zip".to_string()],
            "必须选兼容本机目标的变体（而非列表首位或 options 单清单默认值 {OPTIONS_URL}）"
        );

        // download 与 install 口径一致：install 以被选中清单重验并推进版本。
        let installed = installer.install().unwrap();
        assert_eq!(installed.version, "3.1.0", "install 不得回退到 options 单清单版本 9.9.9");
        assert_eq!(
            std::fs::read_to_string(current.join("app.txt")).unwrap(),
            "v3-new-bytes",
            "真实交换后 current 必须是新归档内容"
        );
    }
}

/// 轮 40：商城下载/安装的**装配腿真路径**（[`DistributeUpgradeInstaller`]）。
///
/// 把「装配 `UpgradeInstaller` 后 `host_market_download` / `host_market_install`
/// 变成真效果」这条链路钉在命令层：
/// - staged 槽位（下载写、安装读；失败清理）；
/// - SHA-256 + 验签两层真跑（mock 只替身网络/证书，顺序与门禁不替身）；
/// - 账本推进时机（成功才推进、失败零账本，含健康检查失败后的回滚）；
/// - provenance 翻转（真效果后 `update_state_simulated = false`）；
/// - 完整 runner 的交换/提交/重启/回滚都是**真实文件效果**（tempdir 上验证）。
#[cfg(test)]
mod round40_market_wiring_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    // ── mock 组件：只替身网络 / 证书 / 归档格式，全部计数证明真被调用 ──────

    struct WireDownloader {
        bytes: Vec<u8>,
        calls: Arc<AtomicUsize>,
    }

    impl tauron_distribute::Downloader for WireDownloader {
        fn download(
            &self,
            _url: &str,
            dest_path: &Path,
            control: &tauron_distribute::PhaseControl,
            progress_callback: &mut dyn FnMut(u64, u64),
        ) -> tauron_distribute::DistributeResult<PathBuf> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if control.is_cancelled() {
                return Err(tauron_distribute::DistributeError::PhaseTimeout {
                    phase: "Download",
                    timeout_secs: 0,
                });
            }
            std::fs::write(dest_path, &self.bytes).map_err(|e| {
                tauron_distribute::DistributeError::FileOperationFailed(format!(
                    "mock 下载写盘失败：{e}"
                ))
            })?;
            progress_callback(self.bytes.len() as u64, self.bytes.len() as u64);
            Ok(dest_path.to_path_buf())
        }
    }

    struct WireVerifier {
        accept: bool,
        calls: Arc<AtomicUsize>,
    }

    impl tauron_distribute::SignatureVerifier for WireVerifier {
        fn verify(
            &self,
            _file_path: &Path,
            _signature: &str,
            _context: &tauron_distribute::VerificationContext<'_>,
        ) -> tauron_distribute::DistributeResult<bool> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.accept)
        }
    }

    struct WireExtractor {
        contents: Vec<u8>,
        calls: Arc<AtomicUsize>,
    }

    impl tauron_distribute::ArchiveExtractor for WireExtractor {
        fn extract(
            &self,
            _archive_path: &Path,
            staging_dir: &Path,
            _limits: &tauron_distribute::ArchiveLimits,
            _control: &tauron_distribute::PhaseControl,
        ) -> tauron_distribute::DistributeResult<tauron_distribute::ExtractedArchive> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::fs::create_dir_all(staging_dir).map_err(|e| {
                tauron_distribute::DistributeError::FileOperationFailed(format!(
                    "mock 建 staging 失败：{e}"
                ))
            })?;
            std::fs::write(staging_dir.join("app.txt"), &self.contents).map_err(|e| {
                tauron_distribute::DistributeError::FileOperationFailed(format!(
                    "mock 写 staging 失败：{e}"
                ))
            })?;
            Ok(tauron_distribute::ExtractedArchive {
                file_count: 1,
                total_bytes: self.contents.len() as u64,
            })
        }
    }

    struct WireHealth {
        healthy: bool,
        calls: Arc<AtomicUsize>,
    }

    impl tauron_distribute::UpgradeHealthCheck for WireHealth {
        fn check(
            &self,
            _current_dir: &Path,
            _control: &tauron_distribute::PhaseControl,
        ) -> tauron_distribute::DistributeResult<bool> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.healthy)
        }
    }

    struct WireRestart {
        calls: Arc<AtomicUsize>,
    }

    impl tauron_distribute::RestartProvider for WireRestart {
        fn restart(&self) -> tauron_distribute::DistributeResult<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    // ── 装配夹具 ─────────────────────────────────────────────────────────

    struct Spec {
        healthy: bool,
        auto_restart: bool,
        verifier_accept: bool,
        /// 覆盖清单声明的 sha256（`None` = 与 payload 相符）。
        declared_sha256: Option<String>,
    }

    fn spec() -> Spec {
        Spec { healthy: true, auto_restart: false, verifier_accept: true, declared_sha256: None }
    }

    struct Fixture {
        root: tempfile::TempDir,
        installer: Arc<DistributeUpgradeInstaller>,
        payload: Vec<u8>,
        downloader_calls: Arc<AtomicUsize>,
        verifier_calls: Arc<AtomicUsize>,
        extractor_calls: Arc<AtomicUsize>,
        health_calls: Arc<AtomicUsize>,
        restart_calls: Arc<AtomicUsize>,
    }

    impl Fixture {
        fn current_app(&self) -> String {
            std::fs::read_to_string(
                self.root.path().join("install").join("current").join("app.txt"),
            )
            .expect("current/app.txt 必须存在")
        }
        fn staged_path(&self) -> PathBuf {
            self.root.path().join("downloads").join(STAGED_PACKAGE_FILE)
        }
    }

    fn fixture(spec: Spec) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let install_dir = root.path().join("install");
        let current = install_dir.join("current");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(current.join("app.txt"), b"v1-old-bytes").unwrap();

        let payload = b"fake-update-archive-v2".to_vec();
        let manifest = tauron_distribute::UpdateManifest {
            version: "2.0.0".into(),
            url: "https://updates.example/tauron-2.0.0.zip".into(),
            signature: "c0ffee".into(),
            release_date: "2026-10-04T00:00:00Z".into(),
            platform_notes: Default::default(),
            sha256: Some(spec.declared_sha256.unwrap_or_else(|| sha256_hex(&payload))),
            signature_algorithm: Some("ed25519".into()),
            public_key_id: Some("fixture-key".into()),
            min_host_version: None,
            abi: None,
            rollback_policy: None,
        };

        let downloader_calls = Arc::new(AtomicUsize::new(0));
        let verifier_calls = Arc::new(AtomicUsize::new(0));
        let extractor_calls = Arc::new(AtomicUsize::new(0));
        let health_calls = Arc::new(AtomicUsize::new(0));
        let restart_calls = Arc::new(AtomicUsize::new(0));

        let downloader: Arc<dyn tauron_distribute::Downloader> =
            Arc::new(WireDownloader { bytes: payload.clone(), calls: downloader_calls.clone() });
        let verifier: Arc<dyn tauron_distribute::SignatureVerifier> =
            Arc::new(WireVerifier { accept: spec.verifier_accept, calls: verifier_calls.clone() });
        let extractor: Arc<dyn tauron_distribute::ArchiveExtractor> = Arc::new(WireExtractor {
            contents: b"v2-new-bytes".to_vec(),
            calls: extractor_calls.clone(),
        });
        let health: Arc<dyn tauron_distribute::UpgradeHealthCheck> =
            Arc::new(WireHealth { healthy: spec.healthy, calls: health_calls.clone() });
        let restart: Option<Arc<dyn tauron_distribute::RestartProvider>> = if spec.auto_restart {
            Some(Arc::new(WireRestart { calls: restart_calls.clone() }))
        } else {
            None
        };

        let options = tauron_distribute::UpgradeOptions {
            manifest,
            download_dir: root.path().join("downloads"),
            install_dir,
            backup_dir: root.path().join("backups"),
            auto_restart: spec.auto_restart,
            download_timeout_secs: 30,
            extract_timeout_secs: 30,
            swap_timeout_secs: 30,
            health_check_timeout_secs: 30,
            archive: tauron_distribute::ArchiveLimits::default(),
            installed_version: Some("1.0.0".into()),
        };

        let installer = Arc::new(DistributeUpgradeInstaller::new(
            options, downloader, verifier, extractor, health, restart,
        ));

        Fixture {
            root,
            installer,
            payload,
            downloader_calls,
            verifier_calls,
            extractor_calls,
            health_calls,
            restart_calls,
        }
    }

    fn wired_state(fixture: &Fixture) -> SubstrateState {
        let mut state = SubstrateState::with_adapter_config(&AdapterConfig::default());
        state.upgrade_installer = fixture.installer.clone();
        state
    }

    fn ledger(state: &SubstrateState) -> (Option<String>, bool) {
        let ext = state.shell_ext.lock();
        (ext.update_state.clone(), ext.update_state_simulated)
    }

    fn find_journal(backup_root: &Path) -> PathBuf {
        let journals: Vec<PathBuf> = std::fs::read_dir(backup_root)
            .expect("备份根必须存在（begin_operation 已建）")
            .map(|entry| entry.unwrap().path().join("journal.json"))
            .filter(|path| path.is_file())
            .collect();
        assert_eq!(journals.len(), 1, "本操作必须恰好一份 journal");
        journals.into_iter().next().unwrap()
    }

    /// 装配腿下载：真落盘 + 真摘要 + 真验签；版本取宿主清单而非调用方入参；
    /// 成功后 provenance 翻 `false`。
    #[test]
    fn wired_download_stages_verifies_and_flips_provenance() {
        let fx = fixture(spec());
        let state = wired_state(&fx);

        let result = cmd_market_download(&state, Some("9.9.9")).unwrap();
        assert!(result.ok);
        assert!(!result.simulated, "装配腿注入后 simulated 必须为 false");
        assert_eq!(
            result.version.as_deref(),
            Some("2.0.0"),
            "版本必须取宿主清单，而不是调用方入参 9.9.9"
        );
        assert_eq!(result.reason, None, "真实效果成功后不得带模拟原因");

        assert_eq!(fx.downloader_calls.load(Ordering::SeqCst), 1, "下载器必须真被调用");
        assert_eq!(fx.verifier_calls.load(Ordering::SeqCst), 1, "摘要通过后必须真进验签");
        assert_eq!(
            std::fs::read(fx.staged_path()).unwrap(),
            fx.payload,
            "staged 必须是下载器写下的原始字节"
        );
        let (state_value, simulated) = ledger(&state);
        assert_eq!(state_value.as_deref(), Some("downloaded:2.0.0"));
        assert!(!simulated, "真实效果落地后 provenance 必须翻成 false");
    }

    /// 装配腿下载失败：**零账本 + 零 staged 残留**。摘要不符时不进验签；
    /// 验签拒绝时也不落地。
    #[test]
    fn wired_download_verify_failure_leaves_ledger_untouched() {
        // ① 清单声明摘要与实际字节不符 → SHA-256 门禁硬失败，验签器根本不该被调用。
        let fx = fixture(Spec { declared_sha256: Some(sha256_hex(b"not-the-payload")), ..spec() });
        let state = wired_state(&fx);
        state.shell_ext.lock().update_state = Some("downloaded:1.0.0".into());
        let err = cmd_market_download(&state, Some("2.0.0")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST, "摘要不符必须硬失败");
        assert_eq!(fx.verifier_calls.load(Ordering::SeqCst), 0, "摘要没过不得进入验签");
        let (state_value, _) = ledger(&state);
        assert_eq!(state_value.as_deref(), Some("downloaded:1.0.0"), "失败必须零账本（哨兵原样）");
        assert!(!fx.staged_path().exists(), "失败路径必须清掉 staged");

        // ② 摘要对但验签拒绝 → 同样硬失败、不落地。
        let fx2 = fixture(Spec { verifier_accept: false, ..spec() });
        let state2 = wired_state(&fx2);
        let err2 = cmd_market_download(&state2, None).unwrap_err();
        assert_eq!(err2.code, ErrorCode::E_INVALID_MANIFEST);
        assert_eq!(fx2.verifier_calls.load(Ordering::SeqCst), 1, "摘要通过后必须真进验签");
        let (state_value2, simulated2) = ledger(&state2);
        assert_eq!(state_value2, None);
        assert!(!simulated2);
        assert!(!fx2.staged_path().exists(), "验签拒绝后不得留下 staged");
    }

    /// 装配腿安装：完整 runner 真跑（备份 → 解压 → 交换 → 健康检查 → 提交 → 重启），
    /// journal 落盘且 commit 标记为真，provenance 翻 `false`。
    #[test]
    fn wired_install_swaps_commits_restarts_and_flips_provenance() {
        let fx = fixture(Spec { auto_restart: true, ..spec() });
        let state = wired_state(&fx);

        cmd_market_download(&state, None).unwrap();
        let installed = cmd_market_install(&state, Some("9.9.9")).unwrap();
        assert!(installed.ok);
        assert!(!installed.simulated);
        assert_eq!(installed.version.as_deref(), Some("2.0.0"));

        assert_eq!(fx.current_app(), "v2-new-bytes", "交换后 current 必须是新树");
        assert_eq!(
            std::fs::read_to_string(
                fx.root.path().join("install").join("previous").join("app.txt")
            )
            .unwrap(),
            "v1-old-bytes",
            "previous 必须持有旧树（可回滚的现场）"
        );
        assert_eq!(fx.extractor_calls.load(Ordering::SeqCst), 1, "解压必须真被调用");
        assert_eq!(fx.health_calls.load(Ordering::SeqCst), 1, "健康检查必须真被调用");
        assert_eq!(fx.restart_calls.load(Ordering::SeqCst), 1, "重启 provider 必须真被调用");

        let journal =
            tauron_distribute::UpgradeJournal::load(&find_journal(&fx.root.path().join("backups")))
                .unwrap();
        assert!(journal.commit_marker, "commit 标记必须落盘");
        assert_eq!(journal.new_version, "2.0.0");
        assert_eq!(journal.package_sha256.as_deref(), Some(sha256_hex(&fx.payload).as_str()));

        let (state_value, simulated) = ledger(&state);
        assert_eq!(state_value.as_deref(), Some("installed:2.0.0"));
        assert!(!simulated, "真实安装成功后的账本必须是真效果");
    }

    /// 未先下载（无 staged）就安装：类型化拒绝（不新增错误码），且**一个操作目录都不建**。
    #[test]
    fn wired_install_without_staged_package_is_typed_error() {
        let fx = fixture(spec());
        let state = wired_state(&fx);

        let err = cmd_market_install(&state, Some("2.0.0")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_STATE_INVALID_TRANSITION);
        assert!(
            err.message.contains("host_market_download"),
            "错误必须指路到下载命令：{}",
            err.message
        );
        let (state_value, simulated) = ledger(&state);
        assert_eq!(state_value, None);
        assert!(!simulated);
        assert!(!fx.root.path().join("backups").exists(), "前置检查必须发生在任何目录创建之前");
    }

    /// 健康检查失败 → 自动回滚恢复旧字节；账本停在 `downloaded:<v>`（不得假装已安装）；
    /// 重启不得被触发。
    #[test]
    fn wired_install_health_failure_rolls_back_and_leaves_ledger_untouched() {
        let fx = fixture(Spec { healthy: false, ..spec() });
        let state = wired_state(&fx);

        cmd_market_download(&state, None).unwrap();
        let err = cmd_market_install(&state, None).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::E_INSTALL_FAILED,
            "健康检查失败（已自动回滚）落在 E_INSTALL_FAILED：{}",
            err.message
        );
        assert_eq!(fx.current_app(), "v1-old-bytes", "回滚必须真实恢复旧字节");
        assert_eq!(fx.health_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fx.restart_calls.load(Ordering::SeqCst), 0, "回滚路径不得触发重启");

        let (state_value, simulated) = ledger(&state);
        assert_eq!(
            state_value.as_deref(),
            Some("downloaded:2.0.0"),
            "安装失败不得把账本推进到 installed（与磁盘真相同步）"
        );
        assert!(!simulated);
    }

    /// 装配腿状态下判定与审计照常生效：插件主体三条全拒且**零账本**；
    /// download/install 的被拒尝试各留一条结构化审计事实；主窗走真路径留 Allowed。
    #[test]
    fn wired_market_commands_gate_and_audit_hold() {
        let fx = fixture(spec());
        let temp = tempfile::tempdir().unwrap();
        let cfg = AdapterConfig::default().with_admin_audit_dir(temp.path().join("audit"));
        let mut state = SubstrateState::with_adapter_config(&cfg);
        state.upgrade_installer = fx.installer.clone();

        let plugin = Caller::Plugin("com.a".to_string());
        assert_eq!(
            cmd_market_check_as(&plugin, &state).unwrap_err().code,
            ErrorCode::E_AUTH_DENIED
        );
        assert_eq!(
            cmd_market_download_as(&plugin, &state, Some("2.0.0")).unwrap_err().code,
            ErrorCode::E_AUTH_DENIED
        );
        assert_eq!(
            cmd_market_install_as(&plugin, &state, None).unwrap_err().code,
            ErrorCode::E_AUTH_DENIED
        );
        let (state_value, simulated) = ledger(&state);
        assert_eq!(state_value, None, "拒绝路径不得推进账本");
        assert!(!simulated);

        let records = state.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 2, "check 不进审计表；download/install 被拒尝试各留一条");
        assert_eq!(records[0].command, "host_market_download");
        assert_eq!(records[0].outcome, tauron_host::AdminAuditOutcome::Denied);
        assert_eq!(records[0].error_code.as_deref(), Some("E_AUTH_DENIED"));
        assert_eq!(records[1].command, "host_market_install");
        assert_eq!(records[1].outcome, tauron_host::AdminAuditOutcome::Denied);

        let ok = cmd_market_download_as(&Caller::MainWindow, &state, Some("2.0.0")).unwrap();
        assert!(ok.ok && !ok.simulated, "主窗在装配腿状态下走真路径");
        let records = state.admin_audit.as_ref().unwrap().records();
        assert_eq!(records.len(), 3);
        assert_eq!(records[2].command, "host_market_download");
        assert_eq!(records[2].outcome, tauron_host::AdminAuditOutcome::Allowed);
        let (state_value, simulated) = ledger(&state);
        assert_eq!(state_value.as_deref(), Some("downloaded:2.0.0"));
        assert!(!simulated);
    }
}
