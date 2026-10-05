// §4.8 WASM 运行时 provider 接缝：**真**引擎执行，或类型化失败，没有第三条路。
//
// **为什么存在**：V7-P0-01 的断链是「`execute()` 不要求模块已加载、host_fn 未注册也
// 造 `mock_<fn>` 成功结果」——调用方看到 `success: true`，而没有任何 WASM 字节、ABI、
// 导出函数被执行。本模块把执行交还给真引擎：字节校验、解析、实例化、调用、fuel 计量、
// 内存页读数全部由 provider 产出；provider 缺席时唯一的出路是类型化错误。
//
// **本仓 provider 的真实边界（一个字不美化）**：
// - 引擎是 `wasmi`（纯 Rust 解释器）。**不是** wasmtime / wasmer / Extism：没有 JIT、
//   没有 ADR-10 要求的独立 supervisor 子进程承载，生产装配方仍需自己把本引擎放进
//   专用子进程；
// - host_fn ABI 只支持**整数签名** `(i64, i64) -> i64`，导入命名空间固定为 `env`。
//   经 linear memory 传字符串的完整 ABI（分配器、长度前缀、UTF-8 校验）**未实现**；
//   需要字符串参数的接入方必须自带 provider，而不是指望本层把 JSON 塞进 i64；
// - 导出函数签名同样只接受整数参数/整数返回值（`i32`/`i64`），其余类型一律
//   [`WasmError::UnsupportedSignature`]，不做有损转换；
// - 配额用 wasmi 的 fuel（`Config::consume_fuel(true)`）。fuel 耗尽 = 引擎真的中断，
//   该实例随后被丢弃，不再复用（见 `execute.rs` 的 `discard` 路径）。

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::{error::WasmResult, HostFnHandler, WasmError};

/// host_fn 表：引擎实例与宿主共享的**同一份**注册表。
///
/// 用 `Arc<Mutex<_>>` 而不是拷贝进实例：guest 每次调用都必须落到宿主当前注册的那组
/// handler 上，`unregister_host_fn` 之后旧实例不能再调到已撤销的能力。
pub type HostFnTable = Arc<Mutex<BTreeMap<String, HostFnHandler>>>;

/// provider 从**真字节**里读出来的模块事实。
///
/// V7-P0-01 的验收口径是「任何 `success: true` 都能关联到可查询的
/// `module_hash/generation/instance_id` 和真实执行计数」，这个结构就是其中
/// `module_hash` / 导出函数 / 导入 host_fn 三项的事实源——它们不来自调用方自报。
#[derive(Debug, Clone)]
pub struct ModuleFacts {
    /// 产出这些事实的 provider 标识（如 `wasmi`）。
    pub provider_id: String,
    /// 字节的 SHA-256（hex 小写）。
    pub module_hash: String,
    /// 本仓对「导入 + 导出签名」的规范指纹（sha256 hex）。
    ///
    /// 规范串形如 `imports:[env.log(i64,i64)->i64] exports:[add(i32,i32)->i32]`，
    /// 按名称字典序排列。manifest 的 `abi.wasm` 必须等于它，否则拒载。
    pub interface_hash: String,
    /// 字节长度。
    pub byte_len: u64,
    /// 导出的函数名（字典序）。
    pub exports: Vec<String>,
    /// 模块导入的 host_fn 名（字典序，仅 `env` 命名空间的函数导入）。
    pub imported_host_fns: Vec<String>,
    /// 非 `env` 命名空间或非函数导入（provider 无法绑定，实例化前即拒）。
    pub unsupported_imports: Vec<String>,
    /// 声明内存的最小页数。
    pub memory_min_pages: u32,
    /// 声明内存的最大页数（模块未声明上限时为 `None`）。
    pub memory_max_pages: Option<u32>,
}

impl ModuleFacts {
    /// 是否导出该函数。
    pub fn exports_function(&self, name: &str) -> bool {
        self.exports.iter().any(|exported| exported == name)
    }
}

/// 一次调用的资源预算（由 `WasmPluginConfig` 推导，不由调用方自报）。
#[derive(Debug, Clone, Copy)]
pub struct ExecutionBudget {
    /// 本次调用的 fuel 上限。
    pub fuel: u64,
    /// 内存页数上限（超出即判失败）。
    pub max_memory_pages: u32,
}

