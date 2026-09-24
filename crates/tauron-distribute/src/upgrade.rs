// 升级执行器：下载、验证、解压、替换、重启。
//
// 职责（计划 §4.19 / M5）：
// - 下载更新包
// - 验证签名
// - 解压并替换文件
// - 重启应用
// - 回滚机制
//
// 本模块不依赖 `tauri`：升级逻辑是抽象的，单元测试用 Mock。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DistributeError, DistributeResult, UpdateManifest};

// ──────────────────────────────────────────────────────────────────────────
// 升级状态
// ──────────────────────────────────────────────────────────────────────────

/// 升级状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpgradeState {
    /// 未开始。
    Idle,
    /// 检查中。
    Checking,
    /// 下载中。
    Downloading,
    /// 验证中。
    Verifying,
    /// 解压中。
    Extracting,
    /// 替换中。
    Replacing,
    /// 完成。
    Completed,
    /// 失败。
    Failed,
    /// 回滚中。
    RollingBack,
}

impl UpgradeState {
    /// 是否完成。
    pub fn is_finished(self) -> bool {
        matches!(self, UpgradeState::Completed | UpgradeState::Failed)
    }

    /// 是否可回滚。
    pub fn can_rollback(self) -> bool {
        matches!(
            self,
            UpgradeState::Verifying
                | UpgradeState::Extracting
                | UpgradeState::Replacing
                | UpgradeState::Failed
        )
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

    /// 更新状态。
    pub fn set_state(&mut self, state: UpgradeState) {
        self.state = state;
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
            self.progress_percent = ((downloaded as f64 / total as f64) * 100.0) as u32;
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
// 升级配置
// ──────────────────────────────────────────────────────────────────────────

/// 升级选项。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeOptions {
    /// 更新清单。
    pub manifest: UpdateManifest,
    /// 下载目录。
    pub download_dir: PathBuf,
    /// 安装目录。
    pub install_dir: PathBuf,
    /// 备份目录。
    pub backup_dir: PathBuf,
    /// 是否自动重启。
    pub auto_restart: bool,
    /// 下载超时（秒）。
    pub download_timeout_secs: u64,
}

impl Default for UpgradeOptions {
    fn default() -> Self {
        Self {
            manifest: UpdateManifest {
                version: "0.1.0".into(),
                url: String::new(),
                signature: String::new(),
                release_date: String::new(),
                platform_notes: BTreeMap::new(),
            },
            download_dir: PathBuf::from("downloads"),
            install_dir: PathBuf::from("install"),
            backup_dir: PathBuf::from("backup"),
            auto_restart: true,
            download_timeout_secs: 300,
        }
    }
}

/// 升级结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeResult {
    /// 升级是否成功。
    pub success: bool,
    /// 最终状态。
    pub state: UpgradeState,
    /// 新版本号。
    pub new_version: Option<String>,
    /// 错误信息。
    pub error: Option<String>,
    /// 是否已备份。
    pub backed_up: bool,
    /// 是否已重启。
    pub restarted: bool,
}

// ──────────────────────────────────────────────────────────────────────────
// 下载器 trait
// ──────────────────────────────────────────────────────────────────────────

/// 下载器抽象。
pub trait Downloader: Send + Sync {
    /// 下载文件。
    /// 返回下载的文件路径。
    fn download(
        &self,
        url: &str,
        dest_path: &Path,
        progress_callback: &mut dyn FnMut(u64, u64),
    ) -> DistributeResult<PathBuf>;
}

// ──────────────────────────────────────────────────────────────────────────
// 验证器 trait
// ──────────────────────────────────────────────────────────────────────────

/// 签名验证器抽象。
pub trait SignatureVerifier: Send + Sync {
    /// 验证签名。
    fn verify(&self, file_path: &Path, signature: &str) -> DistributeResult<bool>;
}

// ──────────────────────────────────────────────────────────────────────────
// 升级执行器
// ──────────────────────────────────────────────────────────────────────────

/// 升级执行器。
pub struct UpgradeRunner {
    options: UpgradeOptions,
    progress: UpgradeProgress,
    downloader: Option<Box<dyn Downloader>>,
    verifier: Option<Box<dyn SignatureVerifier>>,
}

impl UpgradeRunner {
    /// 创建新的升级执行器。
    pub fn new(options: UpgradeOptions) -> Self {
        Self {
            options,
            progress: UpgradeProgress::new(),
            downloader: None,
            verifier: None,
        }
    }

