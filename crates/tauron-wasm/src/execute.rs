// WASM 执行器：真字节加载、按 generation 复用实例、真引擎调用。
//
// **V7-P0-01 / V7-P0-02 的落地点**，一句话版本：
// - `execute()` 不再「没加载也能成功」。它必须查到 active generation 的常驻编译产物
//   （`ModuleCache::active_module`），再用 provider 真调用导出函数；任何失败都以
//   类型化错误返回，**不再有** `mock_<fn>` 或 `{"function":…,"result":"ok"}` 这种
//   宿主凭空构造的成功。
// - `ExecutionResult` / `HostFnCallRecord` 里的返回值、fuel、内存页、host_fn 次数
//   全部来自引擎观测；每条成功结果都能反查 `module_hash` / `generation` /
//   `instance_id` / `provider_id`。
// - 实例按 `(plugin_id, generation)` 复用；换代时旧实例连同引擎句柄一起丢弃，
//   引擎 trap 或 fuel 烧穿的实例绝不复用。
//
// **仍未实现（不要把本模块当成 ADR-10 已达标）**：
// - 没有独立 supervisor 子进程承载——本层跑在宿主进程内，`wasmi` 解释器的越界只会
//   以引擎错误形式返回，做不到「宿主不受插件崩溃影响」；
// - host_fn ABI 只有整数签名（`(i64,i64)->i64`，见 `provider` 模块头）；
// - 没有按需的 host_fn deadline：`call_timeout_ms` 仍只在配置校验层生效。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauron_host::PackLeaseRegistry;

use crate::provider::{
    EngineInstance, ExecutionBudget, HostFnTable, ModuleFacts, WasmRuntimeProvider, WasmiProvider,
};
use crate::{
    validate_host_fn, validate_plugin_config, CachedModule, CrashLimitConfig, InstanceBinding,
    InstancePool, InstancePoolConfig, ModuleCache, ModuleCacheConfig, WasmCrashTracker, WasmError,
    WasmPluginConfig, WasmResult,
};

// ──────────────────────────────────────────────────────────────────────────
// 执行结果
// ──────────────────────────────────────────────────────────────────────────

/// 执行结果。
///
/// 每一条 `success: true` 都由 provider 的真调用产出，且带齐「反查三件套」：
/// `module_hash`（字节内容）+ `generation`（哪一代）+ `instance_id`（哪个实例跑的）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionResult {
    /// 是否成功（只有引擎真的返回了结果才会是 `true`）。
    pub success: bool,
    /// 返回值（JSON 编码的引擎结果数组）。
    pub return_value: Option<String>,
    /// 错误信息。
    pub error: Option<String>,
    /// 执行时间（毫秒，实测，不兜底为 1）。
    pub duration_ms: u64,
    /// 调用结束时该实例 memory 的真实页数（引擎读数）。
    pub memory_pages_used: u32,
    /// guest 在本次调用里真打到的 host_fn 次数（引擎观测）。
    pub host_fn_calls: usize,
    /// 实例 ID。
    pub instance_id: String,
    /// 执行的模块内容 hash（provider 从字节算出）。
    pub module_hash: String,
    /// 执行的模块 generation（A89 cache key）。
    pub generation: u64,
    /// 引擎实际消耗的 fuel。
    pub fuel_consumed: u64,
    /// 产出本次结果的 provider 标识（如 `wasmi`）。
    pub provider_id: String,
}

/// host_fn 调用记录。
///
/// 带 `plugin_id` + `module_generation`：V7-P0-02 里「按函数名子串猜归属」的统计
/// 换成结构化字段，跨插件/跨代不再串账。
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
    /// 发起本次调用的插件。
    pub plugin_id: String,
    /// 该插件当时的模块 generation。
    pub module_generation: u64,
}

/// 一次成功加载的产出（供安装/更新路径对账，不是「声称加载过」的便条）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedModule {
    /// 插件 ID。
    pub plugin_id: String,
    /// 模块内容 hash（provider 算出）。
    pub module_hash: String,
    /// 注册后的 A89 generation。
    pub generation: u64,
    /// provider 实测的接口指纹（等于 manifest 的 `abi.wasm` 才会走到这里）。
    pub interface_hash: String,
    /// 真实导出函数名。
    pub exports: Vec<String>,
    /// 模块声明的 host_fn 导入。
    pub imported_host_fns: Vec<String>,
    /// 字节长度。
    pub byte_len: u64,
    /// 产出事实的 provider。
    pub provider_id: String,
}

// ──────────────────────────────────────────────────────────────────────────
// WASM 引擎
// ──────────────────────────────────────────────────────────────────────────

/// host_fn 处理函数类型。
///
/// 用 `Arc` 而不是 `Box`：注册表要在宿主与每个引擎实例之间**共享同一份**，
/// 撤销某个 host_fn 后旧实例下次调用就找不到 handler（provider 据此失败）。
pub type HostFnHandler = Arc<dyn Fn(String, String) -> Result<String, String> + Send + Sync>;

struct ActivePackLeaseGuard {
    authority: Arc<Mutex<PackLeaseRegistry>>,
    token: Option<String>,
}

impl Drop for ActivePackLeaseGuard {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            self.authority.lock().release(&token);
        }
    }
}

fn wasm_now_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

