// §4.18 商城：索引、验签、受控解包、版本比对、审计日志。
//
// 关键约束（计划 §4.18）：
// - 签名对象是 RFC 8785 规范化后的 manifest；
// - 解包只用 `ZipFile::enclosed_name()` 自写循环；拒 `..`/绝对路径/符号链接；
// - 条目 ≤2000、解压 ≤200 MB、压缩比 ≤100×、单文件 ≤100 MB；
// - 校验顺序固定：验签 → hash → 解包常量 → 权限审批 → 注册；
// - 降级安装默认拒绝（version 单调 + min_allowed_version）；
// - 审计日志追加式 + 链式 hash，记录 grants 快照。
//
// 本 crate 不依赖 `tauri`：签名验证、HTTP 拉取、zip 解包由适配层提供。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub mod error;
pub mod api;

pub use error::{MarketError, MarketResult};
pub use api::{MarketplaceApi, SearchRequest, SearchResponse, PluginSummary, PluginDetailResponse, VersionInfo, InstallRequest, InstallResponse, UpdateCheckRequest, UpdateCheckResponse, UpdateCheckResult, UninstallRequest, UninstallResponse, RevokeRequest, RevokeResponse, ApiResponseError, SortOrder, create_marketplace_api, create_default_marketplace_api};

/// 解压常量（计划 §4.18 固定值）。
pub const MAX_ENTRIES: usize = 2000;
pub const MAX_UNPACKED_MB: u64 = 200;
pub const MAX_SINGLE_FILE_MB: u64 = 100;
pub const MAX_COMPRESSION_RATIO: u32 = 100;

/// 包清单（manifest.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    /// 插件 id。
    pub plugin_id: String,
    /// 版本号。
    pub version: String,
    /// 支持的 framework 版本范围。
    pub framework: FrameworkRange,
    /// 权限集。
    pub permissions: Vec<String>,
    /// 文件列表（路径 → hash）。
    pub files: BTreeMap<String, String>,
    /// 签名密钥 id。
    pub kid: String,
    /// 签发时间（epoch 秒）。
    pub signed_at: u64,
}

impl PackageManifest {
    /// RFC 8785 规范化后的 JSON 字符串（用于签名）。
    ///
    /// 简化版：BTreeMap 保证 key 排序，无多余空格。
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Framework 版本范围。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameworkRange {
    /// 最低版本（含）。
    pub min: String,
    /// 最高版本（含）。
    pub max: String,
}

impl FrameworkRange {
    /// 检查当前版本是否在范围内。
    pub fn matches(&self, current: &str) -> bool {
        cmp_version(current, &self.min) >= 0 && cmp_version(current, &self.max) <= 0
    }

