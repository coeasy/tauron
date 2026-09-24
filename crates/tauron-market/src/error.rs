// §4.18 商城的错误类型。

#[derive(Debug, Clone, thiserror::Error)]
pub enum MarketError {
    /// 签名校验失败。
    #[error("包签名校验失败：{0}")]
    SignatureInvalid(String),

    /// Hash 不符。
    #[error("文件 hash 不符：期望 {expected}，实际 {actual}")]
    HashMismatch { expected: String, actual: String },

    /// 路径穿越攻击。
    #[error("恶意路径：`{0}`（含 `..`/绝对路径/符号链接）")]
    PathTraversal(String),

    /// 条目数超限。
    #[error("压缩包条目数 {actual} 超过上限 {limit}")]
    EntryCountExceeded { actual: usize, limit: usize },

    /// 解压后体积超限。
    #[error("解压后体积 {actual_mb} MB 超过上限 {limit_mb} MB")]
    UnpackedSizeExceeded { actual_mb: u64, limit_mb: u64 },

    /// 单文件超限。
    #[error("文件 `{name}` 体积 {size_mb} MB 超过上限 {limit_mb} MB")]
    FileTooLarge { name: String, size_mb: u64, limit_mb: u64 },

    /// 压缩比超限。
    #[error("压缩比 {ratio:.1}× 超过上限 {limit}×")]
    CompressionRatioExceeded { ratio: f64, limit: u32 },

    /// Framework 不匹配。
    #[error("插件 `{plugin}` 的 framework range `{range}` 不匹配当前 `{current}`")]
    FrameworkMismatch { plugin: String, range: String, current: String },

    /// 降级安装被拒。
    #[error("拒绝降级：当前版本 {current}，目标版本 {target}")]
    DowngradeRejected { current: String, target: String },

    /// 插件已被吊销。
    #[error("插件 `{plugin}` 已被吊销（kid={kid}）")]
    Revoked { plugin: String, kid: String },

    /// 未知签名密钥。
    #[error("未知签名密钥 kid={0}")]
    UnknownKey(String),

    /// 清单格式错误。
    #[error("包清单格式错误：{0}")]
    ManifestFormat(String),

    /// 索引拉取失败。
    #[error("索引拉取失败：{0}")]
    IndexFetch(String),
}

pub type MarketResult<T> = Result<T, MarketError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(MarketError::PathTraversal("../../etc/passwd".into())
            .to_string()
            .contains("../../etc/passwd"));
        assert!(MarketError::DowngradeRejected {
            current: "2.0.0".into(),
            target: "1.0.0".into()
        }
        .to_string()
        .contains("拒绝降级"));
    }
}
