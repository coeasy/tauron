// 进程插件执行器：sidecar spawn、JSON-RPC 通信、心跳监控、崩溃恢复。
//
// 职责（计划 §4.7）：
// - sidecar spawn（模拟）
// - JSON-RPC 通信
// - 心跳监控
// - 崩溃恢复
// - 并发控制
//
// 本模块不依赖 `tauri`：进程管理是抽象的，单元测试用 Mock。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    ConcurrencyTracker, CrashLimit, CrashTracker, HeartbeatConfig, HeartbeatState,
    HeartbeatTracker, ProcError, ProcPluginConfig, ProcResult, RpcConfig, validate_plugin_config,
};

// ──────────────────────────────────────────────────────────────────────────
// 进程状态
// ──────────────────────────────────────────────────────────────────────────

/// 进程状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessState {
    /// 未启动。
    Stopped,
    /// 启动中。
    Starting,
    /// 运行中。
    Running,
    /// 停止中。
    Stopping,
    /// 已崩溃。
    Crashed,
}

/// 进程信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    /// 进程 ID。
    pub process_id: String,
    /// 插件 ID。
    pub plugin_id: String,
    /// 进程状态。
    pub state: ProcessState,
    /// 进程退出码。
    pub exit_code: Option<i32>,
    /// 启动时间。
    pub started_at: Option<DateTime<Utc>>,
    /// 停止时间。
    pub stopped_at: Option<DateTime<Utc>>,
    /// 崩溃次数。
    pub crash_count: u32,
    /// 最后心跳时间。
    pub last_heartbeat: Option<DateTime<Utc>>,
    /// 二进制路径。
    pub binary_path: String,
    /// 参数列表。
    pub args: Vec<String>,
}

// ──────────────────────────────────────────────────────────────────────────
// JSON-RPC 消息
// ──────────────────────────────────────────────────────────────────────────

/// JSON-RPC 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRequest {
    /// JSON-RPC 版本。
    pub jsonrpc: String,
    /// 方法名。
    pub method: String,
    /// 参数。
    pub params: serde_json::Value,
    /// 请求 ID。
    pub id: u64,
}

/// JSON-RPC 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse {
    /// JSON-RPC 版本。
    pub jsonrpc: String,
    /// 响应 ID。
    pub id: u64,
    /// 结果。
    pub result: Option<serde_json::Value>,
    /// 错误。
    pub error: Option<RpcError>,
}

/// JSON-RPC 错误。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    /// 错误码。
    pub code: i32,
    /// 错误消息。
    pub message: String,
    /// 错误数据。
    pub data: Option<serde_json::Value>,
}

// ──────────────────────────────────────────────────────────────────────────
// 进程执行器
// ──────────────────────────────────────────────────────────────────────────

/// 进程执行器配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcRunnerConfig {
    /// RPC 配置。
    pub rpc: RpcConfig,
    /// 心跳配置。
    pub heartbeat: HeartbeatConfig,
    /// 崩溃限制。
    pub crash_limit: CrashLimit,
}

impl Default for ProcRunnerConfig {
    fn default() -> Self {
        Self {
            rpc: RpcConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            crash_limit: CrashLimit::default(),
        }
    }
}

/// 进程执行器。
pub struct ProcRunner {
    config: ProcRunnerConfig,
    processes: HashMap<String, ProcessInfo>,
    crash_tracker: CrashTracker,
    concurrency_tracker: ConcurrencyTracker,
    heartbeat_tracker: HeartbeatTracker,
    next_request_id: u64,
    next_process_id: u32,
}

impl ProcRunner {
    /// 创建新的进程执行器。
    pub fn new(config: ProcRunnerConfig) -> Self {
        let heartbeat_tracker = HeartbeatTracker::new(config.heartbeat.clone());
        let concurrency_tracker = ConcurrencyTracker::new(Default::default());
        let crash_tracker = CrashTracker::new(config.crash_limit.clone());

        Self {
            config,
            processes: HashMap::new(),
            crash_tracker,
            concurrency_tracker,
            heartbeat_tracker,
            next_request_id: 1,
            next_process_id: 1,
        }
    }

    /// 使用默认配置创建。
    pub fn default_runner() -> Self {
        Self::new(ProcRunnerConfig::default())
    }

    /// 获取当前配置。
    pub fn config(&self) -> &ProcRunnerConfig {
        &self.config
    }

    /// 获取进程数量。
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    /// 获取进程信息。
    pub fn get_process(&self, plugin_id: &str) -> Option<&ProcessInfo> {
        self.processes.get(plugin_id)
    }