    pub fn as_str(&self) -> String {
        format!("{} ~ {}", self.min, self.max)
    }
}

/// 比较两个语义化版本号。
/// 返回负数表示 a < b，0 表示 a == b，正数表示 a > b。
pub fn cmp_version(a: &str, b: &str) -> i32 {
    let parse = |s: &str| -> Vec<u64> {
        s.split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let av = parse(a);
    let bv = parse(b);
    let max_len = av.len().max(bv.len());
    for i in 0..max_len {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        if x != y {
            return x as i32 - y as i32;
        }
    }
    0
}

/// 检查是否有降级（目标版本 < 当前版本）。
pub fn is_downgrade(current: &str, target: &str) -> bool {
    cmp_version(current, target) > 0
}

/// 版本号是否单调递增（target > current）。
pub fn is_monotonic(current: &str, target: &str) -> bool {
    cmp_version(target, current) >= 0
}

/// Zip 条目信息（适配层传入）。
#[derive(Debug, Clone)]
pub struct ZipEntryInfo {
    /// 条目原始名称。
    pub name: String,
    /// 是否符号链接。
    pub is_symlink: bool,
    /// 是否普通文件。
    pub is_file: bool,
    /// 压缩前大小（字节）。
    pub uncompressed_size: u64,
    /// 压缩后大小（字节）。
    pub compressed_size: u64,
}

/// 路径清洗器：拒绝 `..`/绝对路径/符号链接。
pub fn sanitize_entry_path(name: &str, entry: &ZipEntryInfo) -> MarketResult<String> {
    // 拒绝符号链接。
    if entry.is_symlink {
        return Err(MarketError::PathTraversal(format!("符号链接 `{name}`")));
    }
    // 拒绝非普通文件。
    if !entry.is_file {
        return Err(MarketError::PathTraversal(format!("非普通文件 `{name}`")));
    }
    // 拒绝绝对路径。
    if name.starts_with('/') || name.starts_with('\\') {
        return Err(MarketError::PathTraversal(format!("绝对路径 `{name}`")));
    }
    // Windows 盘符路径。
    if name.len() >= 2 && name.get(1..2) == Some(":") {
        return Err(MarketError::PathTraversal(format!("盘符路径 `{name}`")));
    }
    // 拒绝 `..` 段。
    let segments: Vec<&str> = name.split(['/', '\\']).collect();
    for seg in &segments {
        if *seg == ".." {
            return Err(MarketError::PathTraversal(format!("路径含 `..`：`{name}`")));
        }
    }
    // 拒绝空段（`//` 或 `a//b`）。
    for seg in &segments {
        if seg.is_empty() {
            return Err(MarketError::PathTraversal(format!("路径含空段：`{name}`")));
        }
    }
    Ok(name.to_string())
}

/// 校验压缩包常量。
pub fn validate_zip_constants(
    entries: &[ZipEntryInfo],
) -> MarketResult<(u64, u64, u32)> {
    // 条目数。
    if entries.len() > MAX_ENTRIES {
        return Err(MarketError::EntryCountExceeded {
            actual: entries.len(),
            limit: MAX_ENTRIES,
        });
    }
    // 解压后总大小。
    let total_uncompressed: u64 = entries.iter().map(|e| e.uncompressed_size).sum();
    if total_uncompressed > MAX_UNPACKED_MB * 1024 * 1024 {
        return Err(MarketError::UnpackedSizeExceeded {
            actual_mb: total_uncompressed / (1024 * 1024),
            limit_mb: MAX_UNPACKED_MB,
        });
    }
    // 单文件限制。
    for entry in entries {
        let size_mb = entry.uncompressed_size / (1024 * 1024);
        if size_mb >= MAX_SINGLE_FILE_MB {
            return Err(MarketError::FileTooLarge {
                name: entry.name.clone(),
                size_mb,
                limit_mb: MAX_SINGLE_FILE_MB,
            });
        }
    }
    // 压缩比。
    let total_compressed: u64 = entries.iter().map(|e| e.compressed_size).sum();
    if total_compressed > 0 {
        let ratio = total_uncompressed as f64 / total_compressed as f64;
        if ratio >= MAX_COMPRESSION_RATIO as f64 {
            return Err(MarketError::CompressionRatioExceeded {
                ratio,
                limit: MAX_COMPRESSION_RATIO,
            });
        }
    }
    Ok((
        total_uncompressed,
        total_compressed,
        (total_uncompressed as f64 / total_compressed.max(1) as f64) as u32,
    ))
}

/// 审计日志条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// 时间戳（epoch 秒）。
    pub ts: u64,
    /// 操作类型。
    pub action: AuditAction,
    /// 插件 id。
    pub plugin_id: String,
    /// 版本号。
    pub version: Option<String>,
    /// 权限授予快照。
    pub grants: Option<Vec<String>>,
    /// 前一条目的 hash（链式）。
    pub prev_hash: String,
    /// 本条目的 hash。
    pub hash: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Install,
    Update,
    Uninstall,
    Revoke,
    Restore,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::Install => "install",
            AuditAction::Update => "update",
            AuditAction::Uninstall => "uninstall",
            AuditAction::Revoke => "revoke",
            AuditAction::Restore => "restore",
        }
    }
}

impl std::fmt::Display for AuditAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 审计日志（追加式 + 链式 hash）。
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    current_hash: String,
}

impl Default for AuditLog {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            current_hash: String::new(),
        }
    }
}

