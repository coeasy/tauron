// §4.7 进程插件 host：sidecar spawn、JSON-RPC(stdio/WS)、取消/超时/流式、心跳与崩溃检测。
//
// 职责（计划 §4.7）：sidecar spawn、JSON-RPC(stdio/WS)、取消/超时/流式、心跳与崩溃检测。
//
// 关键约束：
// - spawn 前校验二进制签名与哈希
// - maxFrameBytes 上限
// - stdout 仅承载协议帧（日志走 stderr）
// - 单插件并发上限 + 背压
// - 空闲超时 kill
// - 意外退出清理注册表并标 ERRORED
// - abi.rust 校验（crate 版本指纹）
// - 崩溃重启有 per-plugin 次数上限（缺省 3 次/5min）
//
// 本 crate 不依赖 `tauri`：进程管理是抽象的，单元测试用 Mock。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub mod error;
pub mod spawn;
pub mod spawner;

pub use error::{ProcError, ProcResult};
pub use spawn::{ProcRunner, ProcRunnerConfig, ProcessInfo, ProcessState, RpcRequest, RpcResponse, RpcError, ProcProcessStats};
pub use spawner::{CommandSpawner, KillOutcome, ProcSpawner, SpawnedProc};

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

/// JSON-RPC 协议配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcConfig {
    /// 最大帧大小（字节）。
    pub max_frame_bytes: usize,
    /// 是否允许 stdout 承载非协议帧（默认 false）。
    pub allow_stdout_pollution: bool,
    /// 请求超时（毫秒）。
    pub request_timeout_ms: u64,
    /// 流式响应最大帧数。
    pub max_stream_frames: usize,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1024 * 1024, // 1 MB
            allow_stdout_pollution: false,
            request_timeout_ms: 30000, // 30s
            max_stream_frames: 1000,
        }
    }
}

/// 心跳配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    /// 心跳间隔（毫秒）。
    pub interval_ms: u64,
    /// 心跳超时（毫秒）。
    pub timeout_ms: u64,
    /// 连续丢失次数触发崩溃检测。
    pub max_missed: u32,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval_ms: 5000, // 5s
            timeout_ms: 15000, // 15s
            max_missed: 3,
        }
    }
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

/// 进程并发配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencyConfig {
    /// 单插件并发请求上限。
    pub max_concurrent: usize,
    /// 队列上限（背压）。
    pub queue_limit: usize,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 10,
            queue_limit: 100,
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

/// 进程插件完整配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcPluginConfig {
    /// 插件 ID。
    pub plugin_id: String,
    /// spawn 配置。
    pub spawn: SpawnConfig,
    /// JSON-RPC 协议配置。
    pub rpc: RpcConfig,
    /// 心跳配置。
    pub heartbeat: HeartbeatConfig,
    /// 崩溃重启限制。
    pub crash_limit: CrashLimit,
    /// 并发配置。
    pub concurrency: ConcurrencyConfig,
    /// 空闲超时（毫秒）。
    pub idle_timeout_ms: u64,
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

/// 验证 RPC 配置。
pub fn validate_rpc_config(config: &RpcConfig) -> ProcResult<()> {
    if config.max_frame_bytes == 0 {
        return Err(ProcError::InvalidSpawnConfig("max_frame_bytes 不能为 0".into()));
    }
    if config.request_timeout_ms == 0 {
        return Err(ProcError::InvalidSpawnConfig("request_timeout_ms 不能为 0".into()));
    }
    if config.max_stream_frames == 0 {
        return Err(ProcError::InvalidSpawnConfig("max_stream_frames 不能为 0".into()));
    }
    Ok(())
}

/// 验证心跳配置。
pub fn validate_heartbeat_config(config: &HeartbeatConfig) -> ProcResult<()> {
    if config.interval_ms == 0 {
        return Err(ProcError::InvalidSpawnConfig("interval_ms 不能为 0".into()));
    }
    if config.timeout_ms == 0 {
        return Err(ProcError::InvalidSpawnConfig("timeout_ms 不能为 0".into()));
    }
    if config.timeout_ms < config.interval_ms {
        return Err(ProcError::InvalidSpawnConfig(
            "timeout_ms 必须大于等于 interval_ms".into(),
        ));
    }
    if config.max_missed == 0 {
        return Err(ProcError::InvalidSpawnConfig("max_missed 不能为 0".into()));
    }
    Ok(())
}

