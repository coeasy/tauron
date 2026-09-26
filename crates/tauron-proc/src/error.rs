// §4.7 进程插件 host 的错误类型。
//
// 不跨 IPC：进程管理逻辑在宿主侧执行。
//
// **0.4 审计纪律**：每个变体必须有**生产产生点**（不是只有映射表/测试引用）。
// 此前的 `SignatureInvalid` / `HashMismatch` / `FrameTooLarge` / `StdoutPollution` /
// `ConcurrencyLimit` / `Timeout` / `HeartbeatLost` / `CrashLimitExceeded` /
// `ConfigParse` 九个变体全仓零产生点（孤儿变体，随孤儿 `ProcRunner` 一并删除）。
// 未来真实实现（真实验签、帧上限、心跳监控）落地时**按需新增**，不预留占位。

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProcError {
    /// ABI 版本不匹配。
    ///
    /// 产生点：[`crate::validate_abi`]（spawn 前 ABI 契约比对）。
    #[error("ABI 版本不匹配：期望 {expected}，实际 {actual}")]
    AbiMismatch { expected: String, actual: String },

    /// 进程已终止。
    ///
    /// 产生点：`spawner.rs` 的 `write_frame` / `kill`（目标进程不存在或已退出）。
    #[error("进程已终止：{0}")]
    ProcessTerminated(String),

    /// 启动失败（exec 失败 / 路径不存在 / 权限不足等）。
    ///
    /// 与 [`ProcError::ProcessTerminated`] 区分：那个是"起来之后死了"，这个是
    /// "从来没起来"。上层据此判断该不该铸租约——启动失败**绝不能**铸租约。
    /// 产生点：`spawner.rs` 的 `CommandSpawner::spawn`。
    #[error("进程启动失败：{0}")]
    SpawnFailed(String),

    /// 无效的 spawn 配置。
    ///
    /// 产生点：[`crate::validate_spawn_config`]（adapter 与本 crate 共用同一份规则）。
    #[error("无效的 spawn 配置：{0}")]
    InvalidSpawnConfig(String),
}

pub type ProcResult<T> = Result<T, ProcError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(ProcError::AbiMismatch { expected: "1.0".into(), actual: "2.0".into() }
            .to_string()
            .contains("1.0"));
        assert!(ProcError::ProcessTerminated("p".into()).to_string().contains("已终止"));
        assert!(ProcError::SpawnFailed("e".into()).to_string().contains("启动失败"));
        assert!(ProcError::InvalidSpawnConfig("x".into()).to_string().contains("无效"));
    }
}
