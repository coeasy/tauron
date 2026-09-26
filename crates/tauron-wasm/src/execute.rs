// WASM 执行器：模块加载、实例创建、host_fn 调用、内存管理。
//
// 职责（计划 §4.8）：
// - Extism 加载（模拟）
// - host_fn 白名单执行
// - 内存配额执行
// - 实例池管理
// - 模块缓存
//
// 本模块不依赖 `tauri`：WASM 管理是抽象的，单元测试用 Mock。

use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    validate_host_fn, validate_plugin_config, CachedModule, CrashLimitConfig, InstancePool,
    InstancePoolConfig, ModuleCache, ModuleCacheConfig, WasmCrashTracker, WasmError,
    WasmPluginConfig, WasmResult,
};

// ──────────────────────────────────────────────────────────────────────────
// 执行结果
// ──────────────────────────────────────────────────────────────────────────

/// 执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionResult {
    /// 是否成功。
    pub success: bool,
    /// 返回值（JSON 序列化的字符串）。
    pub return_value: Option<String>,
    /// 错误信息。
    pub error: Option<String>,
    /// 执行时间（毫秒）。
    pub duration_ms: u64,
    /// 内存使用量（页数）。
    pub memory_pages_used: u32,
    /// host_fn 调用次数。
    pub host_fn_calls: usize,
    /// 实例 ID。
    pub instance_id: String,
}

/// host_fn 调用记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFnCallRecord {
    /// 函数名。
    pub fn_name: String,
    /// 调用参数（JSON）。
    pub args: String,
    /// 返回值（JSON）。
    pub return_value: String,
    /// 执行时间（毫秒）。
    pub duration_ms: u64,
    /// 调用时间。
    pub timestamp: DateTime<Utc>,
}

// ──────────────────────────────────────────────────────────────────────────
// WASM 引擎
// ──────────────────────────────────────────────────────────────────────────

/// host_fn 处理函数类型。
pub type HostFnHandler = Box<dyn Fn(String, String) -> Result<String, String> + Send + Sync>;

/// WASM 引擎配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmEngineConfig {
    /// 实例池配置。
    pub instance_pool: InstancePoolConfig,
    /// 模块缓存配置。
    pub module_cache: ModuleCacheConfig,
    /// 崩溃限制配置。
    pub crash_limit: CrashLimitConfig,
}

/// WASM 引擎。
pub struct WasmEngine {
    config: WasmEngineConfig,
    pool: InstancePool,
    cache: ModuleCache,
    crash_tracker: WasmCrashTracker,
    host_fns: HashMap<String, HostFnHandler>,
    host_fn_calls: Vec<HostFnCallRecord>,
}

impl WasmEngine {
    /// 创建新的 WASM 引擎。
    pub fn new(config: WasmEngineConfig) -> Self {
        Self {
            pool: InstancePool::new(config.instance_pool.clone()),
            cache: ModuleCache::new(config.module_cache.clone()),
            crash_tracker: WasmCrashTracker::new(config.crash_limit.clone()),
            config,
            host_fns: HashMap::new(),
            host_fn_calls: Vec::new(),
        }
    }

    /// 使用默认配置创建。
    pub fn default_engine() -> Self {
        Self::new(WasmEngineConfig::default())
    }

    /// 获取当前配置。
    pub fn config(&self) -> &WasmEngineConfig {
        &self.config
    }

    /// 获取实例池。
    pub fn pool(&self) -> &InstancePool {
        &self.pool
    }

    /// 获取模块缓存。
    pub fn cache(&self) -> &ModuleCache {
        &self.cache
    }

    /// 获取崩溃追踪器。
    pub fn crash_tracker(&self) -> &WasmCrashTracker {
        &self.crash_tracker
    }

    /// 获取 host_fn 调用记录。
    pub fn host_fn_calls(&self) -> &[HostFnCallRecord] {
        &self.host_fn_calls
    }

    /// 获取实例数量。
    pub fn instance_count(&self) -> usize {
        self.pool.instance_count()
    }

    /// 获取缓存数量。
    pub fn cache_count(&self) -> usize {
        self.cache.cache_count()
    }

    /// 注册 host_fn 处理器。
    pub fn register_host_fn(&mut self, fn_name: &str, handler: HostFnHandler) {
        self.host_fns.insert(fn_name.to_string(), handler);
    }

