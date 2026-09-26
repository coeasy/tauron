// §4.19 CI 分发运维：更新通道、灰度批次、崩溃率停发、签名校验。
//
// 职责（计划 §4.19）：多平台矩阵构建 + 代码签名 + mac 公证 +
// 更新产物 + 双 endpoint 发布 + 门禁 + 发布演练。
//
// 关键约束：
// - `latest.json` 无百分比/无 channel → 灰度必须自建动态 endpoint；
// - 纯静态源明示"全量发布"；
// - 批次 `1%→5%→25%→100%` + 每批最小停留 + 崩溃率自动停发；
// - 签名密钥仅存 CI secrets；框架更新根密钥离线 + 双人持有。
//
// 本 crate 不依赖 `tauri`：endpoint 客户端是抽象的 `EndpointClient` trait，
// 真实 HTTP 由适配层实现，单元测试用 Mock。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub mod error;
pub mod upgrade;

pub use error::{DistributeError, DistributeResult};
pub use upgrade::{
    create_default_upgrade_runner, create_upgrade_runner, Downloader, MockDownloader, MockVerifier,
    SignatureVerifier, UpgradeOptions, UpgradeProgress, UpgradeResult, UpgradeRunner, UpgradeState,
};

/// 更新清单（latest.json 格式）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateManifest {
    /// 版本号。
    pub version: String,
    /// 下载 URL。
    pub url: String,
    /// 签名（hex 编码）。
    pub signature: String,
    /// 发布日期（ISO 8601）。
    pub release_date: String,
    /// 平台 → 平台特定信息。
    #[serde(default)]
    pub platform_notes: BTreeMap<String, String>,
}

/// 灰度批次。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrayscaleBatch {
    /// 1% 用户。
    Batch1,
    /// 5% 用户。
    Batch5,
    /// 25% 用户。
    Batch25,
    /// 100% 用户（全量）。
    Batch100,
}

impl GrayscaleBatch {
    pub const ALL: [GrayscaleBatch; 4] = [
        GrayscaleBatch::Batch1,
        GrayscaleBatch::Batch5,
        GrayscaleBatch::Batch25,
        GrayscaleBatch::Batch100,
    ];

    pub fn percentage(self) -> u32 {
        match self {
            GrayscaleBatch::Batch1 => 1,
            GrayscaleBatch::Batch5 => 5,
            GrayscaleBatch::Batch25 => 25,
            GrayscaleBatch::Batch100 => 100,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GrayscaleBatch::Batch1 => "1%",
            GrayscaleBatch::Batch5 => "5%",
            GrayscaleBatch::Batch25 => "25%",
            GrayscaleBatch::Batch100 => "100%",
        }
    }

    /// 下一批次（100% 无下一批）。
    pub fn next(self) -> Option<GrayscaleBatch> {
        match self {
            GrayscaleBatch::Batch1 => Some(GrayscaleBatch::Batch5),
            GrayscaleBatch::Batch5 => Some(GrayscaleBatch::Batch25),
            GrayscaleBatch::Batch25 => Some(GrayscaleBatch::Batch100),
            GrayscaleBatch::Batch100 => None,
        }
    }
}

impl std::fmt::Display for GrayscaleBatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 灰度发布策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrayscalePolicy {
    /// 当前批次。
    pub current: GrayscaleBatch,
    /// 进入当前批次的时间戳（epoch 秒）。
    pub entered_at: u64,
    /// 每批最小停留时间（秒）。
    pub min_dwell_seconds: u64,
}

impl Default for GrayscalePolicy {
    fn default() -> Self {
        Self {
            current: GrayscaleBatch::Batch1,
            entered_at: 0,
            min_dwell_seconds: DEFAULT_MIN_DWELL_SECONDS,
        }
    }
}

/// 默认最小停留时间（秒）：1 小时。
pub const DEFAULT_MIN_DWELL_SECONDS: u64 = 3600;