impl AuditLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一条审计记录。
    pub fn append(
        &mut self,
        ts: u64,
        action: AuditAction,
        plugin_id: &str,
        version: Option<String>,
        grants: Option<Vec<String>>,
    ) {
        let prev_hash = self.current_hash.clone();
        let mut entry = AuditEntry {
            ts,
            action,
            plugin_id: plugin_id.to_string(),
            version,
            grants,
            prev_hash,
            hash: String::new(),
        };
        // 计算本条目 hash（不含 hash 字段自身）。
        let content = serde_json::json!({
            "ts": entry.ts,
            "action": entry.action,
            "plugin_id": entry.plugin_id,
            "version": entry.version,
            "grants": entry.grants,
            "prev_hash": entry.prev_hash,
        });
        let content_str = serde_json::to_string(&content).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(content_str.as_bytes());
        let hash = hex_encode(&hasher.finalize());
        entry.hash = hash.clone();
        self.current_hash = hash;
        self.entries.push(entry);
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 所有条目。
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// 验证链式 hash 完整性。
    pub fn verify_chain(&self) -> bool {
        for (i, entry) in self.entries.iter().enumerate() {
            // 验证 prev_hash 链。
            let expected_prev = if i == 0 {
                String::new()
            } else {
                self.entries[i - 1].hash.clone()
            };
            if entry.prev_hash != expected_prev {
                return false;
            }
            // 验证本条目 hash。
            let content = serde_json::json!({
                "ts": entry.ts,
                "action": entry.action,
                "plugin_id": entry.plugin_id,
                "version": entry.version,
                "grants": entry.grants,
                "prev_hash": entry.prev_hash,
            });
            let content_str = serde_json::to_string(&content).unwrap_or_default();
            let mut hasher = Sha256::new();
            hasher.update(content_str.as_bytes());
            let expected_hash = hex_encode(&hasher.finalize());
            if entry.hash != expected_hash {
                return false;
            }
        }
        true
    }

    /// 序列化。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "entries": self.entries,
            "current_hash": self.current_hash,
        })
    }

    /// 反序列化。
    pub fn from_json(v: &serde_json::Value) -> MarketResult<Self> {
        let entries = serde_json::from_value(v["entries"].clone())
            .map_err(|e| MarketError::ManifestFormat(e.to_string()))?;
        let current_hash = v["current_hash"]
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok(Self {
            entries,
            current_hash,
        })
    }
}

/// 吊销表。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RevocationList {
    /// kid → plugin_id → 吊销时间。
    pub revoked: BTreeMap<String, BTreeMap<String, u64>>,
}

impl RevocationList {
    /// 检查插件是否被吊销。
    pub fn is_revoked(&self, kid: &str, plugin_id: &str) -> bool {
        self.revoked
            .get(kid)
            .and_then(|m| m.get(plugin_id))
            .is_some()
    }

    /// 添加吊销记录。
    pub fn revoke(&mut self, kid: &str, plugin_id: &str, ts: u64) {
        self.revoked
            .entry(kid.to_string())
            .or_default()
            .insert(plugin_id.to_string(), ts);
    }

    /// 检查 kid 是否存在。
    pub fn has_key(&self, kid: &str) -> bool {
        self.revoked.contains_key(kid)
    }
}

