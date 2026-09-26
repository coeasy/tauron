// §4.15 通知中心的错误类型。
//
// 不跨 IPC：通知在宿主进程内被消费，由 §4.10 UI 翻译成
// `tauron_host::error::HostError`。不新增 `ErrorCode`。

/// 通知中心的错误。
#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// 容量已满且无法裁剪（配置错误）。
    #[error("容量为 0，无法写入通知")]
    ZeroCapacity,

    /// 条目 id 重复。
    #[error("通知 id `{0}` 已存在")]
    DuplicateId(String),

    /// 分组键非法（空段、`$` 保留前缀）。
    #[error("非法分组键 `{0}`")]
    InvalidGroup(String),

    /// 存储层 I/O 错误。
    #[error("存储错误：{0}")]
    Store(String),
}

pub type NotifyResult<T> = Result<T, NotifyError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_specific() {
        assert!(NotifyError::ZeroCapacity.to_string().contains("0"));
        assert!(NotifyError::DuplicateId("n1".into()).to_string().contains("n1"));
        assert!(NotifyError::InvalidGroup("bad".into()).to_string().contains("bad"));
    }
}