/// host_fn 在一次调用里被真打到的观测记录。
#[derive(Debug, Clone)]
pub struct HostCallObserved {
    /// 函数名。
    pub fn_name: String,
    /// 宿主 handler 收到的参数（JSON）。
    pub args: String,
    /// handler 返回给 guest 的结果（JSON）。
    pub return_value: String,
    /// handler 自身耗时（毫秒）。
    pub duration_ms: u64,
}

/// provider 一次真调用的产出。
#[derive(Debug, Clone)]
pub struct EngineOutcome {
    /// 返回值的 JSON 编码（整数结果数组；无返回值时为 `[]`）。
    pub return_value: String,
    /// 引擎**实际**消耗的 fuel。
    pub fuel_consumed: u64,
    /// 调用结束时该实例 memory 的真实页数。
    pub memory_pages_used: u32,
    /// guest 真打到的 host_fn 调用（顺序即调用顺序）。
    pub host_calls: Vec<HostCallObserved>,
}

/// 引擎实例句柄：provider 私有状态的容器，对 `tauron-wasm` 之外不透明。
///
/// 带 `Debug` 超trait：句柄装进 `Box<dyn EngineInstance>` 后，调用方仍能用
/// `unwrap()`/`expect()` 拿到可读的失败信息（否则 `dyn Trait` 不满足 `Debug` 边界）。
pub trait EngineInstance: Send + std::fmt::Debug {
    /// 所属 provider。
    fn provider_id(&self) -> &str;
    /// 供 provider 内部取回具体类型。
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// 供 provider 内部取回具体类型（只读）。
    fn as_any(&self) -> &dyn Any;
}

/// 运行时 provider 接缝（V7 Batch 1 第 1 条）。
pub trait WasmRuntimeProvider: Send + Sync {
    /// provider 标识，写进每一条执行结果，便于事后对账。
    fn provider_id(&self) -> &'static str;
    /// 解析并校验字节，返回可查询的模块事实。**不**执行任何代码。
    fn prepare(&self, bytes: &[u8]) -> WasmResult<PreparedModule>;
    /// 实例化：绑定 host_fn、跑 `start` 段。未注册/越白名单的导入必须在此失败。
    fn instantiate(
        &self,
        prepared: &PreparedModule,
        plugin_id: &str,
        module_generation: u64,
        host_fns: HostFnTable,
        budget: ExecutionBudget,
    ) -> WasmResult<Box<dyn EngineInstance>>;
    /// 真调用一个导出函数。结果、fuel、内存页、host_fn 计数全部来自引擎。
    fn invoke(
        &self,
        prepared: &PreparedModule,
        instance: &mut dyn EngineInstance,
        export: &str,
        args: &str,
        budget: ExecutionBudget,
    ) -> WasmResult<EngineOutcome>;
}

/// provider 编译产物 + 事实。可 `Clone`（内部 `Arc`），因此能进模块缓存常驻。
#[derive(Clone)]
pub struct PreparedModule {
    /// 模块事实（hash / 导出 / 导入 / 内存声明）。
    pub facts: ModuleFacts,
    inner: Arc<wasmi::Module>,
}

impl std::fmt::Debug for PreparedModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedModule").field("facts", &self.facts).finish_non_exhaustive()
    }
}

/// wasmi 实例：`Store` + `Instance`，即「池里那个可复用的真实例」。
struct WasmiInstance {
    store: wasmi::Store<HostState>,
    instance: wasmi::Instance,
}

impl std::fmt::Debug for WasmiInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只暴露可对账的身份，不打印引擎内部状态。
        f.debug_struct("WasmiInstance")
            .field("plugin_id", &self.store.data().plugin_id)
            .field("module_generation", &self.store.data().module_generation)
            .finish_non_exhaustive()
    }
}