impl GrayscalePolicy {
    /// 当前批次是否允许推进（到达最小停留时间）。
    pub fn can_advance(&self, now: u64) -> bool {
        if self.current == GrayscaleBatch::Batch100 {
            return false; // 全量无下一批。
        }
        now.saturating_sub(self.entered_at) >= self.min_dwell_seconds
    }

    /// 推进到下一批次。
    /// 返回 `Ok(Some(new_batch))` 表示推进成功；
    /// `Ok(None)` 表示已是全量；
    /// `Err` 表示未达到最小停留时间。
    pub fn advance(&mut self, now: u64) -> DistributeResult<Option<GrayscaleBatch>> {
        if self.current == GrayscaleBatch::Batch100 {
            return Ok(None);
        }
        if !self.can_advance(now) {
            return Err(DistributeError::GrayscaleNotReady {
                current: self.current.as_str().to_string(),
                elapsed: now.saturating_sub(self.entered_at),
                required: self.min_dwell_seconds,
            });
        }
        let new_batch = self.current.next().ok_or_else(|| DistributeError::GrayscaleNotReady {
            current: self.current.as_str().to_string(),
            elapsed: now.saturating_sub(self.entered_at),
            required: self.min_dwell_seconds,
        })?;
        self.current = new_batch;
        self.entered_at = now;
        Ok(Some(new_batch))
    }

    /// 当前批次的百分比。
    pub fn current_percentage(&self) -> u32 {
        self.current.percentage()
    }

    /// 回退到指定批次（崩溃率停发时回退）。
    pub fn rollback(&mut self, batch: GrayscaleBatch, now: u64) {
        self.current = batch;
        self.entered_at = now;
    }
}

/// 崩溃率门禁。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashGate {
    /// 崩溃率阈值（百分比，如 5.0 表示 5%）。
    pub threshold_percent: f64,
    /// 当前崩溃率。
    pub current_percent: f64,
    /// 是否已触发停发。
    pub stopped: bool,
}

impl Default for CrashGate {
    fn default() -> Self {
        Self { threshold_percent: 5.0, current_percent: 0.0, stopped: false }
    }
}

impl CrashGate {
    /// 更新崩溃率。
    /// 返回 `true` 表示触发停发。
    pub fn update(&mut self, crash_count: u32, total_count: u32) -> bool {
        if total_count == 0 {
            self.current_percent = 0.0;
            return false;
        }
        self.current_percent = (crash_count as f64 / total_count as f64) * 100.0;
        if self.current_percent >= self.threshold_percent {
            self.stopped = true;
            true
        } else {
            self.stopped = false;
            false
        }
    }

    /// 是否已停发。
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// 重置停发（手动恢复）。
    pub fn reset(&mut self) {
        self.stopped = false;
    }
}

/// 更新检查响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheckResult {
    /// 有新版本。
    UpdateAvailable(UpdateManifest),
    /// 已是最新版。
    UpToDate,
    /// 端点不可用（404/500）。
    EndpointUnavailable(String),
    /// 签名校验失败。
    SignatureInvalid,
    /// 响应体格式错误（2xx 但非法 JSON）。
    InvalidBody(String),
    /// 灰度未覆盖此用户。
    NotInGrayscale,
}

/// Endpoint 客户端抽象。
pub trait EndpointClient: Send + Sync {
    /// 获取更新清单。
    /// 返回 `Ok(Some(manifest))` 表示有新版本；
    /// `Ok(None)` 表示已是最新；
    /// `Err` 表示错误（由调用方处理）。
    fn fetch_manifest(&self, current_version: &str) -> DistributeResult<Option<UpdateManifest>>;
}

