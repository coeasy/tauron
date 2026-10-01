//! 结构化错误模型。
//!
//! 计划 §4.1 要求：未知 plugin_id 返回结构化错误而非 panic；
//! ADR-04 要求：每个入口 `catch_unwind`，panic → `E_HOST_PANIC`，
//! 且 JS 侧必须在超时前收到错误（Tauri 命令路径本身不捕获 panic）。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 宿主侧协议化错误码。前端据此分流（重试 / 提示用户 / 上报）。
///
/// 变体名刻意采用 `E_*` 大写蛇形：这是**跨 IPC 的线上协议名**（JS 侧按此分流），
/// 改名即破坏兼容，故压制 camelCase 提示。
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    /// handler panic，经 `catch_unwind` 归一化（ADR-04）。
    E_HOST_PANIC,
    /// 未知 `plugin_id`。
    E_UNKNOWN_PLUGIN,
    /// 授权档位不满足，或身份被伪造（§2.1 self 档忽略入参 pluginId）。
    E_AUTH_DENIED,
    /// manifest 不合法：未知字段、非法 id、权限表外字符串、缺 `framework` range。
    E_INVALID_MANIFEST,
    /// 状态机无匹配规则（非法迁移）。
    E_STATE_INVALID_TRANSITION,
    /// pending call 不存在或已结束。
    E_CALL_NOT_FOUND,
    /// pending call 超时。
    E_CALL_TIMEOUT,
    /// 申请了"禁止授予清单"内的权限（§2.1）。
    E_FORBIDDEN_PERMISSION,
    /// ABI 指纹不匹配。
    ///
    /// **真实产生点**：`host_runtime_spawn` 在 spawn 前比对宿主 ABI 契约
    /// （`tauron_proc::current_abi_contract`）与调用方声明的 `profile.abi`，
    /// 不符即拒（映射在 `tauron-adapter` 的 `proc_error_to_host`）。
    ///
    /// **与 `E_INVALID_MANIFEST` 的分工**：清单里 `abi` 字段**缺失或非法**
    /// （D13 加载期硬校验）报 `E_INVALID_MANIFEST`；**两份都合法但彼此不符**才报本码。
    /// 独立于 `E_INSTALL_FAILED`：ABI 不匹配是**版本兼容**问题，调用方该升级
    /// 插件/宿主，而不是重装。
    E_ABI_MISMATCH,
    /// 插件已禁用——`enabled` 标志做即时拒绝（ADR-05）。
    E_PLUGIN_DISABLED,
    /// 注册表达到活跃身份上限（§4.6：缺省 8）。
    E_REGISTRY_FULL,
    /// pending call 表达到上限。
    E_CALL_PENDING_FULL,
    /// 订阅表达到上限（`eventbus::MAX_SUBSCRIPTIONS`）。
    E_SUBSCRIPTION_FULL,
    /// 插件已存在（重复注册）。
    E_PLUGIN_EXISTS,
    /// 安装期失败：验签 / hash / 解包 / range 任一环节（D3：落 `INSTALL_FAILED`）。
    E_INSTALL_FAILED,
    /// 插件被配置过滤器排除（配置化选择加载；用户可修改配置后重试）。
    E_PLUGIN_FILTERED,
    /// 该插件类型没有运行期执行器（例如对 js/rust/wasm 插件调 `host_runtime_spawn`）。
    ///
    /// 诚实失败码：不静默成功、也不伪造一个 pid。P1-2 的 wasm 执行器同样用它。
    E_PLUGIN_TYPE_NO_RUNTIME,
    /// 运行时租约不存在或已失效（进程已回收 / 宿主已重启）。
    ///
    /// **不是** `E_CALL_NOT_FOUND`：这是租约语义，调用方据此分流的动作也不同
    /// （重新 spawn，而不是放弃一次 pending 调用）。
    E_LEASE_EXPIRED,
    /// 流句柄数已达上限（同一进程内并发打开的流太多）。
    ///
    /// **不是** `E_REGISTRY_FULL`：后者说的是**插件**数达上限（`max_plugins`），
    /// 调用方该卸载插件；本码说的是**流句柄**达上限
    /// （[`crate::stream::MAX_STREAMS`]），调用方该先 `host_stream_close` 再开。
    /// 与 `E_SUBSCRIPTION_FULL` 同属"容量闸"族，但对象是流而不是订阅。
    ///
    /// ⚠️ 追加码必须加在枚举**末尾**：TS 侧 `HOST_ERROR_CODES` 按声明顺序比对
    /// （`@tauron/contract-tests` 的 wire-gate 钉死）。
    E_STREAM_FULL,
    /// 一次跨主体调用已被结算，重复回填被拒（0.4-A1）。
    ///
    /// **为什么不接受覆盖**：`host_call_result` 是「执行方报告这次调用的结果」。
    /// 允许重复回填等于让"第一次的结果"可以被第二次悄悄改写——调用方拿到
    /// 哪个结果取决于时序，而不是取决于事实。宁可显式失败（调用方据此判定
    /// 执行方行为异常），也不让它变成竞态。
    ///
    /// **不是** `E_CALL_NOT_FOUND`：条目**还在**（调用方还没取走），只是不再接受
    /// 新的结果；调用方的下一步动作也不同（去 `host_call_take` 取已结算的结果，
    /// 而不是放弃这次调用）。
    E_CALL_ALREADY_SETTLED,
    /// manifest 声明的贡献与 activate 期实际注册的贡献不一致（0.4-W3）。
    ///
    /// **为什么是错误而不是"报告里的一个布尔位"**：`contributes` 是插件对外承诺的
    /// 扩展点清单（命令面板 / 菜单 / 面板 / 设置 Tab 都从它派生 UI）。声明了却没注册
    /// = 用户能看到入口但点了没反应；注册了却没声明 = 存在一个来源不明的入口。
    /// 两者都是「宣称」与「事实」的落差，必须显式失败，不能被调用方当成成功忽略。
    ///
    /// **不是** `E_INVALID_MANIFEST`：manifest 本身合法（`Contributes::validate` 已过），
    /// 不一致发生在**运行期注册**与声明之间——是"没做到"，不是"写错了"。
    E_CONTRIBUTES_DRIFT,
    /// Stream producer exhausted its receiver-granted byte window.
    /// The rejected frame is not delivered and does not consume a sequence number.
    E_STREAM_BACKPRESSURE,
    /// Synchronous call graph would cycle, re-enter a principal, or exceed the bounded hop budget.
    E_CALL_CYCLE,
    /// Event causation chain exceeded the bounded depth budget.
    E_EVENT_CAUSATION_LIMIT,
}