    /// 获取崩溃追踪器。
    pub fn crash_tracker(&self) -> &CrashTracker {
        &self.crash_tracker
    }

    /// 获取并发追踪器。
    pub fn concurrency_tracker(&self) -> &ConcurrencyTracker {
        &self.concurrency_tracker
    }

    /// 获取心跳追踪器。
    pub fn heartbeat_tracker(&self) -> &HeartbeatTracker {
        &self.heartbeat_tracker
    }

    /// 检查进程是否存在。
    pub fn has_process(&self, plugin_id: &str) -> bool {
        self.processes.contains_key(plugin_id)
    }

    /// 检查进程是否运行中。
    pub fn is_running(&self, plugin_id: &str) -> bool {
        self.processes
            .get(plugin_id)
            .map(|p| p.state == ProcessState::Running)
            .unwrap_or(false)
    }

    /// 启动 sidecar 进程。
    pub fn spawn(&mut self, plugin_config: &ProcPluginConfig) -> ProcResult<ProcessInfo> {
        // 1. 验证配置
        validate_plugin_config(plugin_config)?;

        // 2. 检查崩溃限制
        if self.crash_tracker.is_exceeded(&plugin_config.plugin_id) {
            return Err(ProcError::CrashLimitExceeded {
                plugin_id: plugin_config.plugin_id.clone(),
            });
        }

        // 3. 检查进程是否已存在
        if let Some(existing) = self.processes.get(&plugin_config.plugin_id) {
            if existing.state == ProcessState::Running {
                return Err(ProcError::ProcessTerminated(
                    format!("进程已存在：{}", plugin_config.plugin_id),
                ));
            }
        }

        // 4. 创建进程信息
        let process_id = format!("proc-{}", self.next_process_id);
        self.next_process_id += 1;

        let process_info = ProcessInfo {
            process_id: process_id.clone(),
            plugin_id: plugin_config.plugin_id.clone(),
            state: ProcessState::Starting,
            exit_code: None,
            started_at: Some(Utc::now()),
            stopped_at: None,
            crash_count: 0,
            last_heartbeat: None,
            binary_path: plugin_config.spawn.binary_path.clone(),
            args: plugin_config.spawn.args.clone(),
        };

        // 5. 模拟启动过程
        // 实际实现会调用 std::process::Command::new()
        let mut started_process = process_info.clone();
        started_process.state = ProcessState::Running;
        started_process.last_heartbeat = Some(Utc::now());

        // 6. 初始化心跳追踪器
        self.heartbeat_tracker.heartbeat();

        // 7. 存储进程信息
        self.processes
            .insert(plugin_config.plugin_id.clone(), started_process.clone());

        Ok(started_process)
    }

    /// 停止 sidecar 进程。
    pub fn stop(&mut self, plugin_id: &str) -> ProcResult<ProcessInfo> {
        let process = self
            .processes
            .get_mut(plugin_id)
            .ok_or_else(|| {
                ProcError::ProcessTerminated(format!("进程不存在：{}", plugin_id))
            })?;

        process.state = ProcessState::Stopping;
        process.stopped_at = Some(Utc::now());

        let process_info = process.clone();

        // 实际实现会发送终止信号并等待进程退出
        process.state = ProcessState::Stopped;
        process.exit_code = Some(0);

        Ok(process_info)
    }

    /// 杀死 sidecar 进程。
    pub fn kill(&mut self, plugin_id: &str) -> ProcResult<ProcessInfo> {
        let process = self
            .processes
            .remove(plugin_id)
            .ok_or_else(|| {
                ProcError::ProcessTerminated(format!("进程不存在：{}", plugin_id))
            })?;

        let mut killed = process.clone();
        killed.state = ProcessState::Stopped;
        killed.stopped_at = Some(Utc::now());
        killed.exit_code = Some(-1);

        Ok(killed)
    }

    /// 发送 RPC 请求。
    pub fn call(
        &mut self,
        plugin_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> ProcResult<RpcResponse> {
        // 1. 检查进程是否运行中
        if !self.is_running(plugin_id) {
            return Err(ProcError::ProcessTerminated(
                format!("进程未运行：{}", plugin_id),
            ));
        }

        // 2. 检查并发限制
        if !self.concurrency_tracker.try_acquire(plugin_id) {
            return Err(ProcError::ConcurrencyLimit {
                limit: self.concurrency_tracker.config().max_concurrent,
            });
        }

        // 3. 构造 RPC 请求
        let request_id = self.next_request_id;
        self.next_request_id += 1;

        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: params.clone(),
            id: request_id,
        };

        // 4. 模拟 RPC 调用
        // 实际实现会写入 stdout 并等待响应；此处回显请求信封，保持线格式与真实
        // JSON-RPC 一致（响应里能看到它应答的是哪个请求）。
        let response = RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request_id,
            result: Some(serde_json::json!({
                "method": method,
                "params": params,
                "request": request,
                "timestamp": Utc::now().to_rfc3339()
            })),
            error: None,
        };