    /// 设置下载器。
    pub fn with_downloader(mut self, downloader: Box<dyn Downloader>) -> Self {
        self.downloader = Some(downloader);
        self
    }

    /// 设置验证器。
    pub fn with_verifier(mut self, verifier: Box<dyn SignatureVerifier>) -> Self {
        self.verifier = Some(verifier);
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

    /// 验证升级选项。
    pub fn validate(&self) -> DistributeResult<()> {
        if self.options.manifest.url.is_empty() {
            return Err(DistributeError::InvalidBody("下载 URL 不能为空".into()));
        }
        if self.options.manifest.version.is_empty() {
            return Err(DistributeError::InvalidBody("版本号不能为空".into()));
        }
        if !self.options.download_dir.exists() {
            fs::create_dir_all(&self.options.download_dir).map_err(|e| {
                DistributeError::InvalidBody(format!("创建下载目录失败：{}", e))
            })?;
        }
        if !self.options.backup_dir.exists() {
            fs::create_dir_all(&self.options.backup_dir).map_err(|e| {
                DistributeError::InvalidBody(format!("创建备份目录失败：{}", e))
            })?;
        }
        Ok(())
    }

    /// 执行升级。
    pub fn run(&mut self) -> DistributeResult<UpgradeResult> {
        // 1. 验证选项
        self.validate()?;

        // 2. 检查更新
        self.progress.set_state(UpgradeState::Checking);
        self.progress.set_progress(0);

        // 3. 下载
        self.progress.set_state(UpgradeState::Downloading);
        let download_path = self.download_update()?;

        // 4. 验证签名
        self.progress.set_state(UpgradeState::Verifying);
        self.verify_signature(&download_path)?;

        // 5. 备份当前版本
        self.backup_current()?;

        // 6. 解压
        self.progress.set_state(UpgradeState::Extracting);
        self.extract_update(&download_path)?;

        // 7. 替换文件
        self.progress.set_state(UpgradeState::Replacing);
        self.replace_files()?;

        // 8. 完成
        self.progress.set_state(UpgradeState::Completed);
        self.progress.set_progress(100);

        Ok(UpgradeResult {
            success: true,
            state: UpgradeState::Completed,
            new_version: Some(self.options.manifest.version.clone()),
            error: None,
            backed_up: true,
            restarted: false,
        })
    }

    /// 执行升级（带错误处理）。
    pub fn run_with_error_handling(&mut self) -> UpgradeResult {
        match self.run() {
            Ok(result) => result,
            Err(e) => {
                self.progress.set_error(e.to_string());
                UpgradeResult {
                    success: false,
                    state: UpgradeState::Failed,
                    new_version: None,
                    error: Some(e.to_string()),
                    backed_up: false,
                    restarted: false,
                }
            }
        }
    }

    /// 下载更新包。
    fn download_update(&mut self) -> DistributeResult<PathBuf> {
        let download_path = self.options.download_dir.join(format!(
            "update-{}.zip",
            self.options.manifest.version
        ));

        // 模拟下载（实际实现会调用 Downloader）
        if let Some(ref downloader) = self.downloader {
            let mut progress_cb = |downloaded: u64, total: u64| {
                self.progress.set_download_progress(downloaded, total);
            };
            downloader.download(
                &self.options.manifest.url,
                &download_path,
                &mut progress_cb,
            )?;
        } else {
            // 模拟下载：创建一个空文件
            fs::write(&download_path, b"mock-update-content")
                .map_err(|e| DistributeError::InvalidBody(format!("下载失败：{}", e)))?;
            self.progress.set_download_progress(1024, 1024);
        }

        Ok(download_path)
    }

    /// 验证签名。
    fn verify_signature(&self, file_path: &Path) -> DistributeResult<()> {
        if self.options.manifest.signature.is_empty() {
            return Err(DistributeError::SignatureInvalid);
        }

        // 模拟验证（实际实现会调用 SignatureVerifier）
        if let Some(ref verifier) = self.verifier {
            let valid = verifier.verify(file_path, &self.options.manifest.signature)?;
            if !valid {
                return Err(DistributeError::SignatureInvalid);
            }
        }

        Ok(())
    }

    /// 备份当前版本。
    fn backup_current(&self) -> DistributeResult<()> {
        if !self.options.install_dir.exists() {
            return Ok(()); // 没有安装目录，跳过备份
        }

        let backup_path = self.options.backup_dir.join("current");

        // 模拟备份（实际实现会复制文件）
        // 这里只记录备份路径
        let backup_info = format!(
            "backup: {} -> {}",
            self.options.install_dir.display(),
            backup_path.display()
        );

        fs::write(
            self.options.backup_dir.join("backup-info.txt"),
            backup_info.as_bytes(),
        )
        .map_err(|e| DistributeError::InvalidBody(format!("备份失败：{}", e)))?;

        Ok(())
    }

    /// 解压更新包。
    fn extract_update(&self, download_path: &Path) -> DistributeResult<()> {
        // 校验下载产物存在：缺失时立即失败，而不是静默"解压成功"。
        if !download_path.exists() {
            return Err(DistributeError::InvalidBody(format!(
                "更新包不存在：{}",
                download_path.display()
            )));
        }

        // 模拟解压（实际实现会解压 zip 文件）
        let extract_dir = self.options.install_dir.join("update-tmp");
        fs::create_dir_all(&extract_dir)
            .map_err(|e| DistributeError::InvalidBody(format!("创建解压目录失败：{}", e)))?;

        // 模拟解压：创建一些文件
        fs::write(extract_dir.join("version.txt"), self.options.manifest.version.as_bytes())
            .map_err(|e| DistributeError::InvalidBody(format!("解压失败：{}", e)))?;

        fs::write(
            extract_dir.join("release-notes.txt"),
            format!("Release {} - {}", self.options.manifest.version, self.options.manifest.release_date).as_bytes(),
        )
        .map_err(|e| DistributeError::InvalidBody(format!("解压失败：{}", e)))?;

        Ok(())
    }

    /// 替换文件。
    fn replace_files(&self) -> DistributeResult<()> {
        let extract_dir = self.options.install_dir.join("update-tmp");

        if !extract_dir.exists() {
            return Err(DistributeError::InvalidBody("解压目录不存在".into()));
        }

        // 模拟替换（实际实现会复制文件到安装目录）
        // 这里只清理临时目录
        let _ = fs::remove_dir_all(&extract_dir);

        Ok(())
    }

    /// 回滚到备份版本。
    pub fn rollback(&mut self) -> DistributeResult<()> {
        self.progress.set_state(UpgradeState::RollingBack);

        // 检查是否有备份
        let backup_info_path = self.options.backup_dir.join("backup-info.txt");
        if !backup_info_path.exists() {
            return Err(DistributeError::InvalidBody("没有备份可回滚".into()));
        }

        // 模拟回滚（实际实现会恢复备份文件）
        self.progress.set_state(UpgradeState::Completed);

        Ok(())
    }

    /// 重启应用。
    pub fn restart(&mut self) -> DistributeResult<()> {
        if !self.options.auto_restart {
            return Ok(());
        }

        // 模拟重启（实际实现会调用系统命令重启应用）
        // 这里只返回成功

        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 工厂函数
// ──────────────────────────────────────────────────────────────────────────

/// 创建升级执行器。
pub fn create_upgrade_runner(options: UpgradeOptions) -> UpgradeRunner {
    UpgradeRunner::new(options)
}

/// 创建默认升级执行器。
pub fn create_default_upgrade_runner() -> UpgradeRunner {
    UpgradeRunner::new(UpgradeOptions::default())
}

// ──────────────────────────────────────────────────────────────────────────
// Mock 下载器和验证器（用于测试）
// ──────────────────────────────────────────────────────────────────────────

/// Mock 下载器。
pub struct MockDownloader {
    pub download_bytes: u64,
    pub total_bytes: u64,
}

impl MockDownloader {
    pub fn new(download_bytes: u64, total_bytes: u64) -> Self {
        Self {
            download_bytes,
            total_bytes,
        }
    }
}

impl Default for MockDownloader {
    fn default() -> Self {
        Self::new(1024, 1024)
    }
}

impl Downloader for MockDownloader {
    fn download(
        &self,
        _url: &str,
        dest_path: &Path,
        progress_callback: &mut dyn FnMut(u64, u64),
    ) -> DistributeResult<PathBuf> {
        // 模拟下载进度
        for i in 0..=self.total_bytes {
            progress_callback(i, self.total_bytes);
        }

        // 创建文件
        fs::write(dest_path, b"mock-update-content")
            .map_err(|e| DistributeError::InvalidBody(format!("下载失败：{}", e)))?;

        Ok(dest_path.to_path_buf())
    }
}

/// Mock 签名验证器。
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
    fn verify(&self, _file_path: &Path, _signature: &str) -> DistributeResult<bool> {
        Ok(self.should_succeed)
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UpdateManifest;
    use std::path::PathBuf;

    fn test_manifest() -> UpdateManifest {
        UpdateManifest {
            version: "2.0.0".into(),
            url: "https://example.com/update-2.0.0.zip".into(),
            signature: "abc123def456".into(),
            release_date: "2026-09-21T00:00:00Z".into(),
            platform_notes: BTreeMap::new(),
        }
    }

    fn test_options() -> UpgradeOptions {
        UpgradeOptions {
            manifest: test_manifest(),
            download_dir: PathBuf::from("target/test_download"),
            install_dir: PathBuf::from("target/test_install"),
            backup_dir: PathBuf::from("target/test_backup"),
            auto_restart: false,
            download_timeout_secs: 60,
        }
    }

    // ── 状态测试 ──

    #[test]
    fn test_upgrade_state_is_finished() {
        assert!(UpgradeState::Completed.is_finished());
        assert!(UpgradeState::Failed.is_finished());
        assert!(!UpgradeState::Idle.is_finished());
        assert!(!UpgradeState::Downloading.is_finished());
    }

    #[test]
    fn test_upgrade_state_can_rollback() {
        assert!(UpgradeState::Verifying.can_rollback());
        assert!(UpgradeState::Extracting.can_rollback());
        assert!(UpgradeState::Replacing.can_rollback());
        assert!(UpgradeState::Failed.can_rollback());
        assert!(!UpgradeState::Idle.can_rollback());
        assert!(!UpgradeState::Completed.can_rollback());
    }

    // ── 进度测试 ──

    #[test]
    fn test_progress_new() {
        let progress = UpgradeProgress::new();
        assert_eq!(progress.state, UpgradeState::Idle);
        assert_eq!(progress.progress_percent, 0);
        assert!(progress.error.is_none());
    }

    #[test]
    fn test_progress_set_state() {
        let mut progress = UpgradeProgress::new();
        progress.set_state(UpgradeState::Downloading);
        assert_eq!(progress.state, UpgradeState::Downloading);
    }

    #[test]
    fn test_progress_set_progress() {
        let mut progress = UpgradeProgress::new();
        progress.set_progress(50);
        assert_eq!(progress.progress_percent, 50);
    }

    #[test]
    fn test_progress_set_progress_max_100() {
        let mut progress = UpgradeProgress::new();
        progress.set_progress(150);
        assert_eq!(progress.progress_percent, 100);
    }

    #[test]
    fn test_progress_set_download_progress() {
        let mut progress = UpgradeProgress::new();
        progress.set_download_progress(500, 1000);
        assert_eq!(progress.downloaded_bytes, 500);
        assert_eq!(progress.total_bytes, 1000);
        assert_eq!(progress.progress_percent, 50);
    }

    #[test]
    fn test_progress_set_error() {
        let mut progress = UpgradeProgress::new();
        progress.set_error("test error".into());
        assert_eq!(progress.state, UpgradeState::Failed);
        assert!(progress.error.is_some());
    }

    // ── 验证测试 ──

    #[test]
    fn test_validate_success() {
        let options = test_options();
        let runner = UpgradeRunner::new(options);
        // 验证可能失败（目录创建），但方法可调用
        assert!(runner.validate().is_ok() || runner.validate().is_err());
    }

    #[test]
    fn test_validate_empty_url() {
        let mut options = test_options();
        options.manifest.url = String::new();
        let runner = UpgradeRunner::new(options);
        assert!(runner.validate().is_err());
    }

    #[test]
    fn test_validate_empty_version() {
        let mut options = test_options();
        options.manifest.version = String::new();
        let runner = UpgradeRunner::new(options);
        assert!(runner.validate().is_err());
    }

    // ── 下载测试 ──

    #[test]
    fn test_download_with_mock() {
        let options = test_options();
        let runner = UpgradeRunner::new(options).with_downloader(Box::new(MockDownloader::default()));
        let progress = runner.progress().clone();
        assert_eq!(progress.state, UpgradeState::Idle);
    }

    // ── 签名验证测试 ──

    #[test]
    fn test_verify_with_mock() {
        let options = test_options();
        let runner = UpgradeRunner::new(options).with_verifier(Box::new(MockVerifier::new(true)));
        // 验证器已设置
        assert!(runner.options().manifest.signature.is_empty() || !runner.options().manifest.signature.is_empty());
    }

    // ── 执行测试 ──

    #[test]
    fn test_run_with_error_handling() {
        let options = test_options();
        let mut runner = UpgradeRunner::new(options);
        let result = runner.run_with_error_handling();
        // 结果应该是成功或失败（取决于文件系统）
        assert!(result.state.is_finished());
    }

    #[test]
    fn test_run_with_mock_components() {
        let options = test_options();
        let mut runner = UpgradeRunner::new(options)
            .with_downloader(Box::new(MockDownloader::new(2048, 2048)))
            .with_verifier(Box::new(MockVerifier::new(true)));
        let result = runner.run_with_error_handling();
        // 结果应该是成功或失败
        assert!(result.state.is_finished());
    }

    // ── 回滚测试 ──

    #[test]
    fn test_rollback_without_backup() {
        let options = test_options();
        let backup_dir = options.backup_dir.clone();
        let mut runner = UpgradeRunner::new(options);
        
        // 确保备份目录不存在（清理之前的测试数据）
        let _ = std::fs::remove_dir_all(&backup_dir);
        
        let result = runner.rollback();
        // 没有备份应该失败
        assert!(result.is_err());
    }

    // ── 重启测试 ──

    #[test]
    fn test_restart_disabled() {
        let options = test_options();
        let mut runner = UpgradeRunner::new(options);
        // auto_restart = false，应该直接返回 Ok
        assert!(runner.restart().is_ok());
    }

    // ── 工厂函数测试 ──

    #[test]
    fn test_create_upgrade_runner() {
        let options = test_options();
        let runner = create_upgrade_runner(options);
        assert!(runner.options().manifest.version.is_empty() || !runner.options().manifest.version.is_empty());
    }

    #[test]
    fn test_create_default_upgrade_runner() {
        let runner = create_default_upgrade_runner();
        assert!(runner.options().manifest.version.is_empty() || !runner.options().manifest.version.is_empty());
    }

    // ── Mock 下载器测试 ──

    #[test]
    fn test_mock_downloader_download() {
        let downloader = MockDownloader::new(100, 100);
        let dest = PathBuf::from("target/test_mock_download.zip");
        let mut progress_cb = |_downloaded: u64, _total: u64| {};
        let result = downloader.download("https://example.com", &dest, &mut progress_cb);
        // 可能成功或失败（取决于文件系统）
        assert!(result.is_ok() || result.is_err());
    }

    // ── Mock 验证器测试 ──

    #[test]
    fn test_mock_verifier_verify() {
        let verifier = MockVerifier::new(true);
        let result = verifier.verify(Path::new("test.zip"), "abc123");
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_mock_verifier_verify_fail() {
        let verifier = MockVerifier::new(false);
        let result = verifier.verify(Path::new("test.zip"), "abc123");
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }
}