impl EngineInstance for WasmiInstance {
    fn provider_id(&self) -> &str {
        "wasmi"
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// store 的宿主状态：注册表 + 本次调用的观测。
struct HostState {
    host_fns: HostFnTable,
    plugin_id: String,
    module_generation: u64,
    calls: Vec<HostCallObserved>,
    /// handler 报错时留痕：调用落定后据此把整次执行判为失败，
    /// **不**因为 guest 拿到一个 0 就谎报成功。
    host_error: Option<String>,
}

/// 本仓内置的**真**引擎 provider（wasmi）。
pub struct WasmiProvider {
    engine: wasmi::Engine,
}

impl Default for WasmiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmiProvider {
    /// 用「开启 fuel 计量」的配置创建引擎。
    pub fn new() -> Self {
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        Self { engine: wasmi::Engine::new(&config) }
    }

    fn memory_pages_of(&self, instance: &WasmiInstance) -> WasmResult<u32> {
        let Some(export) = instance.instance.get_export(&instance.store, "memory") else {
            return Ok(0);
        };
        let Some(memory) = export.into_memory() else {
            return Ok(0);
        };
        Ok(saturating_u32(memory.size(&instance.store)))
    }
}

impl WasmRuntimeProvider for WasmiProvider {
    fn provider_id(&self) -> &'static str {
        "wasmi"
    }

    fn prepare(&self, bytes: &[u8]) -> WasmResult<PreparedModule> {
        if bytes.is_empty() {
            return Err(WasmError::ModuleLoadFailed {
                plugin_id: "(pre-install)".into(),
                reason: "模块字节为空".into(),
            });
        }
        let module =
            wasmi::Module::new(&self.engine, bytes).map_err(|e| WasmError::ModuleLoadFailed {
                plugin_id: "(pre-install)".into(),
                reason: format!("引擎解析/校验失败：{e}"),
            })?;

        let mut exports: Vec<String> =
            module.exports().map(|export| export.name().to_string()).collect();
        exports.sort();
        let mut imported: Vec<String> = Vec::new();
        let mut unsupported: Vec<String> = Vec::new();
        let mut import_signatures: BTreeMap<String, String> = BTreeMap::new();
        let mut export_signatures: BTreeMap<String, String> = BTreeMap::new();
        let mut min_pages = 0u32;
        let mut max_pages: Option<u32> = None;

        for import in module.imports() {
            let name = format!("{}.{}", import.module(), import.name());
            match import.ty() {
                wasmi::ExternType::Func(func_type) => {
                    if import.module() == HOST_FN_NAMESPACE {
                        imported.push(import.name().to_string());
                    } else {
                        unsupported.push(format!("{name}（非 `{HOST_FN_NAMESPACE}` 命名空间）"));
                    }
                    import_signatures
                        .insert(name, render_signature(func_type.params(), func_type.results()));
                }
                other => unsupported.push(format!("{name}（{}）", describe_extern_type(other))),
            }
        }
        for export in module.exports() {
            if let wasmi::ExternType::Memory(memory_type) = export.ty() {
                // wasmi 的页数是 u64；本仓的配额与读数按 u32 页建模，饱和处理即可
                // （真实内存早在 4 GiB 之前就会被宿主/引擎自身拦住）。
                min_pages = saturating_u32(memory_type.minimum());
                max_pages = memory_type.maximum().map(saturating_u32);
            }
            if let wasmi::ExternType::Func(func_type) = export.ty() {
                export_signatures.insert(
                    export.name().to_string(),
                    render_signature(func_type.params(), func_type.results()),
                );
            }
        }
        imported.sort();
        unsupported.sort();

        let canonical = canonical_interface(&import_signatures, &export_signatures);
        let module_hash = hex_sha256(bytes);
        let interface_hash = format!("sha256:{}", hex_sha256(canonical.as_bytes()));

        Ok(PreparedModule {
            facts: ModuleFacts {
                provider_id: self.provider_id().to_string(),
                module_hash,
                interface_hash,
                byte_len: bytes.len() as u64,
                exports,
                imported_host_fns: imported,
                unsupported_imports: unsupported,
                memory_min_pages: min_pages,
                memory_max_pages: max_pages,
            },
            inner: Arc::new(module),
        })
    }

    fn instantiate(
        &self,
        prepared: &PreparedModule,
        plugin_id: &str,
        module_generation: u64,
        host_fns: HostFnTable,
        budget: ExecutionBudget,
    ) -> WasmResult<Box<dyn EngineInstance>> {
        let facts = &prepared.facts;
        if !facts.unsupported_imports.is_empty() {
            return Err(WasmError::HostFnNotRegistered {
                fn_name: facts.unsupported_imports.join(", "),
            });
        }

        // 白名单之外的导入绝不绑定：这是「未注册 host function 不得成功」的落地点。
        for fn_name in &facts.imported_host_fns {
            let registered = host_fns.lock().contains_key(fn_name);
            if !registered {
                return Err(WasmError::HostFnNotRegistered { fn_name: fn_name.clone() });
            }
        }
        // 模块自己声明的最小页数已经越过插件配额时，连实例都不该起。
        if facts.memory_min_pages > budget.max_memory_pages {
            return Err(WasmError::MemoryLimitExceeded {
                plugin_id: plugin_id.to_string(),
                size: facts.memory_min_pages,
                max: budget.max_memory_pages,
            });
        }

        let mut linker = wasmi::Linker::<HostState>::new(&self.engine);
        for fn_name in &facts.imported_host_fns {
            let bound_name = fn_name.clone();
            linker
                .func_wrap(
                    HOST_FN_NAMESPACE,
                    fn_name,
                    move |mut caller: wasmi::Caller<'_, HostState>, left: i64, right: i64| -> i64 {
                        let args = format!("[{left},{right}]");
                        let started = std::time::Instant::now();
                        // **取 Arc 后立即放锁**：绝不在持锁期间调用宿主 handler
                        // （handler 自己可能触碰注册表）。
                        let handler = caller.data().host_fns.lock().get(&bound_name).cloned();
                        let outcome = match handler {
                            Some(handler) => handler(args.clone(), args.clone()),
                            None => Err(format!("host_fn `{bound_name}` 已撤销")),
                        };
                        let duration_ms = started.elapsed().as_millis() as u64;
                        let state = caller.data_mut();
                        match outcome {
                            Ok(return_value) => {
                                state.calls.push(HostCallObserved {
                                    fn_name: bound_name.clone(),
                                    args,
                                    return_value: return_value.clone(),
                                    duration_ms,
                                });
                                // 数值 ABI：handler 必须给出可解析为 i64 的结果，
                                // 否则这次执行整体失败（见 `host_error`）。
                                match parse_numeric_return(&return_value) {
                                    Some(value) => value,
                                    None => {
                                        state.host_error = Some(format!(
                                            "host_fn `{bound_name}` 返回值不是整数 ABI 可承载的数：{return_value}"
                                        ));
                                        0
                                    }
                                }
                            }
                            Err(error) => {
                                state.host_error =
                                    Some(format!("host_fn `{bound_name}` 失败：{error}"));
                                state.calls.push(HostCallObserved {
                                    fn_name: bound_name.clone(),
                                    args,
                                    return_value: format!("{{\"error\":{error:?}}}"),
                                    duration_ms,
                                });
                                0
                            }
                        }
                    },
                )
                .map_err(|e| WasmError::InstanceCreationFailed {
                    plugin_id: plugin_id.to_string(),
                    reason: format!("host_fn `{fn_name}` 绑定失败：{e}"),
                })?;
        }

        let mut store = wasmi::Store::new(
            &self.engine,
            HostState {
                host_fns,
                plugin_id: plugin_id.to_string(),
                module_generation,
                calls: Vec::new(),
                host_error: None,
            },
        );
        store.set_fuel(budget.fuel).map_err(|e| WasmError::InstanceCreationFailed {
            plugin_id: plugin_id.to_string(),
            reason: format!("fuel 配置失败：{e}"),
        })?;

        let instance = linker.instantiate_and_start(&mut store, &prepared.inner).map_err(|e| {
            WasmError::InstanceCreationFailed {
                plugin_id: plugin_id.to_string(),
                reason: format!("实例化/启动失败：{e}"),
            }
        })?;

        Ok(Box::new(WasmiInstance { store, instance }))
    }