        // 5. 释放并发槽
        self.concurrency_tracker.release(plugin_id);

        Ok(response)
    }

    /// 发送 RPC 请求（带超时）。
    pub fn call_with_timeout(
        &mut self,
        plugin_id: &str,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> ProcResult<RpcResponse> {
        let start = Instant::now();

        // 模拟超时检查
        if start.elapsed() > Duration::from_millis(timeout_ms) {
            return Err(ProcError::Timeout(format!(
                "RPC 调用超时：{}ms",
                timeout_ms
            )));
        }

        self.call(plugin_id, method, params)
    }

    /// 发送心跳。
    pub fn send_heartbeat(&mut self, plugin_id: &str) -> ProcResult<()> {
        if !self.is_running(plugin_id) {
            return Err(ProcError::ProcessTerminated(
                format!("进程未运行：{}", plugin_id),
            ));
        }

        self.heartbeat_tracker.heartbeat();

        if let Some(process) = self.processes.get_mut(plugin_id) {
            process.last_heartbeat = Some(Utc::now());
        }

        Ok(())
    }

    /// 检查心跳状态，**只**影响 `plugin_id` 指定的那个进程。
    ///
    /// ⚠️ 此前这里无差别遍历 `self.processes`，把**所有** `Running` 进程一并标成
    /// `Crashed`（`exit_code = -2`）。后果是：给插件 A 做一次心跳检查，会把健康
    /// 的插件 B、C 一起判死——它们随后被回收，宿主侧表现为"莫名其妙少了一批
    /// sidecar"。心跳检查必须是**按插件作用域**的。
    ///
    /// **诚实边界**：`heartbeat_tracker` 目前仍是**全局单例**（不按插件分账），
    /// 因此"任一插件发心跳即刷新全局计时"这一语义限制依然存在——本函数修掉的是
    /// **误伤**（作用域），不是**共享计时器**（建模）。要做到真正的按插件心跳，
    /// 需要把 `HeartbeatTracker` 改成 `HashMap<plugin_id, HeartbeatTracker>`，
    /// 属独立改动，见 CHANGELOG「已知债务」。
    pub fn check_heartbeat(&mut self, plugin_id: &str) -> ProcResult<HeartbeatState> {
        let state = self.heartbeat_tracker.check();

        if matches!(state, HeartbeatState::Timeout) {
            // 心跳超时，只标记目标进程崩溃（不波及其他插件）。
            if let Some(process) = self.processes.get_mut(plugin_id) {
                if process.state == ProcessState::Running {
                    process.state = ProcessState::Crashed;
                    process.exit_code = Some(-2);
                }
            }
        }

        Ok(state)
    }

    /// 处理崩溃恢复。
    pub fn handle_crash(&mut self, plugin_id: &str) -> ProcResult<Option<ProcessInfo>> {
        // 1. 记录崩溃
        let within_limit = self.crash_tracker.record_crash(plugin_id);

        if !within_limit {
            // 超过崩溃限制
            return Err(ProcError::CrashLimitExceeded {
                plugin_id: plugin_id.to_string(),
            });
        }

        // 2. 标记进程崩溃
        if let Some(process) = self.processes.get_mut(plugin_id) {
            if process.state == ProcessState::Running {
                process.state = ProcessState::Crashed;
                process.exit_code = Some(-3);
                process.crash_count += 1;
            }
        }

        // 3. 返回 None（不自动重启）
        Ok(None)
    }

    /// 重启进程。
    pub fn restart(&mut self, plugin_config: &ProcPluginConfig) -> ProcResult<ProcessInfo> {
        // 1. 停止现有进程
        if self.has_process(&plugin_config.plugin_id) {
            let _ = self.kill(&plugin_config.plugin_id);
        }

        // 2. 清除崩溃记录
        self.crash_tracker.clear(&plugin_config.plugin_id);

        // 3. 启动新进程
        self.spawn(plugin_config)
    }

    /// 停止所有进程。
    pub fn stop_all(&mut self) -> Vec<ProcessInfo> {
        let mut stopped = Vec::new();
        let plugin_ids: Vec<String> = self.processes.keys().cloned().collect();
        for plugin_id in plugin_ids {
            if let Ok(info) = self.kill(&plugin_id) {
                stopped.push(info);
            }
        }
        stopped
    }

    /// 清除指定进程。
    pub fn clear_process(&mut self, plugin_id: &str) -> Option<ProcessInfo> {
        self.processes.remove(plugin_id)
    }

    /// 获取所有进程信息。
    pub fn list_processes(&self) -> Vec<&ProcessInfo> {
        self.processes.values().collect()
    }

    /// 获取进程统计信息。
    pub fn process_stats(&self, plugin_id: &str) -> ProcProcessStats {
        let process = self.processes.get(plugin_id);
        ProcProcessStats {
            plugin_id: plugin_id.to_string(),
            exists: process.is_some(),
            state: process.map(|p| format!("{:?}", p.state)).unwrap_or("N/A".to_string()),
            crash_count: self.crash_tracker.crash_count(plugin_id),
            is_crash_exceeded: self.crash_tracker.is_exceeded(plugin_id),
            active_concurrency: self.concurrency_tracker.active_count(plugin_id),
            queue_length: self.concurrency_tracker.queue_length(plugin_id),
            last_heartbeat: process.and_then(|p| p.last_heartbeat).map(|t| t.to_rfc3339()),
        }
    }
}