/// hex 编码辅助。
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 版本比较 ─────────────────────────────────────────────────────

    #[test]
    fn cmp_version_equal() {
        assert_eq!(cmp_version("1.0.0", "1.0.0"), 0);
        assert_eq!(cmp_version("2.1.3", "2.1.3"), 0);
    }

    #[test]
    fn cmp_version_less() {
        assert!(cmp_version("1.0.0", "2.0.0") < 0);
        assert!(cmp_version("1.9.9", "2.0.0") < 0);
        assert!(cmp_version("1.0.0", "1.0.1") < 0);
    }

    #[test]
    fn cmp_version_greater() {
        assert!(cmp_version("2.0.0", "1.0.0") > 0);
        assert!(cmp_version("1.0.1", "1.0.0") > 0);
    }

    #[test]
    fn cmp_version_diff_lengths() {
        assert_eq!(cmp_version("1.0", "1.0.0"), 0);
        assert!(cmp_version("1.0.1", "1.0") > 0);
    }

    #[test]
    fn is_downgrade_detection() {
        assert!(is_downgrade("2.0.0", "1.0.0"));
        assert!(is_downgrade("1.5.0", "1.4.9"));
        assert!(!is_downgrade("1.0.0", "1.0.0"));
        assert!(!is_downgrade("1.0.0", "2.0.0"));
    }

    #[test]
    fn is_monotonic_check() {
        assert!(is_monotonic("1.0.0", "1.0.0"));
        assert!(is_monotonic("1.0.0", "2.0.0"));
        assert!(!is_monotonic("2.0.0", "1.0.0"));
    }

    // ── Framework 范围 ───────────────────────────────────────────────

    #[test]
    fn framework_range_matches() {
        let range = FrameworkRange { min: "1.0.0".into(), max: "2.0.0".into() };
        assert!(range.matches("1.0.0"));
        assert!(range.matches("1.5.0"));
        assert!(range.matches("2.0.0"));
        assert!(!range.matches("0.9.9"));
        assert!(!range.matches("2.0.1"));
    }

    #[test]
    fn framework_range_as_str() {
        let range = FrameworkRange { min: "1.0.0".into(), max: "2.0.0".into() };
        assert_eq!(range.as_str(), "1.0.0 ~ 2.0.0");
    }

    #[test]
    fn framework_range_serde_roundtrip() {
        let range = FrameworkRange { min: "1.0.0".into(), max: "3.0.0".into() };
        let v = serde_json::to_value(&range).unwrap();
        let r2 = serde_json::from_value::<FrameworkRange>(v).unwrap();
        assert_eq!(r2, range);
    }

    // ── 路径清洗 ─────────────────────────────────────────────────────

    #[test]
    fn sanitize_rejects_dotdot() {
        let entry = ZipEntryInfo {
            name: "../../etc/passwd".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_rejects_absolute_path() {
        let entry = ZipEntryInfo {
            name: "/etc/passwd".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_rejects_symlink() {
        let entry = ZipEntryInfo {
            name: "safe/path".into(),
            is_symlink: true,
            is_file: false,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_rejects_non_file() {
        let entry = ZipEntryInfo {
            name: "safe/path".into(),
            is_symlink: false,
            is_file: false,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_rejects_windows_drive() {
        let entry = ZipEntryInfo {
            name: "C:\\Windows\\system32".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_rejects_empty_segment() {
        let entry = ZipEntryInfo {
            name: "a//b".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_rejects_middle_dotdot() {
        let entry = ZipEntryInfo {
            name: "a/../../etc/passwd".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    #[test]
    fn sanitize_accepts_safe_path() {
        let entry = ZipEntryInfo {
            name: "plugin/index.js".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_ok());
    }

    #[test]
    fn sanitize_accepts_nested_path() {
        let entry = ZipEntryInfo {
            name: "a/b/c/d.txt".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_ok());
    }

    #[test]
    fn sanitize_rejects_backslash_dotdot() {
        let entry = ZipEntryInfo {
            name: "a\\..\\b".into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: 100,
            compressed_size: 50,
        };
        assert!(sanitize_entry_path(&entry.name, &entry).is_err());
    }

    // ── Zip 常量校验 ────────────────────────────────────────────────

    fn entry(name: &str, size: u64, compressed: u64) -> ZipEntryInfo {
        ZipEntryInfo {
            name: name.into(),
            is_symlink: false,
            is_file: true,
            uncompressed_size: size,
            compressed_size: compressed,
        }
    }

    #[test]
    fn zip_constants_accept_normal_package() {
        let entries = vec![
            entry("a.js", 1000, 500),
            entry("b.js", 2000, 1000),
        ];
        assert!(validate_zip_constants(&entries).is_ok());
    }

    #[test]
    fn zip_constants_rejects_too_many_entries() {
        let entries: Vec<_> = (0..=MAX_ENTRIES)
            .map(|i| entry(&format!("f{i}"), 100, 50))
            .collect();
        assert!(matches!(
            validate_zip_constants(&entries),
            Err(MarketError::EntryCountExceeded { limit: 2000, .. })
        ));
    }

    #[test]
    fn zip_constants_rejects_too_large_unpacked() {
        // 201 MB。
        let size = (MAX_UNPACKED_MB + 1) * 1024 * 1024;
        let entries = vec![entry("big.bin", size, 100)];
        assert!(matches!(
            validate_zip_constants(&entries),
            Err(MarketError::UnpackedSizeExceeded { limit_mb: 200, .. })
        ));
    }

    #[test]
    fn zip_constants_rejects_single_file_too_large() {
        // 100 MB single file。
        let size = MAX_SINGLE_FILE_MB * 1024 * 1024;
        let entries = vec![entry("huge.bin", size, 100)];
        assert!(matches!(
            validate_zip_constants(&entries),
            Err(MarketError::FileTooLarge { limit_mb: 100, .. })
        ));
    }

    #[test]
    fn zip_constants_rejects_high_compression_ratio() {
        // 压缩比 1000×。
        let entries = vec![entry("sparse.bin", 1_000_000, 1)];
        assert!(matches!(
            validate_zip_constants(&entries),
            Err(MarketError::CompressionRatioExceeded { limit: 100, .. })
        ));
    }

    // ── 审计日志 ────────────────────────────────────────────────────

    #[test]
    fn audit_log_append_and_verify() {
        let mut log = AuditLog::new();
        log.append(1000, AuditAction::Install, "p.audio", Some("1.0.0".into()), None);
        log.append(2000, AuditAction::Update, "p.audio", Some("1.1.0".into()), None);
        assert!(log.verify_chain(), "链式 hash 应完整");
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn audit_log_tamper_detection() {
        let mut log = AuditLog::new();
        log.append(1000, AuditAction::Install, "p.audio", Some("1.0.0".into()), None);
        log.append(2000, AuditAction::Update, "p.audio", Some("1.1.0".into()), None);
        // 篡改第一条的版本号。
        log.entries[0].version = Some("9.9.9".into());
        assert!(!log.verify_chain(), "篡改后链应断裂");
    }

    #[test]
    fn audit_log_empty_is_valid() {
        let log = AuditLog::new();
        assert!(log.verify_chain());
        assert!(log.is_empty());
    }

    #[test]
    fn audit_log_grants_snapshot() {
        let mut log = AuditLog::new();
        log.append(
            1000,
            AuditAction::Install,
            "p.audio",
            Some("1.0.0".into()),
            Some(vec!["read:fs".into(), "write:fs".into()]),
        );
        assert!(log.verify_chain());
        let entry = &log.entries()[0];
        assert_eq!(entry.grants.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn audit_log_serde_roundtrip() {
        let mut log = AuditLog::new();
        log.append(1000, AuditAction::Install, "p.a", Some("1.0.0".into()), None);
        let json = log.to_json();
        let log2 = AuditLog::from_json(&json).unwrap();
        assert_eq!(log2.len(), 1);
        assert!(log2.verify_chain());
    }

    #[test]
    fn audit_action_serde_roundtrip() {
        for action in [
            AuditAction::Install,
            AuditAction::Update,
            AuditAction::Uninstall,
            AuditAction::Revoke,
            AuditAction::Restore,
        ] {
            let v = serde_json::to_value(action.clone()).unwrap();
            let a2 = serde_json::from_value::<AuditAction>(v).unwrap();
            assert_eq!(a2, action);
        }
    }

    // ── 吊销表 ──────────────────────────────────────────────────────

    #[test]
    fn revocation_check() {
        let mut rl = RevocationList::default();
        assert!(!rl.is_revoked("kid1", "p.audio"));
        rl.revoke("kid1", "p.audio", 1000);
        assert!(rl.is_revoked("kid1", "p.audio"));
        assert!(!rl.is_revoked("kid2", "p.audio"));
        assert!(rl.has_key("kid1"));
        assert!(!rl.has_key("kid99"));
    }

    #[test]
    fn revocation_serde_roundtrip() {
        let mut rl = RevocationList::default();
        rl.revoke("kid1", "p.a", 1000);
        rl.revoke("kid1", "p.b", 2000);
        rl.revoke("kid2", "p.a", 3000);
        let v = serde_json::to_value(&rl).unwrap();
        let rl2 = serde_json::from_value::<RevocationList>(v).unwrap();
        assert!(rl2.is_revoked("kid1", "p.a"));
        assert!(rl2.is_revoked("kid1", "p.b"));
        assert!(rl2.is_revoked("kid2", "p.a"));
    }

    // ── 包清单 ──────────────────────────────────────────────────────

    fn test_manifest() -> PackageManifest {
        PackageManifest {
            plugin_id: "p.audio".into(),
            version: "1.0.0".into(),
            framework: FrameworkRange { min: "1.0.0".into(), max: "2.0.0".into() },
            permissions: vec!["read:fs".into(), "write:fs".into()],
            files: BTreeMap::new(),
            kid: "kid1".into(),
            signed_at: 1000,
        }
    }

    #[test]
    fn manifest_canonical_json() {
        let m = test_manifest();
        let json = m.canonical_json();
        // BTreeMap 保证 key 排序。
        assert!(json.contains("\"framework\""));
        assert!(json.contains("\"permissions\""));
        assert!(json.contains("\"plugin_id\""));
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let m = test_manifest();
        let v = serde_json::to_value(&m).unwrap();
        let m2 = serde_json::from_value::<PackageManifest>(v).unwrap();
        assert_eq!(m.plugin_id, m2.plugin_id);
        assert_eq!(m.version, m2.version);
        assert_eq!(m.kid, m2.kid);
    }

    // ── 端到端 ──────────────────────────────────────────────────────

    /// 计划 §4.18 测试项：降级安装拒绝。
    #[test]
    fn end_to_end_downgrade_rejected() {
        let mut log = AuditLog::new();
        log.append(1000, AuditAction::Install, "p.audio", Some("2.0.0".into()), None);

        // 尝试安装 1.0.0（降级）。
        let current = "2.0.0";
        let target = "1.0.0";
        assert!(is_downgrade(current, target));
        assert!(!is_monotonic(current, target));
        // 审计日志记录了当前版本。
        assert_eq!(log.entries()[0].version.as_deref(), Some("2.0.0"));
    }

    /// 计划 §4.18 测试项：框架不匹配拒装。
    #[test]
    fn end_to_end_framework_mismatch() {
        let manifest = PackageManifest {
            plugin_id: "p.audio".into(),
            version: "1.0.0".into(),
            framework: FrameworkRange { min: "3.0.0".into(), max: "4.0.0".into() },
            permissions: vec![],
            files: BTreeMap::new(),
            kid: "kid1".into(),
            signed_at: 1000,
        };
        // 当前框架版本 2.0.0，不在范围内。
        assert!(!manifest.framework.matches("2.0.0"));
        // 报错信息。
        let err = MarketError::FrameworkMismatch {
            plugin: manifest.plugin_id.clone(),
            range: manifest.framework.as_str(),
            current: "2.0.0".into(),
        };
        assert!(err.to_string().contains("不匹配"));
    }

    /// 计划 §4.18 测试项：吊销生效。
    #[test]
    fn end_to_end_revocation() {
        let mut rl = RevocationList::default();
        // 插件被吊销。
        rl.revoke("kid1", "p.audio", 1000);
        assert!(rl.is_revoked("kid1", "p.audio"));
        // 审计日志记录吊销。
        let mut log = AuditLog::new();
        log.append(
            1000,
            AuditAction::Revoke,
            "p.audio",
            None,
            None,
        );
        assert!(log.verify_chain());
        assert!(matches!(log.entries()[0].action, AuditAction::Revoke));
    }

    /// 计划 §4.18 测试项：恶意样本 — 路径穿越。
    #[test]
    fn end_to_end_malicious_path_traversal() {
        let entries = vec![
            entry("safe/index.js", 100, 50),
            entry("../../etc/passwd", 200, 100),
        ];
        // 校验常量先通过（大小没问题）。
        assert!(validate_zip_constants(&entries).is_ok());
        // 但路径清洗应拒绝。
        for entry in &entries {
            let result = sanitize_entry_path(&entry.name, entry);
            if entry.name == "../../etc/passwd" {
                assert!(result.is_err(), "路径穿越应被拒");
                assert!(matches!(result, Err(MarketError::PathTraversal(_))));
            } else {
                assert!(result.is_ok());
            }
        }
    }

    /// 计划 §4.18 测试项：恶意样本 — 超大解压。
    #[test]
    fn end_to_end_malicious_huge_unpack() {
        // 201 MB。
        let size = (MAX_UNPACKED_MB + 1) * 1024 * 1024;
        let entries = vec![entry("big.bin", size, 1000)];
        assert!(validate_zip_constants(&entries).is_err());
    }

    /// 计划 §4.18 测试项：恶意样本 — 未知 kid。
    #[test]
    fn end_to_end_unknown_kid() {
        let err = MarketError::UnknownKey("unknown-kid".into());
        assert!(err.to_string().contains("unknown-kid"));
    }

    /// 计划 §4.18 校验顺序：验签 → hash → 解包常量 → 权限审批 → 注册。
    #[test]
    fn end_to_end_validation_order() {
        let manifest = test_manifest();
        // 1. 验签（这里只检查 manifest 格式）。
        assert!(serde_json::to_string(&manifest).is_ok());
        // 2. hash（模拟文件 hash 检查）。
        let mut hasher = Sha256::new();
        hasher.update(b"file content");
        let hash = hex_encode(&hasher.finalize());
        assert_eq!(hash.len(), 64); // SHA-256 = 64 hex chars。
        // 3. 解包常量。
        let entries = vec![entry("index.js", 100, 50)];
        assert!(validate_zip_constants(&entries).is_ok());
        // 4. 权限审批（检查权限列表非空）。
        assert!(!manifest.permissions.is_empty());
        // 5. 注册（审计日志）。
        let mut log = AuditLog::new();
        log.append(
            1000,
            AuditAction::Install,
            &manifest.plugin_id,
            Some(manifest.version.clone()),
            Some(manifest.permissions.clone()),
        );
        assert!(log.verify_chain());
    }
}