    /// 移除 host_fn 处理器。
    pub fn unregister_host_fn(&mut self, fn_name: &str) -> Option<HostFnHandler> {
        self.host_fns.remove(fn_name)
    }

    /// 检查 host_fn 是否已注册。
    pub fn has_host_fn(&self, fn_name: &str) -> bool {
        self.host_fns.contains_key(fn_name)
    }

    /// 加载模块到缓存。
    pub fn load_module(
        &mut self,
        plugin_id: &str,
        module_hash: &str,
        size_bytes: u64,
    ) -> WasmResult<()> {
        let module = CachedModule {
            plugin_id: plugin_id.to_string(),
            module_hash: module_hash.to_string(),
            cached_at: Utc::now(),
            size_bytes,
        };
        self.cache.insert(module)
    }

    /// 执行插件代码。
    pub fn execute(
        &mut self,
        plugin_config: &WasmPluginConfig,
        function_name: &str,
        args: &str,
    ) -> WasmResult<ExecutionResult> {
        let start = Instant::now();

        // 1. 验证配置
        validate_plugin_config(plugin_config)?;

        // 2. 验证 host_fn 白名单
        for fn_name in &plugin_config.host_fn_whitelist.allowed_fns {
            validate_host_fn(fn_name, &plugin_config.host_fn_whitelist)?;
        }

        // 3. 检查崩溃限制
        if self.crash_tracker.is_exceeded(&plugin_config.plugin_id) {
            return Err(WasmError::CrashLimitExceeded {
                plugin_id: plugin_config.plugin_id.clone(),
            });
        }

        // 4. 从池获取或创建实例
        let instance = match self.pool.get_instance(&plugin_config.plugin_id) {
            Some(existing) if existing.state == crate::InstanceState::Idle => {
                // 重用空闲实例
                let instance_id = existing.instance_id.clone();
                self.pool.reclaim_instance(&instance_id);
                instance_id
            }
            _ => {
                // 创建新实例
                match self.pool.try_create_instance(&plugin_config.plugin_id) {
                    Ok(instance) => instance.instance_id,
                    Err(e) => {
                        self.crash_tracker.record_crash(&plugin_config.plugin_id);
                        return Err(e);
                    }
                }
            }
        };

        // 5. 模拟执行（实际实现会调用 Extism）
        let mut host_fn_call_count = 0;
        let mut memory_pages_used = plugin_config.memory.initial_pages;

        // 6. 模拟 host_fn 调用
        for (i, fn_name) in plugin_config.host_fn_whitelist.allowed_fns.iter().enumerate() {
            if i >= 3 {
                break; // 最多调用 3 次 host_fn
            }
            let call_start = Instant::now();

            // 检查 host_fn 是否已注册
            if let Some(handler) = self.host_fns.get(fn_name) {
                let result = handler(args.to_string(), args.to_string());
                let duration_ms = call_start.elapsed().as_millis() as u64;

                let call_record = HostFnCallRecord {
                    fn_name: fn_name.clone(),
                    args: args.to_string(),
                    return_value: result.unwrap_or_else(|e| e),
                    duration_ms,
                    timestamp: Utc::now(),
                };
                self.host_fn_calls.push(call_record);
                host_fn_call_count += 1;
            } else {
                // host_fn 未注册，模拟调用
                let return_value = format!("{{\"result\":\"mock_{}\"}}", fn_name);
                let call_record = HostFnCallRecord {
                    fn_name: fn_name.clone(),
                    args: args.to_string(),
                    return_value: return_value.clone(),
                    duration_ms: 1,
                    timestamp: Utc::now(),
                };
                self.host_fn_calls.push(call_record);
                host_fn_call_count += 1;
            }

            // 模拟内存使用增长
            memory_pages_used = (memory_pages_used + 1).min(plugin_config.memory.max_pages);
        }

        // 7. 构造返回结果
        let duration_ms = start.elapsed().as_millis() as u64;
        let return_value = format!("{{\"function\":\"{}\",\"result\":\"ok\"}}", function_name);

        Ok(ExecutionResult {
            success: true,
            return_value: Some(return_value),
            error: None,
            duration_ms: duration_ms.max(1), // 至少 1ms
            memory_pages_used,
            host_fn_calls: host_fn_call_count,
            instance_id: instance.clone(),
        })
    }