/// 判定一次失败是否算「guest 崩溃」（消耗崩溃预算）。
///
/// 未加载、导出缺失、参数不合法、host_fn 未注册这类是**宿主侧/调用方**的问题，
/// 记到插件头上会让配置错误看起来像插件不可靠；只有引擎真的 trap、烧穿 fuel、
/// 内存越界或 `start` 段实例化失败才计一次崩溃。
fn is_execution_crash(error: &WasmError) -> bool {
    matches!(
        error,
        WasmError::EngineTrap { .. }
            | WasmError::FuelExhausted { .. }
            | WasmError::MemoryLimitExceeded { .. }
            | WasmError::InstanceCreationFailed { .. }
    )
}

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
    /// HostInstance identity recorded on shared A89 execution leases.
    host_instance_id: String,
    crash_tracker: WasmCrashTracker,
    host_fns: HostFnTable,
    host_fn_calls: Vec<HostFnCallRecord>,
    provider: Arc<dyn WasmRuntimeProvider>,
    /// `instance_id` → 引擎实例句柄。池里有条目但没有句柄 = 悬空，必须重建。
    handles: HashMap<String, Box<dyn EngineInstance>>,
    /// 每个插件**真成功执行**的次数（V7 要求的真实执行计数）。
    executions: HashMap<String, u64>,
}

impl WasmEngine {
    /// 创建新的 WASM 引擎（内置 `wasmi` provider）。
    pub fn new(config: WasmEngineConfig) -> Self {
        Self::with_pack_lease_registry(
            config,
            "standalone-wasm-engine",
            Arc::new(Mutex::new(PackLeaseRegistry::default())),
        )
    }

