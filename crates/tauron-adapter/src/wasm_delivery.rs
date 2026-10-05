// WASM 插件投递实现（任务二：接通孤儿 crate `tauron-wasm`）。
//
// **为什么需要这一层**：在本次改动之前，`PluginType::Wasm` 在投递侧只落到
// `UnwiredDelivery`——`tauron-wasm` 的配置校验 / ABI 指纹 / 崩溃预算**完全被绕开**，
// 而它却是本仓唯一"WASM 插件该怎么配才合法"的事实源。绕过它意味着 manifest 里
// 一份明显非法的 WASM 配置（空 module_path、非法内存配额…）在调用时不会有任何
// 提示，直到真接了 runtime 才炸。
//
// **本轮的诚实边界（一个字不美化）**：`tauron-wasm` 自 V7 轮 22 起**内置真引擎**
// （`wasmi` provider：字节解析、ABI 指纹、实例化、fuel 计量都是真的），但**宿主投递
// 路径仍未接上它**，原因不是"没引擎"，而是两条缺一不可的前置没落地：
//   1. 本仓 provider 只支持整数 host_fn ABI（`(i64,i64)->i64`），经 linear memory
//      传字符串/JSON 的完整 ABI 未实现——`Call` 的 JSON 参数无法直接喂给它；
//   2. ADR-10 要求的独立 supervisor 子进程承载未实现，在宿主进程内直接跑第三方
//      插件字节不是本仓愿意承诺的生产形态。
// 因此本投递器**只接状态层**：
//   1. 从注册表取目标插件的 manifest，构造 `WasmPluginConfig`；
//   2. 真的跑 `tauron_wasm::validate_plugin_config`（配置层校验，真实）；
//   3. 真的查 `tauron_wasm::WasmCrashTracker` 的崩溃预算（状态层，真实）；
//   4. **执行层保持诚实失败**：返回 `delivered: false`，原因写明"没有可执行通路"，
//      上层据此转 `E_PLUGIN_TYPE_NO_RUNTIME`——**不假装**调用被投递。
//
// 也就是说：`PluginType::Wasm` 的处理路径现在**真的经过** `tauron-wasm` 的校验层，
// 而不是像以前那样完全绕开；但"通过宿主调用一个 wasm 插件"依然不可用，且如实报出。

use std::sync::Arc;

use parking_lot::Mutex;
use tauron_host::{
    call_delivery::{CallDelivery, CallOutcome, DeliveryKind, DeliveryReceipt},
    manifest::{PluginId, PluginManifest},
    registry::Registry,
    ErrorCode, HostError, HostResult, PendingCall,
};
use tauron_wasm::{
    validate_plugin_config, CrashLimitConfig, ExecutionConfig, HostFnWhitelist, InstancePoolConfig,
    MemoryConfig, ModuleCacheConfig, WasmAbiFingerprint, WasmCrashTracker, WasmPluginConfig,
};

/// 从 manifest 构造 WASM 插件配置（`tauron-wasm` 的输入）。
///
/// 字段来源（全部**真实**取自 manifest，不编造）：
/// - `plugin_id`：注册表里的 id；
/// - `module_path`：`manifest.entry.wasm`（D 类插件必填，见 manifest 校验）；
/// - `abi.interface_hash`：`manifest.abi.wasm`（D 类插件必填）。
///
/// 其余（内存 / host_fn 白名单 / 实例池 / 模块缓存 / 崩溃限制）取 `tauron-wasm`
/// 的缺省——manifest 里没有这些字段，**不假装**前端能配。
pub fn wasm_config_from_manifest(manifest: &PluginManifest) -> WasmPluginConfig {
    let abi = WasmAbiFingerprint {
        interface_hash: manifest.abi.as_ref().and_then(|a| a.wasm.clone()).unwrap_or_default(),
        // host_fn 列表：manifest 的 `hostFunctions` 声明就是"这个插件要用的宿主函数"。
        host_fns: manifest.host_functions.clone(),
        generated_at: chrono::Utc::now(),
    };
    WasmPluginConfig {
        plugin_id: manifest.id.to_string(),
        module_path: manifest.entry.wasm.clone().unwrap_or_default(),
        abi,
        memory: MemoryConfig::default(),
        host_fn_whitelist: HostFnWhitelist::default(),
        instance_pool: InstancePoolConfig::default(),
        module_cache: ModuleCacheConfig::default(),
        crash_limit: CrashLimitConfig::default(),
        execution: ExecutionConfig::default(),
    }
}

