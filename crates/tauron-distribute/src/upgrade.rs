// 升级执行器：下载 → 完整性校验（SHA-256 + 注入验签）→ 备份 → 解压 → 原子交换 →
// 健康检查（失败自动回滚）→ 提交 → 重启，全程落盘 JSON 状态日志（journal）。
//
// 职责（计划 §4.19 / M5，审计 V7-P0-03 / §5 Batch 0）：
// - 下载更新包（受 `download_timeout_secs` deadline + 协作取消约束）；
// - 验证下载产物 SHA-256 与清单一致，再走注入的 `SignatureVerifier`；
// - 真实递归备份（逐文件 fsync），备份树哈希集合与源树一致才算备份成功；
// - 真实 zip 解压（zip-slip / 绝对路径 / 盘符 / 符号链接 / 条目数 / 解压预算防护）；
// - staging/current/previous 三段式安装根，`std::fs::rename` 原子交换；
// - 提交前健康检查（注入 seam），失败自动回滚并逐文件哈希校验恢复字节；
// - 重启 seam（注入 `RestartProvider`），`restarted` 如实反映其结果；
// - 任何失败路径清理 temp/staging，清理失败必须上浮为错误。
//
// **诚实说明（V7-P0-03 之后）**：
// - 已真实：上述全部文件机制（备份/解压/交换/回滚/日志/清理）都有真实文件系统
//   副作用，并由 `#[cfg(test)]` 里的真实 tempdir + 真实 zip fixture 测试对账
//   （升级前后逐文件 SHA-256 核对）。
// - 仍不在仓内（不得过度宣称）：
//   1. 没有生产 HTTP `Downloader` 实现——下载由装配方注入；超时约束通过
//      `PhaseControl`（deadline + 取消标志）+ 监控线程实现，**不合作的**
//      Downloader 线程可能在超时后继续存活，此时残留目录不强删，清理失败会
//      上浮为错误。
//   2. 没有生产 `SignatureVerifier`（ed25519 验签在装配方/仓外）；本 crate 只
//      保证「空签名 / 未注入验证器 = 硬失败」，并把包摘要如实交给验证器。
//   3. 执行侧的**仓内生产消费方**是适配层 `UpgradeInstaller` seam（轮 40）：
//      `host_market_download` / `host_market_install` 在装配方注入
//      Downloader / SignatureVerifier / HealthCheck / RestartProvider 后真跑本
//      模块（下载腿共用 `download_bounded` + `verify_package`，安装腿跑
//      `UpgradeRunner::run`）；缺省未注入时命令仍是如实模拟（`simulated: true`）。
//      没有联网 E2E。
//   4. `min_host_version` / `abi` / `rollback_policy` 会被记录进日志供装配方
//      核对，但执行器本身**不解释**这些字段（仓内没有宿主版本/ABI 判定源）。
//   5. 原子交换依赖「staging/current/previous 同在一个安装根目录（同一文件
//      系统）」这一构造性前提；rename 跨文件系统失败会作为 `SwapFailed` 上浮，
//      绝不做部分非原子复制。
//
// 本模块不依赖 `tauri`：升级逻辑是抽象的，单元测试用真实文件效果断言。

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{DistributeError, DistributeResult, UpdateManifest};

// ──────────────────────────────────────────────────────────────────────────
// 安装根目录布局常量（staging / current / previous 三段式）
// ──────────────────────────────────────────────────────────────────────────

/// 当前生效的安装子目录。
pub const CURRENT_DIR: &str = "current";
/// 交换时被移出的旧版本子目录（备份树另有校验副本）。
pub const PREVIOUS_DIR: &str = "previous";
/// 解压暂存子目录根（与 current 同一文件系统，保证 rename 原子性）。
pub const STAGING_DIR: &str = "staging";
/// 健康检查失败后隔离损坏新版本用的子目录前缀。
pub const FAILED_DIR_PREFIX: &str = "failed-";
/// 每个操作的备份树子目录名。
pub const BACKUP_TREE_DIR: &str = "tree";
/// 状态日志文件名。
pub const JOURNAL_FILE: &str = "journal.json";
/// 取消监控线程在 deadline 后等待其合作的宽限期。
const CANCEL_GRACE: Duration = Duration::from_secs(2);

// ──────────────────────────────────────────────────────────────────────────
// 升级状态
// ──────────────────────────────────────────────────────────────────────────

/// 升级状态。
///
/// 瞬态（进行中）：`Checking` / `Downloading` / `Verifying` / `BackingUp` /
/// `Extracting` / `Replacing` / `HealthChecking` / `RollingBack`；
/// 里程碑（已完成，与日志状态序列一致）：`Downloaded` / `Verified` /
/// `BackedUp` / `Extracted` / `Swapped` / `HealthChecked` / `Committed`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpgradeState {
    /// 未开始。
    Idle,
    /// 检查中（前置校验 + 建立操作目录/日志）。
    Checking,
    /// 下载中。
    Downloading,
    /// 下载完成（里程碑）。
    Downloaded,
    /// 验证中（SHA-256 + 签名）。
    Verifying,
    /// 验证通过（里程碑）。
    Verified,
    /// 备份中。
    BackingUp,
    /// 备份完成且逐文件哈希核对通过（里程碑）。
    BackedUp,
    /// 解压中。
    Extracting,
    /// 解压完成到 staging（里程碑）。
    Extracted,
    /// 原子交换中。
    Replacing,
    /// 交换完成，current 已指向新树（里程碑）。
    Swapped,
    /// 健康检查中。
    HealthChecking,
    /// 健康检查通过（里程碑）。
    HealthChecked,
    /// 已提交：commit 标记落盘且 current 哈希与更新包一致（里程碑）。
    Committed,
    /// 完成。
    Completed,
    /// 失败。
    Failed,
    /// 回滚中。
    RollingBack,
    /// 回滚完成，旧版本字节已恢复并哈希核对（里程碑）。
    RolledBack,
}

impl UpgradeState {
    /// 是否结束（终态）。
    pub fn is_finished(self) -> bool {
        matches!(self, UpgradeState::Completed | UpgradeState::Failed | UpgradeState::RolledBack)
    }

    /// 是否处于「备份已存在、可回滚」的阶段（备份完成之后、失败/完成之前）。
    pub fn can_rollback(self) -> bool {
        matches!(
            self,
            UpgradeState::BackingUp
                | UpgradeState::BackedUp
                | UpgradeState::Extracting
                | UpgradeState::Extracted
                | UpgradeState::Replacing
                | UpgradeState::Swapped
                | UpgradeState::HealthChecking
                | UpgradeState::HealthChecked
                | UpgradeState::Committed
                | UpgradeState::Completed
                | UpgradeState::Failed
        )
    }

    /// 该状态对应的进度百分比（用于 `UpgradeProgress`）。
    ///
    /// `Failed` / `RollingBack` / `RolledBack` 返回 `None`：保留既有百分比，
    /// 避免把「失败」渲染成某个假定的完成度。
    pub fn percent_for_state(self) -> Option<u32> {
        let percent = match self {
            UpgradeState::Idle => 0,
            UpgradeState::Checking => 5,
            UpgradeState::Downloading => 10,
            UpgradeState::Downloaded => 25,
            UpgradeState::Verifying => 30,
            UpgradeState::Verified => 40,
            UpgradeState::BackingUp => 45,
            UpgradeState::BackedUp => 55,
            UpgradeState::Extracting => 60,
            UpgradeState::Extracted => 70,
            UpgradeState::Replacing => 75,
            UpgradeState::Swapped => 80,
            UpgradeState::HealthChecking => 85,
            UpgradeState::HealthChecked => 90,
            UpgradeState::Committed => 95,
            UpgradeState::Completed => 100,
            UpgradeState::Failed | UpgradeState::RollingBack | UpgradeState::RolledBack => {
                return None
            }
        };
        Some(percent)
    }
}

impl std::fmt::Display for UpgradeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// 升级进度。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeProgress {
    /// 当前状态。
    pub state: UpgradeState,
    /// 进度百分比（0-100）。
    pub progress_percent: u32,
    /// 已下载字节数。
    pub downloaded_bytes: u64,
    /// 总字节数。
    pub total_bytes: u64,
    /// 错误信息。
    pub error: Option<String>,
}

impl UpgradeProgress {
    /// 创建初始进度。
    pub fn new() -> Self {
        Self {
            state: UpgradeState::Idle,
            progress_percent: 0,
            downloaded_bytes: 0,
            total_bytes: 0,
            error: None,
        }
    }

    /// 更新状态；已知阶段状态同步映射进度百分比（见 `percent_for_state`）。
    pub fn set_state(&mut self, state: UpgradeState) {
        self.state = state;
        if let Some(percent) = state.percent_for_state() {
            self.progress_percent = percent;
        }
    }

    /// 更新进度。
    pub fn set_progress(&mut self, percent: u32) {
        self.progress_percent = percent.min(100);
    }

    /// 更新下载进度。
    pub fn set_download_progress(&mut self, downloaded: u64, total: u64) {
        self.downloaded_bytes = downloaded;
        self.total_bytes = total;
        if total > 0 {
            self.progress_percent = ((downloaded as f64 / total as f64) * 100.0).min(100.0) as u32;
        }
    }

    /// 设置错误。
    pub fn set_error(&mut self, error: String) {
        self.state = UpgradeState::Failed;
        self.error = Some(error);
    }
}

impl Default for UpgradeProgress {
    fn default() -> Self {
        Self::new()
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 归档预算 / 升级配置
// ──────────────────────────────────────────────────────────────────────────

/// 归档解压预算（zip-bomb / 条目数防护）。全部必须 > 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ArchiveLimits {
    /// 最大条目数。
    pub max_entries: u64,
    /// 最大声明总解压字节数（预扫描中央目录，超预算在写任何文件前拒绝）。
    pub max_total_uncompressed_bytes: u64,
    /// 单条目最大解压字节数（流式强制，声明值造假也拦）。
    pub max_entry_uncompressed_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_total_uncompressed_bytes: 512 * 1024 * 1024,
            max_entry_uncompressed_bytes: 256 * 1024 * 1024,
        }
    }
}

fn default_extract_timeout_secs() -> u64 {
    300
}

fn default_swap_timeout_secs() -> u64 {
    60
}

fn default_health_check_timeout_secs() -> u64 {
    60
}

/// 升级选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeOptions {
    /// 更新清单。
    pub manifest: UpdateManifest,
    /// 下载目录（按操作隔离出子目录）。
    pub download_dir: PathBuf,
    /// 安装根目录（内部建模为 staging/current/previous 三段式）。
    pub install_dir: PathBuf,
    /// 备份目录（按操作隔离出子目录，含 journal.json 与备份树）。
    pub backup_dir: PathBuf,
    /// 是否自动重启（为 true 时必须注入 `RestartProvider`，否则校验失败）。
    pub auto_restart: bool,
    /// 下载超时（秒）。
    pub download_timeout_secs: u64,
    /// 解压超时（秒，条目间强制；单条 rename/写盘 syscall 不可中断）。
    #[serde(default = "default_extract_timeout_secs")]
    pub extract_timeout_secs: u64,
    /// 交换阶段启动前校验的 deadline（秒；rename 本身是单次原子 syscall）。
    #[serde(default = "default_swap_timeout_secs")]
    pub swap_timeout_secs: u64,
    /// 健康检查超时（秒）。
    #[serde(default = "default_health_check_timeout_secs")]
    pub health_check_timeout_secs: u64,
    /// 归档解压预算。
    #[serde(default)]
    pub archive: ArchiveLimits,
    /// 当前已安装版本。
    ///
    /// 两个用途（轮 54）：①**降级门禁的基线**——`validate` 用它对比清单目标版本，
    /// 更低即硬拒（见 [`ensure_not_downgrade`]）；②记入 journal 的 `old_version`
    /// 供回滚/审计对账。为 `None` 时门禁无基线可判、如实放行。
    #[serde(default)]
    pub installed_version: Option<String>,
}

impl Default for UpgradeOptions {
    fn default() -> Self {
        Self {
            manifest: UpdateManifest { version: "0.1.0".into(), ..UpdateManifest::default() },
            download_dir: PathBuf::from("downloads"),
            install_dir: PathBuf::from("install"),
            backup_dir: PathBuf::from("backup"),
            auto_restart: true,
            download_timeout_secs: 300,
            extract_timeout_secs: default_extract_timeout_secs(),
            swap_timeout_secs: default_swap_timeout_secs(),
            health_check_timeout_secs: default_health_check_timeout_secs(),
            archive: ArchiveLimits::default(),
            installed_version: None,
        }
    }
}

/// 升级结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeResult {
    /// 升级是否成功（仅当 commit 标记落盘且 current 树哈希与更新包一致才为 true）。
    pub success: bool,
    /// 最终状态。
    pub state: UpgradeState,
    /// 新版本号。
    pub new_version: Option<String>,
    /// 错误信息。
    pub error: Option<String>,
    /// 是否已备份（仅当备份树逐文件哈希核对通过才为 true）。
    pub backed_up: bool,
    /// 是否已重启（如实反映注入 `RestartProvider` 的结果）。
    pub restarted: bool,
    /// 本次升级操作 id（日志/备份目录按它命名）。
    pub operation_id: String,
}

// ──────────────────────────────────────────────────────────────────────────
// 阶段控制（deadline + 协作取消）
// ──────────────────────────────────────────────────────────────────────────

/// 阶段控制：deadline + 协作取消标志。
///
/// 注入侧（`Downloader` / `UpgradeHealthCheck` / `ArchiveExtractor`）必须轮询
/// [`PhaseControl::is_cancelled`]；执行器用监控线程保证调用方**必定**在
/// deadline 后返回错误，不合作的工作线程只会在错误信息里如实上报「目录可能
/// 残留、清理失败」，绝不被忽略。
#[derive(Debug, Clone)]
pub struct PhaseControl {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl PhaseControl {
    /// 以给定 deadline 创建。
    pub fn new(deadline: Instant) -> Self {
        Self { deadline, cancelled: Arc::new(AtomicBool::new(false)) }
    }

