use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 请求信封（前端 → Rust，设计文档 §2.1）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginInvokeRequest {
    /// 插件 ID（反域名：com.example.formatter）
    pub plugin_id: String,
    /// 方法名（如 "format", "load", "save"）
    pub method: String,
    /// 方法参数（JSON 序列化）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// 调用 ID（UUID，用于关联请求-响应/取消）
    pub call_id: String,
    /// 超时毫秒（默认 30000，0=不超时）
    #[serde(default = "default_timeout", skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

fn default_timeout() -> Option<u64> {
    Some(30_000)
}

impl PluginInvokeRequest {
    /// 获取有效超时（默认 30000）
    pub fn effective_timeout(&self) -> u64 {
        self.timeout_ms.unwrap_or(30_000)
    }
}

/// 响应信封（Rust → 前端，设计文档 §2.1）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginInvokeResponse {
    /// 与请求匹配
    pub call_id: String,
    /// 成功/失败
    pub ok: bool,
    /// 成功时的结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// 失败时的错误
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<PluginErrorBody>,
}

/// 错误体
///
/// 三个字段均为单词，camelCase 与 snake_case 等价；显式声明以与
/// 其余信封闭包保持一致的序列化策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl PluginInvokeResponse {
    /// 构建成功响应
    pub fn ok(call_id: String, result: serde_json::Value) -> Self {
        Self { call_id, ok: true, result: Some(result), error: None }
    }

    /// 构建错误响应
    pub fn error(call_id: String, code: crate::PluginErrorCode, message: String) -> Self {
        Self {
            call_id,
            ok: false,
            result: None,
            error: Some(PluginErrorBody {
                code: format!("{code}"),
                message,
                retryable: code.retryable(),
            }),
        }
    }
}

/// 流式进度事件（通过 Tauri Channel，设计文档 §2.1）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProgressEvent {
    pub call_id: String,
    pub step: u64,
    pub total: u64,
    pub message: String,
    pub percentage: u8,
}

/// 取消请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PluginCancelRequest {
    pub call_id: String,
}

/// 生成新的调用 ID
pub fn generate_call_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_call_id() {
        let id = generate_call_id();
        assert_eq!(id.len(), 36); // UUID format
        let uuid = Uuid::parse_str(&id).unwrap();
        assert_eq!(uuid.get_version_num(), 4);
    }

    #[test]
    fn test_generate_call_id_unique() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..100 {
            ids.insert(generate_call_id());
        }
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn test_request_default_timeout() {
        let req = PluginInvokeRequest {
            plugin_id: "test".to_string(),
            method: "format".to_string(),
            payload: None,
            call_id: "call-1".to_string(),
            timeout_ms: None,
        };
        assert_eq!(req.effective_timeout(), 30_000);
    }

    #[test]
    fn test_request_custom_timeout() {
        let req = PluginInvokeRequest {
            plugin_id: "test".to_string(),
            method: "format".to_string(),
            payload: None,
            call_id: "call-1".to_string(),
            timeout_ms: Some(5000),
        };
        assert_eq!(req.effective_timeout(), 5000);
    }

    #[test]
    fn test_response_ok() {
        let resp =
            PluginInvokeResponse::ok("call-1".to_string(), serde_json::json!({"result": 42}));
        assert!(resp.ok);
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_response_error() {
        let resp = PluginInvokeResponse::error(
            "call-1".to_string(),
            crate::PluginErrorCode::Timeout,
            "timed out".to_string(),
        );
        assert!(!resp.ok);
        assert!(resp.result.is_none());
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, "SC-2001");
        assert_eq!(err.message, "timed out");
        assert!(err.retryable);
    }

    #[test]
    fn test_request_serde_roundtrip() {
        let req = PluginInvokeRequest {
            plugin_id: "com.example.test".to_string(),
            method: "format".to_string(),
            payload: Some(serde_json::json!({"code": "let x = 1;"})),
            call_id: "call-123".to_string(),
            timeout_ms: Some(5000),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: PluginInvokeRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.plugin_id, req.plugin_id);
        assert_eq!(deserialized.method, req.method);
        assert_eq!(deserialized.call_id, req.call_id);
        assert_eq!(deserialized.timeout_ms, Some(5000));
    }

    #[test]
    fn test_request_deny_unknown_fields() {
        let json = r#"{"plugin_id":"test","method":"m","call_id":"c","unknown":1}"#;
        let result: Result<PluginInvokeRequest, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_progress_event() {
        let event = ProgressEvent {
            call_id: "call-1".to_string(),
            step: 3,
            total: 10,
            message: "processing".to_string(),
            percentage: 30,
        };
        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ProgressEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.step, 3);
        assert_eq!(deserialized.total, 10);
    }
}