    fn invoke(
        &self,
        prepared: &PreparedModule,
        instance: &mut dyn EngineInstance,
        export: &str,
        args: &str,
        budget: ExecutionBudget,
    ) -> WasmResult<EngineOutcome> {
        let Some(slot) = instance.as_any_mut().downcast_mut::<WasmiInstance>() else {
            return Err(WasmError::NoRuntimeProvider {
                provider: instance.provider_id().to_string(),
                reason: "实例句柄与 provider 不匹配".into(),
            });
        };
        let func = slot.instance.get_func(&slot.store, export).ok_or_else(|| {
            WasmError::ExportNotFound {
                plugin_id: prepared.facts.provider_id.clone(),
                function_name: export.to_string(),
            }
        })?;

        let func_type = func.ty(&slot.store);
        let inputs = decode_args(func_type.params(), args)?;
        let mut outputs = blank_results(func_type.results())?;

        slot.store
            .set_fuel(budget.fuel)
            .map_err(|e| WasmError::EngineFailure { reason: format!("fuel 重置失败：{e}") })?;
        let fuel_before = slot
            .store
            .get_fuel()
            .map_err(|e| WasmError::EngineFailure { reason: e.to_string() })?;

        let call_result = func.call(&mut slot.store, &inputs, &mut outputs);

        let fuel_after = slot
            .store
            .get_fuel()
            .map_err(|e| WasmError::EngineFailure { reason: e.to_string() })?;
        let fuel_consumed = fuel_before.saturating_sub(fuel_after);
        let memory_pages_used = self.memory_pages_of(slot)?;
        let host_calls = slot.store.data_mut().calls.clone();
        slot.store.data_mut().calls.clear();
        let host_error = slot.store.data_mut().host_error.take();

        if let Err(error) = call_result {
            return Err(classify_engine_error(&error, fuel_consumed, budget.fuel));
        }
        if memory_pages_used > budget.max_memory_pages {
            return Err(WasmError::MemoryLimitExceeded {
                plugin_id: prepared.facts.provider_id.clone(),
                size: memory_pages_used,
                max: budget.max_memory_pages,
            });
        }
        if let Some(reason) = host_error {
            return Err(WasmError::HostFnFailed { reason });
        }

        Ok(EngineOutcome {
            return_value: encode_results(&outputs),
            fuel_consumed,
            memory_pages_used,
            host_calls,
        })
    }
}