/// V4 retry semantics. A boolean cannot safely express panic/transaction/reconnect behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetryClass {
    /// Never retry automatically. User/admin action or deterministic repair is required.
    Never,
    /// A human may intentionally repeat the operation after inspecting the failure.
    Manual,
    /// SDK/host may retry only when the operation is independently declared idempotent.
    AutoIdempotent,
    /// Retry is meaningful only after a fresh authenticated session/transport is established.
    AfterReconnect,
}

impl ErrorCode {
    /// Stable retry classification. Operation idempotency and retry budgets remain separate facts.
    pub const fn retry_class(self) -> RetryClass {
        match self {
            // A panic can happen after a partial side effect. catch_unwind is containment,
            // not rollback proof; automatic retry is therefore unsafe by default.
            Self::E_HOST_PANIC => RetryClass::Never,
            Self::E_CALL_TIMEOUT => RetryClass::Manual,
            Self::E_PLUGIN_FILTERED => RetryClass::Manual,
            Self::E_LEASE_EXPIRED => RetryClass::AfterReconnect,
            Self::E_STREAM_BACKPRESSURE => RetryClass::Manual,
            Self::E_CALL_CYCLE => RetryClass::Never,
            Self::E_EVENT_CAUSATION_LIMIT => RetryClass::Never,
            _ => RetryClass::Never,
        }
    }