    /// Production LocalHost/cache owners inject the shared A89 authority and owner identity.
    pub fn with_pack_lease_registry(
        config: WasmEngineConfig,
        host_instance_id: impl Into<String>,
        pack_leases: Arc<Mutex<PackLeaseRegistry>>,
    ) -> Self {
        Self {
            pool: InstancePool::new(config.instance_pool.clone()),
            cache: ModuleCache::with_pack_lease_registry(config.module_cache.clone(), pack_leases),
            host_instance_id: host_instance_id.into(),
            crash_tracker: WasmCrashTracker::new(config.crash_limit.clone()),
            config,
            host_fns: Arc::new(Mutex::new(BTreeMap::new())),
            host_fn_calls: Vec::new(),
            provider: Arc::new(WasmiProvider::new()),
            handles: HashMap::new(),
            executions: HashMap::new(),
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

    /// 内置 provider 的标识（`wasmi`）。装配侧据此如实上报运行时来源。
    pub fn provider_id(&self) -> &'static str {
        self.provider.provider_id()
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

    /// 某插件成功执行过多少次（只由 `execute()` 的真成功累加）。
    pub fn execution_count(&self, plugin_id: &str) -> u64 {
        self.executions.get(plugin_id).copied().unwrap_or(0)
    }

    /// 注册 host_fn 处理器。
    pub fn register_host_fn(&mut self, fn_name: &str, handler: HostFnHandler) {
        self.host_fns.lock().insert(fn_name.to_string(), handler);
    }

    /// 移除 host_fn 处理器。
    pub fn unregister_host_fn(&mut self, fn_name: &str) -> Option<HostFnHandler> {
        self.host_fns.lock().remove(fn_name)
    }

    /// 检查 host_fn 是否已注册。
    pub fn has_host_fn(&self, fn_name: &str) -> bool {
        self.host_fns.lock().contains_key(fn_name)
    }

    /// 当前已注册的 host_fn 名（字典序）。
    pub fn registered_host_fns(&self) -> Vec<String> {
        self.host_fns.lock().keys().cloned().collect()
    }

    /// 加载模块：真字节 → provider 解析 → ABI 对账 → 白名单对账 → A89 generation 注册。
    ///
    /// 这是 `execute()` 唯一的合法前置。失败时**不会**留下任何 active generation，
    /// 因此后续调用必然落到 `ModuleNotLoaded`，而不是一个没有载体的「成功」。
    pub fn load_module(
        &mut self,
        plugin_config: &WasmPluginConfig,
        bytes: &[u8],
    ) -> WasmResult<LoadedModule> {
        validate_plugin_config(plugin_config)?;

        let prepared = self.provider.prepare(bytes)?;
        let facts = prepared.facts.clone();

        // ABI 门禁：manifest 声明的接口指纹必须等于引擎从字节读出的那个。
        validate_abi_hash(plugin_config, &facts)?;

        // 白名单门禁：模块声明的每个 host_fn 导入都必须在白名单内，越界即拒载。
        for fn_name in &facts.imported_host_fns {
            validate_host_fn(fn_name, &plugin_config.host_fn_whitelist)?;
        }

        let plugin_id = plugin_config.plugin_id.clone();
        let key = self.cache.insert(CachedModule::resident(&plugin_id, Arc::new(prepared)))?;
        let generation = key.generation.0;

        // 换代：旧 generation 的实例与引擎句柄一并丢弃（它们绑的是上一代字节）。
        for stale in self.pool.discard_stale_instances(&plugin_id, generation) {
            self.handles.remove(&stale);
        }

        Ok(LoadedModule {
            plugin_id,
            module_hash: facts.module_hash,
            generation,
            interface_hash: facts.interface_hash,
            exports: facts.exports,
            imported_host_fns: facts.imported_host_fns,
            byte_len: facts.byte_len,
            provider_id: facts.provider_id,
        })
    }

    /// 从 `config.module_path` 读字节再加载——生产侧的安装入口。
    ///
    /// 路径不存在/读不出字节都是 `ModuleLoadFailed`：**不**退化成「跳过校验先记着」。
    pub fn load_module_from_path(
        &mut self,
        plugin_config: &WasmPluginConfig,
    ) -> WasmResult<LoadedModule> {
        let bytes =
            std::fs::read(&plugin_config.module_path).map_err(|e| WasmError::ModuleLoadFailed {
                plugin_id: plugin_config.plugin_id.clone(),
                reason: format!("读取 `{}` 失败：{e}", plugin_config.module_path),
            })?;
        self.load_module(plugin_config, &bytes)
    }

    /// 执行插件代码：**必须**已加载 active generation，且真的经 provider 调用导出函数。
    pub fn execute(
        &mut self,
        plugin_config: &WasmPluginConfig,
        function_name: &str,
        args: &str,
    ) -> WasmResult<ExecutionResult> {
        let start = Instant::now();

        validate_plugin_config(plugin_config)?;

        // 崩溃预算：超限插件不再执行（与 supervisor「超限不重启载入」一致）。
        if self.crash_tracker.is_exceeded(&plugin_config.plugin_id) {
            return Err(WasmError::CrashLimitExceeded {
                plugin_id: plugin_config.plugin_id.clone(),
            });
        }

        // V4 A89: pin the exact active module generation for the whole execution. Normal returns
        // release immediately via RAII; owner death leaves a bounded TTL lease for crash safety.
        let lease_ttl_ms = self.config.module_cache.cache_ttl_secs.saturating_mul(1_000).max(1);
        let _pack_lease = self
            .cache
            .acquire_active_lease(
                &self.host_instance_id,
                &plugin_config.plugin_id,
                wasm_now_ms(),
                lease_ttl_ms,
            )?
            .map(|lease| ActivePackLeaseGuard {
                authority: self.cache.pack_lease_registry(),
                token: Some(lease.token),
            });

        // 门禁一：没有 active generation 的常驻字节就没有可执行对象（V7-P0-01）。
        let (key, prepared) = match self.cache.active_module(&plugin_config.plugin_id) {
            Some(entry) => entry,
            None => {
                let reason = if self.cache.active_key(&plugin_config.plugin_id).is_some() {
                    "active generation 的模块字节已不在本 cache owner 中（被 GC 或从未插入）"
                } else {
                    "该插件从未成功加载模块：先调用 load_module() 并让它通过 ABI/白名单校验"
                };
                return Err(WasmError::ModuleNotLoaded {
                    plugin_id: plugin_config.plugin_id.clone(),
                    reason: reason.to_string(),
                });
            }
        };
        let generation = key.generation.0;
        let facts = &prepared.facts;

        // 门禁二：导出函数必须真的存在于该 generation 的导出表。
        if !facts.exports_function(function_name) {
            return Err(WasmError::ExportNotFound {
                plugin_id: plugin_config.plugin_id.clone(),
                function_name: function_name.to_string(),
            });
        }
        // 门禁三（纵深防御）：该代字节声明的导入仍须在白名单内。
        for fn_name in &facts.imported_host_fns {
            validate_host_fn(fn_name, &plugin_config.host_fn_whitelist)?;
        }

        let budget = ExecutionBudget {
            fuel: plugin_config.execution.fuel_per_call,
            max_memory_pages: plugin_config.memory.max_pages,
        };

        // 实例：只复用**绑定当前 generation** 的空闲实例，且引擎句柄必须仍在。
        let reusable = self
            .pool
            .find_idle_instance(&plugin_config.plugin_id, generation)
            .map(|instance| instance.instance_id.clone())
            .filter(|id| self.handles.contains_key(id));

        let instance_id = match reusable {
            Some(id) => {
                self.pool.mark_in_use(&id);
                id
            }
            None => {
                let fresh = self
                    .pool
                    .try_create_instance(
                        &plugin_config.plugin_id,
                        InstanceBinding { generation, module_hash: facts.module_hash.clone() },
                    )
                    .inspect_err(|e| {
                        self.charge_crash(&plugin_config.plugin_id, e);
                    })?;
                let handle = self.provider.instantiate(
                    &prepared,
                    &plugin_config.plugin_id,
                    generation,
                    self.host_fns.clone(),
                    budget,
                );
                match handle {
                    Ok(handle) => {
                        self.handles.insert(fresh.instance_id.clone(), handle);
                        fresh.instance_id
                    }
                    Err(e) => {
                        // 实例化失败（含未注册 host_fn、start 段 trap）：池里不留悬空条目。
                        self.pool.discard_instance(&fresh.instance_id);
                        self.charge_crash(&plugin_config.plugin_id, &e);
                        return Err(e);
                    }
                }
            }
        };

        // 真调用。句柄先取出锁表，避免在持有引擎映射的情况下调用宿主 handler。
        let mut handle =
            self.handles.remove(&instance_id).expect("实例句柄在上面的分支里必然已就位");
        let outcome =
            self.provider.invoke(prepared.as_ref(), handle.as_mut(), function_name, args, budget);

        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(e) => {
                // 失败实例绝不复用：句柄直接丢弃，池里条目也移除。
                self.pool.discard_instance(&instance_id);
                self.charge_crash(&plugin_config.plugin_id, &e);
                return Err(e);
            }
        };

        self.handles.insert(instance_id.clone(), handle);
        self.pool.reclaim_instance(&instance_id);
        self.pool.note_memory_pages(&instance_id, outcome.memory_pages_used);

        for call in &outcome.host_calls {
            self.host_fn_calls.push(HostFnCallRecord {
                fn_name: call.fn_name.clone(),
                args: call.args.clone(),
                return_value: call.return_value.clone(),
                duration_ms: call.duration_ms,
                timestamp: Utc::now(),
                plugin_id: plugin_config.plugin_id.clone(),
                module_generation: generation,
            });
        }
        *self.executions.entry(plugin_config.plugin_id.clone()).or_insert(0) += 1;

        Ok(ExecutionResult {
            success: true,
            return_value: Some(outcome.return_value),
            error: None,
            duration_ms: start.elapsed().as_millis() as u64,
            memory_pages_used: outcome.memory_pages_used,
            host_fn_calls: outcome.host_calls.len(),
            instance_id,
            module_hash: facts.module_hash.clone(),
            generation,
            fuel_consumed: outcome.fuel_consumed,
            provider_id: facts.provider_id.clone(),
        })
    }

    /// 执行并做 supervisor 侧的善后。
    ///
    /// 与 [`WasmEngine::execute`] 的分工很窄但真实：崩溃记账、失败实例销毁都在
    /// `execute()` 内一次完成（避免双重计数）；这里额外负责**超限即停用**——一旦崩溃
    /// 预算用尽，立刻丢掉该插件的实例与模块，让它在人工确认前无法再被执行。
    pub fn execute_with_recovery(
        &mut self,
        plugin_config: &WasmPluginConfig,
        function_name: &str,
        args: &str,
    ) -> WasmResult<ExecutionResult> {
        let result = self.execute(plugin_config, function_name, args);
        if result.is_err() && self.crash_tracker.is_exceeded(&plugin_config.plugin_id) {
            self.clear_plugin(&plugin_config.plugin_id);
        }
        result
    }

    /// 清理超时实例。
    pub fn cleanup_idle(&mut self) -> usize {
        let removed = self.pool.cleanup_idle();
        let len = removed.len();
        for id in removed {
            self.handles.remove(&id);
        }
        len
    }

    /// 清理过期模块。
    pub fn cleanup_expired_modules(&mut self) -> usize {
        self.cache.cleanup_expired().len()
    }

    /// 清除指定插件的所有实例和缓存。
    pub fn clear_plugin(&mut self, plugin_id: &str) -> usize {
        let instances_cleared = self.pool.clear_plugin(plugin_id);
        for id in &instances_cleared {
            self.handles.remove(id);
        }
        let _cache_cleared = self.cache.clear_plugin(plugin_id);
        instances_cleared.len()
    }

    /// 崩溃预算记账（只对 guest 侧失败生效）。
    fn charge_crash(&mut self, plugin_id: &str, error: &WasmError) {
        if is_execution_crash(error) {
            self.crash_tracker.record_crash(plugin_id);
        }
    }

    /// 获取插件统计信息（全部来自真实结构：池索引、A89 active key、执行/调用计数）。
    pub fn plugin_stats(&self, plugin_id: &str) -> WasmPluginStats {
        let active = self.cache.active_entry(plugin_id);
        WasmPluginStats {
            plugin_id: plugin_id.to_string(),
            instance_count: self.pool.instances_of(plugin_id).len(),
            resident_generations: self.cache.generation_count(plugin_id),
            active_generation: active.as_ref().map(|(key, _)| key.generation.0),
            active_module_hash: active.as_ref().map(|(_, module)| module.module_hash.clone()),
            cache_hit: active.is_some() as u32,
            crash_count: self.crash_tracker.crash_count(plugin_id),
            is_crash_exceeded: self.crash_tracker.is_exceeded(plugin_id),
            host_fn_calls: self
                .host_fn_calls
                .iter()
                .filter(|record| record.plugin_id == plugin_id)
                .count() as u32,
            executions: self.execution_count(plugin_id),
        }
    }
}

/// ABI 对账：manifest 的 `abi.interface_hash` 必须等于 provider 从字节读出的指纹。
fn validate_abi_hash(config: &WasmPluginConfig, facts: &ModuleFacts) -> WasmResult<()> {
    if config.abi.interface_hash != facts.interface_hash {
        return Err(WasmError::AbiMismatch {
            expected: config.abi.interface_hash.clone(),
            actual: facts.interface_hash.clone(),
        });
    }
    Ok(())
}

/// 插件统计信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmPluginStats {
    /// 插件 ID。
    pub plugin_id: String,
    /// 池中该插件当前实例数。
    pub instance_count: usize,
    /// 该插件常驻的 generation 数。
    pub resident_generations: usize,
    /// 当前 active generation（未加载时 `None`）。
    pub active_generation: Option<u64>,
    /// 当前 active 模块内容 hash。
    pub active_module_hash: Option<String>,
    /// active 模块是否常驻（0 或 1）。
    pub cache_hit: u32,
    /// 窗口内崩溃次数。
    pub crash_count: u32,
    /// 是否崩溃超限。
    pub is_crash_exceeded: bool,
    /// 该插件的 host_fn 调用次数（按记录归属统计，非名字子串）。
    pub host_fn_calls: u32,
    /// 成功执行次数。
    pub executions: u64,
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::WasmRuntimeProvider;
    use crate::{ExecutionConfig, MemoryConfig, WasmAbiFingerprint};

