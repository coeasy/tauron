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
}

pub type DistributeResult<T> = Result<T, DistributeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(DistributeError::EndpointError("500".into()).to_string().contains("500"));
        assert!(DistributeError::SignatureInvalid.to_string().contains("签名"));
    }
}
