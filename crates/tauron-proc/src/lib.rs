// §4.7 进程插件 host：sidecar spawn（可注入启动面 + stdout 读线程）、崩溃窗口计数、
// ABI 指纹校验。
//
// 职责（计划 §4.7 的真实子集）：
// - `spawner.rs`：`ProcSpawner` trait + `CommandSpawner` 生产实现（piped stdin/stdout
//   + 读线程排水 + 帧回调），真实执行链的**唯一**进程启动面；
// - `lib.rs`：spawn 配置校验（`validate_spawn_config`，与 adapter 共用同一份规则）、
//   ABI 指纹（`current_abi_contract` / `validate_abi`）、崩溃窗口计数（`CrashTracker`，
//   adapter 的崩溃预算**唯一**来源）。
//
// **诚实边界（0.4 审计）**：原 `spawn.rs` 的 `ProcRunner`（模拟执行器：不真起进程
// 却返回 `Running`、RPC 回显模拟、全局心跳单例）全仓**零生产调用方**——真实链路
// 走 `CommandSpawner` + `tauron-host` 的 `RuntimeTable`。孤儿模拟器已整体删除
// （含 RpcConfig / HeartbeatTracker / ConcurrencyTracker 等仅被它使用的类型）；
// 真实链路的**心跳监控尚未实现**（登记为 0.4-A3 的诚实边界，见
// capability-closure-plan.md），不是"已实现但在别处"。
//
// 本 crate 不依赖 `tauri`：进程管理是抽象的，单元测试用 Mock。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod error;
pub mod spawner;

pub use error::{ProcError, ProcResult};
pub use spawner::{CommandSpawner, KillOutcome, ProcessFrameSink, ProcSpawner, SpawnedProc};

// ──────────────────────────────────────────────────────────────────────────
// 配置类型
// ──────────────────────────────────────────────────────────────────────────

/// 二进制签名信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySignature {
    /// 签名算法。
    pub algorithm: String,
    /// 签名（hex 编码）。
    pub signature: String,
    /// 签名者 ID。
    pub signer_id: String,
}

/// ABI 指纹。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbiFingerprint {
    /// crate 版本指纹。
    pub rust_version: String,
    /// 接口哈希。
    pub interface_hash: String,
    /// 生成时间。
    pub generated_at: DateTime<Utc>,
}

impl AbiFingerprint {
    /// 用**宿主当前时钟**构造指纹。
    ///
    /// `generated_at` 刻意不进线格式：时钟由宿主说了算。让调用方传时间戳，
    /// 等于让"本二进制何时被校验过"这个诊断依据听凭前端自报。
    pub fn now(rust_version: impl Into<String>, interface_hash: impl Into<String>) -> Self {
        Self {
            rust_version: rust_version.into(),
            interface_hash: interface_hash.into(),
            generated_at: Utc::now(),
        }
    }
}

/// 宿主当前支持的 sidecar ABI 契约——`rust_version` 维度。
///
/// **是协议版本串，不是 crate 版本号**：crate 版本每次发版都变，前端无法硬编码；
/// 这里只在 **sidecar 协议本身发生不兼容改动**时递增（`/1` → `/2`），因此它可以在
/// TS 侧被镜像成同名常量（`@tauron/host` 的 `SIDECAR_ABI_CONTRACT`）。
pub const SIDECAR_ABI_RUST_VERSION: &str = "tauron-proc-abi/1";

/// 宿主当前支持的 sidecar ABI 契约——`interface_hash` 维度（JSON-RPC 帧格式标识）。
///
/// 帧格式（`maxFrameBytes` / 长度前缀 / 分帧规则）改动即改此串。
pub const SIDECAR_ABI_INTERFACE_HASH: &str = "tauron-sidecar-rpc/1";

/// 宿主当前支持的 sidecar ABI 契约指纹。
///
/// `generated_at` 用宿主时钟（[`AbiFingerprint::now`] 的约定）；[`validate_abi`] 只
/// 比对 `rust_version` 与 `interface_hash` 两维，时间戳不参与判定。
pub fn current_abi_contract() -> AbiFingerprint {
    AbiFingerprint::now(SIDECAR_ABI_RUST_VERSION, SIDECAR_ABI_INTERFACE_HASH)
}

/// 崩溃重启限制。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashLimit {
    /// 时间窗口（秒）。
    pub window_secs: u64,
    /// 窗口内最大崩溃次数。
    pub max_crashes: u32,
}

