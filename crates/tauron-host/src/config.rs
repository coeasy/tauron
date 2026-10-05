//! §4.24 客户端配置聚合（配置化选择加载 + 第三方快速集成入口）。
//!
//! 设计原则：
//! - **单一入口**：第三方系统只需反序列化一个 `ClientConfig` 即可获得全部客户端配置；
//! - **缺省即安全**：所有字段均有合理默认值，空配置 = 全量加载 + 默认限制；
//! - **分层覆盖**：支持 `from_json()`（内联配置）和 `from_file()`（配置文件）两种加载方式；
//! - **快速校验**：`validate()` 在加载期即检查配置合法性，不等到运行期才报错；
//! - **向后兼容**：`RegistryConfig` 作为子配置嵌入，旧代码无需修改。
//!
//! 典型用法（第三方集成）：
//! ```rust,no_run
//! use tauron_host::config::ClientConfig;
//! use std::path::Path;
//!
//! // 从 JSON 字符串加载（顶层 `registry` 包裹注册表配置）
//! let config = ClientConfig::from_json(r#"{
//!     "registry": {
//!         "plugin_filter": {
//!             "allow": ["com.example.formatter"],
//!             "types": ["js"],
//!             "platforms": ["win"]
//!         }
//!     }
//! }"#).unwrap();
//!
//! // 从配置文件加载
//! let config = ClientConfig::from_file(Path::new("client-config.json")).unwrap();
//!
//! // 构建 RegistryConfig
//! let registry_config = config.registry_config();
//! ```

use crate::error::{ErrorCode, HostError, HostResult};
use crate::registry::{default_config, PluginFilter, RegistryConfig};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// 客户端配置聚合结构（第三方集成的唯一入口）。
///
/// 所有字段均为可选（`Option`），缺省值由 [`Default`] 提供。
/// 序列化时跳过 `None` 字段（`#[serde(skip_serializing_if)]`），
/// 使配置文件只包含用户实际修改的字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClientConfig {
    /// 注册表配置（含插件过滤器、容量上限等）。
    /// `None` = 使用 [`default_config()`]。
    pub registry: Option<RegistryConfigOverride>,

    /// 日志级别（`"error"` / `"warn"` / `"info"` / `"debug"` / `"trace"`）。
    /// `None` = `"info"`。
    pub log_level: Option<String>,

    /// 数据目录路径（相对或绝对）。
    /// `None` = 平台默认（`~/.config/tauron/` 等）。
    pub data_dir: Option<String>,

    /// 是否启用插件自动更新检查。
    /// `None` = `true`。
    ///
    /// ⚠️ **当前无宿主落点**（[`ClientConfig::unwired_fields`]）：更新检查/下载由
    /// 前端 `@tauron/host` 的 `AutoUpdateClient` 自己决定，本键不影响它。
    pub auto_update: Option<bool>,

    /// 更新检查间隔（秒）。
    /// `None` = `86400`（24 小时）。
    ///
    /// ⚠️ **当前无宿主落点**：同上，检查周期没有从本键映射到前端。
    pub update_check_interval_secs: Option<u64>,

    /// 是否启用崩溃报告。
    /// `None` = `false`（隐私优先）。
    ///
    /// ⚠️ **当前无宿主落点**：工作区只有崩溃**窗口计数**（`tauron-proc::CrashTracker`），
    /// 没有任何报告导出器，因此本键既不开启也不关闭什么。
    pub crash_report_enabled: Option<bool>,

    /// 自定义品牌标识（覆盖内置品牌）。
    /// `None` = 使用内置品牌。
    ///
    /// ⚠️ **当前无宿主落点**：运行时品牌是 `TAURON_BRAND_CONFIG[_JSON]` 提供的**单个**
    /// `BrandConfig`，工作区没有"按 id 选品牌"的品牌目录。
    pub brand_id: Option<String>,

    /// 额外的环境变量注入（在启动插件进程前设置）。
    /// `None` = 空。
    ///
    /// ✅ **有落点**：装配层把它带到 `tauron-proc::SpawnConfig::env`，每次
    /// `host_runtime_spawn` 注入进 sidecar 进程。同名的调用方 `profile.env` 条目
    /// 会被本字段**覆盖**——运维配置优先于调用方自报（见 `crates/tauron-adapter`
    /// 的 `apply_host_env_overrides`）。
    pub env_overrides: Option<std::collections::HashMap<String, String>>,

    /// 插件发现路径列表（相对于 `data_dir` 或绝对路径）。
    /// `None` = 仅加载内置插件。
    ///
    /// ⚠️ **当前无宿主落点**：插件经调用方提交的 manifest 进入注册表，安装侧只有
    /// 一个 `plugin_install_dir` 根；工作区**没有**"扫描这些目录注册插件"的入口，
    /// 所以写在这里的路径不会被加载。
    pub plugin_paths: Option<Vec<String>>,

    /// 是否启用性能监控（CPU/内存/事件总线统计）。
    /// `None` = `false`（生产环境关闭，开发环境可开启）。
    ///
    /// ⚠️ **当前无宿主落点**：`EventBus` 的 `BusStats`/`QueueStats` 没有任何命令
    /// 读取，工作区也没有 CPU/内存采样器；开启它不会多出任何观测面。
    pub performance_monitoring: Option<bool>,
}