    /// 执行并处理错误。
    pub fn execute_with_recovery(
        &mut self,
        plugin_config: &WasmPluginConfig,
        function_name: &str,
        args: &str,
    ) -> WasmResult<ExecutionResult> {
        match self.execute(plugin_config, function_name, args) {
            Ok(result) => {
                // 成功，回收实例
                self.pool.reclaim_instance(&result.instance_id);
                Ok(result)
            }
            Err(e) => {
                // 失败，记录崩溃并回收实例
                self.crash_tracker.record_crash(&plugin_config.plugin_id);
                // 尝试回收实例（如果存在）
                if let Some(instance_id) =
                    self.pool.get_instance(&plugin_config.plugin_id).map(|i| i.instance_id.clone())
                {
                    self.pool.reclaim_instance(&instance_id);
                }
                Err(e)
            }
        }
    }

    /// 清理超时实例。
    pub fn cleanup_idle(&mut self) -> usize {
        self.pool.cleanup_idle().len()
    }

    /// 清理过期模块。
    pub fn cleanup_expired_modules(&mut self) -> usize {
        self.cache.cleanup_expired().len()
    }

    /// 清除指定插件的所有实例和缓存。
    pub fn clear_plugin(&mut self, plugin_id: &str) -> usize {
        let instances_cleared = self.pool.clear_plugin(plugin_id);
        let _cache_cleared = self.cache.clear_plugin(plugin_id);
        instances_cleared
    }

    /// 获取插件统计信息。
    pub fn plugin_stats(&self, plugin_id: &str) -> WasmPluginStats {
        WasmPluginStats {
            plugin_id: plugin_id.to_string(),
            instance_count: self.pool.get_instance(plugin_id).map(|_| 1).unwrap_or(0),
            cache_hit: self.cache.get(plugin_id).is_some() as u32,
            crash_count: self.crash_tracker.crash_count(plugin_id),
            is_crash_exceeded: self.crash_tracker.is_exceeded(plugin_id),
            host_fn_calls: self
                .host_fn_calls
                .iter()
                .filter(|c| c.fn_name.contains(plugin_id))
                .count() as u32,
        }
    }
}