/// 检查是否有更新（组合 endpoint + 签名 + 灰度）。
pub fn check_for_update(
    client: &dyn EndpointClient,
    current_version: &str,
    grayscale: &GrayscalePolicy,
    user_hash: u32,
) -> DistributeResult<UpdateCheckResult> {
    let Some(manifest) = client.fetch_manifest(current_version)? else {
        return Ok(UpdateCheckResult::UpToDate);
    };

    // 签名校验（当前为占位，真实实现验证数字签名）。
    if manifest.signature.is_empty() {
        return Ok(UpdateCheckResult::SignatureInvalid);
    }

    // 灰度检查。
    let percentage = grayscale.current_percentage();
    if percentage < 100 {
        // 用户 hash 取模 100 决定是否覆盖。
        if user_hash % 100 >= percentage {
            return Ok(UpdateCheckResult::NotInGrayscale);
        }
    }

    Ok(UpdateCheckResult::UpdateAvailable(manifest))
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manifest(version: &str) -> UpdateManifest {
        UpdateManifest {
            version: version.into(),
            url: format!("https://example.com/{version}.zip"),
            signature: "abc123def456".into(),
            release_date: "2026-09-21T00:00:00Z".into(),
            platform_notes: BTreeMap::new(),
        }
    }

    // ── GrayscaleBatch ───────────────────────────────────────────────

    #[test]
    fn batch_percentages() {
        assert_eq!(GrayscaleBatch::Batch1.percentage(), 1);
        assert_eq!(GrayscaleBatch::Batch5.percentage(), 5);
        assert_eq!(GrayscaleBatch::Batch25.percentage(), 25);
        assert_eq!(GrayscaleBatch::Batch100.percentage(), 100);
    }

    #[test]
    fn batch_next_progression() {
        assert_eq!(GrayscaleBatch::Batch1.next(), Some(GrayscaleBatch::Batch5));
        assert_eq!(GrayscaleBatch::Batch5.next(), Some(GrayscaleBatch::Batch25));
        assert_eq!(GrayscaleBatch::Batch25.next(), Some(GrayscaleBatch::Batch100));
        assert_eq!(GrayscaleBatch::Batch100.next(), None);
    }

    #[test]
    fn batch_serde_roundtrip() {
        for b in GrayscaleBatch::ALL {
            let v = serde_json::to_value(b).unwrap();
            let b2 = serde_json::from_value::<GrayscaleBatch>(v).unwrap();
            assert_eq!(b, b2);
        }
    }

    // ── GrayscalePolicy ──────────────────────────────────────────────

    #[test]
    fn grayscale_initial_batch_is_1_percent() {
        let p = GrayscalePolicy::default();
        assert_eq!(p.current, GrayscaleBatch::Batch1);
        assert_eq!(p.current_percentage(), 1);
    }

    #[test]
    fn grayscale_cannot_advance_before_dwell_time() {
        let mut p =
            GrayscalePolicy { entered_at: 1000, min_dwell_seconds: 3600, ..Default::default() };
        // 1 小时未到。
        assert!(!p.can_advance(1000 + 1800));
        assert!(matches!(p.advance(1000 + 1800), Err(DistributeError::GrayscaleNotReady { .. })));
    }

    #[test]
    fn grayscale_advances_after_dwell_time() {
        let mut p =
            GrayscalePolicy { entered_at: 0, min_dwell_seconds: 3600, ..Default::default() };
        // 1 小时后。
        assert!(p.can_advance(3600));
        let result = p.advance(3601).unwrap();
        assert_eq!(result, Some(GrayscaleBatch::Batch5));
        assert_eq!(p.current_percentage(), 5);
    }

    #[test]
    fn grayscale_full_progression() {
        let mut p = GrayscalePolicy {
            entered_at: 0,
            min_dwell_seconds: 0, // 测试用 0 停留。
            ..Default::default()
        };
        assert_eq!(p.advance(1).unwrap(), Some(GrayscaleBatch::Batch5));
        assert_eq!(p.advance(2).unwrap(), Some(GrayscaleBatch::Batch25));
        assert_eq!(p.advance(3).unwrap(), Some(GrayscaleBatch::Batch100));
        assert_eq!(p.advance(4).unwrap(), None, "全量无下一批");
    }

    #[test]
    fn grayscale_rollback() {
        let mut p = GrayscalePolicy {
            current: GrayscaleBatch::Batch25,
            entered_at: 1000,
            min_dwell_seconds: 3600,
        };
        p.rollback(GrayscaleBatch::Batch1, 2000);
        assert_eq!(p.current, GrayscaleBatch::Batch1);
        assert_eq!(p.entered_at, 2000);
    }

    #[test]
    fn grayscale_full_percentage_prevents_advance() {
        let mut p = GrayscalePolicy {
            current: GrayscaleBatch::Batch100,
            entered_at: 0,
            min_dwell_seconds: 0,
        };
        assert!(!p.can_advance(999999));
        assert_eq!(p.advance(999999).unwrap(), None);
    }

    #[test]
    fn grayscale_serde_roundtrip() {
        let p = GrayscalePolicy {
            current: GrayscaleBatch::Batch5,
            entered_at: 12345,
            min_dwell_seconds: 7200,
        };
        let v = serde_json::to_value(&p).unwrap();
        let p2 = serde_json::from_value::<GrayscalePolicy>(v).unwrap();
        assert_eq!(p.current, p2.current);
        assert_eq!(p.entered_at, p2.entered_at);
        assert_eq!(p.min_dwell_seconds, p2.min_dwell_seconds);
    }

    // ── CrashGate ────────────────────────────────────────────────────

    #[test]
    fn crash_gate_allows_below_threshold() {
        let mut gate = CrashGate { threshold_percent: 5.0, ..Default::default() };
        // 1/100 = 1%。
        let stopped = gate.update(1, 100);
        assert!(!stopped);
        assert!(!gate.is_stopped());
        assert!((gate.current_percent - 1.0).abs() < 1e-9);
    }

    #[test]
    fn crash_gate_stops_at_threshold() {
        let mut gate = CrashGate { threshold_percent: 5.0, ..Default::default() };
        // 5/100 = 5%。
        let stopped = gate.update(5, 100);
        assert!(stopped);
        assert!(gate.is_stopped());
    }

    #[test]
    fn crash_gate_stops_above_threshold() {
        let mut gate = CrashGate { threshold_percent: 5.0, ..Default::default() };
        // 10/100 = 10%。
        let stopped = gate.update(10, 100);
        assert!(stopped);
    }

    #[test]
    fn crash_gate_zero_total_does_not_stop() {
        let mut gate = CrashGate::default();
        let stopped = gate.update(0, 0);
        assert!(!stopped);
        assert_eq!(gate.current_percent, 0.0);
    }

    #[test]
    fn crash_gate_reset() {
        let mut gate = CrashGate { threshold_percent: 5.0, ..Default::default() };
        gate.update(10, 100); // 触发停发。
        assert!(gate.is_stopped());
        gate.reset();
        assert!(!gate.is_stopped());
    }

    #[test]
    fn crash_gate_serde_roundtrip() {
        let gate = CrashGate { threshold_percent: 3.5, current_percent: 2.0, stopped: true };
        let v = serde_json::to_value(&gate).unwrap();
        let g2 = serde_json::from_value::<CrashGate>(v).unwrap();
        assert_eq!(g2.threshold_percent, 3.5);
        assert_eq!(g2.current_percent, 2.0);
        assert!(g2.stopped);
    }

    // ── UpdateManifest ───────────────────────────────────────────────

    #[test]
    fn manifest_serde_roundtrip() {
        let m = test_manifest("1.2.3");
        let v = serde_json::to_value(&m).unwrap();
        let m2 = serde_json::from_value::<UpdateManifest>(v).unwrap();
        assert_eq!(m.version, m2.version);
        assert_eq!(m.signature, m2.signature);
    }

    #[test]
    fn manifest_rejects_missing_signature() {
        let json = serde_json::json!({
            "version": "1.0.0",
            "url": "https://example.com/v1.zip",
            "release_date": "2026-09-21T00:00:00Z"
        });
        assert!(serde_json::from_value::<UpdateManifest>(json).is_err());
    }

    // ── EndpointClient (Mock) ────────────────────────────────────────

    struct MockClient {
        /// 模拟返回的 manifest（None 表示已是最新）。
        manifest: Option<UpdateManifest>,
        /// 模拟错误。
        error: Option<DistributeError>,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl MockClient {
        fn new(manifest: Option<UpdateManifest>) -> Self {
            Self { manifest, error: None, calls: std::sync::atomic::AtomicUsize::new(0) }
        }

        fn with_error(err: DistributeError) -> Self {
            Self { manifest: None, error: Some(err), calls: std::sync::atomic::AtomicUsize::new(0) }
        }
    }

    impl EndpointClient for MockClient {
        fn fetch_manifest(
            &self,
            _current_version: &str,
        ) -> DistributeResult<Option<UpdateManifest>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(ref err) = self.error {
                return Err(err.clone());
            }
            Ok(self.manifest.clone())
        }
    }

    #[test]
    fn check_for_update_returns_manifest() {
        let client = MockClient::new(Some(test_manifest("2.0.0")));
        let policy = GrayscalePolicy { current: GrayscaleBatch::Batch100, ..Default::default() };
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(
            matches!(result, Ok(UpdateCheckResult::UpdateAvailable(ref m)) if m.version == "2.0.0")
        );
    }

    #[test]
    fn check_for_update_returns_up_to_date() {
        let client = MockClient::new(None);
        let policy = GrayscalePolicy::default();
        let result = check_for_update(&client, "2.0.0", &policy, 0);
        assert!(matches!(result, Ok(UpdateCheckResult::UpToDate)));
    }

    #[test]
    fn check_for_update_rejects_empty_signature() {
        let mut manifest = test_manifest("2.0.0");
        manifest.signature = String::new();
        let client = MockClient::new(Some(manifest));
        let policy = GrayscalePolicy::default();
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(matches!(result, Ok(UpdateCheckResult::SignatureInvalid)));
    }

    #[test]
    fn check_for_update_endpoint_error_propagates() {
        let client = MockClient::with_error(DistributeError::EndpointError("500".into()));
        let policy = GrayscalePolicy::default();
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(matches!(result, Err(DistributeError::EndpointError(ref s)) if s == "500"));
    }

    #[test]
    fn check_for_update_not_in_grayscale() {
        let client = MockClient::new(Some(test_manifest("2.0.0")));
        let policy = GrayscalePolicy {
            current: GrayscaleBatch::Batch1, // 1%
            ..Default::default()
        };
        // user_hash = 99 → 99%100=99 ≥ 1 → 不在灰度范围。
        let result = check_for_update(&client, "1.0.0", &policy, 99);
        assert!(matches!(result, Ok(UpdateCheckResult::NotInGrayscale)));
    }

    #[test]
    fn check_for_update_in_grayscale() {
        let client = MockClient::new(Some(test_manifest("2.0.0")));
        let policy = GrayscalePolicy {
            current: GrayscaleBatch::Batch1, // 1%
            ..Default::default()
        };
        // user_hash = 0 → 0%100=0 < 1 → 在灰度范围。
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(
            matches!(result, Ok(UpdateCheckResult::UpdateAvailable(ref m)) if m.version == "2.0.0")
        );
    }

    #[test]
    fn check_for_update_full_grayscale_all_users() {
        let client = MockClient::new(Some(test_manifest("2.0.0")));
        let policy = GrayscalePolicy {
            current: GrayscaleBatch::Batch100, // 100%
            ..Default::default()
        };
        // 任何 user_hash 都应命中。
        for hash in [0u32, 1, 50, 99, 255] {
            let result = check_for_update(&client, "1.0.0", &policy, hash);
            assert!(matches!(result, Ok(UpdateCheckResult::UpdateAvailable(_))), "hash={hash}");
        }
    }

    // ── 端到端 ──────────────────────────────────────────────────────

    /// 计划 §4.19 测试项：批次 1%→5%→25%→100% + 每批最小停留。
    #[test]
    fn end_to_end_grayscale_progression() {
        let mut policy =
            GrayscalePolicy { entered_at: 0, min_dwell_seconds: 3600, ..Default::default() };
        let client = MockClient::new(Some(test_manifest("2.0.0")));

        // Batch 1%: 只覆盖 hash < 1 的用户。
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(matches!(result, Ok(UpdateCheckResult::UpdateAvailable(_))));
        let result = check_for_update(&client, "1.0.0", &policy, 50);
        assert!(matches!(result, Ok(UpdateCheckResult::NotInGrayscale)));

        // 未到达停留时间，不能推进。
        assert!(policy.advance(1000).is_err());

        // 1 小时后推进到 5%。
        let new_batch = policy.advance(3601).unwrap();
        assert_eq!(new_batch, Some(GrayscaleBatch::Batch5));

        // Batch 5%: 覆盖 hash < 5 的用户。
        let result = check_for_update(&client, "1.0.0", &policy, 3);
        assert!(matches!(result, Ok(UpdateCheckResult::UpdateAvailable(_))));
        let result = check_for_update(&client, "1.0.0", &policy, 10);
        assert!(matches!(result, Ok(UpdateCheckResult::NotInGrayscale)));

        // 继续推进到 25% 和 100%。
        let _ = policy.advance(3601 + 3601).unwrap();
        assert_eq!(policy.current, GrayscaleBatch::Batch25);
        let _ = policy.advance(3601 + 3601 + 3601).unwrap();
        assert_eq!(policy.current, GrayscaleBatch::Batch100);

        // 100% 后无下一批。
        assert_eq!(policy.advance(999999).unwrap(), None);
    }

    /// 计划 §4.19 测试项：崩溃率触发停发。
    #[test]
    fn end_to_end_crash_rate_auto_stop() {
        let mut gate = CrashGate { threshold_percent: 5.0, ..Default::default() };
        // 正常发布：1/100 = 1%。
        assert!(!gate.update(1, 100));
        assert!(!gate.is_stopped());

        // 崩溃率上升：6/100 = 6%。
        let stopped = gate.update(6, 100);
        assert!(stopped, "崩溃率超过阈值应停发");
        assert!(gate.is_stopped());

        // 回退灰度。
        let mut policy = GrayscalePolicy {
            current: GrayscaleBatch::Batch25,
            entered_at: 0,
            min_dwell_seconds: 0,
        };
        policy.rollback(GrayscaleBatch::Batch1, 100);
        assert_eq!(policy.current, GrayscaleBatch::Batch1);
        assert_eq!(policy.current_percentage(), 1);

        // 手动恢复后继续。
        gate.reset();
        assert!(!gate.is_stopped());
    }

    /// 计划 §4.19 测试项：四种 endpoint 响应路径。
    #[test]
    fn end_to_end_endpoint_error_paths() {
        // 1. 2xx 正常（有更新）。
        let client = MockClient::new(Some(test_manifest("2.0.0")));
        let policy = GrayscalePolicy::default();
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(matches!(result, Ok(UpdateCheckResult::UpdateAvailable(_))));

        // 2. 2xx 正常（已是最新）。
        let client = MockClient::new(None);
        let result = check_for_update(&client, "2.0.0", &policy, 0);
        assert!(matches!(result, Ok(UpdateCheckResult::UpToDate)));

        // 3. 空签名（2xx 但签名缺失）。
        let mut manifest = test_manifest("2.0.0");
        manifest.signature = String::new();
        let client = MockClient::new(Some(manifest));
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(matches!(result, Ok(UpdateCheckResult::SignatureInvalid)));

        // 4. endpoint 错误（404/500）。
        let client = MockClient::with_error(DistributeError::EndpointError("500".into()));
        let result = check_for_update(&client, "1.0.0", &policy, 0);
        assert!(matches!(result, Err(DistributeError::EndpointError(_))));
    }
}
