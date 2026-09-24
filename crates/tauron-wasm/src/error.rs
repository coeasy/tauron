// §4.8 WASM supervisor 的错误类型。
//
// 不跨 IPC：WASM 管理逻辑在宿主侧执行。

#[derive(Debug, Clone, thiserror::Error)]
pub enum WasmError {
    /// ABI 版本不匹配。
    #[error("ABI 版本不匹配：期望 {expected}，实际 {actual}")]
    AbiMismatch { expected: String, actual: String },

    /// host_fn 不在白名单中。
    #[error("host_fn `{fn_name}` 不在白名单中")]
    HostFnNotAllowed { fn_name: String },

    /// 内存配额超限。
    #[error("内存配额超限：{plugin_id}（{size} pages，上限 {max} pages）")]
    MemoryLimitExceeded { plugin_id: String, size: u32, max: u32 },

    /// 实例池已满。
    #[error("实例池已满：{plugin_id}")]
    InstancePoolFull { plugin_id: String },

    /// 模块缓存已满。
    #[error("模块缓存已满：{plugin_id}")]
    ModuleCacheFull { plugin_id: String },

    /// 崩溃计数超限。
    #[error("崩溃计数超限：{plugin_id}")]
    CrashLimitExceeded { plugin_id: String },

    /// 实例创建失败。
    #[error("实例创建失败：{plugin_id}，原因 {reason}")]
    InstanceCreationFailed { plugin_id: String, reason: String },

    /// 模块加载失败。
    #[error("模块加载失败：{plugin_id}，原因 {reason}")]
    ModuleLoadFailed { plugin_id: String, reason: String },

    /// 配置文件解析错误。
    #[error("配置文件解析错误：{0}")]
    ConfigParse(String),

    /// 无效的配置。
    #[error("无效的配置：{0}")]
    InvalidConfig(String),
}

pub type WasmResult<T> = Result<T, WasmError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert!(WasmError::AbiMismatch {
            expected: "1.0".into(),
            actual: "2.0".into(),
        }
        .to_string()
        .contains("1.0"));
        assert!(WasmError::HostFnNotAllowed {
            fn_name: "my_fn".into(),
        }
        .to_string()
        .contains("my_fn"));
        assert!(WasmError::MemoryLimitExceeded {
            plugin_id: "test".into(),
            size: 100,
            max: 50,
        }
        .to_string()
        .contains("100"));
    }
}