/// WASM 插件投递器：**状态层校验真接，执行层诚实失败**。
///
/// `Registry` 只用于读 manifest（不写 pending 表——投递实现不得写 pending，
/// 见 `tauron-host::call_delivery` 的不变量）。
pub struct WasmCallDelivery {
    registry: Arc<Registry>,
    /// 崩溃预算追踪（`tauron-wasm` 的真实状态组件）。
    crash_tracker: Mutex<WasmCrashTracker>,
}

impl WasmCallDelivery {
    /// 用默认崩溃限制（3 次 / 5min）构造。
    pub fn new(registry: Arc<Registry>) -> Self {
        Self { registry, crash_tracker: Mutex::new(WasmCrashTracker::default_tracker()) }
    }

    /// 记录一次 WASM 插件崩溃；返回是否**未超限**（`false` = 已超预算）。
    ///
    /// 真实消费者是宿主侧的崩溃回报路径（supervisor 崩溃 → 宿主调用它）。
    pub fn record_crash(&self, plugin_id: &str) -> bool {
        self.crash_tracker.lock().record_crash(plugin_id)
    }

    /// 窗口内崩溃次数。
    pub fn crash_count(&self, plugin_id: &str) -> u32 {
        self.crash_tracker.lock().crash_count(plugin_id)
    }
}

impl CallDelivery for WasmCallDelivery {
    fn target_kind(&self) -> DeliveryKind {
        DeliveryKind::Wasm
    }

    fn deliver(&self, call: &PendingCall) -> HostResult<DeliveryReceipt> {
        let id = match PluginId::new(&call.target) {
            Ok(id) => id,
            Err(_) => {
                return Ok(DeliveryReceipt {
                    delivered: false,
                    reason: Some(format!("调用目标 `{}` 不是合法插件 id", call.target)),
                })
            }
        };

        // 1. 崩溃预算：已超限的插件**不再投递**（与 supervisor「超限不重启载入」一致）。
        if self.crash_tracker.lock().is_exceeded(&call.target) {
            return Ok(DeliveryReceipt {
                delivered: false,
                reason: Some(format!(
                    "WASM 插件 `{}` 已超过崩溃预算（{} 次 / 窗口），拒绝投递",
                    call.target,
                    self.crash_count(&call.target)
                )),
            });
        }

        // 2. 读 manifest 并跑 **tauron-wasm 的真实配置校验**。
        let manifest = match self.registry.find(&id) {
            Some(entry) => entry.manifest.clone(),
            None => {
                return Ok(DeliveryReceipt {
                    delivered: false,
                    reason: Some(format!("插件 `{}` 不在注册表中", call.target)),
                })
            }
        };
        let config = wasm_config_from_manifest(&manifest);
        if let Err(e) = validate_plugin_config(&config) {
            return Ok(DeliveryReceipt {
                delivered: false,
                reason: Some(format!("WASM 配置校验未通过（tauron-wasm）：{e}")),
            });
        }

        // 3. 执行层：宿主投递路径没有可执行通路，**诚实失败**（不假装投递成功）。
        Ok(DeliveryReceipt {
            delivered: false,
            reason: Some(
                "WASM 配置校验通过，但宿主投递侧没有可执行通路：tauron-wasm 的 wasmi provider \
                 只支持整数 host_fn ABI（JSON 参数需经 linear memory 的完整 ABI 未实现），\
                 且 ADR-10 要求的 supervisor 子进程承载未落地"
                    .into(),
            ),
        })
    }