/// 验证崩溃限制配置。
pub fn validate_crash_limit(config: &CrashLimit) -> ProcResult<()> {
    if config.window_secs == 0 {
        return Err(ProcError::InvalidSpawnConfig("window_secs 不能为 0".into()));
    }
    if config.max_crashes == 0 {
        return Err(ProcError::InvalidSpawnConfig("max_crashes 不能为 0".into()));
    }
    Ok(())
}

/// 验证并发配置。
pub fn validate_concurrency_config(config: &ConcurrencyConfig) -> ProcResult<()> {
    if config.max_concurrent == 0 {
        return Err(ProcError::InvalidSpawnConfig("max_concurrent 不能为 0".into()));
    }
    if config.queue_limit == 0 {
        return Err(ProcError::InvalidSpawnConfig("queue_limit 不能为 0".into()));
    }
    Ok(())
}

/// 验证完整插件配置。
pub fn validate_plugin_config(config: &ProcPluginConfig) -> ProcResult<()> {
    if config.plugin_id.is_empty() {
        return Err(ProcError::InvalidSpawnConfig("plugin_id 不能为空".into()));
    }
    if config.idle_timeout_ms == 0 {
        return Err(ProcError::InvalidSpawnConfig("idle_timeout_ms 不能为 0".into()));
    }
    validate_spawn_config(&config.spawn)?;
    validate_rpc_config(&config.rpc)?;
    validate_heartbeat_config(&config.heartbeat)?;
    validate_crash_limit(&config.crash_limit)?;
    validate_concurrency_config(&config.concurrency)?;
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
        Self {
            records: HashMap::new(),
            limit,
        }
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
        records.push(CrashRecord {
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
// 并发追踪
// ──────────────────────────────────────────────────────────────────────────

/// 并发追踪器。
pub struct ConcurrencyTracker {
    config: ConcurrencyConfig,
    active: HashMap<String, usize>,
    queue: HashMap<String, usize>,
}

impl ConcurrencyTracker {
    /// 创建新的并发追踪器。
    pub fn new(config: ConcurrencyConfig) -> Self {
        Self {
            config,
            active: HashMap::new(),
            queue: HashMap::new(),
        }
    }

    /// 获取当前并发配置。
    pub fn config(&self) -> &ConcurrencyConfig {
        &self.config
    }

    /// 尝试获取并发槽。
    ///
    /// 返回 true 表示成功获取，false 表示已达上限。
    pub fn try_acquire(&mut self, plugin_id: &str) -> bool {
        let current = *self.active.get(plugin_id).unwrap_or(&0);
        if current < self.config.max_concurrent {
            self.active.insert(plugin_id.to_string(), current + 1);
            true
        } else {
            // 检查队列上限
            let queue_len = *self.queue.get(plugin_id).unwrap_or(&0);
            if queue_len < self.config.queue_limit {
                self.queue.insert(plugin_id.to_string(), queue_len + 1);
                true
            } else {
                false
            }
        }
    }

    /// 释放并发槽。
    pub fn release(&mut self, plugin_id: &str) {
        let current = *self.active.get(plugin_id).unwrap_or(&0);
        if current > 0 {
            if current <= 1 {
                self.active.remove(plugin_id);
            } else {
                self.active.insert(plugin_id.to_string(), current - 1);
            }
        }

        // 如果队列中有等待的请求，移到 active
        if let Some(queue_len) = self.queue.get(plugin_id) {
            if *queue_len > 0 && *self.active.get(plugin_id).unwrap_or(&0) < self.config.max_concurrent {
                let new_active = self.active.entry(plugin_id.to_string()).or_insert(0);
                *new_active += 1;
                let new_queue = queue_len - 1;
                if new_queue == 0 {
                    self.queue.remove(plugin_id);
                } else {
                    self.queue.insert(plugin_id.to_string(), new_queue);
                }
            }
        }
    }

    /// 获取当前活跃并发数。
    pub fn active_count(&self, plugin_id: &str) -> usize {
        *self.active.get(plugin_id).unwrap_or(&0)
    }

    /// 获取当前队列长度。
    pub fn queue_length(&self, plugin_id: &str) -> usize {
        *self.queue.get(plugin_id).unwrap_or(&0)
    }

    /// 清除指定插件的追踪。
    pub fn clear(&mut self, plugin_id: &str) {
        self.active.remove(plugin_id);
        self.queue.remove(plugin_id);
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 心跳追踪
// ──────────────────────────────────────────────────────────────────────────

/// 心跳状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HeartbeatState {
    /// 健康。
    Healthy,
    /// 心跳丢失。
    Lost,
    /// 超时。
    Timeout,
}

/// 心跳追踪器。
pub struct HeartbeatTracker {
    config: HeartbeatConfig,
    last_heartbeat: Option<Instant>,
    missed_count: u32,
    state: HeartbeatState,
}

impl HeartbeatTracker {
    /// 创建新的心跳追踪器。
    pub fn new(config: HeartbeatConfig) -> Self {
        Self {
            config,
            last_heartbeat: None,
            missed_count: 0,
            state: HeartbeatState::Healthy,
        }
    }

    /// 获取当前心跳配置。
    pub fn config(&self) -> &HeartbeatConfig {
        &self.config
    }

    /// 记录心跳。
    pub fn heartbeat(&mut self) {
        self.last_heartbeat = Some(Instant::now());
        self.missed_count = 0;
        self.state = HeartbeatState::Healthy;
    }

    /// 检查心跳状态。
    pub fn check(&mut self) -> HeartbeatState {
        let now = Instant::now();
        match self.last_heartbeat {
            None => {
                // 从未收到心跳
                if self.missed_count >= self.config.max_missed {
                    self.state = HeartbeatState::Timeout;
                } else {
                    self.missed_count += 1;
                    self.state = HeartbeatState::Lost;
                }
            }
            Some(last) => {
                let elapsed = now.duration_since(last);
                let timeout = Duration::from_millis(self.config.timeout_ms);
                let interval = Duration::from_millis(self.config.interval_ms);

                if elapsed > timeout {
                    self.state = HeartbeatState::Timeout;
                } else if elapsed > interval {
                    self.missed_count += 1;
                    if self.missed_count >= self.config.max_missed {
                        self.state = HeartbeatState::Timeout;
                    } else {
                        self.state = HeartbeatState::Lost;
                    }
                } else {
                    self.state = HeartbeatState::Healthy;
                }
            }
        }
        self.state.clone()
    }

    /// 获取当前状态。
    pub fn state(&self) -> &HeartbeatState {
        &self.state
    }

    /// 重置追踪器。
    pub fn reset(&mut self) {
        self.last_heartbeat = None;
        self.missed_count = 0;
        self.state = HeartbeatState::Healthy;
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
pub fn validate_abi(
    expected: &AbiFingerprint,
    actual: &AbiFingerprint,
) -> ProcResult<()> {
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

    fn make_plugin_config() -> ProcPluginConfig {
        ProcPluginConfig {
            plugin_id: "test.plugin".into(),
            spawn: make_spawn_config(),
            rpc: RpcConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            crash_limit: CrashLimit::default(),
            concurrency: ConcurrencyConfig::default(),
            idle_timeout_ms: 300000,
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

    #[test]
    fn test_validate_rpc_config_valid() {
        let config = RpcConfig::default();
        assert!(validate_rpc_config(&config).is_ok());
    }

    #[test]
    fn test_validate_rpc_config_zero_max_frame() {
        let mut config = RpcConfig::default();
        config.max_frame_bytes = 0;
        assert!(validate_rpc_config(&config).is_err());
    }

    #[test]
    fn test_validate_heartbeat_config_valid() {
        let config = HeartbeatConfig::default();
        assert!(validate_heartbeat_config(&config).is_ok());
    }

    #[test]
    fn test_validate_heartbeat_config_timeout_less_than_interval() {
        let mut config = HeartbeatConfig::default();
        config.timeout_ms = 1000;
        config.interval_ms = 5000;
        assert!(validate_heartbeat_config(&config).is_err());
    }

    #[test]
    fn test_validate_crash_limit_valid() {
        let config = CrashLimit::default();
        assert!(validate_crash_limit(&config).is_ok());
    }

    #[test]
    fn test_validate_crash_limit_zero_window() {
        let mut config = CrashLimit::default();
        config.window_secs = 0;
        assert!(validate_crash_limit(&config).is_err());
    }

    #[test]
    fn test_validate_concurrency_config_valid() {
        let config = ConcurrencyConfig::default();
        assert!(validate_concurrency_config(&config).is_ok());
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
    fn test_validate_plugin_config_zero_idle_timeout() {
        let mut config = make_plugin_config();
        config.idle_timeout_ms = 0;
        assert!(validate_plugin_config(&config).is_err());
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
        let limit = CrashLimit {
            window_secs: 300,
            max_crashes: 3,
        };
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

    // ── 并发追踪测试 ──

    #[test]
    fn test_concurrency_tracker_acquire() {
        let config = ConcurrencyConfig {
            max_concurrent: 2,
            queue_limit: 5,
        };
        let mut tracker = ConcurrencyTracker::new(config);

        assert!(tracker.try_acquire("test.plugin"));
        assert_eq!(tracker.active_count("test.plugin"), 1);

        assert!(tracker.try_acquire("test.plugin"));
        assert_eq!(tracker.active_count("test.plugin"), 2);
    }

    #[test]
    fn test_concurrency_tracker_queue() {
        let config = ConcurrencyConfig {
            max_concurrent: 1,
            queue_limit: 2,
        };
        let mut tracker = ConcurrencyTracker::new(config);

        assert!(tracker.try_acquire("test.plugin"));
        assert_eq!(tracker.active_count("test.plugin"), 1);

        // 超出并发上限，进入队列
        assert!(tracker.try_acquire("test.plugin"));
        assert_eq!(tracker.queue_length("test.plugin"), 1);

        // 队列未满
        assert!(tracker.try_acquire("test.plugin"));
        assert_eq!(tracker.queue_length("test.plugin"), 2);

        // 超出队列上限
        assert!(!tracker.try_acquire("test.plugin"));
    }

    #[test]
    fn test_concurrency_tracker_release() {
        let config = ConcurrencyConfig {
            max_concurrent: 2,
            queue_limit: 5,
        };
        let mut tracker = ConcurrencyTracker::new(config);

        tracker.try_acquire("test.plugin");
        tracker.try_acquire("test.plugin");
        assert_eq!(tracker.active_count("test.plugin"), 2);

        tracker.release("test.plugin");
        assert_eq!(tracker.active_count("test.plugin"), 1);
    }

    #[test]
    fn test_concurrency_tracker_release_from_queue() {
        let config = ConcurrencyConfig {
            max_concurrent: 1,
            queue_limit: 5,
        };
        let mut tracker = ConcurrencyTracker::new(config);

        tracker.try_acquire("test.plugin");
        assert_eq!(tracker.active_count("test.plugin"), 1);

        // 进入队列
        tracker.try_acquire("test.plugin");
        assert_eq!(tracker.queue_length("test.plugin"), 1);

        // 释放一个，队列中的请求移到 active
        tracker.release("test.plugin");
        assert_eq!(tracker.active_count("test.plugin"), 1);
        assert_eq!(tracker.queue_length("test.plugin"), 0);
    }

    #[test]
    fn test_concurrency_tracker_clear() {
        let config = ConcurrencyConfig::default();
        let mut tracker = ConcurrencyTracker::new(config);

        tracker.try_acquire("test.plugin");
        tracker.clear("test.plugin");
        assert_eq!(tracker.active_count("test.plugin"), 0);
    }

    // ── 心跳追踪测试 ──

    #[test]
    fn test_heartbeat_tracker_healthy() {
        let config = HeartbeatConfig::default();
        let mut tracker = HeartbeatTracker::new(config);

        tracker.heartbeat();
        assert!(matches!(tracker.check(), HeartbeatState::Healthy));
    }

    #[test]
    fn test_heartbeat_tracker_lost() {
        let config = HeartbeatConfig {
            interval_ms: 1,
            timeout_ms: 100,
            max_missed: 2,
        };
        let mut tracker = HeartbeatTracker::new(config);

        // 第一次检查，无心跳
        std::thread::sleep(Duration::from_millis(5));
        let state = tracker.check();
        assert!(matches!(state, HeartbeatState::Lost));

        // 第二次检查，仍无心跳
        std::thread::sleep(Duration::from_millis(5));
        let state = tracker.check();
        assert!(matches!(state, HeartbeatState::Lost));
    }

    #[test]
    fn test_heartbeat_tracker_timeout() {
        let config = HeartbeatConfig {
            interval_ms: 1,
            timeout_ms: 100,
            max_missed: 2,
        };
        let mut tracker = HeartbeatTracker::new(config);

        // 多次检查，累积 missed_count 直到超时
        std::thread::sleep(Duration::from_millis(150));
        tracker.check(); // missed_count = 1
        tracker.check(); // missed_count = 2, 达到 max_missed
        let state = tracker.check(); // 超过 timeout
        assert!(matches!(state, HeartbeatState::Timeout));
    }

    #[test]
    fn test_heartbeat_tracker_reset() {
        let config = HeartbeatConfig {
            interval_ms: 1,
            timeout_ms: 100,
            max_missed: 2,
        };
        let mut tracker = HeartbeatTracker::new(config);

        std::thread::sleep(Duration::from_millis(150));
        tracker.check();
        tracker.reset();
        tracker.heartbeat();
        assert!(matches!(tracker.check(), HeartbeatState::Healthy));
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