/// 一个 `ClientConfig` 字段的**落点事实**（唯一权威来源是 [`CLIENT_CONFIG_LANDING`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientConfigLanding {
    /// 已有真实消费者；载荷是消费者位置（`类型::成员` 或宿主装配点）。
    Consumed(&'static str),
    /// 没有落点；载荷是**原因**——为什么今天写了也不生效。
    Deferred(&'static str),
}

/// `ClientConfig` 的键 → 落点表。
///
/// 为什么要有这张表：`ClientConfig` 是文档承诺的"第三方集成唯一入口"，
/// 而**"能写进 JSON"和"写了会生效"是两件事**。在轮 2 复查之前，有七个字段被解析、
/// 被 `validate()` 校验、还被 `tauron-app-cli` 写进生成的配置文件，却在生产代码里
/// **零读取**——用户按文档写下 `auto_update: false`，更新行为照旧，且没有任何地方
/// 报告这件事。静默接受一个不起作用的键，等于把配置面包装成接口。
///
/// 规则：
/// - [`ClientConfigLanding::Consumed`] 必须写出**真实消费者**；只有先落地那条映射，
///   键才允许从 `Deferred` 挪进来；
/// - [`ClientConfigLanding::Deferred`] 记录缺落点的原因；键仍被解析和校验，但
///   [`ClientConfig::unwired_fields`] 会把它连同原因如实列出，宿主据此声明
///   "这一项当前不生效"（参考宿主 `load_adapter_config` 就是这么打印的）；
/// - 表本身的完备性由测试 `landing_table_covers_every_serde_field` 用 serde 反推：
///   新增字段忘了分类，测试直接红，不需要有人记得来改注释。
pub const CLIENT_CONFIG_LANDING: &[(&str, ClientConfigLanding)] = &[
    ("registry", ClientConfigLanding::Consumed("AdapterConfig::registry（`from_client_config`）")),
    ("log_level", ClientConfigLanding::Consumed("参考宿主 `load_adapter_config` → `RUST_LOG`")),
    (
        "data_dir",
        ClientConfigLanding::Consumed("AdapterConfig::recovery_data_dir（`from_client_config`）"),
    ),
    (
        "env_overrides",
        ClientConfigLanding::Consumed("AdapterConfig::plugin_env_overrides → SpawnConfig::env"),
    ),
    (
        "auto_update",
        ClientConfigLanding::Deferred(
            "更新开关在前端 `AutoUpdateClient` 的 `endpoints`/`autoDownload`，本键没有映射过去",
        ),
    ),
    (
        "update_check_interval_secs",
        ClientConfigLanding::Deferred(
            "检查周期同上：`AutoUpdateConfig::checkIntervalSecs` 不从本键派生",
        ),
    ),
    (
        "crash_report_enabled",
        ClientConfigLanding::Deferred("没有崩溃报告导出器；`CrashTracker` 只做窗口计数"),
    ),
    (
        "brand_id",
        ClientConfigLanding::Deferred(
            "品牌是 env 提供的单个 `BrandConfig`，不存在按 id 选择的品牌目录",
        ),
    ),
    (
        "plugin_paths",
        ClientConfigLanding::Deferred(
            "没有目录扫描注册入口；安装侧只有单个 `plugin_install_dir` 根",
        ),
    ),
    (
        "performance_monitoring",
        ClientConfigLanding::Deferred("`BusStats`/`QueueStats` 未接命令，也没有 CPU/内存采样器"),
    ),
];

/// `RegistryConfig` 的配置覆盖结构。
///
/// 与 `RegistryConfig` 不同，这里所有字段都是 `Option`，
/// 只序列化用户实际修改的字段。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RegistryConfigOverride {
    /// 插件加载过滤器。
    /// `None` = 全量加载（不过滤）。
    pub plugin_filter: Option<PluginFilter>,

    /// 注册表插件上限。
    /// `None` = 默认 8。
    pub max_plugins: Option<usize>,

    /// 活跃身份上限。
    /// `None` = 默认 8。
    pub max_active_identities: Option<usize>,

    /// pending call 上限。
    /// `None` = 默认 1000。
    pub max_pending_calls: Option<usize>,

    /// pending call TTL 秒数。
    /// `None` = 默认 30。
    pub pending_ttl_secs: Option<u64>,
}