    /// deadline。
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// 是否已越过 deadline。
    pub fn is_past_deadline(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// 是否被要求取消（显式取消或已超时——超时即取消，语义统一）。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst) || self.is_past_deadline()
    }

    /// 请求取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 下载器 trait
// ──────────────────────────────────────────────────────────────────────────

/// 下载器抽象。
///
/// 实现必须：把字节写到 `dest_path`；通过 `progress_callback` 汇报进度；
/// **轮询 `control.is_cancelled()`** 并在取消时尽快返回错误（协作取消）。
pub trait Downloader: Send + Sync + 'static {
    /// 下载文件，返回落盘路径。
    fn download(
        &self,
        url: &str,
        dest_path: &Path,
        control: &PhaseControl,
        progress_callback: &mut dyn FnMut(u64, u64),
    ) -> DistributeResult<PathBuf>;
}

// ──────────────────────────────────────────────────────────────────────────
// 验证器 trait
// ──────────────────────────────────────────────────────────────────────────

/// 交给验证器的上下文：执行器已核对过的包摘要与清单声明的算法/密钥 id。
#[derive(Debug, Clone)]
pub struct VerificationContext<'a> {
    /// 下载产物的实际 SHA-256（hex 小写；已按清单核对通过）。
    pub package_sha256_hex: String,
    /// 清单声明的签名算法（可能为 None——由验证器决定是否接受）。
    pub signature_algorithm: Option<&'a str>,
    /// 清单声明的验签公钥 id（可能为 None）。
    pub public_key_id: Option<&'a str>,
}

/// 签名验证器抽象。
///
/// **诚实边界**：执行器只保证「空签名 / 未注入验证器 / 验证器返回 false」都是
/// 硬失败，并把包摘要如实放进上下文；仓内没有生产验签实现（ed25519 在装配方）。
pub trait SignatureVerifier: Send + Sync + 'static {
    /// 验证签名。返回 `Ok(false)` 或 `Err` 都按失败处理。
    fn verify(
        &self,
        file_path: &Path,
        signature: &str,
        context: &VerificationContext<'_>,
    ) -> DistributeResult<bool>;
}

// ──────────────────────────────────────────────────────────────────────────
// 归档解压 seam
// ──────────────────────────────────────────────────────────────────────────

/// 解压结果统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractedArchive {
    /// 写出的文件条目数（不含目录条目）。
    pub file_count: u64,
    /// 实际写出的总字节数。
    pub total_bytes: u64,
}

/// 归档解压抽象（seam）：测试可注入失败实现验证「解压错误必然致命、不被吞」。
pub trait ArchiveExtractor: Send + Sync + 'static {
    /// 把 `archive_path` 解压到 `staging_dir`，受 `limits` 预算与 `control`
    /// deadline 约束。任何拒绝/超限/IO 错误必须返回 `Err`，禁止部分成功后
    /// 假装成功。
    fn extract(
        &self,
        archive_path: &Path,
        staging_dir: &Path,
        limits: &ArchiveLimits,
        control: &PhaseControl,
    ) -> DistributeResult<ExtractedArchive>;
}

/// 真实 zip 解压（`zip` crate，deflate + store）。
///
/// 防线（全部在写第一个字节之前预扫描中央目录完成声明值判定，流式再兜底）：
/// - 条目名含 `..` / `.` 之外的路径穿越、`\`（Windows 转义）、`:`（盘符/流）、
///   绝对路径（`/` 开头）、NUL → 拒绝；
/// - 符号链接条目（unix mode S_IFLNK）或非常规文件条目（设备/FIFO/socket）→
///   拒绝；
/// - 条目数超 `max_entries`、声明总解压字节超 `max_total_uncompressed_bytes`、
///   单条目声明超 `max_entry_uncompressed_bytes` → 拒绝（zip-bomb 防护）；
/// - 实际写出字节流式封顶（声明值造假也拒）；
/// - 每写一个文件 flush + `sync_all`。
pub struct ZipCrateExtractor;

/// unix 文件类型掩码与常量（zip crate 的 ffi 常量不公开，就地定义）。
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;

/// 校验单个条目名，返回可安全拼到 staging 下的相对路径。
fn validate_entry_name(raw: &str) -> DistributeResult<PathBuf> {
    if raw.contains('\0') {
        return Err(DistributeError::ArchiveRejected(format!("条目名含 NUL：`{raw}`")));
    }
    if raw.contains('\\') {
        return Err(DistributeError::ArchiveRejected(format!(
            "条目名含反斜杠转义（Windows 路径逃逸）：`{raw}`"
        )));
    }
    if raw.contains(':') {
        return Err(DistributeError::ArchiveRejected(format!("条目名含盘符/数据流冒号：`{raw}`")));
    }
    if raw.starts_with('/') {
        return Err(DistributeError::ArchiveRejected(format!("条目名为绝对路径：`{raw}`")));
    }
    let mut rel = PathBuf::new();
    for segment in raw.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                return Err(DistributeError::ArchiveRejected(format!(
                    "条目名含 `..` 路径穿越（zip-slip）：`{raw}`"
                )))
            }
            other => rel.push(other),
        }
    }
    Ok(rel)
}

/// 流式解压封顶：读超 `remaining` 立即报 IO 错误（防声明尺寸造假）。
struct CappedRead<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for CappedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        let n = n as u64;
        if n > self.remaining {
            return Err(io::Error::other("解压实际字节数超过预算上限"));
        }
        self.remaining -= n;
        Ok(n as usize)
    }
}

impl ArchiveExtractor for ZipCrateExtractor {
    fn extract(
        &self,
        archive_path: &Path,
        staging_dir: &Path,
        limits: &ArchiveLimits,
        control: &PhaseControl,
    ) -> DistributeResult<ExtractedArchive> {
        let file = File::open(archive_path).map_err(|e| {
            DistributeError::FileOperationFailed(format!(
                "打开更新包 `{}` 失败：{e}",
                archive_path.display()
            ))
        })?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|e| DistributeError::ArchiveRejected(format!("zip 结构无法解析：{e}")))?;
        let count = zip.len();
        if count as u64 > limits.max_entries {
            return Err(DistributeError::ArchiveRejected(format!(
                "条目数 {count} 超过上限 {}",
                limits.max_entries
            )));
        }

        // 预扫描：全部条目名/模式/声明尺寸校验完并累计预算，才允许写第一个字节。
        struct Validated {
            index: usize,
            rel: PathBuf,
            is_dir: bool,
        }
        let mut validated: Vec<Validated> = Vec::with_capacity(count);
        let mut declared_total: u64 = 0;
        let mut file_entries = 0u64;
        for index in 0..count {
            let entry = zip.by_index(index).map_err(|e| {
                DistributeError::ArchiveRejected(format!("读取中央目录条目 #{index} 失败：{e}"))
            })?;
            let raw_name = entry.name().to_string();
            let rel = validate_entry_name(&raw_name)?;
            if entry.is_symlink() {
                return Err(DistributeError::ArchiveRejected(format!(
                    "拒绝符号链接条目：`{raw_name}`"
                )));
            }
            if let Some(mode) = entry.unix_mode() {
                let file_type = mode & S_IFMT;
                if file_type != S_IFREG && file_type != S_IFDIR {
                    return Err(DistributeError::ArchiveRejected(format!(
                        "拒绝非常规文件条目 `{raw_name}`（unix mode 0o{mode:o}）"
                    )));
                }
            }
            let declared = entry.size();
            if declared > limits.max_entry_uncompressed_bytes {
                return Err(DistributeError::ArchiveRejected(format!(
                    "条目 `{raw_name}` 声明解压尺寸 {declared} 超过单条目上限 {}",
                    limits.max_entry_uncompressed_bytes
                )));
            }
            declared_total = declared_total
                .checked_add(declared)
                .ok_or_else(|| DistributeError::ArchiveRejected("声明解压尺寸累计溢出".into()))?;
            if declared_total > limits.max_total_uncompressed_bytes {
                return Err(DistributeError::ArchiveRejected(format!(
                    "声明解压总尺寸 {declared_total} 超过预算 {}",
                    limits.max_total_uncompressed_bytes
                )));
            }
            let is_dir = entry.is_dir();
            if !is_dir {
                file_entries += 1;
            }
            validated.push(Validated { index, rel, is_dir });
        }
        if file_entries == 0 {
            return Err(DistributeError::ArchiveRejected("归档包内没有文件条目".into()));
        }

        fs::create_dir_all(staging_dir).map_err(|e| {
            DistributeError::FileOperationFailed(format!(
                "创建解压目录 `{}` 失败：{e}",
                staging_dir.display()
            ))
        })?;
        let mut written_total: u64 = 0;
        let mut written_files: u64 = 0;
        for meta in validated {
            if control.is_cancelled() {
                return Err(DistributeError::PhaseTimeout { phase: "Extract", timeout_secs: 0 });
            }
            let target = staging_dir.join(&meta.rel);
            if meta.is_dir {
                fs::create_dir_all(&target).map_err(|e| {
                    DistributeError::FileOperationFailed(format!(
                        "创建目录 `{}` 失败：{e}",
                        target.display()
                    ))
                })?;
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    DistributeError::FileOperationFailed(format!(
                        "创建目录 `{}` 失败：{e}",
                        parent.display()
                    ))
                })?;
            }
            let mut entry = zip.by_index(meta.index).map_err(|e| {
                DistributeError::ArchiveRejected(format!("打开条目 #{} 失败：{e}", meta.index))
            })?;
            let mut out = File::create(&target).map_err(|e| {
                DistributeError::FileOperationFailed(format!(
                    "创建文件 `{}` 失败：{e}",
                    target.display()
                ))
            })?;
            let mut capped =
                CappedRead { inner: &mut entry, remaining: limits.max_entry_uncompressed_bytes };
            let copied = io::copy(&mut capped, &mut out).map_err(|e| {
                DistributeError::ArchiveRejected(format!(
                    "解压条目到 `{}` 失败：{e}",
                    target.display()
                ))
            })?;
            out.flush()
                .and_then(|_| out.sync_all())
                .map_err(|e| DistributeError::BackupVerifyFailed(format!("落盘 {e}")))?;
            written_total += copied;
            written_files += 1;
            if written_total > limits.max_total_uncompressed_bytes {
                return Err(DistributeError::ArchiveRejected(format!(
                    "实际解压总尺寸超过预算 {}",
                    limits.max_total_uncompressed_bytes
                )));
            }
        }
        Ok(ExtractedArchive { file_count: written_files, total_bytes: written_total })
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 健康检查 / 重启 seam
// ──────────────────────────────────────────────────────────────────────────

/// 提交前健康检查抽象：交换完成后、提交标记写入前对 `current_dir` 验收。
/// 返回 `Ok(false)` 或 `Err` 都触发**自动回滚**（真实恢复备份字节并哈希核对）。
pub trait UpgradeHealthCheck: Send + Sync + 'static {
    /// 检查刚交换进 `current_dir` 的新版本。必须轮询 `control.is_cancelled()`。
    fn check(&self, current_dir: &Path, control: &PhaseControl) -> DistributeResult<bool>;
}

/// 重启抽象：`auto_restart = true` 时**必须**注入，否则前置校验硬失败。
pub trait RestartProvider: Send + Sync + 'static {
    /// 执行真实重启动作（由装配方实现，如请求宿主重启进程）。
    fn restart(&self) -> DistributeResult<()>;
}

// ──────────────────────────────────────────────────────────────────────────
// 状态日志（journal）
// ──────────────────────────────────────────────────────────────────────────

/// 持久化 JSON 状态日志：中断后的操作可事后检视。
///
/// 里程碑序列：`Downloaded → Verified → BackedUp → Extracted → Swapped →
/// HealthChecked → Committed`（未注入健康检查则跳过 `HealthChecked`；
/// 失败追加 `Failed`，自动回滚追加 `RollingBack`/`RolledBack`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeJournal {
    /// 操作 id。
    pub operation_id: String,
    /// 升级前版本（来自 `UpgradeOptions::installed_version`，可为 None）。
    pub old_version: Option<String>,
    /// 升级目标版本。
    pub new_version: String,
    /// 更新包 SHA-256（hex；下载核对后的实际值）。
    pub package_sha256: Option<String>,
    /// 备份目录（本操作）。
    pub backup_path: String,
    /// staging 目录（本操作）。
    pub staging_path: String,
    /// 有序里程碑状态序列。
    pub states: Vec<UpgradeState>,
    /// 当前状态（最后一个里程碑或 `Checking`）。
    pub current_state: UpgradeState,
    /// commit 标记：仅提交成功后为 true，且必须落盘可复读。
    pub commit_marker: bool,
    /// 失败原因。
    pub error: Option<String>,
}

impl UpgradeJournal {
    /// 从磁盘读取日志。
    pub fn load(path: &Path) -> DistributeResult<Self> {
        let raw = fs::read_to_string(path).map_err(|e| {
            DistributeError::Journal(format!("读取日志 `{}` 失败：{e}", path.display()))
        })?;
        serde_json::from_str(&raw)
            .map_err(|e| DistributeError::Journal(format!("解析日志失败：{e}")))
    }