    fn settle(&self, _call_id: &str, _outcome: CallOutcome) -> HostResult<PendingCall> {
        Err(HostError::new(
            ErrorCode::E_CALL_NOT_FOUND,
            "WASM 投递实现无法结算调用（投递侧无可执行通路）",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauron_host::manifest::{
        Contributes, EntrySpec, EventsDecl, PermissionEntry, PermissionIndex, PluginType, Risk,
    };

    /// 空权限词表（本测试只走 WASM 校验层，不涉权限判定）。
    fn index() -> PermissionIndex {
        PermissionIndex {
            version: 1,
            generated_at: None,
            entries: vec![PermissionEntry {
                identifier: "store:allow-get".into(),
                risk: Risk::Low,
                description: "读取 store".into(),
                scoped: false,
            }],
        }
    }

    fn wasm_manifest(id: &str, wasm_entry: Option<&str>, abi: Option<&str>) -> PluginManifest {
        PluginManifest {
            id: PluginId::new(id).unwrap(),
            name: format!("plugin {id}"),
            version: semver::Version::new(1, 0, 0),
            plugin_type: PluginType::Wasm,
            entry: EntrySpec { wasm: wasm_entry.map(str::to_string), ..Default::default() },
            permissions: Vec::new(),
            scopes: serde_json::Map::new(),
            platforms: Vec::new(),
            framework: tauron_host::manifest::parse_version_range(">=1.0.0, <2.0.0").unwrap(),
            abi: abi.map(|h| tauron_host::manifest::AbiFingerprint {
                rust: None,
                wasm: Some(h.to_string()),
            }),
            contributes: Contributes::default(),
            settings_schema: None,
            events: EventsDecl::default(),
            host_functions: Vec::new(),
            min_allowed_version: None,
            signature: None,
            publisher: None,
        }
    }

    fn pending(target: &str) -> PendingCall {
        let now = std::time::Instant::now();
        PendingCall::new(
            "c1".into(),
            "main".into(),
            "run".into(),
            serde_json::Value::Null,
            "main".into(),
            target.into(),
            1,
            now,
            now,
        )
    }

    /// **接通证据**：合法 WASM 配置 → 校验通过 → 仍按"无可执行通路"诚实失败
    /// （`delivered: false` 且原因写明缺口在哪，**不是**假成功）。
    #[test]
    fn valid_wasm_config_passes_validation_then_fails_honestly_on_runtime() {
        let registry = Arc::new(Registry::default());
        let id = registry
            .install(
                &index(),
                wasm_manifest("com.example.w", Some("plugin.wasm"), Some("sha256:abc")),
            )
            .unwrap();
        registry.admin_op(&id, tauron_host::authz::RegistryAdminOp::Enable).unwrap();

        let delivery = WasmCallDelivery::new(registry);
        assert_eq!(delivery.target_kind(), DeliveryKind::Wasm);
        let receipt = delivery.deliver(&pending("com.example.w")).unwrap();
        assert!(!receipt.delivered, "无可执行通路不得假装投递成功");
        let reason = receipt.reason.expect("未投递时 reason 必非空");
        assert!(reason.contains("没有可执行通路"), "原因应写明缺口：{reason}");
        assert!(reason.contains("整数 host_fn ABI"), "缺口要具体到 ABI：{reason}");
        assert!(!reason.contains("配置校验未通过"), "合法配置不应被校验拦下：{reason}");
    }

    /// **校验层真的生效**：把一份**缺 `abi.wasm`** 的 manifest 交给投递层的
    /// 配置构造 + `tauron-wasm` 校验，必须被拦下并带上校验器话术——证明
    /// `PluginType::Wasm` 的处理路径真的经过 `tauron-wasm`。
    ///
    /// 注：这种 manifest 在 `registry.install` 阶段就会被 manifest 校验拒绝
    /// （A/D 类必填 `abi.wasm`）——两道门互相独立，下面同时断言"更早的那道门"
    /// 也拦得住，避免误以为这里只是重复校验。
    #[test]
    fn invalid_wasm_config_is_rejected_by_the_validation_layer() {
        // `abi` 为 None → `wasm_config_from_manifest` 得到空 interface_hash。
        let bad = wasm_manifest("com.example.bad", Some("plugin.wasm"), None);

        // ① 投递层的配置构造 + tauron-wasm 校验：空 interface_hash 被拦下。
        let config = wasm_config_from_manifest(&bad);
        let err = validate_plugin_config(&config).expect_err("缺 abi.wasm 必须被校验层拒绝");
        assert!(format!("{err}").contains("abi.interface_hash"), "应带上校验器话术：{err}");

        // ② 更早的一道门（纵深防御）：registry.install 也不接受这种 manifest。
        let registry = Registry::default();
        assert!(
            registry.install(&index(), bad).is_err(),
            "manifest 校验应在安装阶段就拒绝缺 abi.wasm 的 wasm 插件"
        );
    }

    /// 崩溃预算真实生效：超限后拒绝投递。
    #[test]
    fn crash_budget_is_enforced() {
        let registry = Arc::new(Registry::default());
        let delivery = WasmCallDelivery::new(registry);
        // 缺省 max_crashes = 3：记 4 次后 `is_exceeded` 为真。
        for _ in 0..4 {
            delivery.record_crash("com.example.w");
        }
        assert_eq!(delivery.crash_count("com.example.w"), 4);
        let receipt = delivery.deliver(&pending("com.example.w")).unwrap();
        assert!(!receipt.delivered);
        assert!(receipt.reason.unwrap().contains("崩溃预算"));
    }

    /// `settle` 显式拒绝（投递侧无可执行通路时不得结算成"成功"）。
    #[test]
    fn settle_is_refused() {
        let registry = Arc::new(Registry::default());
        let delivery = WasmCallDelivery::new(registry);
        assert!(delivery
            .settle("c1", CallOutcome { ok: true, result: None, error_code: None })
            .is_err());
    }
}