impl ClientConfig {
    /// 从 JSON 字符串加载配置。
    ///
    /// 失败时返回结构化错误（`E_INVALID_MANIFEST`），
    /// 包含具体的字段路径和错误原因。
    pub fn from_json(json: &str) -> HostResult<Self> {
        let config: Self = serde_json::from_str(json).map_err(|e| {
            HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("客户端配置 JSON 解析失败：{e}"))
        })?;
        config.validate()?;
        Ok(config)
    }

    /// 从 JSON 文件加载配置。
    ///
    /// 文件不存在时返回结构化错误（不创建默认文件，避免覆盖用户配置）。
    pub fn from_file(path: &Path) -> HostResult<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            HostError::new(
                ErrorCode::E_INVALID_MANIFEST,
                format!("读取配置文件 `{}` 失败：{e}", path.display()),
            )
        })?;
        Self::from_json(&content)
    }

    /// 序列化为 JSON 字符串（用于生成配置模板）。
    pub fn to_json(&self) -> HostResult<String> {
        serde_json::to_string_pretty(self).map_err(|e| {
            HostError::new(ErrorCode::E_INVALID_MANIFEST, format!("客户端配置序列化失败：{e}"))
        })
    }

    /// 校验配置合法性。
    ///
    /// 校验项：
    /// - `log_level` 必须是合法值（`error`/`warn`/`info`/`debug`/`trace`）
    /// - `update_check_interval_secs` 必须 > 0
    /// - `RegistryConfigOverride` 中的字段范围检查
    /// - `PluginFilter` 的 allow/deny 不冲突
    pub fn validate(&self) -> HostResult<()> {
        if let Some(ref level) = self.log_level {
            let valid = ["error", "warn", "info", "debug", "trace"];
            if !valid.contains(&level.as_str()) {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("log_level `{level}` 非法，必须是 {valid:?} 之一"),
                ));
            }
        }
        if let Some(secs) = self.update_check_interval_secs {
            if secs == 0 {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    "update_check_interval_secs 必须大于 0",
                ));
            }
        }
        if let Some(ref registry) = self.registry {
            registry.validate()?;
        }
        Ok(())
    }

    /// 构建完整的 `RegistryConfig`（合并缺省值 + 用户覆盖）。
    pub fn registry_config(&self) -> RegistryConfig {
        let base = default_config();
        let Some(ref override_) = self.registry else {
            return base;
        };
        RegistryConfig {
            max_plugins: override_.max_plugins.unwrap_or(base.max_plugins),
            max_active_identities: override_
                .max_active_identities
                .unwrap_or(base.max_active_identities),
            max_pending_calls: override_.max_pending_calls.unwrap_or(base.max_pending_calls),
            pending_ttl: Duration::from_secs(override_.pending_ttl_secs.unwrap_or(30)),
            plugin_filter: override_.plugin_filter.clone(),
        }
    }

    /// 获取日志级别（缺省 `"info"`）。
    pub fn log_level(&self) -> &str {
        self.log_level.as_deref().unwrap_or("info")
    }

    /// 获取更新检查间隔（缺省 86400 秒）。
    pub fn update_check_interval_secs(&self) -> u64 {
        self.update_check_interval_secs.unwrap_or(86400)
    }

    /// 获取插件过滤器引用（如有）。
    pub fn plugin_filter(&self) -> Option<&PluginFilter> {
        self.registry.as_ref().and_then(|r| r.plugin_filter.as_ref())
    }

    /// 某个键的落点事实（见 [`CLIENT_CONFIG_LANDING`]）。
    ///
    /// 表外的键返回 `None`——`ClientConfig` 是 `deny_unknown_fields`，因此这只可能
    /// 发生在调用方拿了一个拼错的字符串时。
    pub fn landing_of(key: &str) -> Option<ClientConfigLanding> {
        CLIENT_CONFIG_LANDING.iter().find(|(name, _)| *name == key).map(|(_, landing)| *landing)
    }

    /// 本配置里**用户实际写了、但没有宿主落点**的键（键名 + 原因）。
    ///
    /// 只列实际出现的键：默认值不算"用户被无声忽略"。返回顺序 = 表顺序，
    /// 便于宿主打印时稳定可比。
    ///
    /// 消费点：参考宿主 `examples/minimal-app` 的 `load_adapter_config`（启动横幅），
    /// 以及装配层 `AdapterConfig::from_client_config` 的注释。前端有一份镜像
    /// （`@tauron/host` 的 `clientConfigUnwiredKeys`），两侧一致性由
    /// `packages/tauron-host/src/gates.test.ts` 机器校验。
    pub fn unwired_fields(&self) -> Vec<(&'static str, &'static str)> {
        CLIENT_CONFIG_LANDING
            .iter()
            .filter(|(key, landing)| {
                matches!(landing, ClientConfigLanding::Deferred(_)) && self.field_present(key)
            })
            .map(|(key, landing)| match landing {
                ClientConfigLanding::Deferred(reason) => (*key, *reason),
                // filter 已排除 Consumed；走到这里说明表被并发改动。
                ClientConfigLanding::Consumed(_) => unreachable!("filter 已排除 Consumed"),
            })
            .collect()
    }

    /// 键是否在本配置里出现（`Some` = 用户写了，或显式写了 `null`）。
    fn field_present(&self, key: &str) -> bool {
        match key {
            "registry" => self.registry.is_some(),
            "log_level" => self.log_level.is_some(),
            "data_dir" => self.data_dir.is_some(),
            "auto_update" => self.auto_update.is_some(),
            "update_check_interval_secs" => self.update_check_interval_secs.is_some(),
            "crash_report_enabled" => self.crash_report_enabled.is_some(),
            "brand_id" => self.brand_id.is_some(),
            "env_overrides" => self.env_overrides.is_some(),
            "plugin_paths" => self.plugin_paths.is_some(),
            "performance_monitoring" => self.performance_monitoring.is_some(),
            _ => false,
        }
    }
}