    /// 原子落盘：写临时文件 → flush → `sync_all` → rename。
    pub(crate) fn save(&self, path: &Path) -> DistributeResult<()> {
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| DistributeError::Journal(format!("序列化日志失败：{e}")))?;
        let tmp = path.with_extension("json.tmp");
        let mut file = File::create(&tmp).map_err(|e| {
            DistributeError::Journal(format!("创建日志临时文件 `{}` 失败：{e}", tmp.display()))
        })?;
        file.write_all(&json)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|e| DistributeError::Journal(format!("写入日志失败：{e}")))?;
        drop(file);
        fs::rename(&tmp, path).map_err(|e| {
            DistributeError::Journal(format!("日志临时文件落位 `{}` 失败：{e}", path.display()))
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 文件系统工具（真实效果 + 逐文件哈希核对）
// ──────────────────────────────────────────────────────────────────────────

pub(crate) fn io_fail(context: String) -> DistributeError {
    DistributeError::FileOperationFailed(context)
}

/// 清理结果并入错误：清理失败绝不吞掉，而是如实附加到主错误上。
fn with_cleanup_note(
    cleanup: io::Result<()>,
    cleanup_path: &Path,
    error: DistributeError,
) -> DistributeError {
    match cleanup {
        Ok(()) => error,
        Err(e) => DistributeError::FileOperationFailed(format!(
            "{error}；且清理 `{}` 失败：{e}",
            cleanup_path.display()
        )),
    }
}

/// 单文件 SHA-256（hex 小写）。
fn sha256_file(path: &Path) -> DistributeResult<String> {
    let mut file =
        File::open(path).map_err(|e| io_fail(format!("打开 `{}` 失败：{e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| io_fail(format!("读取 `{}` 失败：{e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 字节集 SHA-256（hex 小写；当前仅测试对账使用）。
#[cfg(test)]
fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// 相对路径 key（`/` 分隔，跨平台稳定）。
fn relative_key(root: &Path, path: &Path) -> DistributeResult<String> {
    let rel = path.strip_prefix(root).map_err(|e| {
        io_fail(format!("计算 `{}` 相对 `{}` 失败：{e}", path.display(), root.display()))
    })?;
    let parts: Vec<String> =
        rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Ok(parts.join("/"))
}

/// 递归枚举树内普通文件的相对路径 → SHA-256 集合。
pub(crate) fn tree_hashes(root: &Path) -> DistributeResult<BTreeMap<String, String>> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)
            .map_err(|e| io_fail(format!("读取目录 `{}` 失败：{e}", dir.display())))?;
        for entry in entries {
            let entry = entry
                .map_err(|e| io_fail(format!("读取目录项（`{}`）失败：{e}", dir.display())))?;
            let path = entry.path();
            // 用 lstat（`symlink_metadata`），绝不用会跟随链接的 `fs::metadata`：安装 / 备份 /
            // 暂存树都来自解包，而解包侧已拒符号链接条目（见 zip 校验里的 `entry.is_symlink()`
            // 分支），所以走到这里的树内任何链接都是异常。若在此跟随：一个指向祖先目录的符号
            // 链接会让这趟**无访问集、无深度上界**的栈遍历永不收敛（死循环），指向字符设备
            // （如 `/dev/zero`）的文件链接会让 `sha256_file` 的读循环无限跑，普通文件链接则被
            // 越界取哈希——三者都让升级快照 / 校验 / 回滚 / 轮 67 恢复对账整条腿挂死或误判。
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| io_fail(format!("读取元数据 `{}` 失败：{e}", path.display())))?;
            if meta.is_symlink() {
                return Err(io_fail(format!(
                    "树 `{}` 含符号链接条目 `{}`，拒绝跟随（解包树内不得有链接）",
                    root.display(),
                    path.display()
                )));
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let key = relative_key(root, &path)?;
                out.insert(key, sha256_file(&path)?);
            } else {
                return Err(io_fail(format!(
                    "树 `{}` 含非普通文件条目 `{}`，拒绝处理",
                    root.display(),
                    path.display()
                )));
            }
        }
    }
    Ok(out)
}

/// 复制单文件并 fsync（flush + `sync_all`）。
fn copy_file_fsynced(src: &Path, dst: &Path) -> DistributeResult<()> {
    let bytes =
        fs::read(src).map_err(|e| io_fail(format!("读取 `{}` 失败：{e}", src.display())))?;
    let mut file =
        File::create(dst).map_err(|e| io_fail(format!("创建 `{}` 失败：{e}", dst.display())))?;
    file.write_all(&bytes)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|e| io_fail(format!("写入并 fsync `{}` 失败：{e}", dst.display())))
}

/// 递归复制整棵树，逐文件 fsync。
pub(crate) fn copy_tree_fsynced(src: &Path, dst: &Path) -> DistributeResult<()> {
    fs::create_dir_all(dst)
        .map_err(|e| io_fail(format!("创建目录 `{}` 失败：{e}", dst.display())))?;
    let entries = fs::read_dir(src)
        .map_err(|e| io_fail(format!("读取目录 `{}` 失败：{e}", src.display())))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| io_fail(format!("读取目录项（`{}`）失败：{e}", src.display())))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_tree_fsynced(&src_path, &dst_path)?;
        } else {
            copy_file_fsynced(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// 存在才删；NotFound 视为成功（幂等清理）。
pub(crate) fn remove_tree_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 升级执行器
// ──────────────────────────────────────────────────────────────────────────

/// 单次操作的目录上下文。
struct OpContext {
    operation_id: String,
    /// `download_dir/<op>`：本操作的下载工作目录（失败即清理）。
    download_dir: PathBuf,
    /// `backup_dir/<op>`：本操作的备份 + 日志目录（失败保留供检视/回滚）。
    backup_dir: PathBuf,
    /// `install_dir/staging/<op>`：本操作的解压目录（交换后已不存在）。
    staging_dir: PathBuf,
    journal_path: PathBuf,
    journal: UpgradeJournal,
}

impl OpContext {
    fn record(&mut self, state: UpgradeState) -> DistributeResult<()> {
        self.journal.states.push(state);
        self.journal.current_state = state;
        self.journal.save(&self.journal_path)
    }

    fn mark_failed(&mut self, error: &str) -> DistributeResult<()> {
        self.journal.states.push(UpgradeState::Failed);
        self.journal.current_state = UpgradeState::Failed;
        self.journal.error = Some(error.to_string());
        self.journal.save(&self.journal_path)
    }

    fn record_error_note(&mut self, error: &str) -> DistributeResult<()> {
        self.journal.error = Some(error.to_string());
        self.journal.save(&self.journal_path)
    }

    /// 写 commit 标记并复读验证——提交成功的唯一凭据。
    fn mark_committed(&mut self) -> DistributeResult<()> {
        self.journal.states.push(UpgradeState::Committed);
        self.journal.current_state = UpgradeState::Committed;
        self.journal.commit_marker = true;
        self.journal.save(&self.journal_path)?;
        let reloaded = UpgradeJournal::load(&self.journal_path)?;
        if !reloaded.commit_marker {
            return Err(DistributeError::Journal("commit 标记写入后复读缺失，拒绝宣告成功".into()));
        }
        Ok(())
    }
}

/// 升级执行器。
pub struct UpgradeRunner {
    options: UpgradeOptions,
    progress: UpgradeProgress,
    downloader: Option<Arc<dyn Downloader>>,
    verifier: Option<Arc<dyn SignatureVerifier>>,
    extractor: Arc<dyn ArchiveExtractor>,
    health_check: Option<Arc<dyn UpgradeHealthCheck>>,
    restart_provider: Option<Arc<dyn RestartProvider>>,
    /// 上一次 `run()` 是否完成过可验证的备份（供错误兜底路径如实上报）。
    last_backed_up: bool,
}

impl UpgradeRunner {
    /// 创建新的升级执行器。
    pub fn new(options: UpgradeOptions) -> Self {
        Self {
            options,
            progress: UpgradeProgress::new(),
            downloader: None,
            verifier: None,
            extractor: Arc::new(ZipCrateExtractor),
            health_check: None,
            restart_provider: None,
            last_backed_up: false,
        }
    }

    /// 设置下载器。
    pub fn with_downloader(self, downloader: Box<dyn Downloader>) -> Self {
        self.with_downloader_arc(Arc::from(downloader))
    }

    /// 设置下载器（`Arc` 形态：装配层要把同一组件交给多个运行器/下载腿时用）。
    pub fn with_downloader_arc(mut self, downloader: Arc<dyn Downloader>) -> Self {
        self.downloader = Some(downloader);
        self
    }

    /// 设置验证器。
    pub fn with_verifier(self, verifier: Box<dyn SignatureVerifier>) -> Self {
        self.with_verifier_arc(Arc::from(verifier))
    }

    /// 设置验证器（`Arc` 形态，见 [`Self::with_downloader_arc`]）。
    pub fn with_verifier_arc(mut self, verifier: Arc<dyn SignatureVerifier>) -> Self {
        self.verifier = Some(verifier);
        self
    }

    /// 设置解压实现（默认 [`ZipCrateExtractor`]；测试可注入 seam 验证失败路径）。
    pub fn with_extractor(self, extractor: Box<dyn ArchiveExtractor>) -> Self {
        self.with_extractor_arc(Arc::from(extractor))
    }

    /// 设置解压实现（`Arc` 形态，见 [`Self::with_downloader_arc`]）。
    pub fn with_extractor_arc(mut self, extractor: Arc<dyn ArchiveExtractor>) -> Self {
        self.extractor = extractor;
        self
    }

    /// 设置提交前健康检查。
    pub fn with_health_check(self, health_check: Box<dyn UpgradeHealthCheck>) -> Self {
        self.with_health_check_arc(Arc::from(health_check))
    }

    /// 设置提交前健康检查（`Arc` 形态，见 [`Self::with_downloader_arc`]）。
    pub fn with_health_check_arc(mut self, health_check: Arc<dyn UpgradeHealthCheck>) -> Self {
        self.health_check = Some(health_check);
        self
    }

    /// 设置重启提供方（`auto_restart = true` 时必需）。
    pub fn with_restart_provider(self, provider: Box<dyn RestartProvider>) -> Self {
        self.with_restart_provider_arc(Arc::from(provider))
    }

    /// 设置重启提供方（`Arc` 形态，见 [`Self::with_downloader_arc`]）。
    pub fn with_restart_provider_arc(mut self, provider: Arc<dyn RestartProvider>) -> Self {
        self.restart_provider = Some(provider);
        self
    }

    /// 获取当前选项。
    pub fn options(&self) -> &UpgradeOptions {
        &self.options
    }

    /// 获取当前进度。
    pub fn progress(&self) -> &UpgradeProgress {
        &self.progress
    }

    /// 当前生效的安装目录（`install_dir/current`）。
    pub fn current_dir(&self) -> PathBuf {
        self.options.install_dir.join(CURRENT_DIR)
    }

    /// 前置校验：**纯检查，零文件系统副作用**。
    ///
    /// 任何组件缺失 / 清单不完整 / 安装根缺失都在这里拒绝，保证失败路径
    /// 「一个文件都不创建」。
    pub fn validate(&self) -> DistributeResult<()> {
        let manifest = &self.options.manifest;
        if manifest.url.is_empty() {
            return Err(DistributeError::InvalidBody("下载 URL 不能为空".into()));
        }
        if manifest.version.is_empty() {
            return Err(DistributeError::InvalidBody("版本号不能为空".into()));
        }
        // 降级门禁（轮 54）：见 [`ensure_not_downgrade`]。判定落在 `validate` 这个
        // 「纯检查、零文件系统副作用」段里，坏清单不会留下 staged/backup/journal 任何痕迹。
        ensure_not_downgrade(self.options.installed_version.as_deref(), &manifest.version)?;
        if manifest.signature.is_empty() {
            return Err(DistributeError::SignatureInvalid);
        }
        let sha = manifest.sha256.as_deref().unwrap_or("").trim();
        if sha.is_empty() {
            return Err(DistributeError::InvalidBody("清单缺少 sha256（包摘要必须提供）".into()));
        }
        self.downloader
            .as_ref()
            .ok_or(DistributeError::UpgradeComponentMissing { component: "Downloader" })?;
        self.verifier
            .as_ref()
            .ok_or(DistributeError::UpgradeComponentMissing { component: "SignatureVerifier" })?;
        if self.options.auto_restart && self.restart_provider.is_none() {
            return Err(DistributeError::UpgradeComponentMissing { component: "RestartProvider" });
        }
        if !self.current_dir().is_dir() {
            return Err(DistributeError::InstallRootMissing {
                path: self.current_dir().display().to_string(),
            });
        }
        if self.options.download_timeout_secs == 0
            || self.options.extract_timeout_secs == 0
            || self.options.swap_timeout_secs == 0
            || self.options.health_check_timeout_secs == 0
        {
            return Err(DistributeError::InvalidBody("各阶段超时必须 > 0".into()));
        }
        let limits = &self.options.archive;
        if limits.max_entries == 0
            || limits.max_total_uncompressed_bytes == 0
            || limits.max_entry_uncompressed_bytes == 0
        {
            return Err(DistributeError::InvalidBody("归档预算必须 > 0".into()));
        }
        Ok(())
    }

    /// 执行升级：完整状态机，每一步都是真实文件效果；失败路径清理并上浮。
    pub fn run(&mut self) -> DistributeResult<UpgradeResult> {
        // 1. 纯检查（零副作用）。
        self.validate()?;
        self.last_backed_up = false;
        self.progress = UpgradeProgress::new();

        // 2. 建立本操作目录 + 初始日志（此后中断可事后检视）。
        let mut ctx = self.begin_operation()?;
        let outcome = self.run_phases(&mut ctx);
        match outcome {
            Ok(result) => Ok(result),
            Err(error) => Err(self.handle_failure(&mut ctx, error)),
        }
    }

    /// 执行升级（带错误处理）。
    pub fn run_with_error_handling(&mut self) -> UpgradeResult {
        match self.run() {
            Ok(result) => result,
            Err(e) => {
                let backed_up = self.last_backed_up;
                self.progress.set_error(e.to_string());
                UpgradeResult {
                    success: false,
                    state: UpgradeState::Failed,
                    new_version: None,
                    error: Some(e.to_string()),
                    backed_up,
                    restarted: false,
                    operation_id: String::new(),
                }
            }
        }
    }

    // ── 内部：操作生命周期 ────────────────────────────────────────────

    fn begin_operation(&mut self) -> DistributeResult<OpContext> {
        let operation_id = uuid::Uuid::new_v4().to_string();
        let download_dir = self.options.download_dir.join(&operation_id);
        let backup_dir = self.options.backup_dir.join(&operation_id);
        let staging_root = self.options.install_dir.join(STAGING_DIR);
        let staging_dir = staging_root.join(&operation_id);
        if let Err(e) = fs::create_dir_all(&download_dir) {
            return Err(io_fail(format!("创建下载目录 `{}` 失败：{e}", download_dir.display())));
        }
        if let Err(e) = fs::create_dir_all(&backup_dir) {
            let error = io_fail(format!("创建备份目录 `{}` 失败：{e}", backup_dir.display()));
            return Err(with_cleanup_note(
                remove_tree_if_exists(&download_dir),
                &download_dir,
                error,
            ));
        }
        if let Err(e) = fs::create_dir_all(&staging_root) {
            let error = io_fail(format!("创建 staging 根 `{}` 失败：{e}", staging_root.display()));
            let error =
                with_cleanup_note(remove_tree_if_exists(&download_dir), &download_dir, error);
            return Err(with_cleanup_note(remove_tree_if_exists(&backup_dir), &backup_dir, error));
        }
        let journal_path = backup_dir.join(JOURNAL_FILE);
        let journal = UpgradeJournal {
            operation_id: operation_id.clone(),
            old_version: self.options.installed_version.clone(),
            new_version: self.options.manifest.version.clone(),
            package_sha256: None,
            backup_path: backup_dir.display().to_string(),
            staging_path: staging_dir.display().to_string(),
            states: Vec::new(),
            current_state: UpgradeState::Checking,
            commit_marker: false,
            error: None,
        };
        if let Err(e) = journal.save(&journal_path) {
            let error = e;
            let error =
                with_cleanup_note(remove_tree_if_exists(&download_dir), &download_dir, error);
            return Err(with_cleanup_note(remove_tree_if_exists(&backup_dir), &backup_dir, error));
        }
        self.progress.set_state(UpgradeState::Checking);
        Ok(OpContext { operation_id, download_dir, backup_dir, staging_dir, journal_path, journal })
    }

    fn run_phases(&mut self, ctx: &mut OpContext) -> DistributeResult<UpgradeResult> {
        let version = self.options.manifest.version.clone();

        // 3. 下载（deadline + 监控线程 + 协作取消）。
        self.progress.set_state(UpgradeState::Downloading);
        let package_path = ctx.download_dir.join(format!("update-{version}.zip"));
        self.download_bounded(&package_path)?;
        ctx.record(UpgradeState::Downloaded)?;

        // 4. 完整性门禁（与适配层下载腿共用 `verify_package` 唯一一份实现）：
        //    先 SHA-256，后注入验签器；空签名/缺席验证器已在 validate 拒绝，
        //    返回 false 一律硬失败。
        self.progress.set_state(UpgradeState::Verifying);
        let verifier = self
            .verifier
            .clone()
            .ok_or(DistributeError::UpgradeComponentMissing { component: "SignatureVerifier" })?;
        let actual_sha = verify_package(&self.options.manifest, &package_path, verifier.as_ref())?;
        ctx.journal.package_sha256 = Some(actual_sha);
        ctx.record(UpgradeState::Verified)?;

        // 5. 真实备份：递归复制 + 逐文件 fsync + 备份树哈希集合 == 源树才作数。
        self.progress.set_state(UpgradeState::BackingUp);
        let current_dir = self.current_dir();
        let source_hashes = tree_hashes(&current_dir)?;
        if source_hashes.is_empty() {
            return Err(DistributeError::InstallRootMissing {
                path: format!("{}（安装树为空，拒绝无底可回滚的升级）", current_dir.display()),
            });
        }
        let backup_tree = ctx.backup_dir.join(BACKUP_TREE_DIR);
        copy_tree_fsynced(&current_dir, &backup_tree)?;
        let backup_hashes = tree_hashes(&backup_tree)?;
        if backup_hashes != source_hashes {
            return Err(DistributeError::BackupVerifyFailed(format!(
                "备份树与源树哈希不一致（源 {} 项 / 备份 {} 项）",
                source_hashes.len(),
                backup_hashes.len()
            )));
        }
        self.last_backed_up = true;
        ctx.record(UpgradeState::BackedUp)?;

        // 6. 真实解压到 staging（zip-slip / bomb / symlink 防护 + deadline；
        //    解压错误一律致命，不吞）。
        self.progress.set_state(UpgradeState::Extracting);
        let extract_control = PhaseControl::new(
            Instant::now() + Duration::from_secs(self.options.extract_timeout_secs),
        );
        let report = self.extractor.extract(
            &package_path,
            &ctx.staging_dir,
            &self.options.archive,
            &extract_control,
        )?;
        if extract_control.is_past_deadline() {
            return Err(DistributeError::PhaseTimeout {
                phase: "Extract",
                timeout_secs: self.options.extract_timeout_secs,
            });
        }
        let staged_hashes = tree_hashes(&ctx.staging_dir)?;
        if staged_hashes.is_empty() {
            return Err(DistributeError::ArchiveRejected(format!(
                "解压产物为空（统计文件 {} 个），拒绝交换空树",
                report.file_count
            )));
        }
        ctx.record(UpgradeState::Extracted)?;

        // 7. 原子交换：current → previous，staging/<op> → current（同一安装根、
        //    同一文件系统；rename 失败即还原，绝不做部分非原子复制）。
        self.progress.set_state(UpgradeState::Replacing);
        let swap_deadline = Instant::now() + Duration::from_secs(self.options.swap_timeout_secs);
        if Instant::now() >= swap_deadline {
            return Err(DistributeError::PhaseTimeout {
                phase: "Swap",
                timeout_secs: self.options.swap_timeout_secs,
            });
        }
        let previous_dir = self.options.install_dir.join(PREVIOUS_DIR);
        if previous_dir.exists() {
            // 上一操作的 previous 副本已被其备份目录覆盖记录，删除后腾位。
            remove_tree_if_exists(&previous_dir)
                .map_err(|e| DistributeError::SwapFailed(format!("清理旧 previous 失败：{e}")))?;
        }
        fs::rename(&current_dir, &previous_dir).map_err(|e| {
            DistributeError::SwapFailed(format!(
                "移出现装 `{}` → `{}` 失败（跨文件系统时 rename 必败，拒绝降级为部分复制）：{e}",
                current_dir.display(),
                previous_dir.display()
            ))
        })?;
        if let Err(e) = fs::rename(&ctx.staging_dir, &current_dir) {
            // 还原现场：previous → current。
            let restore = fs::rename(&previous_dir, &current_dir);
            return Err(match restore {
                Ok(()) => DistributeError::SwapFailed(format!(
                    "换入新树失败（已还原现场）：{e}"
                )),
                Err(restore_err) => DistributeError::SwapFailed(format!(
                    "换入新树失败：{e}；且还原现场失败：{restore_err}，安装根处于中间态，需人工按日志 `{}` 处理",
                    ctx.journal_path.display()
                )),
            });
        }
        ctx.record(UpgradeState::Swapped)?;

        // 8. 健康检查（注入才跑；失败或超时 → 自动回滚，真实恢复旧字节）。
        if let Some(health) = self.health_check.clone() {
            self.progress.set_state(UpgradeState::HealthChecking);
            let health_control = PhaseControl::new(
                Instant::now() + Duration::from_secs(self.options.health_check_timeout_secs),
            );
            let outcome = health.check(&current_dir, &health_control);
            let healthy = match outcome {
                Ok(flag) => flag && !health_control.is_past_deadline(),
                Err(e) => {
                    let note = format!("健康检查报错：{e}");
                    return self.rollback_after_swap(ctx, &source_hashes, note);
                }
            };
            if !healthy {
                let note = if health_control.is_past_deadline() {
                    format!("健康检查超时（{}s）", self.options.health_check_timeout_secs)
                } else {
                    "健康检查返回不健康".to_string()
                };
                return self.rollback_after_swap(ctx, &source_hashes, note);
            }
            ctx.record(UpgradeState::HealthChecked)?;
        }

        // 9. 提交门禁：live current 树哈希必须等于新包（staging 期）哈希，
        //    然后 commit 标记落盘 + 复读，才允许 success = true。
        let live_hashes = tree_hashes(&current_dir)?;
        if live_hashes != staged_hashes {
            return self.rollback_after_swap(
                ctx,
                &source_hashes,
                "交换后 current 哈希与更新包不一致".to_string(),
            );
        }
        ctx.mark_committed()?;
        self.progress.set_state(UpgradeState::Completed);

        // 10. 成功后清理本操作下载工作目录（备份与日志保留供检视）。
        remove_tree_if_exists(&ctx.download_dir).map_err(|e| {
            DistributeError::FileOperationFailed(format!(
                "升级已提交，但清理下载目录 `{}` 失败：{e}",
                ctx.download_dir.display()
            ))
        })?;

        // 11. 重启 seam：auto_restart 时 provider 已由 validate 保证存在。
        let mut restarted = false;
        if self.options.auto_restart {
            self.restart()?;
            restarted = true;
        }

        Ok(UpgradeResult {
            success: true,
            state: UpgradeState::Completed,
            new_version: Some(version),
            error: None,
            backed_up: true,
            restarted,
            operation_id: ctx.operation_id.clone(),
        })
    }

    /// 下载：监控线程 + deadline + 协作取消；实现只在自由函数
    /// [`download_bounded`] 一份（轮 40 提取——适配层「真实下载腿」同一份）。
    ///
    /// 超时后：置取消标志，在 [`CANCEL_GRACE`] 内等待 worker 收尾（协作式
    /// Downloader 会很快返回）；worker 返回后统一走失败路径清理 temp 目录，
    /// 清理失败会由 [`UpgradeRunner::handle_failure`] 上浮。
    fn download_bounded(&mut self, dest: &Path) -> DistributeResult<PathBuf> {
        let downloader = self
            .downloader
            .clone()
            .ok_or(DistributeError::UpgradeComponentMissing { component: "Downloader" })?;
        let url = self.options.manifest.url.clone();
        let timeout_secs = self.options.download_timeout_secs;
        let mut progress = |downloaded: u64, total: u64| {
            self.progress.set_download_progress(downloaded, total);
        };
        download_bounded(downloader, url, dest.to_path_buf(), timeout_secs, &mut progress)
    }

    /// 交换之后失败 → 自动回滚：把 previous 移回 current，并与备份树（= 升级
    /// 前源树）逐文件哈希核对；恢复不属实就如实报错，绝不谎报。
    fn rollback_after_swap(
        &mut self,
        ctx: &mut OpContext,
        source_hashes: &BTreeMap<String, String>,
        reason: String,
    ) -> DistributeResult<UpgradeResult> {
        let _ = ctx.record(UpgradeState::RollingBack);
        let current_dir = self.current_dir();
        let previous_dir = self.options.install_dir.join(PREVIOUS_DIR);
        let quarantine =
            self.options.install_dir.join(format!("{FAILED_DIR_PREFIX}{}", ctx.operation_id));
        let mut notes: Vec<String> = Vec::new();

        // 1. 隔离损坏的新树。
        let _ = remove_tree_if_exists(&quarantine); // 旧隔离残留尽力清，失败在下一步暴露
        if let Err(e) = fs::rename(&current_dir, &quarantine) {
            notes.push(format!("隔离新树失败：{e}"));
            let combined = notes.join("；");
            let _ = ctx.record_error_note(&format!("{reason}；自动回滚未完成：{combined}"));
            self.progress.set_state(UpgradeState::Failed);
            return Err(DistributeError::RollbackFailed(format!(
                "{reason}；自动回滚未完成：{combined}"
            )));
        }

        // 2. previous → current（原子移回旧树）。
        if let Err(e) = fs::rename(&previous_dir, &current_dir) {
            // 尽力回退隔离，保住现场。
            if let Err(undo_err) = fs::rename(&quarantine, &current_dir) {
                notes.push(format!("还原隔离目录也失败：{undo_err}"));
            }
            let combined = notes.join("；");
            let _ = ctx.record_error_note(&format!("{reason}；自动回滚未完成：{combined}"));
            self.progress.set_state(UpgradeState::Failed);
            return Err(DistributeError::RollbackFailed(format!(
                "{reason}；把 previous 移回 current 失败：{e}；{combined}，安装根处于中间态，需人工按日志 `{}` 处理",
                ctx.journal_path.display()
            )));
        }

        // 3. 核对恢复字节 == 备份树 == 升级前源树（逐文件 SHA-256）。
        match tree_hashes(&current_dir) {
            Ok(restored) if &restored == source_hashes => {}
            Ok(_) => notes.push("恢复后的树哈希与升级前源树不一致".to_string()),
            Err(e) => notes.push(format!("恢复树哈希读取失败：{e}")),
        }

        // 4. 删除隔离的新树。
        if let Err(e) = remove_tree_if_exists(&quarantine) {
            notes.push(format!("清理隔离目录 `{}` 失败：{e}", quarantine.display()));
        }

        if !notes.is_empty() {
            let combined = notes.join("；");
            let _ = ctx.record_error_note(&format!("{reason}；自动回滚存在未完成项：{combined}"));
            self.progress.set_state(UpgradeState::Failed);
            return Err(DistributeError::RollbackFailed(format!(
                "{reason}；自动回滚存在未完成项：{combined}"
            )));
        }
        let _ = ctx.record(UpgradeState::RolledBack);
        self.progress.set_state(UpgradeState::RolledBack);
        Err(DistributeError::HealthCheckFailed(format!(
            "{reason}；已自动回滚，旧版本字节已恢复并哈希核对"
        )))
    }

    /// 失败路径统一处理：清理 temp/staging 残留、日志记录失败、清理失败上浮。
    fn handle_failure(&mut self, ctx: &mut OpContext, error: DistributeError) -> DistributeError {
        let mut notes: Vec<String> = Vec::new();
        if let Err(e) = remove_tree_if_exists(&ctx.download_dir) {
            notes.push(format!("清理下载临时目录 `{}` 失败：{e}", ctx.download_dir.display()));
        }
        if let Err(e) = remove_tree_if_exists(&ctx.staging_dir) {
            notes.push(format!("清理 staging 目录 `{}` 失败：{e}", ctx.staging_dir.display()));
        }
        let message = error.to_string();
        if let Err(je) = ctx.mark_failed(&message) {
            notes.push(format!("记录失败日志失败：{je}"));
        }
        // 自动回滚已完成时保留 RolledBack（更精确的终态），否则标 Failed。
        if self.progress.state != UpgradeState::RolledBack {
            self.progress.set_state(UpgradeState::Failed);
        }
        if notes.is_empty() {
            error
        } else {
            DistributeError::FileOperationFailed(format!(
                "升级失败：{message}；且收尾失败：{}",
                notes.join("；")
            ))
        }
    }

    /// 回滚到最近一次操作的备份：**真实恢复**备份树字节（复制到 staging →
    /// 哈希核对 → 原子换入 current → 再核对），不再是「尚未接入」的假错误。
    pub fn rollback(&mut self) -> DistributeResult<()> {
        self.progress.set_state(UpgradeState::RollingBack);
        let Some((op_backup_dir, journal_path)) = self.find_latest_backup() else {
            let message = "备份目录下没有任何可回滚的备份（含 tree/ 的 journal 操作目录）";
            self.progress.set_error(message.to_string());
            return Err(DistributeError::RollbackFailed(message.into()));
        };
        let backup_tree = op_backup_dir.join(BACKUP_TREE_DIR);
        if !backup_tree.is_dir() {
            let message = format!("找到日志但备份树 `{}` 缺失，无法恢复", backup_tree.display());
            self.progress.set_error(message.clone());
            return Err(DistributeError::RollbackFailed(message));
        }
        let operation_id = op_backup_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".into());
        let result = self.restore_from_backup(&backup_tree, &operation_id);
        match result {
            Ok(()) => {
                // 日志尽力更新（读不回则不影响回滚事实本身）。
                if let Ok(mut journal) = UpgradeJournal::load(&journal_path) {
                    journal.states.push(UpgradeState::RolledBack);
                    journal.current_state = UpgradeState::RolledBack;
                    let _ = journal.save(&journal_path);
                }
                self.progress.set_state(UpgradeState::RolledBack);
                Ok(())
            }
            Err(e) => {
                self.progress.set_error(e.to_string());
                Err(e)
            }
        }
    }

    /// 按备份树恢复 current：复制 → 核对 → 原子换入 → 再核对 → 清理损坏树。
    fn restore_from_backup(
        &mut self,
        backup_tree: &Path,
        operation_id: &str,
    ) -> DistributeResult<()> {
        restore_backup_tree(&self.options.install_dir, backup_tree, operation_id)
    }

    /// 找 backup_dir 下最近（按 journal 文件修改时间）含备份树的操作目录。
    fn find_latest_backup(&self) -> Option<(PathBuf, PathBuf)> {
        let entries = fs::read_dir(&self.options.backup_dir).ok()?;
        let mut candidates: Vec<(std::time::SystemTime, PathBuf, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            let journal_path = dir.join(JOURNAL_FILE);
            if dir.is_dir() && journal_path.is_file() {
                let modified = fs::metadata(&journal_path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                candidates.push((modified, dir, journal_path));
            }
        }
        candidates.sort_by_key(|(t, _, _)| *t);
        candidates.pop().map(|(_, dir, journal)| (dir, journal))
    }

    /// 重启：`auto_restart = false` 直接返回 Ok（用户自己重启）；为 true 时
    /// 必须已注入 `RestartProvider`（validate 已保证，这里再兜底），并把
    /// provider 的真实结果如实返回。
    pub fn restart(&mut self) -> DistributeResult<()> {
        if !self.options.auto_restart {
            return Ok(());
        }
        let provider = self
            .restart_provider
            .clone()
            .ok_or(DistributeError::UpgradeComponentMissing { component: "RestartProvider" })?;
        provider.restart().map_err(|e| {
            DistributeError::RestartFailed(format!("注入的 RestartProvider 返回失败：{e}"))
        })
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 工厂函数
// ──────────────────────────────────────────────────────────────────────────

/// 从备份树**真实**恢复 `current`：复制 → 核对 → 原子换入 → 再核对 → 清理损坏树。
///
/// 这是 [`UpgradeRunner::restore_from_backup`] 与恢复对账腿
/// （[`crate::UpgradeReconciler::restore`]）**唯一一份**实现：两条路都必须走
/// 逐文件哈希核对，任何一份分叉都会让「重启恢复」与「运行期回滚」的字节一致性
/// 判定漂移。`install_dir` 内建模为 staging/current/previous + failed- 隔离目录。
pub(crate) fn restore_backup_tree(
    install_dir: &Path,
    backup_tree: &Path,
    operation_id: &str,
) -> DistributeResult<()> {
    let want = tree_hashes(backup_tree)?;
    if want.is_empty() {
        return Err(DistributeError::RollbackFailed("备份树为空，拒绝恢复".into()));
    }
    let staging_root = install_dir.join(STAGING_DIR);
    fs::create_dir_all(&staging_root).map_err(|e| io_fail(format!("创建 staging 根失败：{e}")))?;
    let restore_dir = staging_root.join(format!("rollback-{operation_id}"));
    remove_tree_if_exists(&restore_dir)
        .map_err(|e| io_fail(format!("清理残留恢复目录失败：{e}")))?;
    copy_tree_fsynced(backup_tree, &restore_dir)?;
    let staged = tree_hashes(&restore_dir)?;
    if staged != want {
        let _ = remove_tree_if_exists(&restore_dir);
        return Err(DistributeError::BackupVerifyFailed(
            "恢复暂存树与备份树哈希不一致，拒绝换入".into(),
        ));
    }
    let current_dir = install_dir.join(CURRENT_DIR);
    let quarantine = install_dir.join(format!("{FAILED_DIR_PREFIX}rollback-{operation_id}"));
    remove_tree_if_exists(&quarantine).map_err(|e| io_fail(format!("清理旧隔离目录失败：{e}")))?;
    if current_dir.exists() {
        fs::rename(&current_dir, &quarantine).map_err(|e| {
            DistributeError::RollbackFailed(format!(
                "移出现装 `{}` 失败：{e}",
                current_dir.display()
            ))
        })?;
    }
    if let Err(e) = fs::rename(&restore_dir, &current_dir) {
        let undo =
            if current_dir.exists() { Ok(()) } else { fs::rename(&quarantine, &current_dir) };
        return Err(match undo {
            Ok(()) => DistributeError::RollbackFailed(format!("换入恢复树失败（现场已保住）：{e}")),
            Err(undo_err) => DistributeError::RollbackFailed(format!(
                "换入恢复树失败：{e}；且还原现场失败：{undo_err}，需人工处理"
            )),
        });
    }
    let restored = tree_hashes(&current_dir)?;
    if restored != want {
        return Err(DistributeError::BackupVerifyFailed(
            "换入后的 current 哈希与备份树不一致（备份本身可信，未回退）".into(),
        ));
    }
    remove_tree_if_exists(&quarantine).map_err(|e| {
        DistributeError::RollbackFailed(format!(
            "恢复已完成且哈希核对通过，但清理隔离损坏树 `{}` 失败：{e}",
            quarantine.display()
        ))
    })?;
    Ok(())
}

/// 降级门禁（轮 54）：**默认拒绝**把 `target` 装到低于已安装版本 `installed`。
///
/// - 序关系用 `tauron_market::is_downgrade`——全仓唯一一份版本算术（数值分段比较，
///   对 ≥2³¹ 分段不截断）。本模块刻意不再写第二套比较：两套算术意味着改一处不会让
///   另一处变红。
/// - `installed` 为 `None` = 没有基线可判（首次装配 / 装配方未上报安装版本），如实放行，
///   不假装知道不知道的事。
/// - 回滚不经这里：[`UpgradeRunner::rollback`] 恢复既有树、不下载新包。
/// - 「同版本重装」不算降级（`is_downgrade` 只在 target 严格更低时为真），放行。
pub fn ensure_not_downgrade(installed: Option<&str>, target: &str) -> DistributeResult<()> {
    if let Some(current) = installed {
        if tauron_market::is_downgrade(current, target) {
            return Err(DistributeError::DowngradeRejected {
                current: current.to_string(),
                target: target.to_string(),
            });
        }
    }
    Ok(())
}

/// 创建升级执行器。
pub fn create_upgrade_runner(options: UpgradeOptions) -> UpgradeRunner {
    UpgradeRunner::new(options)
}

/// 创建默认升级执行器（注意：默认清单不完整，`validate`/`run` 会如实拒绝）。
pub fn create_default_upgrade_runner() -> UpgradeRunner {
    UpgradeRunner::new(UpgradeOptions::default())
}

/// 有界下载：监控线程 + deadline + 协作取消。
///
/// `UpgradeRunner::download_bounded` 与适配层「真实下载腿」共用这**唯一一份**
/// 实现（轮 40 从方法体提取；此前只有 run_phases 一条调用路径）。语义与提取
/// 前逐字一致：deadline 之后到达的结果（无论成败）一律按超时处理；不合作的
/// `Downloader` 在取消后只给 [`CANCEL_GRACE`] 宽限，其可能仍在写入的目录由
/// 失败路径清理。
pub fn download_bounded(
    downloader: Arc<dyn Downloader>,
    url: String,
    dest: PathBuf,
    timeout_secs: u64,
    on_progress: &mut dyn FnMut(u64, u64),
) -> DistributeResult<PathBuf> {
    let control = PhaseControl::new(Instant::now() + Duration::from_secs(timeout_secs));
    let worker_control = control.clone();
    let (res_tx, res_rx) = mpsc::channel::<DistributeResult<PathBuf>>();
    let (prog_tx, prog_rx) = mpsc::channel::<(u64, u64)>();
    let worker = std::thread::spawn(move || {
        let mut progress_cb = |downloaded: u64, total: u64| {
            let _ = prog_tx.send((downloaded, total));
        };
        let result = downloader.download(&url, &dest, &worker_control, &mut progress_cb);
        let _ = res_tx.send(result);
    });
    let deadline = control.deadline();
    loop {
        while let Ok((downloaded, total)) = prog_rx.try_recv() {
            on_progress(downloaded, total);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match res_rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(result) => {
                while let Ok((downloaded, total)) = prog_rx.try_recv() {
                    on_progress(downloaded, total);
                }
                let _ = worker.join();
                // deadline 之后到达的结果（无论成败）一律按超时处理：
                // 语义统一，超时错误不依赖 worker 自身的返回时序。
                if control.is_past_deadline() {
                    return Err(DistributeError::PhaseTimeout { phase: "Download", timeout_secs });
                }
                return result;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                return Err(DistributeError::FileOperationFailed(
                    "下载 worker 线程未返回结果即退出".into(),
                ));
            }
        }
    }
    // deadline 已过：取消并给协作收尾一个宽限期。
    control.cancel();
    match res_rx.recv_timeout(CANCEL_GRACE) {
        Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            let _ = worker.join();
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // 不合作的 Downloader：如实上报，不强删其可能正在写入的目录
            // （由失败路径清理，清理失败会上浮）。
        }
    }
    Err(DistributeError::PhaseTimeout { phase: "Download", timeout_secs })
}

/// 包完整性门禁：SHA-256 必须与清单 `sha256` 逐字相符（先），随后交给注入的
/// 验签器（空签名 / 返回 false / 报错一律硬失败）；返回实际包摘要（hex 小写）。
///
/// `UpgradeRunner::run_phases` 与适配层「真实下载腿」共用这**唯一一份**实现
/// （轮 40 从 run_phases 内联块提取——两份会漂，一份不会）。
pub fn verify_package(
    manifest: &UpdateManifest,
    package_path: &Path,
    verifier: &dyn SignatureVerifier,
) -> DistributeResult<String> {
    if manifest.signature.is_empty() {
        return Err(DistributeError::SignatureInvalid);
    }
    let actual_sha = sha256_file(package_path)?;
    let expected_sha = manifest.sha256.as_deref().unwrap_or("").trim().to_lowercase();
    if expected_sha != actual_sha {
        return Err(DistributeError::PackageHashMismatch {
            expected: expected_sha,
            actual: actual_sha,
        });
    }
    let vctx = VerificationContext {
        package_sha256_hex: actual_sha.clone(),
        signature_algorithm: manifest.signature_algorithm.as_deref(),
        public_key_id: manifest.public_key_id.as_deref(),
    };
    let valid = verifier.verify(package_path, &manifest.signature, &vctx)?;
    if !valid {
        return Err(DistributeError::SignatureInvalid);
    }
    Ok(actual_sha)
}

// ──────────────────────────────────────────────────────────────────────────
// Mock 下载器和验证器（仅供测试；不进生产可达路径）
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// 测试下载器：把预设字节写到目标路径，并汇报进度。
    pub struct MockDownloader {
        pub payload: Vec<u8>,
    }

    impl MockDownloader {
        pub fn new(payload: Vec<u8>) -> Self {
            Self { payload }
        }
    }

    impl Downloader for MockDownloader {
        fn download(
            &self,
            _url: &str,
            dest_path: &Path,
            control: &PhaseControl,
            progress_callback: &mut dyn FnMut(u64, u64),
        ) -> DistributeResult<PathBuf> {
            if control.is_cancelled() {
                return Err(DistributeError::PhaseTimeout { phase: "Download", timeout_secs: 0 });
            }
            let total = self.payload.len() as u64;
            progress_callback(0, total);
            fs::write(dest_path, &self.payload)
                .map_err(|e| DistributeError::FileOperationFailed(format!("下载写盘失败：{e}")))?;
            progress_callback(total, total);
            Ok(dest_path.to_path_buf())
        }
    }

    /// 测试签名验证器：按布尔开关返回，绝不「缺席即通过」。
    pub struct MockVerifier {
        pub should_succeed: bool,
    }

    impl MockVerifier {
        pub fn new(should_succeed: bool) -> Self {
            Self { should_succeed }
        }
    }

    impl Default for MockVerifier {
        fn default() -> Self {
            Self::new(true)
        }
    }

    impl SignatureVerifier for MockVerifier {
        fn verify(
            &self,
            _file_path: &Path,
            _signature: &str,
            _context: &VerificationContext<'_>,
        ) -> DistributeResult<bool> {
            Ok(self.should_succeed)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    // ── 测试基建：真实 tempdir + 真实 zip fixture ────────────────────

    fn build_zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data) in entries {
            writer.start_file(*name, opts).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    /// 把 zip 中央目录里指定条目的 unix mode 改写为符号链接（0o120644），
    /// 构造执行器必须拒绝的条目（zip crate 写入端会丢掉文件类型位，只能改字节）。
    fn patch_entry_to_symlink(bytes: &mut [u8], name: &str) {
        const SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
        let mut pos = 0usize;
        while let Some(i) = bytes[pos..].windows(4).position(|w| w == SIG) {
            let record = pos + i;
            let name_len = u16::from_le_bytes([bytes[record + 28], bytes[record + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[record + 30], bytes[record + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[record + 32], bytes[record + 33]]) as usize;
            let name_start = record + 46;
            let entry_name =
                String::from_utf8_lossy(&bytes[name_start..name_start + name_len]).into_owned();
            if entry_name == name {
                bytes[record + 5] = 3; // host system = Unix
                let mode: u32 = 0o120_644u32 << 16;
                bytes[record + 38..record + 42].copy_from_slice(&mode.to_le_bytes());
                return;
            }
            pos = name_start + name_len + extra_len + comment_len;
        }
        panic!("中央目录里找不到条目 `{name}`");
    }

    struct Harness {
        tmp: tempfile::TempDir,
    }

    impl Harness {
        fn new() -> Self {
            Self { tmp: tempfile::tempdir().unwrap() }
        }

        fn root(&self) -> &Path {
            self.tmp.path()
        }

        fn options(&self, manifest: UpdateManifest) -> UpgradeOptions {
            UpgradeOptions {
                manifest,
                download_dir: self.root().join("download"),
                install_dir: self.root().join("install"),
                backup_dir: self.root().join("backup"),
                auto_restart: false,
                download_timeout_secs: 5,
                extract_timeout_secs: 10,
                swap_timeout_secs: 5,
                health_check_timeout_secs: 5,
                archive: ArchiveLimits::default(),
                installed_version: Some("1.0.0".into()),
            }
        }

        /// 预置安装树 `install/current/...`。
        fn seed_current(&self, files: &[(&str, &[u8])]) {
            let current = self.root().join("install").join(CURRENT_DIR);
            for (rel, bytes) in files {
                let path = current.join(rel);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, bytes).unwrap();
            }
        }

        fn journal(&self, operation_id: &str) -> UpgradeJournal {
            let path = self.root().join("backup").join(operation_id).join(JOURNAL_FILE);
            UpgradeJournal::load(&path).expect("日志必须存在且可解析")
        }

        fn current_tree(&self) -> BTreeMap<String, String> {
            tree_hashes(&self.root().join("install").join(CURRENT_DIR)).unwrap()
        }
    }

    fn manifest_for(package: &[u8]) -> UpdateManifest {
        UpdateManifest {
            version: "2.0.0".into(),
            url: "https://example.invalid/update-2.0.0.zip".into(),
            signature: "deadbeef-signature".into(),
            release_date: "2026-10-01T00:00:00Z".into(),
            platform_notes: BTreeMap::new(),
            sha256: Some(sha256_bytes(package)),
            signature_algorithm: Some("ed25519".into()),
            public_key_id: Some("release-key-1".into()),
            min_host_version: Some("1.1.0".into()),
            abi: None,
            rollback_policy: Some("auto-on-health-failure".into()),
        }
    }

    fn tree_of(files: &[(&str, &[u8])]) -> BTreeMap<String, String> {
        files.iter().map(|(k, v)| (k.to_string(), sha256_bytes(v))).collect()
    }

    fn dir_names(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    struct FixedHealth(bool);
    impl UpgradeHealthCheck for FixedHealth {
        fn check(&self, _dir: &Path, _control: &PhaseControl) -> DistributeResult<bool> {
            Ok(self.0)
        }
    }

    struct CountingRestart {
        count: std::sync::atomic::AtomicUsize,
    }
    impl CountingRestart {
        fn new() -> Self {
            Self { count: std::sync::atomic::AtomicUsize::new(0) }
        }
    }

    struct RecordingVerifier {
        seen: std::sync::Mutex<Option<(String, String, Option<String>, Option<String>)>>,
    }
    impl RecordingVerifier {
        fn new() -> Self {
            Self { seen: std::sync::Mutex::new(None) }
        }
    }
    impl SignatureVerifier for RecordingVerifier {
        fn verify(
            &self,
            file_path: &Path,
            signature: &str,
            context: &VerificationContext<'_>,
        ) -> DistributeResult<bool> {
            let actual_file_sha = sha256_file(file_path)?;
            assert_eq!(
                actual_file_sha, context.package_sha256_hex,
                "上下文里的包摘要必须等于文件真实内容（诚实交付）"
            );
            *self.seen.lock().unwrap() = Some((
                signature.to_string(),
                context.package_sha256_hex.clone(),
                context.signature_algorithm.map(|s| s.to_string()),
                context.public_key_id.map(|s| s.to_string()),
            ));
            Ok(true)
        }
    }

    struct FailExtractor;
    impl ArchiveExtractor for FailExtractor {
        fn extract(
            &self,
            _archive: &Path,
            _staging: &Path,
            _limits: &ArchiveLimits,
            _control: &PhaseControl,
        ) -> DistributeResult<ExtractedArchive> {
            Err(DistributeError::ArchiveRejected("注入式解压失败（测试 seam）".into()))
        }
    }

    struct SlowDownloader;
    impl Downloader for SlowDownloader {
        fn download(
            &self,
            _url: &str,
            _dest: &Path,
            control: &PhaseControl,
            _progress: &mut dyn FnMut(u64, u64),
        ) -> DistributeResult<PathBuf> {
            // 协作式慢下载器：轮询取消，永不写盘。
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(30) {
                if control.is_cancelled() {
                    return Err(DistributeError::PhaseTimeout {
                        phase: "Download",
                        timeout_secs: 0,
                    });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(DistributeError::PhaseTimeout { phase: "Download", timeout_secs: 30 })
        }
    }

    /// 组装一个「一切就绪」的成功 runner 所需素材。
    struct SuccessFixture {
        h: Harness,
        package: Vec<u8>,
        old_files: Vec<(&'static str, &'static [u8])>,
    }

    fn success_fixture() -> SuccessFixture {
        let h = Harness::new();
        let old_files =
            vec![("app.bin", &b"old-app-bytes-v1"[..]), ("data/cfg.txt", &b"cfg-old"[..])];
        h.seed_current(&old_files);
        let package = build_zip_bytes(&[
            ("app.bin", b"new-app-bytes-v2".as_slice()),
            ("README.md", b"hello v2".as_slice()),
            ("sub/note.txt", b"nested note".as_slice()),
        ]);
        SuccessFixture { h, package, old_files }
    }

    // ── 成功路径：前后文件哈希对账 + 日志全序列 ──────────────────────

    #[test]
    fn run_success_performs_real_file_effects_and_journal_sequence() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let expected_old = tree_of(&fx.old_files);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)))
            .with_health_check(Box::new(FixedHealth(true)));
        let result = runner.run().expect("真实全链路必须成功");

        assert!(result.success);
        assert_eq!(result.state, UpgradeState::Completed);
        assert!(result.backed_up);
        assert!(!result.restarted);
        assert_eq!(result.new_version.as_deref(), Some("2.0.0"));

        // 新旧字节对账：current 必须恰好是新包内容（旧文件被整体替换）。
        let expected_new: BTreeMap<String, String> = [
            ("app.bin", sha256_bytes(b"new-app-bytes-v2")),
            ("README.md", sha256_bytes(b"hello v2")),
            ("sub/note.txt", sha256_bytes(b"nested note")),
        ]
        .into_iter()
        .map(|(name, hash)| (name.to_string(), hash))
        .collect();
        assert_eq!(fx.h.current_tree(), expected_new);
        assert_eq!(
            fs::read(fx.h.root().join("install").join(CURRENT_DIR).join("app.bin")).unwrap(),
            b"new-app-bytes-v2"
        );

        // 备份树 == 升级前源树（逐文件哈希）。
        let backup_tree =
            fx.h.root().join("backup").join(&result.operation_id).join(BACKUP_TREE_DIR);
        assert_eq!(tree_hashes(&backup_tree).unwrap(), expected_old);
        // previous 保留被替换的旧树（与备份同源）。
        assert_eq!(
            tree_hashes(&fx.h.root().join("install").join(PREVIOUS_DIR)).unwrap(),
            expected_old
        );

        // 日志：完整有序里程碑序列 + commit 标记落盘 + 包摘要。
        let journal = fx.h.journal(&result.operation_id);
        assert_eq!(
            journal.states,
            vec![
                UpgradeState::Downloaded,
                UpgradeState::Verified,
                UpgradeState::BackedUp,
                UpgradeState::Extracted,
                UpgradeState::Swapped,
                UpgradeState::HealthChecked,
                UpgradeState::Committed,
            ]
        );
        assert!(journal.commit_marker);
        assert_eq!(journal.package_sha256.as_deref(), Some(sha256_bytes(&fx.package).as_str()));
        assert_eq!(journal.old_version.as_deref(), Some("1.0.0"));
        assert_eq!(journal.new_version, "2.0.0");
        assert!(journal.error.is_none());

        // 成功后：本操作下载工作目录清理；staging 无残留操作目录。
        assert!(!fx.h.root().join("download").join(&result.operation_id).exists());
        assert!(!fx.h.root().join("install").join(STAGING_DIR).join(&result.operation_id).exists());
    }

    #[test]
    fn run_success_without_health_check_still_commits() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let result = runner.run().expect("未注入健康检查时允许直接提交");
        let journal = fx.h.journal(&result.operation_id);
        assert_eq!(
            journal.states,
            vec![
                UpgradeState::Downloaded,
                UpgradeState::Verified,
                UpgradeState::BackedUp,
                UpgradeState::Extracted,
                UpgradeState::Swapped,
                UpgradeState::Committed,
            ]
        );
    }

    #[test]
    fn verifier_receives_honest_context_and_signature() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let verifier = Arc::new(RecordingVerifier::new());
        let recording: Arc<dyn SignatureVerifier> = verifier.clone();
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(CountingPassThrough(recording)));
        runner.run().unwrap();
        let seen = verifier.seen.lock().unwrap();
        let (signature, sha, algorithm, key_id) = seen.as_ref().expect("验证器必须被调用");
        assert_eq!(signature, "deadbeef-signature");
        assert_eq!(sha, &sha256_bytes(&fx.package));
        assert_eq!(algorithm.as_deref(), Some("ed25519"));
        assert_eq!(key_id.as_deref(), Some("release-key-1"));
    }

    /// 把 Arc<dyn SignatureVerifier> 适配成 trait object 的中转（测试注入用）。
    struct CountingPassThrough(Arc<dyn SignatureVerifier>);
    impl SignatureVerifier for CountingPassThrough {
        fn verify(&self, f: &Path, s: &str, c: &VerificationContext<'_>) -> DistributeResult<bool> {
            self.0.verify(f, s, c)
        }
    }

    // ── 组件缺失：类型化错误 + 零文件创建 ────────────────────────────

    #[test]
    fn run_without_downloader_is_typed_error_and_creates_no_files() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(&err, DistributeError::UpgradeComponentMissing { component } if *component == "Downloader"),
            "实际错误：{err:?}"
        );
        // 除预置的安装根外，没有创建任何下载/备份目录。
        assert_eq!(dir_names(fx.h.root()), vec!["install".to_string()]);
    }

    #[test]
    fn run_without_verifier_is_hard_failure() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(&err, DistributeError::UpgradeComponentMissing { component } if *component == "SignatureVerifier"),
            "实际错误：{err:?}"
        );
        assert_eq!(dir_names(fx.h.root()), vec!["install".to_string()]);
    }

    #[test]
    fn auto_restart_without_provider_is_typed_error_without_effects() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let mut options = fx.h.options(manifest);
        options.auto_restart = true;
        let mut runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(&err, DistributeError::UpgradeComponentMissing { component } if *component == "RestartProvider"),
            "实际错误：{err:?}"
        );
        assert_eq!(dir_names(fx.h.root()), vec!["install".to_string()]);
    }

    #[test]
    fn empty_signature_is_hard_failure_and_missing_sha256_rejected() {
        let fx = success_fixture();
        let mut manifest = manifest_for(&fx.package);
        manifest.signature = String::new();
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        assert!(matches!(runner.run().unwrap_err(), DistributeError::SignatureInvalid));

        let mut manifest = manifest_for(&fx.package);
        manifest.sha256 = None;
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        assert!(matches!(runner.run().unwrap_err(), DistributeError::InvalidBody(_)));
        // 两个失败路径都必须零副作用。
        assert_eq!(dir_names(fx.h.root()), vec!["install".to_string()]);
    }

    // ── 验证失败：错误 + 日志如实记录 ────────────────────────────────

    #[test]
    fn verifier_false_fails_and_journal_records_failure() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(false)));
        let err = runner.run().unwrap_err();
        assert!(matches!(err, DistributeError::SignatureInvalid), "实际错误：{err:?}");

        // 找到该操作的日志（失败路径不暴露 op id，从备份目录扫描）。
        let backup_root = fx.h.root().join("backup");
        let ops = dir_names(&backup_root);
        assert_eq!(ops.len(), 1, "必须留下恰好一个操作目录供检视");
        let journal = fx.h.journal(&ops[0]);
        assert_eq!(journal.states, vec![UpgradeState::Downloaded, UpgradeState::Failed]);
        assert!(journal.error.as_deref().unwrap().contains("签名"));
        assert!(!journal.commit_marker);
        // 现场未动：旧字节仍在。
        assert_eq!(fx.h.current_tree(), tree_of(&fx.old_files));
        // 临时目录已清理。
        assert!(dir_names(&fx.h.root().join("download")).is_empty());
    }

    #[test]
    fn sha256_mismatch_is_typed_error_and_cleans_temp() {
        let fx = success_fixture();
        let mut manifest = manifest_for(&fx.package);
        manifest.sha256 = Some("0".repeat(64));
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(&err, DistributeError::PackageHashMismatch { expected, actual }
                if expected == &"0".repeat(64) && *actual == sha256_bytes(&fx.package)),
            "实际错误：{err:?}"
        );
        assert_eq!(fx.h.current_tree(), tree_of(&fx.old_files));
        assert!(dir_names(&fx.h.root().join("download")).is_empty());
    }

    // ── 解压防线 ─────────────────────────────────────────────────────

    #[test]
    fn zip_slip_entry_rejected_and_nothing_written_outside_staging() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let package = build_zip_bytes(&[("app.bin", b"new"), ("../../evil.txt", b"evil")]);
        let manifest = manifest_for(&package);
        let mut runner = UpgradeRunner::new(h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(err, DistributeError::ArchiveRejected(ref m) if m.contains("zip-slip")),
            "实际：{err:?}"
        );
        // 穿越目标位置（install/ 层）与更外层都不许出现 evil.txt。
        assert!(!h.root().join("install").join("evil.txt").exists());
        assert!(!h.root().join("evil.txt").exists());
        // 预扫描在写第一个字节前拒绝：staging 无残留，现场未动。
        assert_eq!(
            tree_hashes(&h.root().join("install").join(CURRENT_DIR)).unwrap(),
            tree_of(&[("app.bin", b"old")])
        );
        let staging_root = h.root().join("install").join(STAGING_DIR);
        assert!(
            fs::read_dir(&staging_root).unwrap().next().is_none(),
            "staging 根必须无残留操作目录"
        );
    }

    #[test]
    fn zip_bomb_declared_size_over_budget_rejected_before_writing() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let package = build_zip_bytes(&[
            ("a.bin", vec![7u8; 4000].as_slice()),
            ("b.bin", vec![7u8; 4000].as_slice()),
        ]);
        let manifest = manifest_for(&package);
        let mut options = h.options(manifest);
        options.archive.max_total_uncompressed_bytes = 8;
        let mut runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(MockDownloader::new(package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(err, DistributeError::ArchiveRejected(ref m) if m.contains("预算")),
            "实际：{err:?}"
        );
        assert_eq!(
            tree_hashes(&h.root().join("install").join(CURRENT_DIR)).unwrap(),
            tree_of(&[("app.bin", b"old")])
        );
    }

    #[test]
    fn entry_count_over_limit_rejected() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let package = build_zip_bytes(&[("a.bin", b"1".as_slice()), ("b.bin", b"2".as_slice())]);
        let manifest = manifest_for(&package);
        let mut options = h.options(manifest);
        options.archive.max_entries = 1;
        let mut runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(MockDownloader::new(package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(err, DistributeError::ArchiveRejected(ref m) if m.contains("条目数")),
            "实际：{err:?}"
        );
    }

    #[test]
    fn symlink_and_weird_mode_entries_rejected() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let mut package = build_zip_bytes(&[("evil-link", b"target")]);
        patch_entry_to_symlink(&mut package, "evil-link");
        // 自检：fixture 确实带上了 symlink 标记。
        let mut archive = zip::ZipArchive::new(Cursor::new(package.clone())).unwrap();
        assert!(archive.by_index(0).unwrap().is_symlink(), "fixture 必须真造出 symlink");
        let manifest = manifest_for(&package);
        let mut runner = UpgradeRunner::new(h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(err, DistributeError::ArchiveRejected(ref m) if m.contains("符号链接")),
            "实际：{err:?}"
        );
        assert_eq!(
            tree_hashes(&h.root().join("install").join(CURRENT_DIR)).unwrap(),
            tree_of(&[("app.bin", b"old")])
        );
    }

    /// 轮 69（死循环回归）：`tree_hashes` 必须用 lstat 并在下降前**拒绝**符号链接，
    /// 一个指向祖先目录的符号链接不得让这趟无访问集 / 无深度上界的栈遍历永不收敛。
    /// 用超时线程守：修复后瞬间返回 `Err`；若退回跟随链接的 `fs::metadata`，会挂死 →
    /// 断言在 `recv_timeout` 处变红，而不是把整个测试二进制拖挂（与轮 68 同类快红设计）。
    #[cfg(unix)]
    #[test]
    fn tree_hashes_rejects_directory_symlink_cycle_instead_of_looping_forever() {
        use std::os::unix::fs::symlink;
        use std::sync::mpsc;
        use std::time::Duration;

        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let current = h.root().join("install").join(CURRENT_DIR);
        symlink(h.root().join("install"), current.join("loop-link")).unwrap();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(tree_hashes(&current).map(|_| ()));
        });
        let outcome = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("tree_hashes 必须有限返回；超时＝符号链接环被跟随（死循环回归）");
        let err = outcome.unwrap_err();
        assert!(
            matches!(err, DistributeError::FileOperationFailed(ref m) if m.contains("符号链接")),
            "目录符号链接应在下降前被拒绝，实际：{err:?}"
        );
    }

    /// 轮 69（越界哈希回归）：指向树外普通文件的符号链接不得被取哈希后当成树内条目——
    /// 那会让「快照 / 校验」对同一棵树给出被链接目标污染的结果。
    #[cfg(unix)]
    #[test]
    fn tree_hashes_rejects_out_of_tree_file_symlink_instead_of_hashing_target() {
        use std::os::unix::fs::symlink;

        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let current = h.root().join("install").join(CURRENT_DIR);
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        symlink(outside.path().join("secret.txt"), current.join("escape-link")).unwrap();

        let err = tree_hashes(&current).unwrap_err();
        assert!(
            matches!(err, DistributeError::FileOperationFailed(ref m) if m.contains("符号链接")),
            "树外文件符号链接应被拒绝而非越界取哈希，实际：{err:?}"
        );
    }

    /// 轮 69（未破正常遍历）：改用 lstat 后，纯真实嵌套目录树仍被完整、正确地枚举。
    #[test]
    fn tree_hashes_still_walks_nested_real_directories() {
        let h = Harness::new();
        let current = h.root().join("install").join(CURRENT_DIR);
        fs::create_dir_all(current.join("pkg/sub")).unwrap();
        fs::write(current.join("app.bin"), b"old").unwrap();
        fs::write(current.join("pkg/sub/deep.bin"), b"deep").unwrap();
        assert_eq!(
            tree_hashes(&current).unwrap(),
            tree_of(&[("app.bin", b"old"), ("pkg/sub/deep.bin", b"deep")])
        );
    }

    #[test]
    fn injected_extractor_failure_is_fatal_and_staged_nothing() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)))
            .with_extractor(Box::new(FailExtractor));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(err, DistributeError::ArchiveRejected(ref m) if m.contains("seam")),
            "实际：{err:?}"
        );
        assert_eq!(fx.h.current_tree(), tree_of(&fx.old_files));
        let staging_root = fx.h.root().join("install").join(STAGING_DIR);
        assert!(fs::read_dir(&staging_root).unwrap().next().is_none());
    }

    // ── 备份真实性 ───────────────────────────────────────────────────

    #[test]
    fn missing_install_dir_fails_without_fake_backed_up() {
        let h = Harness::new();
        // 不预置安装树：install/ 不存在。
        let package = build_zip_bytes(&[("app.bin", b"new")]);
        let manifest = manifest_for(&package);
        let mut runner = UpgradeRunner::new(h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let result = runner.run_with_error_handling();
        assert!(!result.success);
        assert!(!result.backed_up, "安装目录缺失时绝不允许谎报已备份");
        assert!(result.error.as_deref().unwrap().contains("install"));
        // validate 先于任何目录创建。
        assert!(!h.root().join("download").exists());
        assert!(!h.root().join("backup").exists());
    }

    // ── 回滚真实性 ───────────────────────────────────────────────────

    #[test]
    fn failed_health_check_auto_rolls_back_old_bytes_on_disk() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let expected_old = tree_of(&fx.old_files);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)))
            .with_health_check(Box::new(FixedHealth(false)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(err, DistributeError::HealthCheckFailed(ref m) if m.contains("自动回滚")),
            "实际：{err:?}"
        );

        // 磁盘上留下的必须是旧版本字节（逐文件哈希核对）。
        assert_eq!(fx.h.current_tree(), expected_old);
        assert_eq!(
            fs::read(fx.h.root().join("install").join(CURRENT_DIR).join("app.bin")).unwrap(),
            b"old-app-bytes-v1"
        );
        // previous 已移回、无隔离残留。
        assert!(!fx.h.root().join("install").join(PREVIOUS_DIR).exists());
        let install_root = fx.h.root().join("install");
        let quarantine_leftovers: Vec<String> = fs::read_dir(&install_root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(FAILED_DIR_PREFIX))
            .collect();
        assert!(quarantine_leftovers.is_empty(), "隔离目录必须被清理：{quarantine_leftovers:?}");
        assert_eq!(runner.progress().state, UpgradeState::RolledBack);

        // 日志如实记录整段历程。
        let backup_root = fx.h.root().join("backup");
        let ops = dir_names(&backup_root);
        assert_eq!(ops.len(), 1);
        let journal = fx.h.journal(&ops[0]);
        assert_eq!(
            journal.states,
            vec![
                UpgradeState::Downloaded,
                UpgradeState::Verified,
                UpgradeState::BackedUp,
                UpgradeState::Extracted,
                UpgradeState::Swapped,
                UpgradeState::RollingBack,
                UpgradeState::RolledBack,
                UpgradeState::Failed,
            ]
        );
        assert!(!journal.commit_marker);
        // 备份树保留（供人工二次处理）。
        assert!(backup_root.join(&ops[0]).join(BACKUP_TREE_DIR).is_dir());
    }

    #[test]
    fn rollback_after_success_really_restores_old_tree() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let expected_old = tree_of(&fx.old_files);
        let mut runner = UpgradeRunner::new(fx.h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)))
            .with_health_check(Box::new(FixedHealth(true)));
        let result = runner.run().unwrap();

        // 先确认已是新版本。
        assert_ne!(fx.h.current_tree(), expected_old);

        // 手动回滚：必须真实成功，且恢复的字节与升级前逐文件哈希一致。
        runner.rollback().expect("回滚必须真实成功");
        assert_eq!(fx.h.current_tree(), expected_old);
        assert_eq!(runner.progress().state, UpgradeState::RolledBack);
        let journal = fx.h.journal(&result.operation_id);
        assert_eq!(journal.current_state, UpgradeState::RolledBack);
    }

    #[test]
    fn rollback_without_any_backup_fails_honestly() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let manifest = manifest_for(b"whatever");
        let mut runner = UpgradeRunner::new(h.options(manifest));
        let err = runner.rollback().unwrap_err();
        assert!(
            matches!(err, DistributeError::RollbackFailed(ref m) if m.contains("没有")),
            "实际：{err:?}"
        );
        assert_eq!(runner.progress().state, UpgradeState::Failed);
    }

    // ── 重启 seam ────────────────────────────────────────────────────

    #[test]
    fn restart_provider_invoked_exactly_once_and_reflected_in_result() {
        let fx = success_fixture();
        let manifest = manifest_for(&fx.package);
        let counter = Arc::new(CountingRestart::new());
        struct Probe(Arc<CountingRestart>);
        impl RestartProvider for Probe {
            fn restart(&self) -> DistributeResult<()> {
                self.0.count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let mut options = fx.h.options(manifest);
        options.auto_restart = true;
        let mut runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(MockDownloader::new(fx.package.clone())))
            .with_verifier(Box::new(MockVerifier::new(true)))
            .with_health_check(Box::new(FixedHealth(true)))
            .with_restart_provider(Box::new(Probe(counter.clone())));
        let result = runner.run().unwrap();
        assert!(result.restarted, "restarted 必须如实反映 provider 成功");
        assert_eq!(counter.count.load(Ordering::SeqCst), 1, "provider 必须且只能被调用一次");
    }

    #[test]
    fn restart_disabled_is_ok_and_not_called() {
        let options = Harness::new().options(UpdateManifest::default());
        let mut runner = UpgradeRunner::new(options);
        assert!(runner.restart().is_ok(), "auto_restart = false 必须直接 Ok（无动作）");
    }

    // ── 超时与清理 ───────────────────────────────────────────────────

    #[test]
    fn slow_downloader_hits_deadline_and_leaves_no_temp_leftovers() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let package = build_zip_bytes(&[("app.bin", b"new")]);
        let manifest = manifest_for(&package);
        let mut options = h.options(manifest);
        options.download_timeout_secs = 1;
        let mut runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(SlowDownloader))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let err = runner.run().unwrap_err();
        assert!(
            matches!(&err, DistributeError::PhaseTimeout { phase, timeout_secs }
                if *phase == "Download" && *timeout_secs == 1),
            "实际错误：{err:?}"
        );
        // 协作式 worker 已收尾，本操作下载目录被清理（download 根下为空）。
        assert!(dir_names(&h.root().join("download")).is_empty(), "超时必须清掉 temp 残留");
        assert_eq!(
            tree_hashes(&h.root().join("install").join(CURRENT_DIR)).unwrap(),
            tree_of(&[("app.bin", b"old")])
        );
    }

    // ── 状态机 / 进度单元 ────────────────────────────────────────────

    #[test]
    fn upgrade_state_machine_predicates() {
        assert!(UpgradeState::Completed.is_finished());
        assert!(UpgradeState::Failed.is_finished());
        assert!(UpgradeState::RolledBack.is_finished());
        assert!(!UpgradeState::Idle.is_finished());
        assert!(!UpgradeState::Swapped.is_finished());

        // 备份存在之前不可回滚；之后（含失败/完成）皆可。
        for s in [
            UpgradeState::Idle,
            UpgradeState::Checking,
            UpgradeState::Downloading,
            UpgradeState::Downloaded,
            UpgradeState::Verifying,
            UpgradeState::Verified,
        ] {
            assert!(!s.can_rollback(), "{s:?} 尚无备份，不可回滚");
        }
        for s in [
            UpgradeState::BackedUp,
            UpgradeState::Extracted,
            UpgradeState::Swapped,
            UpgradeState::Committed,
            UpgradeState::Completed,
            UpgradeState::Failed,
        ] {
            assert!(s.can_rollback(), "{s:?} 必须可回滚");
        }
        assert!(!UpgradeState::RollingBack.can_rollback(), "正在回滚不重复可回滚");
    }

    #[test]
    fn progress_states_map_onto_new_machine() {
        let mut progress = UpgradeProgress::new();
        assert_eq!(progress.state, UpgradeState::Idle);
        assert_eq!(progress.progress_percent, 0);

        progress.set_state(UpgradeState::Downloading);
        assert_eq!(progress.progress_percent, 10);
        progress.set_state(UpgradeState::BackedUp);
        assert_eq!(progress.progress_percent, 55);
        progress.set_state(UpgradeState::Swapped);
        assert_eq!(progress.progress_percent, 80);
        progress.set_state(UpgradeState::Committed);
        assert_eq!(progress.progress_percent, 95);
        progress.set_state(UpgradeState::Completed);
        assert_eq!(progress.progress_percent, 100);

        // 失败/回滚保留既有百分比。
        let mut p2 = UpgradeProgress::new();
        p2.set_state(UpgradeState::Extracting);
        let before = p2.progress_percent;
        p2.set_state(UpgradeState::Failed);
        assert_eq!(p2.progress_percent, before);
        assert_eq!(p2.state, UpgradeState::Failed);

        let mut p3 = UpgradeProgress::new();
        p3.set_error("boom".into());
        assert_eq!(p3.state, UpgradeState::Failed);
        assert_eq!(p3.error.as_deref(), Some("boom"));

        let mut p4 = UpgradeProgress::new();
        p4.set_download_progress(500, 1000);
        assert_eq!(p4.progress_percent, 50);
        p4.set_progress(150);
        assert_eq!(p4.progress_percent, 100);
    }

    // ── 校验 / 工厂 / mock 单元（全部有效断言）──────────────────────

    fn legacy_options() -> UpgradeOptions {
        UpgradeOptions {
            manifest: UpdateManifest {
                version: "2.0.0".into(),
                url: "https://example.com/update-2.0.0.zip".into(),
                signature: "abc123def456".into(),
                release_date: "2026-09-21T00:00:00Z".into(),
                platform_notes: BTreeMap::new(),
                sha256: Some("a".repeat(64)),
                ..UpdateManifest::default()
            },
            ..UpgradeOptions::default()
        }
    }

    #[test]
    fn validate_rejects_incomplete_manifest_and_creates_no_dirs() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);

        // 空 URL。
        let mut manifest = manifest_for(b"pkg");
        manifest.url = String::new();
        let runner = UpgradeRunner::new(h.options(manifest));
        assert!(matches!(runner.validate(), Err(DistributeError::InvalidBody(_))));
        assert!(!h.root().join("download").exists());

        // 空版本。
        let mut manifest = manifest_for(b"pkg");
        manifest.version = String::new();
        let runner = UpgradeRunner::new(h.options(manifest));
        assert!(matches!(runner.validate(), Err(DistributeError::InvalidBody(_))));

        // 清单完整但组件未注入 → 组件缺失，而不是目录被创建。
        let runner = UpgradeRunner::new(h.options(manifest_for(b"pkg")));
        assert!(matches!(
            runner.validate(),
            Err(DistributeError::UpgradeComponentMissing { component: "Downloader" })
        ));
        assert!(!h.root().join("download").exists());
        assert!(!h.root().join("backup").exists());
    }

    #[test]
    fn validate_accepts_complete_options_without_side_effects() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let manifest = manifest_for(b"ignored");
        let runner = UpgradeRunner::new(h.options(manifest))
            .with_downloader(Box::new(MockDownloader::new(vec![])))
            .with_verifier(Box::new(MockVerifier::new(true)));
        assert!(matches!(runner.validate(), Ok(())), "完整配置必须通过校验");
        assert!(!h.root().join("download").exists(), "validate 必须零副作用");
        assert!(!h.root().join("backup").exists());
    }

    #[test]
    fn validate_rejects_downgrade_with_zero_file_effect() {
        let h = Harness::new();
        h.seed_current(&[("app.bin", b"old")]);
        let mut manifest = manifest_for(b"pkg");
        manifest.version = "1.0.0".into();
        let mut options = h.options(manifest);
        options.installed_version = Some("2.0.0".into());
        let runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(MockDownloader::new(vec![])))
            .with_verifier(Box::new(MockVerifier::new(true)));
        assert!(
            matches!(
                runner.validate(),
                Err(DistributeError::DowngradeRejected { current, target })
                    if current == "2.0.0" && target == "1.0.0"
            ),
            "已装 2.0.0 却给 1.0.0 清单必须具名拒绝，而不是撞成别的校验错误"
        );
        assert!(!h.root().join("download").exists(), "降级拒绝必须零文件副作用");
        assert!(!h.root().join("backup").exists());
    }

    #[test]
    fn downgrade_gate_uses_numeric_version_order_not_strings() {
        // 字符串序会把 1.10.0 判成低于 1.9.0；数值分段才是真序关系。
        assert!(ensure_not_downgrade(Some("1.9.0"), "1.10.0").is_ok());
        assert!(ensure_not_downgrade(Some("1.10.0"), "1.9.0").is_err());
        // 同版本重装不是降级；无基线（首次装配）如实放行。
        assert!(ensure_not_downgrade(Some("2.0.0"), "2.0.0").is_ok());
        assert!(ensure_not_downgrade(None, "0.0.1").is_ok());
        // ≥2³¹ 的分段不得因截断被判成「不小于」而绕过门禁。
        assert!(ensure_not_downgrade(Some("2147483648.0.0"), "2147483647.0.0").is_err());
    }

    #[test]
    fn run_with_error_handling_reports_component_failure() {
        let mut runner = UpgradeRunner::new(legacy_options());
        let result = runner.run_with_error_handling();
        assert!(!result.success);
        assert_eq!(result.state, UpgradeState::Failed);
        assert_eq!(runner.progress().state, UpgradeState::Failed);
        assert!(!result.backed_up);
        assert!(!result.restarted);
        assert!(result.error.as_deref().unwrap().contains("Downloader"));
    }

    #[test]
    fn factory_functions_preserve_options() {
        let options = legacy_options();
        let runner = create_upgrade_runner(options.clone());
        assert_eq!(runner.options().manifest.version, "2.0.0");
        assert_eq!(runner.options().manifest.sha256, Some("a".repeat(64)));
        assert_eq!(runner.progress().state, UpgradeState::Idle);

        let default_runner = create_default_upgrade_runner();
        assert_eq!(default_runner.options().manifest.version, "0.1.0");
        assert_eq!(default_runner.options().download_timeout_secs, 300);
        assert!(default_runner.options().auto_restart);
        // 默认清单不完整：必须如实拒绝，而不是跑起来写 mock 文件。
        assert!(default_runner.validate().is_err());
    }

    #[test]
    fn mock_downloader_writes_payload_and_reports_progress() {
        let h = Harness::new();
        let dest = h.root().join("pkg.bin");
        let control = PhaseControl::new(Instant::now() + Duration::from_secs(30));
        let mut events: Vec<(u64, u64)> = Vec::new();
        let downloader = MockDownloader::new(vec![1u8, 2, 3]);
        let path = downloader
            .download("https://example.invalid", &dest, &control, &mut |d, t| events.push((d, t)))
            .unwrap();
        assert_eq!(path, dest);
        assert_eq!(fs::read(&dest).unwrap(), vec![1u8, 2, 3]);
        assert_eq!(events.last(), Some(&(3, 3)));

        // 已取消的 control 必须拒绝写盘。
        let cancelled = PhaseControl::new(Instant::now() - Duration::from_secs(1));
        let mut cb = |_d: u64, _t: u64| {};
        assert!(matches!(
            downloader.download("u", &h.root().join("nope.bin"), &cancelled, &mut cb),
            Err(DistributeError::PhaseTimeout { .. })
        ));
        assert!(!h.root().join("nope.bin").exists());
    }

    #[test]
    fn mock_verifier_respects_switch() {
        let ctx = VerificationContext {
            package_sha256_hex: "aa".into(),
            signature_algorithm: None,
            public_key_id: None,
        };
        assert!(MockVerifier::new(true).verify(Path::new("x"), "sig", &ctx).unwrap());
        assert!(!MockVerifier::new(false).verify(Path::new("x"), "sig", &ctx).unwrap());
        assert!(MockVerifier::default().verify(Path::new("x"), "sig", &ctx).unwrap());
    }

    #[test]
    fn entry_name_validation_covers_all_escape_shapes() {
        // zip-slip / 绝对路径 / 盘符 / 反斜杠 / NUL。
        for bad in ["../x", "a/../../x", "/abs/x", "C:/x", "a\\b", "a\0b", "..\\x"] {
            assert!(
                matches!(validate_entry_name(bad), Err(DistributeError::ArchiveRejected(_))),
                "必须拒绝条目名：{bad:?}"
            );
        }
        assert_eq!(validate_entry_name("a/./b.txt").unwrap(), PathBuf::from("a").join("b.txt"));
        assert_eq!(validate_entry_name("dir/").unwrap(), PathBuf::from("dir"));
    }

    #[test]
    fn phase_control_deadline_semantics() {
        let passed = PhaseControl::new(Instant::now() - Duration::from_millis(1));
        assert!(passed.is_past_deadline());
        assert!(passed.is_cancelled(), "超时即取消：语义统一");
        let live = PhaseControl::new(Instant::now() + Duration::from_secs(60));
        assert!(!live.is_cancelled());
        live.cancel();
        assert!(live.is_cancelled());
        assert!(!live.is_past_deadline());
    }

    #[test]
    fn journal_serde_roundtrip_and_state_strings_are_camel_case() {
        let h = Harness::new();
        let state: UpgradeState =
            serde_json::from_str("\"downloaded\"").expect("camelCase 序列化名");
        assert_eq!(state, UpgradeState::Downloaded);
        assert_eq!(
            serde_json::to_value(UpgradeState::HealthChecked).unwrap(),
            serde_json::json!("healthChecked")
        );

        let journal = UpgradeJournal {
            operation_id: "op-1".into(),
            old_version: Some("1.0.0".into()),
            new_version: "2.0.0".into(),
            package_sha256: Some("beef".into()),
            backup_path: "b".into(),
            staging_path: "s".into(),
            states: vec![UpgradeState::Downloaded],
            current_state: UpgradeState::Downloaded,
            commit_marker: true,
            error: None,
        };
        let path = h.root().join("journal.json");
        journal.save(&path).unwrap();
        assert!(!h.root().join("journal.json.tmp").exists(), "原子落盘后不应残留临时文件");
        let loaded = UpgradeJournal::load(&path).unwrap();
        assert_eq!(loaded, journal);
    }

    #[test]
    fn manifest_new_fields_are_optional_and_backward_compatible() {
        let legacy_json = serde_json::json!({
            "version": "2.0.0",
            "url": "https://example.com/app.zip",
            "signature": "abc",
            "release_date": "2026-10-01T00:00:00Z",
        });
        let m: UpdateManifest = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(m.sha256, None);
        assert_eq!(m.signature_algorithm, None);

        let extended = serde_json::json!({
            "version": "2.0.0",
            "url": "https://example.com/app.zip",
            "signature": "abc",
            "release_date": "2026-10-01T00:00:00Z",
            "sha256": "aa",
            "signatureAlgorithm": "ed25519",
            "publicKeyId": "k1",
            "minHostVersion": "1.1.0",
            "abi": "x64-msvc",
            "rollbackPolicy": "auto",
        });
        let m: UpdateManifest = serde_json::from_value(extended).unwrap();
        assert_eq!(m.sha256.as_deref(), Some("aa"));
        assert_eq!(m.signature_algorithm.as_deref(), Some("ed25519"));
        assert_eq!(m.public_key_id.as_deref(), Some("k1"));
        assert_eq!(m.min_host_version.as_deref(), Some("1.1.0"));
        assert_eq!(m.abi.as_deref(), Some("x64-msvc"));
        assert_eq!(m.rollback_policy.as_deref(), Some("auto"));
    }
}