/// 进程统计信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcProcessStats {
    /// 插件 ID。
    pub plugin_id: String,
    /// 进程是否存在。
    pub exists: bool,
    /// 进程状态。
    pub state: String,
    /// 崩溃次数。
    pub crash_count: u32,
    /// 是否崩溃超限。
    pub is_crash_exceeded: bool,
    /// 活跃并发数。
    pub active_concurrency: usize,
    /// 队列长度。
    pub queue_length: usize,
    /// 最后心跳时间。
    pub last_heartbeat: Option<String>,
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AbiFingerprint, BinarySignature, ConcurrencyConfig, SpawnConfig};

    fn make_spawn_config() -> SpawnConfig {
        SpawnConfig {
            binary_path: "/path/to/sidecar".into(),
            args: vec!["--flag".into()],
            env: HashMap::new(),
            signature: BinarySignature {
                algorithm: "ed25519".into(),
                signature: "abc123".into(),
                signer_id: "signer-001".into(),
            },
            binary_hash: "a".repeat(64),
            abi: AbiFingerprint {
                rust_version: "1.98.0".into(),
                interface_hash: "hash123".into(),
                generated_at: Utc::now(),
            },
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

    // ── 执行器创建测试 ──

    #[test]
    fn test_runner_create() {
        let runner = ProcRunner::default_runner();
        assert_eq!(runner.process_count(), 0);
    }

    #[test]
    fn test_runner_custom_config() {
        let config = ProcRunnerConfig {
            rpc: RpcConfig {
                max_frame_bytes: 2048,
                ..Default::default()
            },
            ..Default::default()
        };
        let runner = ProcRunner::new(config);
        assert_eq!(runner.config().rpc.max_frame_bytes, 2048);
    }

    // ── spawn 测试 ──

    #[test]
    fn test_spawn_success() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        let result = runner.spawn(&config);
        assert!(result.is_ok());
        let process = result.unwrap();
        assert_eq!(process.plugin_id, "test.plugin");
        assert_eq!(process.state, ProcessState::Running);
        assert!(process.started_at.is_some());
        assert!(runner.has_process("test.plugin"));
        assert!(runner.is_running("test.plugin"));
    }

    #[test]
    fn test_spawn_duplicate() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();
        let result = runner.spawn(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_spawn_crash_limit_exceeded() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();

        // 手动记录崩溃超限
        for _ in 0..4 {
            runner.crash_tracker.record_crash("test.plugin");
        }

        let result = runner.spawn(&config);
        assert!(result.is_err());
    }

    // ── stop 测试 ──

    #[test]
    fn test_stop_success() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let result = runner.stop("test.plugin");
        assert!(result.is_ok());
        assert!(!runner.is_running("test.plugin"));
    }

    #[test]
    fn test_stop_nonexistent() {
        let mut runner = ProcRunner::default_runner();
        let result = runner.stop("nonexistent");
        assert!(result.is_err());
    }

    // ── kill 测试 ──

    #[test]
    fn test_kill_success() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let result = runner.kill("test.plugin");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().exit_code, Some(-1));
    }

    // ── RPC 调用测试 ──

    #[test]
    fn test_call_success() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let params = serde_json::json!({"key": "value"});
        let result = runner.call("test.plugin", "test_method", params);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.jsonrpc, "2.0");
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_call_process_not_running() {
        let mut runner = ProcRunner::default_runner();
        let params = serde_json::json!({});
        let result = runner.call("nonexistent", "test_method", params);
        assert!(result.is_err());
    }

    #[test]
    fn test_call_with_timeout() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let params = serde_json::json!({});
        let result = runner.call_with_timeout("test.plugin", "test_method", params, 1000);
        assert!(result.is_ok());
    }

    // ── 心跳测试 ──

    #[test]
    fn test_send_heartbeat() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let result = runner.send_heartbeat("test.plugin");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_heartbeat_healthy() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let state = runner.check_heartbeat("test.plugin").unwrap();
        assert_eq!(state, HeartbeatState::Healthy);
    }

    /// 心跳超时**不得**波及其他插件的进程。
    ///
    /// 回归保护：此前 `check_heartbeat()` 无差别把所有 `Running` 进程标成
    /// `Crashed`，给 A 检查一次心跳会把健康的 B 一起判死。
    #[test]
    fn test_check_heartbeat_timeout_does_not_kill_other_plugins() {
        let mut runner = ProcRunner::default_runner();
        let a = make_plugin_config();
        let mut b = a.clone();
        b.plugin_id = "other.plugin".into();
        runner.spawn(&a).unwrap();
        runner.spawn(&b).unwrap();
        assert!(runner.is_running("test.plugin"));
        assert!(runner.is_running("other.plugin"));

        // 把 tracker 摆成 Timeout（缺省 `timeout_ms` 15s，真等太慢），然后只对 A 检查。
        // `spawn()` 会把 `last_heartbeat` 置为"现在"，故必须显式推进，不能靠 sleep。
        runner.heartbeat_tracker.force_timeout();
        let state = runner.check_heartbeat("test.plugin").unwrap();
        assert_eq!(state, HeartbeatState::Timeout);

        assert!(
            !runner.is_running("test.plugin"),
            "被检查的插件应被判死"
        );
        assert!(
            runner.is_running("other.plugin"),
            "未参与检查的插件不得被连带判死"
        );
    }

    // ── 崩溃处理测试 ──

    #[test]
    fn test_handle_crash_within_limit() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let result = runner.handle_crash("test.plugin");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_handle_crash_exceed_limit() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        // 记录 3 次崩溃
        for _ in 0..3 {
            runner.crash_tracker.record_crash("test.plugin");
        }

        // 第 4 次超限
        let result = runner.handle_crash("test.plugin");
        assert!(result.is_err());
    }

    // ── 重启测试 ──

    #[test]
    fn test_restart_success() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        // 模拟崩溃
        runner.crash_tracker.record_crash("test.plugin");

        let result = runner.restart(&config);
        assert!(result.is_ok());
        assert!(runner.is_running("test.plugin"));
    }

    // ── stop_all 测试 ──

    #[test]
    fn test_stop_all() {
        let mut runner = ProcRunner::default_runner();
        let config1 = make_plugin_config();
        let mut config2 = make_plugin_config();
        config2.plugin_id = "plugin.b".into();

        runner.spawn(&config1).unwrap();
        runner.spawn(&config2).unwrap();

        let stopped = runner.stop_all();
        assert_eq!(stopped.len(), 2);
        assert_eq!(runner.process_count(), 0);
    }

    // ── clear_process 测试 ──

    #[test]
    fn test_clear_process() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let cleared = runner.clear_process("test.plugin");
        assert!(cleared.is_some());
        assert!(!runner.has_process("test.plugin"));
    }

    // ── list_processes 测试 ──

    #[test]
    fn test_list_processes() {
        let mut runner = ProcRunner::default_runner();
        let config1 = make_plugin_config();
        let mut config2 = make_plugin_config();
        config2.plugin_id = "plugin.b".into();

        runner.spawn(&config1).unwrap();
        runner.spawn(&config2).unwrap();

        let processes = runner.list_processes();
        assert_eq!(processes.len(), 2);
    }

    // ── 统计测试 ──

    #[test]
    fn test_process_stats() {
        let mut runner = ProcRunner::default_runner();
        let config = make_plugin_config();
        runner.spawn(&config).unwrap();

        let stats = runner.process_stats("test.plugin");
        assert_eq!(stats.plugin_id, "test.plugin");
        assert!(stats.exists);
        assert_eq!(stats.state, "Running");
        assert_eq!(stats.crash_count, 0);
        assert!(!stats.is_crash_exceeded);
    }

    #[test]
    fn test_process_stats_nonexistent() {
        let runner = ProcRunner::default_runner();
        let stats = runner.process_stats("nonexistent");
        assert!(!stats.exists);
        assert_eq!(stats.state, "N/A");
    }
}