/// host_fn 导入的命名空间（本仓 provider 的固定约定）。
pub const HOST_FN_NAMESPACE: &str = "env";

/// 页数读数饱和到 `u32`（wasmi 用 `u64` 计页）。
fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn render_signature(params: &[wasmi::ValType], results: &[wasmi::ValType]) -> String {
    let params =
        params.iter().map(|t| format!("{t:?}").to_ascii_lowercase()).collect::<Vec<_>>().join(",");
    let results =
        results.iter().map(|t| format!("{t:?}").to_ascii_lowercase()).collect::<Vec<_>>().join(",");
    format!("({params})->({results})")
}

fn describe_extern_type(kind: &wasmi::ExternType) -> &'static str {
    match kind {
        wasmi::ExternType::Global(_) => "global 导入",
        wasmi::ExternType::Table(_) => "table 导入",
        wasmi::ExternType::Memory(_) => "memory 导入",
        wasmi::ExternType::Func(_) => "函数导入",
    }
}

fn canonical_interface(
    imports: &BTreeMap<String, String>,
    exports: &BTreeMap<String, String>,
) -> String {
    let import_list =
        imports.iter().map(|(name, sig)| format!("{name}{sig}")).collect::<Vec<_>>().join(" ");
    let export_list =
        exports.iter().map(|(name, sig)| format!("{name}{sig}")).collect::<Vec<_>>().join(" ");
    format!("imports:[{import_list}] exports:[{export_list}]")
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex::encode(digest.finalize())
}

/// handler 返回 JSON → guest 的 i64。接受裸整数与 `{"result":<int>}` 两种形态。
fn parse_numeric_return(return_value: &str) -> Option<i64> {
    let trimmed = return_value.trim();
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(value);
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    parsed.get("result").and_then(serde_json::Value::as_i64)
}

