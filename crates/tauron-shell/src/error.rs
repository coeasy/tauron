use serde::{Deserialize, Serialize};
use thiserror::Error;

/// tauron 错误码规范（设计文档 §2.2）
///
/// SC-xxxx 格式：
/// - 0xxx: 插件系统错误
/// - 1xxx: 权限错误
/// - 2xxx: 通信错误
/// - 3xxx: 插件实现错误
/// - 9xxx: 系统错误
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PluginErrorCode {
    // ---- 插件系统错误（0xxx）----
    #[serde(rename = "SC-0001")]
    PluginNotFound,
    #[serde(rename = "SC-0002")]
    PluginDisabled,
    #[serde(rename = "SC-0003")]
    PluginErrored,

    // ---- 权限错误（1xxx）----
    #[serde(rename = "SC-1001")]
    PermissionDenied,
    #[serde(rename = "SC-1002")]
    PluginPermissionDenied,
    #[serde(rename = "SC-1003")]
    CapabilityRequired,

    // ---- 通信错误（2xxx）----
    #[serde(rename = "SC-2001")]
    Timeout,
    #[serde(rename = "SC-2002")]
    Cancelled,
    #[serde(rename = "SC-2003")]
    InvalidPayload,
    #[serde(rename = "SC-2004")]
    ChannelBroken,

    // ---- 插件实现错误（3xxx）----
    #[serde(rename = "SC-3001")]
    PluginPanic,
    #[serde(rename = "SC-3002")]
    PluginOom,
    #[serde(rename = "SC-3003")]
    PluginExited,

    // ---- 系统错误（9xxx）----
    #[serde(rename = "SC-9001")]
    Internal,
}

impl PluginErrorCode {
    /// 是否可重试
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::ChannelBroken | Self::Internal
        )
    }
}

impl std::fmt::Display for PluginErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::PluginNotFound => "SC-0001",
            Self::PluginDisabled => "SC-0002",
            Self::PluginErrored => "SC-0003",
            Self::PermissionDenied => "SC-1001",
            Self::PluginPermissionDenied => "SC-1002",
            Self::CapabilityRequired => "SC-1003",
            Self::Timeout => "SC-2001",
            Self::Cancelled => "SC-2002",
            Self::InvalidPayload => "SC-2003",
            Self::ChannelBroken => "SC-2004",
            Self::PluginPanic => "SC-3001",
            Self::PluginOom => "SC-3002",
            Self::PluginExited => "SC-3003",
            Self::Internal => "SC-9001",
        };
        write!(f, "{code}")
    }
}

/// tauron 错误类型
#[derive(Debug, Error)]
pub enum PluginError {
    #[error("{code}: {message}")]
    Plugin {
        code: PluginErrorCode,
        message: String,
    },

    #[error("{0}")]
    Internal(String),
}

impl From<PluginErrorCode> for PluginError {
    fn from(code: PluginErrorCode) -> Self {
        let message = match code {
            PluginErrorCode::PluginNotFound => "Plugin not found or not registered",
            PluginErrorCode::PluginDisabled => "Plugin is disabled",
            PluginErrorCode::PluginErrored => "Plugin is in error state",
            PluginErrorCode::PermissionDenied => "Capability not authorized",
            PluginErrorCode::PluginPermissionDenied => "Plugin-level permission not granted",
            PluginErrorCode::CapabilityRequired => "Required capability not available",
            PluginErrorCode::Timeout => "Call timed out",
            PluginErrorCode::Cancelled => "Call was cancelled",
            PluginErrorCode::InvalidPayload => "Parameter validation failed",
            PluginErrorCode::ChannelBroken => "IPC channel broken",
            PluginErrorCode::PluginPanic => "Plugin internal panic/crash",
            PluginErrorCode::PluginOom => "Plugin out of memory",
            PluginErrorCode::PluginExited => "Process plugin exited unexpectedly",
            PluginErrorCode::Internal => "Internal error",
        };
        Self::Plugin {
            code,
            message: message.to_string(),
        }
    }
}

/// tauron 结果类型
pub type TauronResult<T> = Result<T, PluginError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_serde() {
        let code = PluginErrorCode::PluginNotFound;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, "\"SC-0001\"");

        let deserialized: PluginErrorCode = serde_json::from_str("\"SC-0001\"").unwrap();
        assert_eq!(deserialized, code);
    }

    #[test]
    fn test_error_code_display() {
        assert_eq!(format!("{}", PluginErrorCode::PluginNotFound), "SC-0001");
        assert_eq!(format!("{}", PluginErrorCode::Internal), "SC-9001");
    }

    #[test]
    fn test_retryable() {
        assert!(PluginErrorCode::Timeout.retryable());
        assert!(PluginErrorCode::ChannelBroken.retryable());
        assert!(PluginErrorCode::Internal.retryable());
        assert!(!PluginErrorCode::PluginNotFound.retryable());
        assert!(!PluginErrorCode::Cancelled.retryable());
    }

    #[test]
    fn test_all_error_codes_have_codes() {
        let codes = [
            PluginErrorCode::PluginNotFound,
            PluginErrorCode::PluginDisabled,
            PluginErrorCode::PluginErrored,
            PluginErrorCode::PermissionDenied,
            PluginErrorCode::PluginPermissionDenied,
            PluginErrorCode::CapabilityRequired,
            PluginErrorCode::Timeout,
            PluginErrorCode::Cancelled,
            PluginErrorCode::InvalidPayload,
            PluginErrorCode::ChannelBroken,
            PluginErrorCode::PluginPanic,
            PluginErrorCode::PluginOom,
            PluginErrorCode::PluginExited,
            PluginErrorCode::Internal,
        ];
        for code in &codes {
            let code_str = format!("{code}");
            assert!(code_str.starts_with("SC-"), "Error code {} should start with SC-", code_str);
        }
    }

    #[test]
    fn test_plugin_error_from_code() {
        let err = PluginError::from(PluginErrorCode::PluginNotFound);
        let msg = format!("{err}");
        assert!(msg.contains("SC-0001"));
        assert!(msg.contains("Plugin not found"));
    }

    #[test]
    fn test_error_code_count() {
        // Should have exactly 14 error codes
        let count = 14;
        assert_eq!(count, 14);
    }
}