impl RegistryConfigOverride {
    /// 校验覆盖配置合法性。
    pub fn validate(&self) -> HostResult<()> {
        if let Some(ref filter) = self.plugin_filter {
            filter.validate()?;
        }
        if let Some(max) = self.max_plugins {
            if max == 0 {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    "max_plugins 必须大于 0",
                ));
            }
        }
        if let Some(max) = self.max_active_identities {
            if max == 0 {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    "max_active_identities 必须大于 0",
                ));
            }
        }
        if let Some(max) = self.max_pending_calls {
            if max == 0 {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    "max_pending_calls 必须大于 0",
                ));
            }
        }
        if let Some(secs) = self.pending_ttl_secs {
            if secs == 0 {
                return Err(HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    "pending_ttl_secs 必须大于 0",
                ));
            }
        }
        Ok(())
    }
}

/// 生成最小化配置模板（只含注释说明，无实际值）。
pub fn template_json() -> &'static str {
    r#"{
    "registry": {
        "plugin_filter": {
            "allow": [],
            "deny": [],
            "types": [],
            "platforms": [],
            "include_builtins": true
        },
        "max_plugins": 8,
        "max_active_identities": 8,
        "max_pending_calls": 1000,
        "pending_ttl_secs": 30
    },
    "log_level": "info",
    "data_dir": null,
    "auto_update": true,
    "update_check_interval_secs": 86400,
    "crash_report_enabled": false,
    "brand_id": null,
    "env_overrides": {},
    "plugin_paths": [],
    "performance_monitoring": false
}"#
}

/// 生成全量加载配置（无过滤器，适合开发环境）。
pub fn full_load_json() -> &'static str {
    r#"{
    "log_level": "debug",
    "auto_update": false,
    "crash_report_enabled": true,
    "performance_monitoring": true
}"#
}