fn decode_args(params: &[wasmi::ValType], args: &str) -> WasmResult<Vec<wasmi::Val>> {
    let trimmed = args.trim();
    let values: Vec<serde_json::Value> = if trimmed.is_empty() || trimmed == "[]" {
        Vec::new()
    } else {
        let parsed: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| WasmError::InvalidArgs(format!("参数不是合法 JSON：{e}")))?;
        match parsed {
            serde_json::Value::Array(list) => list,
            serde_json::Value::Null => Vec::new(),
            serde_json::Value::Object(map) => match map.get("args") {
                Some(serde_json::Value::Array(list)) => list.clone(),
                Some(other) => vec![other.clone()],
                None => {
                    return Err(WasmError::InvalidArgs("对象参数必须带 `args` 数组".to_string()))
                }
            },
            other => vec![other],
        }
    };
    if values.len() != params.len() {
        return Err(WasmError::InvalidArgs(format!(
            "参数个数不匹配：导出函数需要 {} 个，收到 {}",
            params.len(),
            values.len()
        )));
    }
    let mut out = Vec::with_capacity(params.len());
    for (kind, value) in params.iter().zip(values.iter()) {
        out.push(match kind {
            wasmi::ValType::I32 => {
                let raw = value.as_i64().ok_or_else(|| {
                    WasmError::InvalidArgs(format!("i32 参数需要整数，收到 {value}"))
                })?;
                wasmi::Val::I32(
                    i32::try_from(raw)
                        .map_err(|_| WasmError::InvalidArgs(format!("i32 参数越界：{raw}")))?,
                )
            }
            wasmi::ValType::I64 => wasmi::Val::I64(value.as_i64().ok_or_else(|| {
                WasmError::InvalidArgs(format!("i64 参数需要整数，收到 {value}"))
            })?),
            other => {
                return Err(WasmError::UnsupportedSignature {
                    signature: format!("参数类型 {other:?}"),
                })
            }
        });
    }
    Ok(out)
}

fn blank_results(results: &[wasmi::ValType]) -> WasmResult<Vec<wasmi::Val>> {
    results
        .iter()
        .map(|kind| match kind {
            wasmi::ValType::I32 => Ok(wasmi::Val::I32(0)),
            wasmi::ValType::I64 => Ok(wasmi::Val::I64(0)),
            other => Err(WasmError::UnsupportedSignature {
                signature: format!("返回类型 {other:?}"),
            }),
        })
        .collect()
}

