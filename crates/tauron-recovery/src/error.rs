// §4.14 崩溃恢复的错误类型。
//
// 不跨 IPC：恢复逻辑在宿主进程启动阶段执行，
// 由 Tauri 适配层翻译成结构化错误。

#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    /// 快照格式错误。
    #[error("快照格式错误：{0}")]
    SnapshotFormat(String),

    /// 快照陈旧（schema_version 不兼容）。
    #[error("快照 schema_version `{0}` 不兼容（期望 `{1}`）")]
    SnapshotVersion(String, String),

    /// 快照半写/截断。
    #[error("快照被截断或半写（seq={0}）")]
    SnapshotTruncated(u64),

    /// 写入者不匹配。
    #[error("快照写入者 `{0}` 与期望 `{1}` 不匹配")]
    WriterMismatch(String, String),

    /// 动作已执行（幂等键重复）。
    #[error("恢复动作已执行（幂等键 `{0}`）")]
    ActionAlreadyExecuted(String),

    /// 效果已执行。
    #[error("外部副作用 `{0}` 已执行，不重放")]
    EffectAlreadyExecuted(String),

    /// 恢复进行中（不可重入）。
    #[error("恢复正在进行中，不可重入")]
    RecoveryInProgress,

    /// 安全模式下不可操作。
    #[error("安全模式下不可执行此操作")]
    RestrictedInSafemode,

    /// 修复模式下不可操作。
    #[error("修复模式下不可执行此操作")]
    RestrictedInRepairmode,

    /// 试验启用次数已达上限。
    #[error("插件 `{0}` 的试验启用次数已达上限")]
    TrialExhausted(String),

    /// 计数器文件损坏。
    #[error("计数器文件损坏：{0}")]
    CounterCorrupt(String),
}

pub type RecoveryResult<T> = Result<T, RecoveryError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_specific() {
        assert!(RecoveryError::TrialExhausted("p.x".into()).to_string().contains("p.x"));
    }
}