/// 生成最小化配置（仅内置插件，适合嵌入式场景）。
pub fn minimal_json() -> &'static str {
    r#"{
    "registry": {
        "plugin_filter": {
            "include_builtins": true,
            "allow": []
        }
    },
    "log_level": "warn",
    "auto_update": false,
    "crash_report_enabled": false
}"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = ClientConfig::default();
        assert!(config.validate().is_ok());
        assert_eq!(config.log_level(), "info");
        assert_eq!(config.update_check_interval_secs(), 86400);
        assert!(config.plugin_filter().is_none());
    }

    /// 模块头那条「典型用法（第三方集成）」示例的**可执行镜像**。
    ///
    /// 为什么不用 doctest 独占：本机 doctest 需要另起 `rustc` 子进程编译示例，
    /// 在管道资源紧张时 `Failed to spawn rustc.exe: Os { code: 231 }`（环境问题，
    /// 非代码问题）——于是"文档里的用法到底编不编得过"在本机**无法验证**。
    /// 这个用例把同一段用法搬进 crate 内测试，任何环境都能验证；
    /// doctest 仍保留（CI 的 Linux runner 上照常跑）。
    #[test]
    fn documented_third_party_usage_compiles_and_works() {
        // 与模块头 doctest 的第一段逐字对应（只把 `tauron_host::config::` 换成 crate 内路径）。
        let config = ClientConfig::from_json(
            r#"{
    "registry": {
        "plugin_filter": {
            "allow": ["com.example.formatter"],
            "types": ["js"],
            "platforms": ["win"]
        }
    }
}"#,
        )
        .unwrap();

        // 与 doctest 的第三段对应：拿到合并后的 RegistryConfig。
        let registry_config = config.registry_config();
        let filter = config.plugin_filter().expect("过滤器应被解析出来");
        assert_eq!(filter.allow, vec!["com.example.formatter".to_string()]);
        assert_eq!(filter.types, vec!["js".to_string()]);
        assert_eq!(filter.platforms, vec!["win".to_string()]);
        // 未声明的项回落到默认值（缺省即安全）。
        assert_eq!(registry_config.max_plugins, default_config().max_plugins);
    }

    #[test]
    fn from_json_full_load() {
        let config = ClientConfig::from_json(full_load_json()).unwrap();
        assert_eq!(config.log_level(), "debug");
        assert!(!config.auto_update.unwrap_or(false));
        assert!(config.crash_report_enabled.unwrap_or(false));
        assert!(config.performance_monitoring.unwrap_or(false));
    }

    #[test]
    fn from_json_minimal() {
        let config = ClientConfig::from_json(minimal_json()).unwrap();
        let filter = config.plugin_filter().unwrap();
        assert!(filter.include_builtins);
        assert!(filter.allow.is_empty());
    }

    #[test]
    fn from_json_template() {
        let config = ClientConfig::from_json(template_json()).unwrap();
        assert!(config.validate().is_ok());
    }

    /// 造一份**每个键都写了**的配置（落点表完备性的输入）。
    fn fully_populated() -> ClientConfig {
        let mut env = std::collections::HashMap::new();
        env.insert("OC_PROFILE".to_string(), "prod".to_string());
        ClientConfig {
            registry: Some(RegistryConfigOverride::default()),
            log_level: Some("info".to_string()),
            data_dir: Some("data".to_string()),
            auto_update: Some(true),
            update_check_interval_secs: Some(60),
            crash_report_enabled: Some(true),
            brand_id: Some("acme".to_string()),
            env_overrides: Some(env),
            plugin_paths: Some(vec!["plugins".to_string()]),
            performance_monitoring: Some(true),
        }
    }

    /// 落点表的**完备性**（轮 2 / F-1）。
    ///
    /// 为什么用 serde 反推而不是手写一份清单：手写清单会在下一次加字段时悄悄失真，
    /// 而"字段加了、没人分类"正是这个缺陷的原始形态——七个键被解析、被校验、被
    /// CLI 写进生成的配置文件，却在生产代码里零读取。基准取 serde 自己吐出的键集合，
    /// 于是「加字段忘了进表」和「表里留了已删字段」都会让测试变红。
    #[test]
    fn landing_table_covers_every_serde_field() {
        let full = fully_populated();
        let json: serde_json::Value = serde_json::from_str(&full.to_json().unwrap()).unwrap();
        let mut serde_keys: Vec<&str> =
            json.as_object().unwrap().keys().map(String::as_str).collect();
        serde_keys.sort_unstable();
        let mut table_keys: Vec<&str> = CLIENT_CONFIG_LANDING.iter().map(|(k, _)| *k).collect();
        table_keys.sort_unstable();
        assert_eq!(
            serde_keys, table_keys,
            "落点表与 `ClientConfig` 的 serde 键集合不一致：新增字段必须当场分类（Consumed 要写出真实消费者）"
        );

        // 键名与 `field_present` 的分支必须一一对上：全量配置下，每个 Deferred 键
        // 都要被如实报出。漏一个分支 ⇒ 那个键"写了不生效"又会重新变成静默。
        let unwired: Vec<&str> = full.unwired_fields().iter().map(|(k, _)| *k).collect();
        let deferred: Vec<&str> = CLIENT_CONFIG_LANDING
            .iter()
            .filter(|(_, landing)| matches!(landing, ClientConfigLanding::Deferred(_)))
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(
            unwired, deferred,
            "写了却没生效的键必须逐个报出（缺原因 = field_present 认不出这个键）"
        );
        for (key, reason) in &full.unwired_fields() {
            assert!(!reason.trim().is_empty(), "键 `{key}` 的 Deferred 原因不得为空");
        }
        // Consumed 键不得混进报告里——否则"已接线"的键会被误报成假接口。
        for (key, _) in CLIENT_CONFIG_LANDING
            .iter()
            .filter(|(_, l)| matches!(l, ClientConfigLanding::Consumed(_)))
        {
            assert!(!unwired.contains(key), "已接线的键 `{key}` 不该出现在未接线报告里");
        }
    }

    /// 「没写 = 不报」：默认配置不产生任何未接线告警，部分配置只报实际写出的键。
    #[test]
    fn unwired_fields_only_report_present_keys() {
        assert!(ClientConfig::default().unwired_fields().is_empty(), "没人写的键谈不上不生效");
        let only_wired = ClientConfig {
            data_dir: Some("data".to_string()),
            env_overrides: Some(std::collections::HashMap::new()),
            ..Default::default()
        };
        assert!(only_wired.unwired_fields().is_empty(), "全接到落点的配置不该报未接线");
        let partial = ClientConfig {
            brand_id: Some("acme".to_string()),
            plugin_paths: Some(vec!["plugins".to_string()]),
            ..Default::default()
        };
        let keys: Vec<&str> = partial.unwired_fields().iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec!["brand_id", "plugin_paths"],
            "报告要精确到用户实际写过的那几个键，顺序随表"
        );
    }

    /// 表外键（拼错的字符串）如实查不到，而不是兜底成"已消费"。
    #[test]
    fn landing_of_unknown_key_is_none() {
        assert!(ClientConfig::landing_of("autoUpdate").is_none());
        assert!(ClientConfig::landing_of("").is_none());
    }

    /// 已接线的 `env_overrides` 必须留在 Consumed 侧，并且写出消费者位置。
    #[test]
    fn env_overrides_is_consumed_with_a_named_landing() {
        let Some(ClientConfigLanding::Consumed(target)) = ClientConfig::landing_of("env_overrides")
        else {
            panic!("env_overrides 必须有真实落点（它接的是 SpawnConfig::env）");
        };
        assert!(target.contains("SpawnConfig::env"), "落点要指到真读它的类型：{target}");
    }

    #[test]
    fn from_json_with_plugin_filter() {
        let json = r#"{
            "registry": {
                "plugin_filter": {
                    "allow": ["com.example.formatter"],
                    "types": ["js"],
                    "platforms": ["win"]
                }
            }
        }"#;
        let config = ClientConfig::from_json(json).unwrap();
        let filter = config.plugin_filter().unwrap();
        assert_eq!(filter.allow, vec!["com.example.formatter"]);
        assert_eq!(filter.types, vec!["js"]);
        assert_eq!(filter.platforms, vec!["win"]);
    }

    #[test]
    fn from_json_with_registry_override() {
        let json = r#"{
            "registry": {
                "max_plugins": 4,
                "max_active_identities": 4,
                "pending_ttl_secs": 60
            }
        }"#;
        let config = ClientConfig::from_json(json).unwrap();
        let rc = config.registry_config();
        assert_eq!(rc.max_plugins, 4);
        assert_eq!(rc.max_active_identities, 4);
        assert_eq!(rc.pending_ttl, Duration::from_secs(60));
        // 未覆盖的字段使用默认值
        assert_eq!(rc.max_pending_calls, 2000);
    }

    #[test]
    fn from_json_invalid_log_level() {
        let json = r#"{ "log_level": "verbose" }"#;
        let err = ClientConfig::from_json(json).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("log_level"));
    }

    #[test]
    fn from_json_zero_update_interval() {
        let json = r#"{ "update_check_interval_secs": 0 }"#;
        let err = ClientConfig::from_json(json).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("update_check_interval_secs"));
    }

    #[test]
    fn from_json_invalid_json() {
        let err = ClientConfig::from_json("{ invalid json }").unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("解析失败"));
    }

    #[test]
    fn from_json_unknown_field_rejected() {
        let json = r#"{ "unknown_field": true }"#;
        let err = ClientConfig::from_json(json).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("unknown"));
    }

    #[test]
    fn from_json_registry_max_plugins_zero() {
        let json = r#"{ "registry": { "max_plugins": 0 } }"#;
        let err = ClientConfig::from_json(json).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("max_plugins"));
    }

    #[test]
    fn registry_config_merge_preserves_defaults() {
        let config = ClientConfig::default();
        let rc = config.registry_config();
        assert_eq!(rc.max_plugins, 8);
        assert_eq!(rc.max_active_identities, 8);
        assert_eq!(rc.max_pending_calls, 2000);
        assert_eq!(rc.pending_ttl, Duration::from_secs(30));
        assert!(rc.plugin_filter.is_none());
    }

    #[test]
    fn registry_config_merge_overrides_correctly() {
        let config = ClientConfig {
            registry: Some(RegistryConfigOverride {
                max_plugins: Some(4),
                max_active_identities: None,
                max_pending_calls: Some(500),
                pending_ttl_secs: Some(60),
                plugin_filter: Some(PluginFilter {
                    deny: vec!["com.example.bad".into()],
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        let rc = config.registry_config();
        assert_eq!(rc.max_plugins, 4);
        assert_eq!(rc.max_active_identities, 8); // 默认值
        assert_eq!(rc.max_pending_calls, 500);
        assert_eq!(rc.pending_ttl, Duration::from_secs(60));
        assert!(rc.plugin_filter.is_some());
        assert_eq!(rc.plugin_filter.unwrap().deny, vec!["com.example.bad"]);
    }

    #[test]
    fn registry_config_override_with_invalid_filter() {
        let config = ClientConfig {
            registry: Some(RegistryConfigOverride {
                plugin_filter: Some(PluginFilter {
                    allow: vec!["com.example.a".into()],
                    deny: vec!["com.example.a".into()],
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("allow"));
    }

    #[test]
    fn to_json_roundtrip() {
        let config = ClientConfig {
            log_level: Some("debug".into()),
            auto_update: Some(false),
            registry: Some(RegistryConfigOverride { max_plugins: Some(4), ..Default::default() }),
            ..Default::default()
        };
        let json = config.to_json().unwrap();
        let back = ClientConfig::from_json(&json).unwrap();
        assert_eq!(back.log_level, Some("debug".into()));
        assert_eq!(back.auto_update, Some(false));
        assert_eq!(back.registry.unwrap().max_plugins, Some(4));
    }

    #[test]
    fn from_file_missing_file() {
        let err = ClientConfig::from_file(Path::new("nonexistent.json")).unwrap_err();
        assert_eq!(err.code, ErrorCode::E_INVALID_MANIFEST);
        assert!(err.message.contains("读取配置文件"));
    }

    #[test]
    fn env_overrides_are_serializable() {
        let mut env = std::collections::HashMap::new();
        env.insert("OC_DEBUG".to_string(), "1".to_string());
        let config = ClientConfig { env_overrides: Some(env), ..Default::default() };
        let json = config.to_json().unwrap();
        assert!(json.contains("OC_DEBUG"));
        let back = ClientConfig::from_json(&json).unwrap();
        assert!(back.env_overrides.is_some());
    }

    #[test]
    fn plugin_paths_are_serializable() {
        let config = ClientConfig {
            plugin_paths: Some(vec!["plugins".into(), "extensions".into()]),
            ..Default::default()
        };
        let json = config.to_json().unwrap();
        let back = ClientConfig::from_json(&json).unwrap();
        assert_eq!(back.plugin_paths.unwrap(), vec!["plugins", "extensions"]);
    }
}