fn encode_results(results: &[wasmi::Val]) -> String {
    let items: Vec<String> = results
        .iter()
        .map(|value| match value {
            wasmi::Val::I32(v) => v.to_string(),
            wasmi::Val::I64(v) => v.to_string(),
            other => format!("\"{other:?}\""),
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// 引擎错误分类：fuel 耗尽单独成码，其余归 trap。
fn classify_engine_error(error: &wasmi::Error, fuel_consumed: u64, fuel_budget: u64) -> WasmError {
    let text = error.to_string();
    if text.contains("fuel") || text.contains("Fuel") {
        return WasmError::FuelExhausted { fuel_consumed, fuel_budget };
    }
    WasmError::EngineTrap { reason: text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler(return_value: &'static str) -> HostFnHandler {
        Arc::new(move |_, _| Ok(return_value.to_string()))
    }

    fn table(entries: &[(&str, HostFnHandler)]) -> HostFnTable {
        let mut map: BTreeMap<String, HostFnHandler> = BTreeMap::new();
        for (name, handler) in entries {
            map.insert((*name).to_string(), handler.clone());
        }
        Arc::new(Mutex::new(map))
    }

    const ADD_WAT: &str = r#"(module
        (func (export "add") (param i32 i32) (result i32)
            local.get 0
            local.get 1
            i32.add)
    )"#;

    const LOG_WAT: &str = r#"(module
        (import "env" "log" (func $log (param i64 i64) (result i64)))
        (func (export "call_log") (result i64)
            i64.const 7
            i64.const 11
            call $log)
    )"#;

    #[test]
    fn prepare_reads_real_exports_and_hash() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(ADD_WAT).unwrap();
        let prepared = provider.prepare(&bytes).expect("真字节必须能解析");
        assert_eq!(prepared.facts.provider_id, "wasmi");
        assert_eq!(prepared.facts.exports, vec!["add".to_string()]);
        assert_eq!(prepared.facts.imported_host_fns, Vec::<String>::new());
        assert_eq!(prepared.facts.byte_len, bytes.len() as u64);
        assert_eq!(prepared.facts.module_hash, hex::encode(Sha256::digest(&bytes)));
        assert!(prepared.facts.interface_hash.starts_with("sha256:"));
    }

    #[test]
    fn prepare_rejects_garbage_bytes_instead_of_reporting_success() {
        let provider = WasmiProvider::new();
        let error = provider.prepare(b"this is not wasm at all").expect_err("非 wasm 字节必须失败");
        assert!(matches!(error, WasmError::ModuleLoadFailed { .. }), "实际：{error:?}");
    }

    #[test]
    fn invoke_returns_the_real_computed_sum() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(ADD_WAT).unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        let budget = ExecutionBudget { fuel: 10_000_000, max_memory_pages: 1024 };
        let mut instance = provider.instantiate(&prepared, "p", 1, table(&[]), budget).unwrap();
        let outcome =
            provider.invoke(&prepared, instance.as_mut(), "add", "[40,2]", budget).unwrap();
        assert_eq!(outcome.return_value, "[42]");
        assert!(outcome.fuel_consumed > 0, "fuel 必须真实消耗");
    }

    #[test]
    fn host_fn_call_is_observed_by_the_engine_not_faked_by_the_host() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(LOG_WAT).unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        assert_eq!(prepared.facts.imported_host_fns, vec!["log".to_string()]);
        let budget = ExecutionBudget { fuel: 10_000_000, max_memory_pages: 1024 };
        let mut instance = provider
            .instantiate(&prepared, "p", 1, table(&[("log", handler("42"))]), budget)
            .unwrap();
        let outcome =
            provider.invoke(&prepared, instance.as_mut(), "call_log", "[]", budget).unwrap();
        assert_eq!(outcome.return_value, "[42]", "guest 必须拿到 host 的真返回");
        assert_eq!(outcome.host_calls.len(), 1);
        assert_eq!(outcome.host_calls[0].fn_name, "log");
    }

    #[test]
    fn unregistered_host_fn_import_fails_closed() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(LOG_WAT).unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        let budget = ExecutionBudget { fuel: 10_000_000, max_memory_pages: 1024 };
        let error = provider
            .instantiate(&prepared, "p", 1, table(&[]), budget)
            .expect_err("未注册 host_fn 不得实例化");
        assert!(matches!(error, WasmError::HostFnNotRegistered { .. }), "实际：{error:?}");
    }

    #[test]
    fn fuel_exhaustion_is_its_own_typed_error() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(
            r#"(module
                (memory (export "memory") 1 4)
                (func (export "spin")
                    (loop $again
                        br $again))
            )"#,
        )
        .unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        let budget = ExecutionBudget { fuel: 1_000, max_memory_pages: 1024 };
        let mut instance = provider.instantiate(&prepared, "p", 1, table(&[]), budget).unwrap();
        let error = provider
            .invoke(&prepared, instance.as_mut(), "spin", "[]", budget)
            .expect_err("死循环必须被 fuel 打断");
        assert!(
            matches!(error, WasmError::FuelExhausted { .. }),
            "死循环应归为 fuel 耗尽，实际：{error:?}"
        );
    }

    #[test]
    fn trap_is_reported_as_trap() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(r#"(module (func (export "boom") unreachable))"#).unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        let budget = ExecutionBudget { fuel: 10_000_000, max_memory_pages: 1024 };
        let mut instance = provider.instantiate(&prepared, "p", 1, table(&[]), budget).unwrap();
        let error = provider
            .invoke(&prepared, instance.as_mut(), "boom", "[]", budget)
            .expect_err("trap 必须失败");
        assert!(matches!(error, WasmError::EngineTrap { .. }), "实际：{error:?}");
    }

    #[test]
    fn memory_declaration_is_read_from_the_module() {
        let provider = WasmiProvider::new();
        let bytes = wat::parse_str(r#"(module (memory (export "memory") 2 8))"#).unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        assert_eq!(prepared.facts.memory_min_pages, 2);
        assert_eq!(prepared.facts.memory_max_pages, Some(8));
    }

    #[test]
    fn float_signature_is_rejected_not_silently_truncated() {
        let provider = WasmiProvider::new();
        let bytes =
            wat::parse_str(r#"(module (func (export "f") (param f32) (result f32) local.get 0))"#)
                .unwrap();
        let prepared = provider.prepare(&bytes).unwrap();
        let budget = ExecutionBudget { fuel: 10_000_000, max_memory_pages: 1024 };
        let mut instance = provider.instantiate(&prepared, "p", 1, table(&[]), budget).unwrap();
        let error = provider
            .invoke(&prepared, instance.as_mut(), "f", "[1.5]", budget)
            .expect_err("浮点 ABI 未实现，必须显式拒绝");
        assert!(matches!(error, WasmError::UnsupportedSignature { .. }), "实际：{error:?}");
    }
}
