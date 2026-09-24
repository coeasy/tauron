// §4.8 WASM supervisor：Extism 加载、host_fn 白名单、资源上限、独立子进程承载。
//
// 职责（计划 §4.8）：Extism 加载、host_fn 白名单、资源上限、独立子进程承载。
//
// 关键约束：
// - 跑在专用 supervisor 子进程内（ADR-10）
// - memory.max_pages 配额（默认 64 MB/实例）
// - host_fn 自带 deadline + 非阻塞 IO
// - 实例池（插件单线程）
// - 模块缓存 LRU 有界
// - 移动端不承诺（架构 §4.8 第 4 条）
// - abi.wasm 校验（接口 hash）与框架期望的 host_fn ABI 指纹，不匹配即拒载
// - per-plugin 崩溃计数有上限（缺省 3 次/5min 窗口），超限 → 该插件 `ERRORED(user-confirm)` 且不再随 supervisor 重启载入
//
// 本 crate 不依赖 `tauri`：WASM 管理是抽象的，单元测试用 Mock。

use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod error;
pub mod execute;

pub use error::{WasmError, WasmResult};
pub use execute::{WasmEngine, WasmEngineConfig, ExecutionResult, HostFnCallRecord, WasmPluginStats, HostFnHandler};

// ──────────────────────────────────────────────────────────────────────────
// 配置类型
// ──────────────────────────────────────────────────────────────────────────

/// ABI 指纹（WASM）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmAbiFingerprint {
    /// 接口哈希。
    pub interface_hash: String,
    /// 支持的 host_fn 列表。
    pub host_fns: Vec<String>,
    /// 生成时间。
    pub generated_at: DateTime<Utc>,
}

/// 内存配额配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 最大内存页数（默认 64 MB = 1024 pages）。
    pub max_pages: u32,
    /// 初始内存页数。
    pub initial_pages: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_pages: 1024, // 64 MB
            initial_pages: 16, // 1 MB
        }
    }
}

/// host_fn 白名单配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostFnWhitelist {
    /// 允许的 host_fn 名称列表。
    pub allowed_fns: Vec<String>,
    /// host_fn 调用超时（毫秒）。
    pub call_timeout_ms: u64,
}

impl Default for HostFnWhitelist {
    fn default() -> Self {
        Self {
            allowed_fns: vec![
                "invoke_command".into(),
                "get_setting".into(),
                "set_setting".into(),
                "log".into(),
            ],
            call_timeout_ms: 5000,
        }
    }
}

/// 实例池配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancePoolConfig {
    /// 最大实例数。
    pub max_instances: usize,
    /// 实例空闲超时（毫秒）。
    pub idle_timeout_ms: u64,
    /// 是否启用实例池。
    pub enable_pool: bool,
}

impl Default for InstancePoolConfig {
    fn default() -> Self {
        Self {
            max_instances: 8,
            idle_timeout_ms: 300000, // 5 min
            enable_pool: true,
        }
    }
}

/// 模块缓存配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleCacheConfig {
    /// 最大缓存模块数。
    pub max_cached_modules: usize,
    /// 缓存有效期（秒）。
    pub cache_ttl_secs: u64,
}

impl Default for ModuleCacheConfig {
    fn default() -> Self {
        Self {
            max_cached_modules: 32,
            cache_ttl_secs: 3600, // 1 hour
        }
    }
}

/// 崩溃计数限制。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashLimitConfig {
    /// 时间窗口（秒）。
    pub window_secs: u64,
    /// 窗口内最大崩溃次数。
    pub max_crashes: u32,
}

impl Default for CrashLimitConfig {
    fn default() -> Self {
        Self {
            window_secs: 300, // 5 min
            max_crashes: 3,
        }
    }
}

