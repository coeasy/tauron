// §4.7 进程插件 host 的错误类型。
//
// 不跨 IPC：进程管理逻辑在宿主侧执行。

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProcError {
    /// 二进制签名校验失败。
    #[error("二进制签名校验失败")]
    SignatureInvalid,

    /// 二进制哈希不匹配。
    #[error("二进制哈希不匹配")]
    HashMismatch,

    /// ABI 版本不匹配。
    #[error("ABI 版本不匹配：期望 {expected}，实际 {actual}")]
    AbiMismatch { expected: String, actual: String },

    /// 帧过大。
    #[error("帧过大：{size} bytes，上限 {max} bytes")]
    FrameTooLarge { size: usize, max: usize },

    /// stdout 污染帧检测。
    #[error("stdout 污染帧检测：{0}")]
    StdoutPollution(String),

    /// 并发上限。
    #[error("并发上限：{limit}")]
    ConcurrencyLimit { limit: usize },

    /// 超时。
    #[error("超时：{0}")]
    Timeout(String),

    /// 心跳丢失。
    #[error("心跳丢失：{0}")]
    HeartbeatLost(String),

    /// 崩溃重启超限。
    #[error("崩溃重启超限：{plugin_id}")]
    CrashLimitExceeded { plugin_id: String },

    /// 进程已终止。
    #[error("进程已终止：{0}")]
    ProcessTerminated(String),

    /// 启动失败（exec 失败 / 路径不存在 / 权限不足等）。
    ///
    /// 与 [`ProcError::ProcessTerminated`] 区分：那个是"起来之后死了"，这个是
    /// "从来没起来"。上层据此判断该不该铸租约——启动失败**绝不能**铸租约。
    #[error("进程启动失败：{0}")]
    SpawnFailed(String),

    /// 配置文件解析错误。
    #[error("配置文件解析错误：{0}")]
    ConfigParse(String),

    /// 无效的 spawn 配置。
    #[error("无效的 spawn 配置：{0}")]
    InvalidSpawnConfig(String),
}

pub type ProcResult<T> = Result<T, ProcError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(ProcError::SignatureInvalid.to_string().contains("签名"));
        assert!(ProcError::HashMismatch.to_string().contains("哈希"));
        assert!(ProcError::AbiMismatch {
            expected: "1.0".into(),
            actual: "2.0".into(),
        }
        .to_string()
        .contains("1.0"));
        assert!(ProcError::FrameTooLarge {
            size: 1000,
            max: 512,
        }
        .to_string()
        .contains("1000"));
    }
}