impl Default for CrashLimit {
    fn default() -> Self {
        Self {
            window_secs: 300, // 5 min
            max_crashes: 3,
        }
    }
}

/// Sidecar spawn 配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnConfig {
    /// sidecar 二进制路径。
    pub binary_path: String,
    /// sidecar 参数列表。
    pub args: Vec<String>,
    /// 环境变量。
    pub env: HashMap<String, String>,
    /// 二进制签名。
    pub signature: BinarySignature,
    /// 二进制哈希（sha256）。
    pub binary_hash: String,
    /// ABI 指纹。
    pub abi: AbiFingerprint,
}

// ──────────────────────────────────────────────────────────────────────────
// 配置验证
// ──────────────────────────────────────────────────────────────────────────

/// 验证 spawn 配置。
pub fn validate_spawn_config(config: &SpawnConfig) -> ProcResult<()> {
    // 检查二进制路径
    if config.binary_path.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("binary_path 不能为空".into()));
    }

    // 检查签名
    if config.signature.algorithm.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("signature.algorithm 不能为空".into()));
    }
    if config.signature.signature.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("signature.signature 不能为空".into()));
    }
    if config.signature.signer_id.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("signature.signer_id 不能为空".into()));
    }

    // 检查哈希
    if config.binary_hash.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("binary_hash 不能为空".into()));
    }
    // 哈希应为 64 字符的 hex
    if config.binary_hash.len() != 64 {
        return Err(ProcError::InvalidSpawnConfig(format!(
            "binary_hash 应为 64 字符 hex，实际 {} 字符",
            config.binary_hash.len()
        )));
    }

    // 检查 ABI
    if config.abi.rust_version.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("abi.rust_version 不能为空".into()));
    }
    if config.abi.interface_hash.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("abi.interface_hash 不能为空".into()));
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// 崩溃追踪
// ──────────────────────────────────────────────────────────────────────────

/// 崩溃记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CrashRecord {
    timestamp: DateTime<Utc>,
    plugin_id: String,
}

/// 崩溃追踪器。
pub struct CrashTracker {
    records: HashMap<String, Vec<CrashRecord>>,
    limit: CrashLimit,
}

impl CrashTracker {
    /// 创建新的崩溃追踪器。
    pub fn new(limit: CrashLimit) -> Self {
        Self { records: HashMap::new(), limit }
    }

    /// 使用默认限制创建。
    pub fn default_tracker() -> Self {
        Self::new(CrashLimit::default())
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
        records.push(CrashRecord { timestamp: now, plugin_id: plugin_id.to_string() });

        // 检查是否超限
        records.len() as u32 <= self.limit.max_crashes
    }