/// WASM 插件完整配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginConfig {
    /// 插件 ID。
    pub plugin_id: String,
    /// WASM 模块路径。
    pub module_path: String,
    /// ABI 指纹。
    pub abi: WasmAbiFingerprint,
    /// 内存配额。
    pub memory: MemoryConfig,
    /// host_fn 白名单。
    pub host_fn_whitelist: HostFnWhitelist,
    /// 实例池配置。
    pub instance_pool: InstancePoolConfig,
    /// 模块缓存配置。
    pub module_cache: ModuleCacheConfig,
    /// 崩溃计数限制。
    pub crash_limit: CrashLimitConfig,
}

// ──────────────────────────────────────────────────────────────────────────
// 配置验证
// ──────────────────────────────────────────────────────────────────────────

/// 验证内存配置。
pub fn validate_memory_config(config: &MemoryConfig) -> WasmResult<()> {
    if config.max_pages == 0 {
        return Err(WasmError::InvalidConfig("max_pages 不能为 0".into()));
    }
    if config.initial_pages > config.max_pages {
        return Err(WasmError::InvalidConfig(format!(
            "initial_pages ({}) 不能超过 max_pages ({})",
            config.initial_pages, config.max_pages
        )));
    }
    Ok(())
}

/// 验证 host_fn 白名单配置。
pub fn validate_host_fn_whitelist(config: &HostFnWhitelist) -> WasmResult<()> {
    if config.call_timeout_ms == 0 {
        return Err(WasmError::InvalidConfig("call_timeout_ms 不能为 0".into()));
    }
    Ok(())
}

/// 验证实例池配置。
pub fn validate_instance_pool_config(config: &InstancePoolConfig) -> WasmResult<()> {
    if config.max_instances == 0 {
        return Err(WasmError::InvalidConfig("max_instances 不能为 0".into()));
    }
    if config.idle_timeout_ms == 0 {
        return Err(WasmError::InvalidConfig("idle_timeout_ms 不能为 0".into()));
    }
    Ok(())
}

/// 验证模块缓存配置。
pub fn validate_module_cache_config(config: &ModuleCacheConfig) -> WasmResult<()> {
    if config.max_cached_modules == 0 {
        return Err(WasmError::InvalidConfig("max_cached_modules 不能为 0".into()));
    }
    if config.cache_ttl_secs == 0 {
        return Err(WasmError::InvalidConfig("cache_ttl_secs 不能为 0".into()));
    }
    Ok(())
}

/// 验证崩溃限制配置。
pub fn validate_crash_limit_config(config: &CrashLimitConfig) -> WasmResult<()> {
    if config.window_secs == 0 {
        return Err(WasmError::InvalidConfig("window_secs 不能为 0".into()));
    }
    if config.max_crashes == 0 {
        return Err(WasmError::InvalidConfig("max_crashes 不能为 0".into()));
    }
    Ok(())
}

/// 验证完整插件配置。
pub fn validate_plugin_config(config: &WasmPluginConfig) -> WasmResult<()> {
    if config.plugin_id.is_empty() {
        return Err(WasmError::InvalidConfig("plugin_id 不能为空".into()));
    }
    if config.module_path.is_empty() {
        return Err(WasmError::InvalidConfig("module_path 不能为空".into()));
    }
    if config.abi.interface_hash.is_empty() {
        return Err(WasmError::InvalidConfig("abi.interface_hash 不能为空".into()));
    }
    validate_memory_config(&config.memory)?;
    validate_host_fn_whitelist(&config.host_fn_whitelist)?;
    validate_instance_pool_config(&config.instance_pool)?;
    validate_module_cache_config(&config.module_cache)?;
    validate_crash_limit_config(&config.crash_limit)?;
    Ok(())
}

/// 验证 host_fn 是否在白名单中。
pub fn validate_host_fn(
    fn_name: &str,
    whitelist: &HostFnWhitelist,
) -> WasmResult<()> {
    if whitelist.allowed_fns.contains(&fn_name.to_string()) {
        Ok(())
    } else {
        Err(WasmError::HostFnNotAllowed {
            fn_name: fn_name.to_string(),
        })
    }
}