    /// Legacy wire compatibility. True only when an automatic retry may be considered.
    ///
    /// V4 intentionally returns false for panic/timeouts until a specific operation proves
    /// idempotency and a bounded RetryPolicy elects to retry.
    pub const fn retryable(self) -> bool {
        matches!(self.retry_class(), RetryClass::AutoIdempotent)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::E_HOST_PANIC => write!(f, "E_HOST_PANIC"),
            Self::E_UNKNOWN_PLUGIN => write!(f, "E_UNKNOWN_PLUGIN"),
            Self::E_AUTH_DENIED => write!(f, "E_AUTH_DENIED"),
            Self::E_INVALID_MANIFEST => write!(f, "E_INVALID_MANIFEST"),
            Self::E_STATE_INVALID_TRANSITION => write!(f, "E_STATE_INVALID_TRANSITION"),
            Self::E_CALL_NOT_FOUND => write!(f, "E_CALL_NOT_FOUND"),
            Self::E_CALL_TIMEOUT => write!(f, "E_CALL_TIMEOUT"),
            Self::E_FORBIDDEN_PERMISSION => write!(f, "E_FORBIDDEN_PERMISSION"),
            Self::E_ABI_MISMATCH => write!(f, "E_ABI_MISMATCH"),
            Self::E_PLUGIN_DISABLED => write!(f, "E_PLUGIN_DISABLED"),
            Self::E_REGISTRY_FULL => write!(f, "E_REGISTRY_FULL"),
            Self::E_CALL_PENDING_FULL => write!(f, "E_CALL_PENDING_FULL"),
            Self::E_SUBSCRIPTION_FULL => write!(f, "E_SUBSCRIPTION_FULL"),
            Self::E_PLUGIN_EXISTS => write!(f, "E_PLUGIN_EXISTS"),
            Self::E_INSTALL_FAILED => write!(f, "E_INSTALL_FAILED"),
            Self::E_PLUGIN_FILTERED => write!(f, "E_PLUGIN_FILTERED"),
            // 新码一律**追加在末尾**：TS 侧 `HOST_ERROR_CODES` 与本 impl 逐项同序
            // （`tauron-contract-tests` 的门禁按声明顺序比对），插在中间会让既有
            // 客户端的分流表整体错位。
            Self::E_PLUGIN_TYPE_NO_RUNTIME => write!(f, "E_PLUGIN_TYPE_NO_RUNTIME"),
            Self::E_LEASE_EXPIRED => write!(f, "E_LEASE_EXPIRED"),
            Self::E_STREAM_FULL => write!(f, "E_STREAM_FULL"),
            Self::E_CALL_ALREADY_SETTLED => write!(f, "E_CALL_ALREADY_SETTLED"),
            Self::E_CONTRIBUTES_DRIFT => write!(f, "E_CONTRIBUTES_DRIFT"),
            Self::E_STREAM_BACKPRESSURE => write!(f, "E_STREAM_BACKPRESSURE"),
            Self::E_CALL_CYCLE => write!(f, "E_CALL_CYCLE"),
            Self::E_EVENT_CAUSATION_LIMIT => write!(f, "E_EVENT_CAUSATION_LIMIT"),
        }
    }
}

/// 宿主侧统一错误。
///
/// 派生 `Serialize`：跨 IPC 时以结构化 JSON（`{ code, message, retryable }`）
/// 穿越，而不是 `{:?}` 文本转储。前端 `normalizeError` 的分支 1 依此还原
/// `message` 与 `retryable`；若只传字符串，前端只能靠正则从
/// `HostError { code: E_XXX, … }` 转储里反抠错误码，`message` 字段整个丢失。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(rename_all = "camelCase")]
#[error("{code}: {message}")]
pub struct HostError {
    /// 协议化错误码。
    pub code: ErrorCode,
    /// 面向开发者/日志的说明。
    pub message: String,
    /// Legacy compatibility flag. Prefer `retry_class`.
    pub retryable: bool,
    /// V4 structured retry semantics; never implies an unbounded SDK retry loop.
    pub retry_class: RetryClass,
}