    /// 检查是否已超限。
    pub fn is_exceeded(&self, plugin_id: &str) -> bool {
        let now = Utc::now();
        let window_start = now - chrono::Duration::seconds(self.limit.window_secs as i64);

        match self.records.get(plugin_id) {
            Some(records) => {
                records.iter().filter(|r| r.timestamp >= window_start).count() as u32
                    > self.limit.max_crashes
            }
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
            .map(|records| records.iter().filter(|r| r.timestamp >= window_start).count() as u32)
            .unwrap_or(0)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// ABI 校验
// ──────────────────────────────────────────────────────────────────────────

/// 验证 ABI 指纹。
///
/// **比对两个维度**：`rust_version`（协议版本串）与 `interface_hash`（帧格式标识）。
/// `generated_at` 不参与——它记录的是"何时校验过"，两边必然不同。
///
/// 生产调用点：`tauron-adapter` 的 `cmd_runtime_spawn` 在 spawn 前用它比对
/// [`current_abi_contract`]（宿主契约）与调用方声明的 `SpawnConfig.abi`；不符 →
/// `ErrorCode::E_ABI_MISMATCH`（**不是** `E_INSTALL_FAILED`：ABI 不匹配是版本兼容
/// 问题，调用方该升级插件/宿主，而不是重装）。
///
/// **诚实边界**：`SpawnConfig.abi` 是调用方**自报**的，因此本函数挡的是
/// "配置错配 / 前端用了旧模板"，**不是**"恶意调用方伪造 ABI"——真正的可信校验
/// 需要 sidecar 在 RPC 握手时自报指纹（尚未实现）。它与 `validate_spawn_config`
/// 的签名/哈希检查同属"配置一致性"层，不是安全边界。
pub fn validate_abi(expected: &AbiFingerprint, actual: &AbiFingerprint) -> ProcResult<()> {
    if expected.rust_version != actual.rust_version {
        return Err(ProcError::AbiMismatch {
            expected: expected.rust_version.clone(),
            actual: actual.rust_version.clone(),
        });
    }
    if expected.interface_hash != actual.interface_hash {
        return Err(ProcError::AbiMismatch {
            expected: expected.interface_hash.clone(),
            actual: actual.interface_hash.clone(),
        });
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signature() -> BinarySignature {
        BinarySignature {
            algorithm: "ed25519".into(),
            signature: "abc123".into(),
            signer_id: "signer-001".into(),
        }
    }

    fn make_hash() -> String {
        "a".repeat(64)
    }

    fn make_abi() -> AbiFingerprint {
        AbiFingerprint {
            rust_version: "1.98.0".into(),
            interface_hash: "hash123".into(),
            generated_at: Utc::now(),
        }
    }

    fn make_spawn_config() -> SpawnConfig {
        SpawnConfig {
            binary_path: "/path/to/sidecar".into(),
            args: vec!["--flag".into()],
            env: HashMap::new(),
            signature: make_signature(),
            binary_hash: make_hash(),
            abi: make_abi(),
        }
    }

    // ── 配置验证测试 ──

    #[test]
    fn test_validate_spawn_config_valid() {
        let config = make_spawn_config();
        assert!(validate_spawn_config(&config).is_ok());
    }

    #[test]
    fn test_validate_spawn_config_empty_path() {
        let mut config = make_spawn_config();
        config.binary_path = String::new();
        assert!(validate_spawn_config(&config).is_err());
    }

    #[test]
    fn test_validate_spawn_config_invalid_hash_length() {
        let mut config = make_spawn_config();
        config.binary_hash = "short".into();
        let err = validate_spawn_config(&config).unwrap_err();
        assert!(matches!(err, ProcError::InvalidSpawnConfig(_)));
    }

    #[test]
    fn test_validate_spawn_config_empty_signature() {
        let mut config = make_spawn_config();
        config.signature.algorithm = String::new();
        assert!(validate_spawn_config(&config).is_err());
    }

    // ── 崩溃追踪测试 ──

    #[test]
    fn test_crash_tracker_record_crash() {
        let mut tracker = CrashTracker::default_tracker();
        let exceeded = tracker.record_crash("test.plugin");
        assert!(exceeded); // 未超限
        assert_eq!(tracker.crash_count("test.plugin"), 1);
    }

    #[test]
    fn test_crash_tracker_exceed_limit() {
        let limit = CrashLimit { window_secs: 300, max_crashes: 3 };
        let mut tracker = CrashTracker::new(limit);

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
    fn test_crash_tracker_clear() {
        let mut tracker = CrashTracker::default_tracker();
        tracker.record_crash("test.plugin");
        tracker.clear("test.plugin");
        assert_eq!(tracker.crash_count("test.plugin"), 0);
    }

    #[test]
    fn test_crash_tracker_multiple_plugins() {
        let mut tracker = CrashTracker::default_tracker();
        tracker.record_crash("plugin.a");
        tracker.record_crash("plugin.b");
        assert_eq!(tracker.crash_count("plugin.a"), 1);
        assert_eq!(tracker.crash_count("plugin.b"), 1);
        assert_eq!(tracker.crash_count("plugin.c"), 0);
    }

    // ── ABI 校验测试 ──

    #[test]
    fn test_validate_abi_match() {
        let expected = make_abi();
        let actual = make_abi();
        assert!(validate_abi(&expected, &actual).is_ok());
    }

    #[test]
    fn test_validate_abi_rust_version_mismatch() {
        let expected = make_abi();
        let mut actual = make_abi();
        actual.rust_version = "1.99.0".into();
        let err = validate_abi(&expected, &actual).unwrap_err();
        assert!(matches!(err, ProcError::AbiMismatch { .. }));
    }

    #[test]
    fn test_validate_abi_interface_hash_mismatch() {
        let expected = make_abi();
        let mut actual = make_abi();
        actual.interface_hash = "different".into();
        let err = validate_abi(&expected, &actual).unwrap_err();
        assert!(matches!(err, ProcError::AbiMismatch { .. }));
    }
}