    /// 纯计算导出：`(add i32 i32) -> i32`。
    const ADD_WAT: &str = r#"(module
        (func (export "add") (param i32 i32) (result i32)
            local.get 0
            local.get 1
            i32.add)
    )"#;

    /// 调用 host_fn 两次：证明计数来自 guest，而不是宿主按白名单条数编出来。
    const DOUBLE_LOG_WAT: &str = r#"(module
        (import "env" "log" (func $log (param i64 i64) (result i64)))
        (func (export "run") (result i64)
            i64.const 1
            i64.const 2
            call $log
            i64.const 3
            i64.const 4
            call $log
            i64.add)
    )"#;

    /// 只有 `sub` 导出（换代用）。
    const SUB_WAT: &str = r#"(module
        (func (export "sub") (param i32 i32) (result i32)
            local.get 0
            local.get 1
            i32.sub)
    )"#;

    /// 死循环（fuel 打断用）。
    const SPIN_WAT: &str = r#"(module
        (func (export "spin")
            (loop $again
                br $again))
    )"#;

    /// 导入不在默认白名单里的 host_fn。
    const EVIL_WAT: &str = r#"(module
        (import "env" "read_secret" (func $s (param i64 i64) (result i64)))
        (func (export "run") (result i64)
            i64.const 0
            i64.const 0
            call $s)
    )"#;

    fn wasm_bytes(wat_src: &str) -> Vec<u8> {
        wat::parse_str(wat_src).expect("测试 WAT 必须合法")
    }

    /// 由**真字节**算出 ABI 指纹，构造与该模块配套的插件配置。
    fn config_for(wat_src: &str, plugin_id: &str) -> WasmPluginConfig {
        let facts = facts_of(wat_src);
        WasmPluginConfig {
            plugin_id: plugin_id.to_string(),
            module_path: format!("/{plugin_id}.wasm"),
            abi: WasmAbiFingerprint {
                interface_hash: facts.interface_hash,
                host_fns: facts.imported_host_fns,
                generated_at: Utc::now(),
            },
            memory: MemoryConfig::default(),
            host_fn_whitelist: Default::default(),
            instance_pool: Default::default(),
            module_cache: Default::default(),
            crash_limit: Default::default(),
            execution: ExecutionConfig::default(),
        }
    }

    fn config_with_fuel(wat_src: &str, fuel: u64) -> WasmPluginConfig {
        let mut config = config_for(wat_src, "test.plugin");
        config.execution = ExecutionConfig { fuel_per_call: fuel };
        config
    }

    fn facts_of(wat_src: &str) -> ModuleFacts {
        WasmiProvider::new().prepare(&wasm_bytes(wat_src)).unwrap().facts
    }

    fn numeric_handler(return_value: &'static str) -> HostFnHandler {
        Arc::new(move |_, _| Ok(return_value.to_string()))
    }

    // ── 引擎创建测试 ──

    #[test]
    fn test_engine_create() {
        let engine = WasmEngine::default_engine();
        assert_eq!(engine.instance_count(), 0);
        assert_eq!(engine.cache_count(), 0);
        assert_eq!(engine.provider_id(), "wasmi");
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
        engine.register_host_fn("test_fn", numeric_handler("{}"));
        assert!(engine.has_host_fn("test_fn"));
        assert_eq!(engine.registered_host_fns(), vec!["test_fn".to_string()]);
    }

    #[test]
    fn test_unregister_host_fn() {
        let mut engine = WasmEngine::default_engine();
        engine.register_host_fn("test_fn", numeric_handler("{}"));
        assert!(engine.unregister_host_fn("test_fn").is_some());
        assert!(!engine.has_host_fn("test_fn"));
    }

    // ── 模块加载测试 ──

    #[test]
    fn load_module_registers_real_facts_and_generation() {
        let mut engine = WasmEngine::default_engine();
        let bytes = wasm_bytes(ADD_WAT);
        let loaded = engine.load_module(&config_for(ADD_WAT, "test.plugin"), &bytes).unwrap();

        assert_eq!(loaded.generation, 1);
        assert_eq!(loaded.exports, vec!["add".to_string()]);
        assert_eq!(loaded.byte_len, bytes.len() as u64);
        assert_eq!(loaded.provider_id, "wasmi");
        // module_hash 是字节内容算出来的，不是调用方给的。
        assert_eq!(loaded.module_hash, facts_of(ADD_WAT).module_hash);
        assert_eq!(engine.cache_count(), 1);
    }

    #[test]
    fn load_module_rejects_bytes_that_do_not_match_declared_abi() {
        let mut engine = WasmEngine::default_engine();
        // 用 sub 模块的字节去顶 add 的 manifest：接口指纹不同 → 拒载。
        let error = engine
            .load_module(&config_for(ADD_WAT, "test.plugin"), &wasm_bytes(SUB_WAT))
            .expect_err("ABI 不符必须拒载");
        assert!(matches!(error, WasmError::AbiMismatch { .. }), "实际：{error:?}");
        assert_eq!(engine.cache_count(), 0, "拒载不得留下任何常驻模块");
        assert!(engine.cache().active_key("test.plugin").is_none());
    }

    #[test]
    fn load_module_rejects_import_outside_whitelist() {
        let mut engine = WasmEngine::default_engine();
        engine.register_host_fn("read_secret", numeric_handler("0"));
        let error = engine
            .load_module(&config_for(EVIL_WAT, "test.plugin"), &wasm_bytes(EVIL_WAT))
            .expect_err("白名单外的 host_fn 必须拒载");
        assert!(matches!(error, WasmError::HostFnNotAllowed { .. }), "实际：{error:?}");
        assert_eq!(engine.cache_count(), 0);
    }

    #[test]
    fn load_module_rejects_garbage_bytes() {
        let mut engine = WasmEngine::default_engine();
        let error = engine
            .load_module(&config_for(ADD_WAT, "test.plugin"), b"not wasm at all")
            .expect_err("非法字节必须失败");
        assert!(matches!(error, WasmError::ModuleLoadFailed { .. }), "实际：{error:?}");
    }

    #[test]
    fn load_module_from_path_reads_the_configured_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.wasm");
        std::fs::write(&path, wasm_bytes(ADD_WAT)).unwrap();

        let mut engine = WasmEngine::default_engine();
        let mut config = config_for(ADD_WAT, "test.plugin");
        config.module_path = path.to_string_lossy().to_string();
        let loaded = engine.load_module_from_path(&config).unwrap();
        assert_eq!(loaded.module_hash, facts_of(ADD_WAT).module_hash);

        // 路径不存在 → 类型化失败，不退化成「先记着」。
        let mut missing = config_for(ADD_WAT, "other.plugin");
        missing.module_path = dir.path().join("absent.wasm").to_string_lossy().to_string();
        let error = engine.load_module_from_path(&missing).expect_err("缺文件必须失败");
        assert!(matches!(error, WasmError::ModuleLoadFailed { .. }), "实际：{error:?}");
    }

    #[test]
    fn test_load_module_capacity_never_evicts_active_generation() {
        let mut engine = WasmEngine::default_engine();
        let bytes = wasm_bytes(ADD_WAT);
        for i in 0..32 {
            engine.load_module(&config_for(ADD_WAT, &format!("plugin.{i}")), &bytes).unwrap();
        }
        assert!(matches!(
            engine.load_module(&config_for(ADD_WAT, "plugin.32"), &bytes),
            Err(WasmError::ModuleCacheFull { .. })
        ));

        engine.cache().pack_lease_registry().lock().deactivate("plugin.0");
        engine.load_module(&config_for(ADD_WAT, "plugin.32"), &bytes).unwrap();
        assert_eq!(engine.cache_count(), 32);
        assert_eq!(engine.cache().generation_count("plugin.0"), 0);
        assert!(engine.cache().get("plugin.32").is_some());
    }

    // ── 执行测试 ──

    #[test]
    fn execution_uses_shared_pack_authority_and_releases_lease_on_return() {
        let authority = Arc::new(Mutex::new(PackLeaseRegistry::default()));
        let mut engine = WasmEngine::with_pack_lease_registry(
            WasmEngineConfig::default(),
            "host-a",
            authority.clone(),
        );
        let config = config_for(ADD_WAT, "test.plugin");
        let loaded = engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        assert_eq!(authority.lock().lease_count(), 0);

        engine.execute(&config, "add", "[1,2]").unwrap();
        assert_eq!(authority.lock().lease_count(), 0);
        assert_eq!(
            authority.lock().active_key(&config.plugin_id).unwrap().version,
            loaded.module_hash
        );
    }

    /// **V7-P0-01 的反向断言**（取代旧的 `test_execute_success`）：
    /// 不加载模块就执行，必须是 `ModuleNotLoaded`，绝不能是 `success: true`。
    #[test]
    fn execute_without_loaded_module_fails_closed() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        let error = engine.execute(&config, "add", "[40,2]").expect_err("未加载不得成功");
        assert!(matches!(error, WasmError::ModuleNotLoaded { .. }), "实际：{error:?}");
        assert_eq!(engine.instance_count(), 0, "失败路径不得凭空造实例");
        assert_eq!(engine.execution_count("test.plugin"), 0);
    }

    #[test]
    fn execute_returns_the_engine_computed_result() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        let module_hash = facts_of(ADD_WAT).module_hash.clone();
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();

        let exec = engine.execute(&config, "add", "[40,2]").unwrap();
        assert!(exec.success);
        assert_eq!(exec.return_value.as_deref(), Some("[42]"));
        assert_eq!(exec.module_hash, module_hash);
        assert_eq!(exec.generation, 1);
        assert!(exec.instance_id.starts_with("instance-"));
        assert_eq!(exec.provider_id, "wasmi");
        assert!(exec.fuel_consumed > 0, "fuel 必须真实消耗，不能是固定值");
        assert_eq!(exec.host_fn_calls, 0);
        assert_eq!(engine.execution_count("test.plugin"), 1);
    }

    #[test]
    fn execute_reuses_one_instance_across_100_calls() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();

        let mut ids = Vec::new();
        for i in 0..100u32 {
            let exec = engine.execute(&config, "add", &format!("[{i},1]")).unwrap();
            assert_eq!(exec.return_value.as_deref(), Some(format!("[{}]", i + 1).as_str()));
            ids.push(exec.instance_id);
        }
        // V7-P0-02 的验收口径：100 次调用只用 1 个实例，且都是同一个。
        assert_eq!(engine.instance_count(), 1);
        assert!(ids.iter().all(|id| id == &ids[0]));
        assert_eq!(engine.execution_count("test.plugin"), 100);
    }

    #[test]
    fn execute_rejects_unknown_export() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        let error = engine.execute(&config, "nope", "[]").expect_err("不存在的导出必须失败");
        assert!(matches!(error, WasmError::ExportNotFound { .. }), "实际：{error:?}");
        assert_eq!(engine.instance_count(), 0, "导出门禁失败不应留下实例");
    }

    #[test]
    fn host_fn_counts_come_from_the_guest_not_the_whitelist() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(DOUBLE_LOG_WAT, "test.plugin");
        engine.register_host_fn("log", numeric_handler("10"));
        engine.load_module(&config, &wasm_bytes(DOUBLE_LOG_WAT)).unwrap();

        // 白名单里有 4 个函数，但 guest 只真打了 `log` 两次。
        assert_eq!(config.host_fn_whitelist.allowed_fns.len(), 4);
        let exec = engine.execute(&config, "run", "[]").unwrap();
        assert_eq!(exec.host_fn_calls, 2);
        assert_eq!(exec.return_value.as_deref(), Some("[20]"));
        assert_eq!(engine.host_fn_calls().len(), 2);
        let record = &engine.host_fn_calls()[0];
        assert_eq!(record.fn_name, "log");
        assert_eq!(record.plugin_id, "test.plugin");
        assert_eq!(record.module_generation, 1);
        assert_eq!(engine.plugin_stats("test.plugin").host_fn_calls, 2);
    }

    #[test]
    fn unregistered_host_fn_import_fails_without_faking_success() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(DOUBLE_LOG_WAT, "test.plugin");
        engine.load_module(&config, &wasm_bytes(DOUBLE_LOG_WAT)).unwrap();

        let error = engine.execute(&config, "run", "[]").expect_err("未注册 host_fn 必须失败");
        assert!(matches!(error, WasmError::HostFnNotRegistered { .. }), "实际：{error:?}");
        assert_eq!(engine.instance_count(), 0, "失败的实例不得留在池里");
        assert_eq!(engine.execution_count("test.plugin"), 0);
    }

    /// 撤销已注册的 host_fn：已实例化的 guest 再打到它时，整次执行判失败。
    ///
    /// 归到 `HostFnFailed` 而不是 `HostFnNotRegistered`：绑定在实例化时已成功，
    /// 是**运行中**被撤销，provider 的闭包在调用点查不到 handler 并留痕。
    #[test]
    fn revoked_host_fn_fails_the_next_execution() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(DOUBLE_LOG_WAT, "test.plugin");
        engine.register_host_fn("log", numeric_handler("10"));
        engine.load_module(&config, &wasm_bytes(DOUBLE_LOG_WAT)).unwrap();
        engine.execute(&config, "run", "[]").unwrap();

        engine.unregister_host_fn("log");
        let error = engine.execute(&config, "run", "[]").expect_err("撤销后不得继续成功");
        assert!(matches!(error, WasmError::HostFnFailed { .. }), "实际：{error:?}");
        assert_eq!(engine.execution_count("test.plugin"), 1, "失败那次不得计数");
    }

    #[test]
    fn host_handler_error_is_reported_as_failure() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(DOUBLE_LOG_WAT, "test.plugin");
        engine.register_host_fn("log", Arc::new(|_, _| Err("宿主拒绝".to_string())));
        engine.load_module(&config, &wasm_bytes(DOUBLE_LOG_WAT)).unwrap();

        let error = engine.execute(&config, "run", "[]").expect_err("handler 报错必须整体失败");
        assert!(matches!(error, WasmError::HostFnFailed { .. }), "实际：{error:?}");
        assert_eq!(engine.execution_count("test.plugin"), 0);
    }

    #[test]
    fn fuel_exhaustion_interrupts_and_discards_the_instance() {
        let mut engine = WasmEngine::default_engine();
        let config = config_with_fuel(SPIN_WAT, 1_000);
        engine.load_module(&config, &wasm_bytes(SPIN_WAT)).unwrap();

        let error = engine.execute(&config, "spin", "[]").expect_err("死循环必须被 fuel 打断");
        assert!(matches!(error, WasmError::FuelExhausted { .. }), "实际：{error:?}");
        assert_eq!(engine.instance_count(), 0, "烧穿 fuel 的实例绝不能复用");
        assert_eq!(engine.crash_tracker().crash_count("test.plugin"), 1);
    }

    #[test]
    fn engine_failure_charges_crash_budget_and_stops_execution() {
        let mut engine = WasmEngine::default_engine();
        let config = config_with_fuel(SPIN_WAT, 1_000);
        engine.load_module(&config, &wasm_bytes(SPIN_WAT)).unwrap();

        // 缺省预算 3 次/5min：第 4 次失败后超限。
        for _ in 0..4 {
            assert!(matches!(
                engine.execute(&config, "spin", "[]"),
                Err(WasmError::FuelExhausted { .. })
            ));
        }
        assert!(engine.crash_tracker().is_exceeded("test.plugin"));
        assert!(matches!(
            engine.execute(&config, "spin", "[]"),
            Err(WasmError::CrashLimitExceeded { .. })
        ));
    }

    #[test]
    fn execute_with_recovery_disables_plugin_once_budget_is_spent() {
        let mut engine = WasmEngine::default_engine();
        let config = config_with_fuel(SPIN_WAT, 1_000);
        engine.load_module(&config, &wasm_bytes(SPIN_WAT)).unwrap();

        for _ in 0..3 {
            assert!(engine.execute_with_recovery(&config, "spin", "[]").is_err());
        }
        assert_eq!(engine.instance_count(), 0);
        assert_eq!(engine.cache_count(), 1, "未超限前不得提前清缓存");

        assert!(engine.execute_with_recovery(&config, "spin", "[]").is_err());
        // 超限 → 实例与模块一并停用（人工确认前无法再执行）。
        assert_eq!(engine.cache_count(), 0);
        assert_eq!(engine.cache().active_key("test.plugin"), None);
        assert!(matches!(
            engine.execute(&config, "spin", "[]"),
            Err(WasmError::CrashLimitExceeded { .. })
        ));
    }

    #[test]
    fn config_errors_do_not_charge_the_crash_budget() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");

        // 未加载 + 未知导出 + 非法参数：全是宿主/调用方问题。
        assert!(engine.execute(&config, "add", "[1,2]").is_err());
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        assert!(engine.execute(&config, "nope", "[]").is_err());
        assert!(engine.execute(&config, "add", "[\"x\",2]").is_err());
        assert_eq!(engine.crash_tracker().crash_count("test.plugin"), 0);
    }

    #[test]
    fn reload_replaces_generation_and_discards_stale_instance() {
        let mut engine = WasmEngine::default_engine();
        let add_config = config_for(ADD_WAT, "test.plugin");
        engine.load_module(&add_config, &wasm_bytes(ADD_WAT)).unwrap();
        engine.execute(&add_config, "add", "[1,1]").unwrap();
        assert_eq!(engine.instance_count(), 1);

        // 同一插件换内容：新 generation，旧实例（绑旧字节）必须消失。
        let sub_facts = facts_of(SUB_WAT);
        let sub_config = WasmPluginConfig {
            abi: WasmAbiFingerprint {
                interface_hash: sub_facts.interface_hash.clone(),
                host_fns: Vec::new(),
                generated_at: Utc::now(),
            },
            ..add_config.clone()
        };
        let loaded = engine.load_module(&sub_config, &wasm_bytes(SUB_WAT)).unwrap();
        assert_eq!(loaded.generation, 2);
        assert_eq!(engine.instance_count(), 0, "换代后旧实例不得留在池里");
        assert_eq!(engine.cache().generation_count("test.plugin"), 2);

        // 旧导出名在新代不存在：门禁拦住，而不是拿旧实例跑。
        assert!(matches!(
            engine.execute(&sub_config, "add", "[1,1]"),
            Err(WasmError::ExportNotFound { .. })
        ));
        let exec = engine.execute(&sub_config, "sub", "[5,2]").unwrap();
        assert_eq!(exec.generation, 2);
        assert_eq!(exec.return_value.as_deref(), Some("[3]"));
    }

    #[test]
    fn test_execute_invalid_config() {
        let mut engine = WasmEngine::default_engine();
        let mut config = config_for(ADD_WAT, "test.plugin");
        config.plugin_id = String::new();
        assert!(config.execution.fuel_per_call > 0);
        let error = engine.execute(&config, "add", "[]").expect_err("空 plugin_id 必须拒绝");
        assert!(matches!(error, WasmError::InvalidConfig { .. }), "实际：{error:?}");
    }

    #[test]
    fn zero_fuel_config_is_rejected_before_execution() {
        let mut engine = WasmEngine::default_engine();
        let mut config = config_for(ADD_WAT, "test.plugin");
        config.execution = ExecutionConfig { fuel_per_call: 0 };
        assert!(matches!(
            engine.execute(&config, "add", "[1,1]"),
            Err(WasmError::InvalidConfig { .. })
        ));
        assert!(matches!(
            engine.load_module(&config, &wasm_bytes(ADD_WAT)),
            Err(WasmError::InvalidConfig { .. })
        ));
    }

    #[test]
    fn test_execute_crash_limit_exceeded() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        for _ in 0..4 {
            engine.crash_tracker.record_crash(&config.plugin_id);
        }
        let result = engine.execute(&config, "add", "[1,1]");
        assert!(matches!(result, Err(WasmError::CrashLimitExceeded { .. })));
    }

    // ── 清理测试 ──

    #[test]
    fn test_cleanup_idle() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        engine.execute(&config, "add", "[1,1]").unwrap();
        // 空闲超时未达到：清理不动它。
        assert_eq!(engine.cleanup_idle(), 0);
        assert_eq!(engine.instance_count(), 1);
    }

    #[test]
    fn test_clear_plugin() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        engine.execute(&config, "add", "[1,1]").unwrap();

        let cleared = engine.clear_plugin(&config.plugin_id);
        assert_eq!(cleared, 1);
        assert_eq!(engine.cache_count(), 0);
        assert_eq!(engine.instance_count(), 0);
        // 实例与模块都没了：再执行必须是 ModuleNotLoaded，而不是复用旧句柄。
        assert!(matches!(
            engine.execute(&config, "add", "[1,1]"),
            Err(WasmError::ModuleNotLoaded { .. })
        ));
    }

    // ── 统计测试 ──

    #[test]
    fn test_plugin_stats_reports_real_counts() {
        let mut engine = WasmEngine::default_engine();
        let config = config_for(ADD_WAT, "test.plugin");
        let module_hash = facts_of(ADD_WAT).module_hash.clone();
        engine.load_module(&config, &wasm_bytes(ADD_WAT)).unwrap();
        engine.execute(&config, "add", "[1,1]").unwrap();
        engine.execute(&config, "add", "[2,2]").unwrap();

        let stats = engine.plugin_stats("test.plugin");
        assert_eq!(stats.plugin_id, config.plugin_id);
        assert_eq!(stats.instance_count, 1);
        assert_eq!(stats.resident_generations, 1);
        assert_eq!(stats.active_generation, Some(1));
        assert_eq!(stats.active_module_hash.as_deref(), Some(module_hash.as_str()));
        assert_eq!(stats.cache_hit, 1);
        assert_eq!(stats.crash_count, 0);
        assert!(!stats.is_crash_exceeded);
        assert_eq!(stats.host_fn_calls, 0);
        assert_eq!(stats.executions, 2);

        // 别的插件账目不能串：未加载的一律是 0/None。
        let other = engine.plugin_stats("other.plugin");
        assert_eq!(other.instance_count, 0);
        assert_eq!(other.executions, 0);
        assert_eq!(other.active_generation, None);
        assert_eq!(other.cache_hit, 0);
    }
}