impl HostError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.retryable(),
            retry_class: code.retry_class(),
        }
    }
}

/// 宿主侧结果别名。
pub type HostResult<T> = std::result::Result<T, HostError>;

/// 把闭包包进 `catch_unwind`，panic 归一化为 `E_HOST_PANIC`。
///
/// 计划 §4.1："每个入口 `catch_unwind`"。这是 ADR-04 的落地单点——
/// Tauri 命令路径不捕获 panic，若不在此处收口，JS 侧 `invoke()` 会永久挂起
/// （官方 issue #10327），用户看到的是"卡死"而非报错。
pub fn guard<F, T>(what: &str, f: F) -> HostResult<T>
where
    F: FnOnce() -> T,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Ok(v),
        Err(payload) => Err(HostError::new(
            ErrorCode::E_HOST_PANIC,
            format!("{} panicked: {}", what, panic_message(&payload)),
        )),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_captures_panic_as_e_host_panic() {
        let res: HostResult<u32> = guard("boom", || panic!("kaboom"));
        let err = res.expect_err("panic 必须被捕获");
        assert_eq!(err.code, ErrorCode::E_HOST_PANIC);
        assert!(!err.retryable);
        assert_eq!(err.retry_class, RetryClass::Never);
        assert!(err.message.contains("kaboom"), "payload 应可读：{}", err.message);
    }

    #[test]
    fn guard_passes_through_ok() {
        let res: HostResult<u32> = guard("fine", || 42);
        assert_eq!(res, Ok(42));
    }

    #[test]
    fn guard_handles_string_payload() {
        let res: HostResult<()> = guard("s", || std::panic::panic_any(String::from("owned")));
        let err = res.unwrap_err();
        assert_eq!(err.code, ErrorCode::E_HOST_PANIC);
        assert!(err.message.contains("owned"));
    }

    #[test]
    fn error_code_retryability() {
        assert!(!ErrorCode::E_CALL_TIMEOUT.retryable());
        assert!(!ErrorCode::E_HOST_PANIC.retryable());
        assert_eq!(ErrorCode::E_HOST_PANIC.retry_class(), RetryClass::Never);
        assert_eq!(ErrorCode::E_CALL_TIMEOUT.retry_class(), RetryClass::Manual);
        assert_eq!(ErrorCode::E_PLUGIN_FILTERED.retry_class(), RetryClass::Manual);
        assert_eq!(ErrorCode::E_LEASE_EXPIRED.retry_class(), RetryClass::AfterReconnect);
        assert!(!ErrorCode::E_INVALID_MANIFEST.retryable());
        assert!(!ErrorCode::E_ABI_MISMATCH.retryable());
        // 声明/事实不一致是**确定性**故障：重试不会让缺失的注册出现。
        assert!(!ErrorCode::E_CONTRIBUTES_DRIFT.retryable());
    }

    #[test]
    fn contributes_drift_uses_its_wire_name() {
        assert_eq!(format!("{}", ErrorCode::E_CONTRIBUTES_DRIFT), "E_CONTRIBUTES_DRIFT");
        let s = serde_json::to_string(&ErrorCode::E_CONTRIBUTES_DRIFT).unwrap();
        assert_eq!(s, "\"E_CONTRIBUTES_DRIFT\"");
    }

    #[test]
    fn error_code_roundtrips_through_json() {
        for code in
            [ErrorCode::E_HOST_PANIC, ErrorCode::E_AUTH_DENIED, ErrorCode::E_FORBIDDEN_PERMISSION]
        {
            let s = serde_json::to_string(&code).unwrap();
            assert_eq!(s, format!("\"{code}\""));
            let back: ErrorCode = serde_json::from_str(&s).unwrap();
            assert_eq!(back, code);
        }
    }
}