/// 验证 ABI 指纹。
pub fn validate_abi(
    expected: &WasmAbiFingerprint,
    actual: &WasmAbiFingerprint,
) -> WasmResult<()> {
    if expected.interface_hash != actual.interface_hash {
        return Err(WasmError::AbiMismatch {
            expected: expected.interface_hash.clone(),
            actual: actual.interface_hash.clone(),
        });
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// 实例池管理
// ──────────────────────────────────────────────────────────────────────────

/// 实例状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceState {
    /// 空闲。
    Idle,
    /// 使用中。
    InUse,
    /// 已回收。
    Reclaimed,
}

/// WASM 实例。
#[derive(Debug, Clone)]
pub struct WasmInstance {
    /// 实例 ID。
    pub instance_id: String,
    /// 插件 ID。
    pub plugin_id: String,
    /// 实例状态。
    pub state: InstanceState,
    /// 最后使用时间。
    pub last_used: Instant,
    /// 内存使用量（页数）。
    pub memory_pages: u32,
    /// 创建时间。
    pub created_at: Instant,
}

/// 实例池。
pub struct InstancePool {
    config: InstancePoolConfig,
    instances: HashMap<String, WasmInstance>,
    order: Vec<String>, // LRU 顺序
    next_instance_id: u32,
}

impl InstancePool {
    /// 创建新的实例池。
    pub fn new(config: InstancePoolConfig) -> Self {
        Self {
            config,
            instances: HashMap::new(),
            order: Vec::new(),
            next_instance_id: 1,
        }
    }

    /// 获取当前配置。
    pub fn config(&self) -> &InstancePoolConfig {
        &self.config
    }

    /// 获取实例数量。
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// 尝试创建实例。
    pub fn try_create_instance(&mut self, plugin_id: &str) -> WasmResult<WasmInstance> {
        if !self.config.enable_pool {
            return Err(WasmError::InstancePoolFull {
                plugin_id: plugin_id.to_string(),
            });
        }

        if self.instances.len() >= self.config.max_instances {
            // 尝试驱逐最旧的空闲实例
            if self.evict_lru().is_none() {
                return Err(WasmError::InstancePoolFull {
                    plugin_id: plugin_id.to_string(),
                });
            }
        }

        let instance = WasmInstance {
            instance_id: format!("instance-{}", self.next_instance_id),
            plugin_id: plugin_id.to_string(),
            state: InstanceState::InUse,
            last_used: Instant::now(),
            memory_pages: 0,
            created_at: Instant::now(),
        };
        self.next_instance_id += 1;

        self.instances.insert(instance.instance_id.clone(), instance.clone());
        self.order.push(instance.instance_id.clone());
        Ok(instance)
    }

    /// 回收实例。
    pub fn reclaim_instance(&mut self, instance_id: &str) -> Option<WasmInstance> {
        // 更新实例状态
        if let Some(instance) = self.instances.get_mut(instance_id) {
            instance.state = InstanceState::Idle;
            instance.last_used = Instant::now();
        }

        // 更新 LRU 顺序
        let pos = self.order.iter().position(|id| id == instance_id)?;
        self.order.remove(pos);
        self.order.push(instance_id.to_string());

        // 获取并返回实例
        self.instances.get(instance_id).cloned()
    }

    /// 驱逐 LRU 空闲实例。
    pub fn evict_lru(&mut self) -> Option<String> {
        // 收集空闲实例的 ID
        let idle_ids: Vec<String> = self
            .order
            .iter()
            .rev()
            .filter_map(|id| {
                if let Some(instance) = self.instances.get(id) {
                    if instance.state == InstanceState::Idle {
                        return Some(id.clone());
                    }
                }
                None
            })
            .collect();

        if let Some(id) = idle_ids.into_iter().next() {
            self.instances.remove(&id);
            let pos = self.order.iter().position(|x| x == &id)?;
            self.order.remove(pos);
            Some(id)
        } else {
            None
        }
    }

    /// 获取实例。
    pub fn get_instance(&self, instance_id: &str) -> Option<&WasmInstance> {
        self.instances.get(instance_id)
    }

    /// 清理超时实例。
    pub fn cleanup_idle(&mut self) -> Vec<String> {
        let now = Instant::now();
        let timeout = std::time::Duration::from_millis(self.config.idle_timeout_ms);
        let mut to_remove = Vec::new();

        for (id, instance) in self.instances.iter() {
            if instance.state == InstanceState::Idle
                && now.duration_since(instance.last_used) > timeout
            {
                to_remove.push(id.clone());
            }
        }

        for id in &to_remove {
            self.instances.remove(id);
            let pos = self.order.iter().position(|x| x == id);
            if let Some(pos) = pos {
                self.order.remove(pos);
            }
        }

        to_remove
    }

    /// 清除指定插件的所有实例。
    pub fn clear_plugin(&mut self, plugin_id: &str) -> usize {
        let to_remove: Vec<String> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.plugin_id == plugin_id)
            .map(|(id, _)| id.clone())
            .collect();

        for id in &to_remove {
            self.instances.remove(id);
            let pos = self.order.iter().position(|x| x == id);
            if let Some(pos) = pos {
                self.order.remove(pos);
            }
        }

        to_remove.len()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 模块缓存
// ──────────────────────────────────────────────────────────────────────────

/// 缓存模块。
#[derive(Debug, Clone)]
pub struct CachedModule {
    /// 插件 ID。
    pub plugin_id: String,
    /// 模块哈希。
    pub module_hash: String,
    /// 缓存时间。
    pub cached_at: DateTime<Utc>,
    /// 模块大小（字节）。
    pub size_bytes: u64,
}

/// 模块缓存。
pub struct ModuleCache {
    config: ModuleCacheConfig,
    cache: HashMap<String, CachedModule>,
    order: Vec<String>, // LRU 顺序
}

impl ModuleCache {
    /// 创建新的模块缓存。
    pub fn new(config: ModuleCacheConfig) -> Self {
        Self {
            config,
            cache: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// 获取当前配置。
    pub fn config(&self) -> &ModuleCacheConfig {
        &self.config
    }

    /// 获取缓存数量。
    pub fn cache_count(&self) -> usize {
        self.cache.len()
    }

    /// 获取缓存的模块。
    pub fn get(&self, plugin_id: &str) -> Option<&CachedModule> {
        self.cache.get(plugin_id)
    }

    /// 插入模块到缓存。
    pub fn insert(&mut self, module: CachedModule) -> WasmResult<()> {
        if self.cache.len() >= self.config.max_cached_modules {
            // 驱逐最旧的
            if let Some(evicted_id) = self.order.first().cloned() {
                self.cache.remove(&evicted_id);
                self.order.remove(0);
            } else {
                return Err(WasmError::ModuleCacheFull {
                    plugin_id: module.plugin_id.clone(),
                });
            }
        }

        // 如果已存在，更新 LRU 顺序
        if self.cache.contains_key(&module.plugin_id) {
            let pos = self.order.iter().position(|id| id == &module.plugin_id);
            if let Some(pos) = pos {
                self.order.remove(pos);
            }
        }

        let plugin_id = module.plugin_id.clone();
        self.cache.insert(plugin_id.clone(), module);
        self.order.push(plugin_id);
        Ok(())
    }

    /// 移除缓存的模块。
    pub fn remove(&mut self, plugin_id: &str) -> Option<CachedModule> {
        let module = self.cache.remove(plugin_id);
        let pos = self.order.iter().position(|id| id == plugin_id);
        if let Some(pos) = pos {
            self.order.remove(pos);
        }
        module
    }

    /// 清理过期模块。
    pub fn cleanup_expired(&mut self) -> Vec<String> {
        let now = Utc::now();
        let ttl = chrono::Duration::seconds(self.config.cache_ttl_secs as i64);
        let mut to_remove = Vec::new();

        for (id, module) in self.cache.iter() {
            if now - module.cached_at > ttl {
                to_remove.push(id.clone());
            }
        }

        for id in &to_remove {
            self.cache.remove(id);
            let pos = self.order.iter().position(|x| x == id);
            if let Some(pos) = pos {
                self.order.remove(pos);
            }
        }

        to_remove
    }

    /// 清除指定插件的缓存。
    pub fn clear_plugin(&mut self, plugin_id: &str) -> Option<CachedModule> {
        self.remove(plugin_id)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 崩溃追踪
// ──────────────────────────────────────────────────────────────────────────

/// 崩溃记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WasmCrashRecord {
    timestamp: DateTime<Utc>,
    plugin_id: String,
}

/// 崩溃追踪器。
pub struct WasmCrashTracker {
    records: HashMap<String, Vec<WasmCrashRecord>>,
    limit: CrashLimitConfig,
}

impl WasmCrashTracker {
    /// 创建新的崩溃追踪器。
    pub fn new(limit: CrashLimitConfig) -> Self {
        Self {
            records: HashMap::new(),
            limit,
        }
    }

    /// 使用默认限制创建。
    pub fn default_tracker() -> Self {
        Self::new(CrashLimitConfig::default())
    }

    /// 记录崩溃。
    ///
    /// 返回 true 表示未超限，false 表示已超限。
    pub fn record_crash(&mut self, plugin_id: &str) -> bool {
        let now = Utc::now();
        let window_start = now - chrono::Duration::seconds(self.limit.window_secs as i64);

        // 获取该插件的崩溃记录
        let records = self.records.entry(plugin_id.to_string()).or_default();

        // 清理过期记录
        records.retain(|r| r.timestamp >= window_start);

        // 记录新崩溃
        records.push(WasmCrashRecord {
            timestamp: now,
            plugin_id: plugin_id.to_string(),
        });

        // 检查是否超限
        records.len() as u32 <= self.limit.max_crashes
    }

    /// 检查是否已超限。
    pub fn is_exceeded(&self, plugin_id: &str) -> bool {
        let now = Utc::now();
        let window_start = now - chrono::Duration::seconds(self.limit.window_secs as i64);

        match self.records.get(plugin_id) {
            Some(records) => records
                .iter()
                .filter(|r| r.timestamp >= window_start)
                .count() as u32
                > self.limit.max_crashes,
            None => false,
        }
    }

    /// 清除指定插件的崩溃记录。
    pub fn clear(&mut self, plugin_id: &str) {
        self.records.remove(plugin_id);
    }

    /// 获取指定插件的崩溃次数（窗口内）。
    pub fn crash_count(&self, plugin_id: &str) -> u32 {
        let now = Utc::now();
        let window_start = now - chrono::Duration::seconds(self.limit.window_secs as i64);

        self.records
            .get(plugin_id)
            .map(|records| {
                records
                    .iter()
                    .filter(|r| r.timestamp >= window_start)
                    .count() as u32
            })
            .unwrap_or(0)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_abi() -> WasmAbiFingerprint {
        WasmAbiFingerprint {
            interface_hash: "hash123".into(),
            host_fns: vec!["invoke_command".into(), "get_setting".into()],
            generated_at: Utc::now(),
        }
    }

    fn make_plugin_config() -> WasmPluginConfig {
        WasmPluginConfig {
            plugin_id: "test.plugin".into(),
            module_path: "/path/to/module.wasm".into(),
            abi: make_abi(),
            memory: MemoryConfig::default(),
            host_fn_whitelist: HostFnWhitelist::default(),
            instance_pool: InstancePoolConfig::default(),
            module_cache: ModuleCacheConfig::default(),
            crash_limit: CrashLimitConfig::default(),
        }
    }

    // ── 配置验证测试 ──

    #[test]
    fn test_validate_memory_config_valid() {
        let config = MemoryConfig::default();
        assert!(validate_memory_config(&config).is_ok());
    }

    #[test]
    fn test_validate_memory_config_zero_max_pages() {
        let mut config = MemoryConfig::default();
        config.max_pages = 0;
        assert!(validate_memory_config(&config).is_err());
    }

    #[test]
    fn test_validate_memory_config_initial_exceeds_max() {
        let mut config = MemoryConfig::default();
        config.initial_pages = 2048;
        config.max_pages = 1024;
        assert!(validate_memory_config(&config).is_err());
    }

    #[test]
    fn test_validate_host_fn_whitelist_valid() {
        let config = HostFnWhitelist::default();
        assert!(validate_host_fn_whitelist(&config).is_ok());
    }

    #[test]
    fn test_validate_host_fn_whitelist_zero_timeout() {
        let mut config = HostFnWhitelist::default();
        config.call_timeout_ms = 0;
        assert!(validate_host_fn_whitelist(&config).is_err());
    }

    #[test]
    fn test_validate_instance_pool_config_valid() {
        let config = InstancePoolConfig::default();
        assert!(validate_instance_pool_config(&config).is_ok());
    }

    #[test]
    fn test_validate_instance_pool_config_zero_max() {
        let mut config = InstancePoolConfig::default();
        config.max_instances = 0;
        assert!(validate_instance_pool_config(&config).is_err());
    }

    #[test]
    fn test_validate_module_cache_config_valid() {
        let config = ModuleCacheConfig::default();
        assert!(validate_module_cache_config(&config).is_ok());
    }

    #[test]
    fn test_validate_module_cache_config_zero_max() {
        let mut config = ModuleCacheConfig::default();
        config.max_cached_modules = 0;
        assert!(validate_module_cache_config(&config).is_err());
    }

    #[test]
    fn test_validate_crash_limit_config_valid() {
        let config = CrashLimitConfig::default();
        assert!(validate_crash_limit_config(&config).is_ok());
    }

    #[test]
    fn test_validate_crash_limit_config_zero_window() {
        let mut config = CrashLimitConfig::default();
        config.window_secs = 0;
        assert!(validate_crash_limit_config(&config).is_err());
    }

    #[test]
    fn test_validate_plugin_config_valid() {
        let config = make_plugin_config();
        assert!(validate_plugin_config(&config).is_ok());
    }

    #[test]
    fn test_validate_plugin_config_empty_plugin_id() {
        let mut config = make_plugin_config();
        config.plugin_id = String::new();
        assert!(validate_plugin_config(&config).is_err());
    }

    #[test]
    fn test_validate_plugin_config_empty_module_path() {
        let mut config = make_plugin_config();
        config.module_path = String::new();
        assert!(validate_plugin_config(&config).is_err());
    }

    #[test]
    fn test_validate_plugin_config_empty_interface_hash() {
        let mut config = make_plugin_config();
        config.abi.interface_hash = String::new();
        assert!(validate_plugin_config(&config).is_err());
    }

    // ── host_fn 白名单测试 ──

    #[test]
    fn test_validate_host_fn_allowed() {
        let whitelist = HostFnWhitelist::default();
        assert!(validate_host_fn("invoke_command", &whitelist).is_ok());
    }

    #[test]
    fn test_validate_host_fn_not_allowed() {
        let whitelist = HostFnWhitelist::default();
        assert!(validate_host_fn("dangerous_fn", &whitelist).is_err());
    }

    #[test]
    fn test_validate_host_fn_empty_list() {
        let whitelist = HostFnWhitelist {
            allowed_fns: vec![],
            call_timeout_ms: 5000,
        };
        assert!(validate_host_fn("any_fn", &whitelist).is_err());
    }

    // ── ABI 校验测试 ──

    #[test]
    fn test_validate_abi_match() {
        let expected = make_abi();
        let actual = make_abi();
        assert!(validate_abi(&expected, &actual).is_ok());
    }

    #[test]
    fn test_validate_abi_mismatch() {
        let expected = make_abi();
        let mut actual = make_abi();
        actual.interface_hash = "different".into();
        let err = validate_abi(&expected, &actual).unwrap_err();
        assert!(matches!(err, WasmError::AbiMismatch { .. }));
    }

    // ── 实例池测试 ──

    #[test]
    fn test_instance_pool_create() {
        let config = InstancePoolConfig {
            max_instances: 2,
            idle_timeout_ms: 300000,
            enable_pool: true,
        };
        let mut pool = InstancePool::new(config);

        let instance = pool.try_create_instance("test.plugin").unwrap();
        assert!(instance.instance_id.starts_with("instance-"));
        assert_eq!(pool.instance_count(), 1);
    }

    #[test]
    fn test_instance_pool_full() {
        let config = InstancePoolConfig {
            max_instances: 1,
            idle_timeout_ms: 300000,
            enable_pool: true,
        };
        let mut pool = InstancePool::new(config);

        pool.try_create_instance("test.plugin").unwrap();
        // 第二个实例应失败（没有空闲实例可驱逐）
        let result = pool.try_create_instance("test.plugin2");
        assert!(result.is_err());
    }

    #[test]
    fn test_instance_pool_evict_lru() {
        let config = InstancePoolConfig {
            max_instances: 1,
            idle_timeout_ms: 300000,
            enable_pool: true,
        };
        let mut pool = InstancePool::new(config);

        let instance = pool.try_create_instance("test.plugin").unwrap();
        pool.reclaim_instance(&instance.instance_id);

        // 现在应该可以创建新实例（驱逐旧的）
        let new_instance = pool.try_create_instance("test.plugin2").unwrap();
        assert_ne!(new_instance.instance_id, instance.instance_id);
    }

    #[test]
    fn test_instance_pool_reclaim() {
        let config = InstancePoolConfig::default();
        let mut pool = InstancePool::new(config);

        let instance = pool.try_create_instance("test.plugin").unwrap();
        assert!(pool.get_instance(&instance.instance_id).is_some());

        pool.reclaim_instance(&instance.instance_id);
        let reclaimed = pool.get_instance(&instance.instance_id);
        assert!(reclaimed.is_some());
        assert_eq!(reclaimed.unwrap().state, InstanceState::Idle);
    }

    #[test]
    fn test_instance_pool_clear_plugin() {
        let config = InstancePoolConfig::default();
        let mut pool = InstancePool::new(config);

        pool.try_create_instance("plugin.a").unwrap();
        pool.try_create_instance("plugin.b").unwrap();
        pool.try_create_instance("plugin.a").unwrap();

        let cleared = pool.clear_plugin("plugin.a");
        assert_eq!(cleared, 2);
        assert_eq!(pool.instance_count(), 1);
    }

    #[test]
    fn test_instance_pool_disabled() {
        let config = InstancePoolConfig {
            enable_pool: false,
            ..Default::default()
        };
        let mut pool = InstancePool::new(config);
        let result = pool.try_create_instance("test.plugin");
        assert!(result.is_err());
    }

    // ── 模块缓存测试 ──

    #[test]
    fn test_module_cache_insert() {
        let config = ModuleCacheConfig::default();
        let mut cache = ModuleCache::new(config);

        let module = CachedModule {
            plugin_id: "test.plugin".into(),
            module_hash: "hash123".into(),
            cached_at: Utc::now(),
            size_bytes: 1024,
        };
        cache.insert(module).unwrap();
        assert_eq!(cache.cache_count(), 1);
    }

    #[test]
    fn test_module_cache_get() {
        let config = ModuleCacheConfig::default();
        let mut cache = ModuleCache::new(config);

        let module = CachedModule {
            plugin_id: "test.plugin".into(),
            module_hash: "hash123".into(),
            cached_at: Utc::now(),
            size_bytes: 1024,
        };
        cache.insert(module).unwrap();

        let retrieved = cache.get("test.plugin");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().module_hash, "hash123");
    }

    #[test]
    fn test_module_cache_evict() {
        let config = ModuleCacheConfig {
            max_cached_modules: 1,
            cache_ttl_secs: 3600,
        };
        let mut cache = ModuleCache::new(config);

        let module1 = CachedModule {
            plugin_id: "plugin.a".into(),
            module_hash: "hash1".into(),
            cached_at: Utc::now(),
            size_bytes: 1024,
        };
        let module2 = CachedModule {
            plugin_id: "plugin.b".into(),
            module_hash: "hash2".into(),
            cached_at: Utc::now(),
            size_bytes: 2048,
        };

        cache.insert(module1).unwrap();
        cache.insert(module2).unwrap(); // 应驱逐 module1

        assert_eq!(cache.cache_count(), 1);
        assert!(cache.get("plugin.a").is_none());
        assert!(cache.get("plugin.b").is_some());
    }

    #[test]
    fn test_module_cache_remove() {
        let config = ModuleCacheConfig::default();
        let mut cache = ModuleCache::new(config);

        let module = CachedModule {
            plugin_id: "test.plugin".into(),
            module_hash: "hash123".into(),
            cached_at: Utc::now(),
            size_bytes: 1024,
        };
        cache.insert(module).unwrap();

        let removed = cache.remove("test.plugin");
        assert!(removed.is_some());
        assert_eq!(cache.cache_count(), 0);
    }

    #[test]
    fn test_module_cache_clear_plugin() {
        let config = ModuleCacheConfig::default();
        let mut cache = ModuleCache::new(config);

        let module = CachedModule {
            plugin_id: "test.plugin".into(),
            module_hash: "hash123".into(),
            cached_at: Utc::now(),
            size_bytes: 1024,
        };
        cache.insert(module).unwrap();

        let cleared = cache.clear_plugin("test.plugin");
        assert!(cleared.is_some());
        assert_eq!(cache.cache_count(), 0);
    }

    // ── 崩溃追踪测试 ──

    #[test]
    fn test_wasm_crash_tracker_record() {
        let mut tracker = WasmCrashTracker::default_tracker();
        let exceeded = tracker.record_crash("test.plugin");
        assert!(exceeded); // 未超限
        assert_eq!(tracker.crash_count("test.plugin"), 1);
    }

    #[test]
    fn test_wasm_crash_tracker_exceed_limit() {
        let limit = CrashLimitConfig {
            window_secs: 300,
            max_crashes: 3,
        };
        let mut tracker = WasmCrashTracker::new(limit);

        // 记录 3 次崩溃
        for _ in 0..3 {
            tracker.record_crash("test.plugin");
        }
        assert_eq!(tracker.crash_count("test.plugin"), 3);
        assert!(!tracker.is_exceeded("test.plugin"));

        // 第 4 次崩溃超限
        let exceeded = tracker.record_crash("test.plugin");
        assert!(!exceeded); // 已超限
        assert!(tracker.is_exceeded("test.plugin"));
    }

    #[test]
    fn test_wasm_crash_tracker_clear() {
        let mut tracker = WasmCrashTracker::default_tracker();
        tracker.record_crash("test.plugin");
        tracker.clear("test.plugin");
        assert_eq!(tracker.crash_count("test.plugin"), 0);
    }

    #[test]
    fn test_wasm_crash_tracker_multiple_plugins() {
        let mut tracker = WasmCrashTracker::default_tracker();
        tracker.record_crash("plugin.a");
        tracker.record_crash("plugin.b");
        assert_eq!(tracker.crash_count("plugin.a"), 1);
        assert_eq!(tracker.crash_count("plugin.b"), 1);
        assert_eq!(tracker.crash_count("plugin.c"), 0);
    }
}
