// §4.19 CI 分发运维的错误类型。
//
// 不跨 IPC：分发逻辑在 CI/发布管道中执行。

#[derive(Debug, Clone, thiserror::Error)]
pub enum DistributeError {
    /// Endpoint 错误（404/500 等）。
    #[error("更新端点错误：{0}")]
    EndpointError(String),

    /// 灰度批次未就绪（未到最小停留时间）。
    #[error("灰度批次 `{current}` 未就绪（已等待 {elapsed}s，需 {required}s）")]
    GrayscaleNotReady { current: String, elapsed: u64, required: u64 },

    /// 签名校验失败。
    #[error("更新签名校验失败")]
    SignatureInvalid,

    /// 响应体格式错误。
    #[error("更新响应体格式错误：{0}")]
    InvalidBody(String),

    /// 清单解析错误。
    #[error("更新清单解析错误：{0}")]
    ManifestParse(String),

    /// 安装身份（§9.1 InstallationIdentityProvider）读写或校验失败。
    #[error("安装身份错误：{0}")]
    InstallationIdentity(String),

    /// 升级执行器缺少必需组件（未注入 Downloader / SignatureVerifier /
    /// RestartProvider）。缺组件 = 硬失败，绝不做 mock 兜底。
    #[error("升级组件未注入：{component}")]
    UpgradeComponentMissing { component: &'static str },

    /// 安装根 `current` 目录缺失：无可备份、无可回滚，拒绝升级。
    #[error("安装目录缺失：{path}")]
    InstallRootMissing { path: String },

    /// 下载产物 SHA-256 与清单声明不一致。
    #[error("更新包 sha256 不一致：期望 {expected}，实际 {actual}")]
    PackageHashMismatch { expected: String, actual: String },

    /// 降级被拒：清单目标版本低于已安装版本。
    ///
    /// 判定用的是 `tauron-market` 的序关系（唯一算术源），**默认拒绝**：
    /// 更新链没有任何「故意装旧版」的合法入口（回滚走
    /// [`crate::UpgradeRunner::rollback`] 的既有树恢复，不下载新包，因此不受此门禁影响）。
    #[error("拒绝降级：已安装 {current}，清单目标版本 {target}")]
    DowngradeRejected { current: String, target: String },

    /// 归档被拒绝（zip-slip 路径穿越 / 绝对路径 / 盘符 / 符号链接 /
    /// 非常规条目 / 条目数或解压尺寸超限）。
    #[error("归档被拒绝：{0}")]
    ArchiveRejected(String),

    /// 备份核验失败（备份树哈希集合与源树不一致）。
    #[error("备份核验失败：{0}")]
    BackupVerifyFailed(String),

    /// 阶段超时（Download / Extract / Swap / HealthCheck）。
    #[error("升级阶段 {phase} 超过期限 {timeout_secs}s")]
    PhaseTimeout { phase: &'static str, timeout_secs: u64 },

    /// 原子交换失败（含跨文件系统导致的 rename 失败；绝不降级为部分复制）。
    #[error("原子交换失败：{0}")]
    SwapFailed(String),

    /// 提交前健康检查失败（错误文本注明是否已自动回滚）。
    #[error("健康检查失败：{0}")]
    HealthCheckFailed(String),

    /// 回滚失败或无可回滚备份。
    #[error("回滚失败：{0}")]
    RollbackFailed(String),

    /// 注入的 RestartProvider 返回失败。
    #[error("重启失败：{0}")]
    RestartFailed(String),

    /// 通用文件操作失败（备份 / 解压 / 清理；清理失败必须上浮，不得吞掉）。
    #[error("文件操作失败：{0}")]
    FileOperationFailed(String),

    /// 升级状态日志（journal）读写失败。
    #[error("升级状态日志失败：{0}")]
    Journal(String),
}

pub type DistributeResult<T> = Result<T, DistributeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(DistributeError::EndpointError("500".into()).to_string().contains("500"));
        assert!(DistributeError::SignatureInvalid.to_string().contains("签名"));
        assert!(DistributeError::UpgradeComponentMissing { component: "Downloader" }
            .to_string()
            .contains("Downloader"));
        assert!(DistributeError::PhaseTimeout { phase: "Download", timeout_secs: 30 }
            .to_string()
            .contains("30"));
        assert!(DistributeError::PackageHashMismatch { expected: "e".into(), actual: "a".into() }
            .to_string()
            .contains("sha256"));
    }
}