/// 插件统计信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmPluginStats {
    /// 插件 ID。
    pub plugin_id: String,
    /// 实例数量。
    pub instance_count: usize,
    /// 缓存命中（0 或 1）。
    pub cache_hit: u32,
    /// 崩溃次数。
    pub crash_count: u32,
    /// 是否崩溃超限。
    pub is_crash_exceeded: bool,
    /// host_fn 调用次数。
    pub host_fn_calls: u32,
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryConfig, WasmAbiFingerprint};

    fn make_plugin_config() -> WasmPluginConfig {
        WasmPluginConfig {
            plugin_id: "test.plugin".into(),
            module_path: "/path/to/module.wasm".into(),
            abi: WasmAbiFingerprint {
                interface_hash: "hash123".into(),
                host_fns: vec!["invoke_command".into(), "get_setting".into()],
                generated_at: Utc::now(),
            },
            memory: MemoryConfig::default(),
            host_fn_whitelist: Default::default(),
            instance_pool: Default::default(),
            module_cache: Default::default(),
            crash_limit: Default::default(),
        }
    }

    // ── 引擎创建测试 ──

    #[test]
    fn test_engine_create() {
        let engine = WasmEngine::default_engine();
        assert_eq!(engine.instance_count(), 0);
        assert_eq!(engine.cache_count(), 0);
    }

    #[test]
    fn test_engine_custom_config() {
        let config = WasmEngineConfig {
            instance_pool: InstancePoolConfig {
                max_instances: 5,
                idle_timeout_ms: 60000,
                enable_pool: true,
            },
            ..Default::default()
        };
        let engine = WasmEngine::new(config);
        assert_eq!(engine.config().instance_pool.max_instances, 5);
    }

    // ── host_fn 注册测试 ──

    #[test]
    fn test_register_host_fn() {
        let mut engine = WasmEngine::default_engine();
        let handler: HostFnHandler = Box::new(|_, _| Ok("{}".to_string()));
        engine.register_host_fn("test_fn", handler);
        assert!(engine.has_host_fn("test_fn"));
    }

    #[test]
    fn test_unregister_host_fn() {
        let mut engine = WasmEngine::default_engine();
        let handler: HostFnHandler = Box::new(|_, _| Ok("{}".to_string()));
        engine.register_host_fn("test_fn", handler);
        assert!(engine.unregister_host_fn("test_fn").is_some());
        assert!(!engine.has_host_fn("test_fn"));
    }

    // ── 模块加载测试 ──

    #[test]
    fn test_load_module() {
        let mut engine = WasmEngine::default_engine();
        let result = engine.load_module("test.plugin", "hash123", 1024);
        assert!(result.is_ok());
        assert_eq!(engine.cache_count(), 1);
    }

    #[test]
    fn test_load_module_evict() {
        let mut engine = WasmEngine::default_engine();
        // 默认 max_cached_modules = 32，加载 33 个模块
        for i in 0..33 {
            engine.load_module(&format!("plugin.{}", i), &format!("hash{}", i), 1024).unwrap();
        }
        // 缓存应被驱逐到 32 个
        assert_eq!(engine.cache_count(), 32);
    }

    // ── 执行测试 ──

    #[test]
    fn test_execute_success() {
        let mut engine = WasmEngine::default_engine();
        let config = make_plugin_config();
        let result = engine.execute(&config, "main", "{}");
        assert!(result.is_ok());
        let exec = result.unwrap();
        assert!(exec.success);
        assert!(exec.return_value.is_some());
        assert_eq!(exec.instance_id, "instance-1");
        assert!(exec.duration_ms >= 1);
    }

    #[test]
    fn test_execute_with_recovery() {
        let mut engine = WasmEngine::default_engine();
        let config = make_plugin_config();
        let result = engine.execute_with_recovery(&config, "main", "{}");
        assert!(result.is_ok());
        let exec = result.unwrap();
        assert!(exec.success);
        // 实例应被回收
        assert_eq!(engine.instance_count(), 1); // 实例仍在池中，但状态为 Idle
    }

    #[test]
    fn test_execute_with_host_fn() {
        let mut engine = WasmEngine::default_engine();
        let handler: HostFnHandler = Box::new(|_, _| Ok("{\"result\":\"ok\"}".to_string()));
        engine.register_host_fn("invoke_command", handler);

        let config = make_plugin_config();
        let result = engine.execute(&config, "main", "{}");
        assert!(result.is_ok());
        let exec = result.unwrap();
        assert!(exec.host_fn_calls > 0);
        assert!(!engine.host_fn_calls().is_empty());
    }

    #[test]
    fn test_execute_invalid_config() {
        let mut engine = WasmEngine::default_engine();
        let mut config = make_plugin_config();
        config.plugin_id = String::new();
        let result = engine.execute(&config, "main", "{}");
        assert!(result.is_err());
    }

    #[test]
    fn test_execute_crash_limit_exceeded() {
        let mut engine = WasmEngine::default_engine();
        let config = make_plugin_config();

        // 手动记录崩溃超限
        for _ in 0..4 {
            engine.crash_tracker.record_crash(&config.plugin_id);
        }

        let result = engine.execute(&config, "main", "{}");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WasmError::CrashLimitExceeded { .. }));
    }

    // ── 清理测试 ──

    #[test]
    fn test_cleanup_idle() {
        let mut engine = WasmEngine::default_engine();
        let config = make_plugin_config();
        engine.execute(&config, "main", "{}").unwrap();
        // 手动回收实例
        engine.pool.reclaim_instance("instance-1");
        // 清理空闲实例
        let cleaned = engine.cleanup_idle();
        assert_eq!(cleaned, 0); // 实例不会立即被清理（超时未达）
    }

    #[test]
    fn test_clear_plugin() {
        let mut engine = WasmEngine::default_engine();
        let config = make_plugin_config();
        engine.execute(&config, "main", "{}").unwrap();
        engine.load_module(&config.plugin_id, "hash123", 1024).unwrap();

        let cleared = engine.clear_plugin(&config.plugin_id);
        assert!(cleared > 0);
        assert_eq!(engine.cache_count(), 0);
    }

    // ── 统计测试 ──

    #[test]
    fn test_plugin_stats() {
        let mut engine = WasmEngine::default_engine();
        let config = make_plugin_config();
        engine.execute(&config, "main", "{}").unwrap();
        engine.load_module(&config.plugin_id, "hash123", 1024).unwrap();

        let stats = engine.plugin_stats(&config.plugin_id);
        assert_eq!(stats.plugin_id, config.plugin_id);
        assert!(stats.cache_hit == 1);
        assert_eq!(stats.crash_count, 0);
        assert!(!stats.is_crash_exceeded);
    }
}
