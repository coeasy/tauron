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

    /// 模块声明导入的 host_fn 没有注册处理器（V7-P0-01：不得返回 mock 成功）。
    #[error("host_fn `{fn_name}` 未注册处理器：拒绝执行")]
    HostFnNotRegistered { fn_name: String },

    /// 宿主 handler 明确报错（guest 拿到的是失败，不是成功）。
    #[error("host_fn 执行失败：{reason}")]
    HostFnFailed { reason: String },

    /// 没有任何模块 generation 处于 active 状态（V7-P0-01 的核心门禁）。
    #[error("插件 `{plugin_id}` 没有已加载的 WASM 模块 generation：{reason}")]
    ModuleNotLoaded { plugin_id: String, reason: String },

    /// provider 缺席：本仓不暴露"成功形状"的结果。
    #[error("未装配 WASM 运行时 provider（{provider}）：{reason}")]
    NoRuntimeProvider { provider: String, reason: String },

    /// 请求的导出函数不在模块的真实导出表里。
    #[error("{plugin_id} 的已加载模块没有导出 `{function_name}`")]
    ExportNotFound { plugin_id: String, function_name: String },

    /// 引擎 trap（guest 代码真的陷入不可恢复状态）。
    #[error("WASM 引擎 trap：{reason}")]
    EngineTrap { reason: String },

    /// fuel 用尽 = 引擎真的中断了执行。
    #[error("WASM 执行超出 fuel 预算（已用 {fuel_consumed}，预算 {fuel_budget}）")]
    FuelExhausted { fuel_consumed: u64, fuel_budget: u64 },

    /// 引擎侧的其它失败（配置 fuel、计量读数等），一律是失败，不是成功。
    #[error("WASM 引擎失败：{reason}")]
    EngineFailure { reason: String },

    /// 调用参数无法映射到导出函数的真实签名。
    #[error("WASM 调用参数无效：{0}")]
    InvalidArgs(String),

    /// provider 不支持的签名（本仓内置 provider 只支持整数 ABI）。
    #[error("WASM 签名不受支持：{signature}（内置 provider 仅支持整数 ABI）")]
    UnsupportedSignature { signature: String },

    /// 实例属于已被替换的旧模块 generation。
    #[error(
        "实例 `{instance_id}` 属于插件 `{plugin_id}` 的旧模块 generation {generation}，已失效"
    )]
    StaleInstanceGeneration { plugin_id: String, instance_id: String, generation: u64 },

    /// 内存配额超限。
    #[error("内存配额超限：{plugin_id}（{size} pages，上限 {max} pages）")]
    MemoryLimitExceeded { plugin_id: String, size: u32, max: u32 },

    /// 实例池已满。
    #[error("实例池已满：{plugin_id}")]
    InstancePoolFull { plugin_id: String },

    /// 模块缓存已满。
    #[error("模块缓存已满：{plugin_id}")]
    ModuleCacheFull { plugin_id: String },

    /// Pack/cache generation authority rejected activation or execution lease.
    #[error("模块缓存租约失败：{plugin_id}，原因 {reason}")]
    ModuleLease { plugin_id: String, reason: String },

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
        assert!(WasmError::AbiMismatch { expected: "1.0".into(), actual: "2.0".into() }
            .to_string()
            .contains("1.0"));
        assert!(WasmError::HostFnNotAllowed { fn_name: "my_fn".into() }
            .to_string()
            .contains("my_fn"));
        assert!(WasmError::MemoryLimitExceeded { plugin_id: "test".into(), size: 100, max: 50 }
            .to_string()
            .contains("100"));
    }
}
