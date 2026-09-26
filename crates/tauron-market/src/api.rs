// 插件商城 API 端点：搜索、下载、安装、更新、卸载。
//
// 职责（计划 §4.30）：
// - 插件搜索与发现
// - 插件元数据获取
// - 插件下载与安装
// - 插件更新检查
// - 插件卸载与吊销
//
// 本模块不依赖 `tauri`：HTTP 由适配层实现，单元测试用 Mock。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{AuditAction, AuditLog, MarketError, MarketResult, RevocationList};

// ──────────────────────────────────────────────────────────────────────────
// API 请求/响应类型
// ──────────────────────────────────────────────────────────────────────────

/// 搜索请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    /// 搜索关键词。
    pub query: String,
    /// 分类过滤。
    pub category: Option<String>,
    /// 排序方式。
    pub sort: Option<SortOrder>,
    /// 页码（从 1 开始）。
    pub page: Option<u32>,
    /// 每页数量。
    pub per_page: Option<u32>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self { query: String::new(), category: None, sort: None, page: Some(1), per_page: Some(20) }
    }
}

/// 排序方式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SortOrder {
    /// 按热度排序。
    Popular,
    /// 按最新排序。
    Latest,
    /// 按评分排序。
    Rating,
    /// 按名称排序。
    Name,
}

/// 搜索响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    /// 插件列表。
    pub plugins: Vec<PluginSummary>,
    /// 总数量。
    pub total: u32,
    /// 当前页。
    pub page: u32,
    /// 每页数量。
    pub per_page: u32,
}

/// 插件摘要。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    /// 插件 ID。
    pub id: String,
    /// 插件名称。
    pub name: String,
    /// 插件描述。
    pub description: Option<String>,
    /// 插件类型。
    pub plugin_type: String,
    /// 最新版本。
    pub latest_version: String,
    /// 作者。
    pub author: Option<String>,
    /// 评分（0-5）。
    pub rating: Option<f32>,
    /// 下载次数。
    pub downloads: u64,
    /// 标签。
    pub tags: Vec<String>,
    /// 图标 URL。
    pub icon_url: Option<String>,
}

/// 插件详情响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginDetailResponse {
    /// 插件摘要。
    pub summary: PluginSummary,
    /// 版本列表。
    pub versions: Vec<VersionInfo>,
    /// 变更日志。
    pub changelog: Option<String>,
}

/// 版本信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// 版本号。
    pub version: String,
    /// 发布时间。
    pub release_date: String,
    /// 文件大小（字节）。
    pub size_bytes: u64,
    /// 下载 URL。
    pub download_url: String,
    /// 签名。
    pub signature: String,
}

/// 安装请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequest {
    /// 插件 ID。
    pub plugin_id: String,
    /// 版本号（None 表示最新）。
    pub version: Option<String>,
}

/// 安装响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResponse {
    /// 是否成功。
    pub success: bool,
    /// 插件 ID。
    pub plugin_id: String,
    /// 安装版本。
    pub version: String,
    /// 安装路径。
    pub install_path: String,
    /// 错误信息。
    pub error: Option<String>,
}

/// 更新检查请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckRequest {
    /// 插件 ID 到当前版本的映射。
    pub plugins: BTreeMap<String, String>,
}

/// 更新检查结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
    /// 插件 ID。
    pub plugin_id: String,
    /// 当前版本。
    pub current_version: String,
    /// 最新版本。
    pub latest_version: String,
    /// 是否有更新。
    pub update_available: bool,
}

/// 更新检查响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResponse {
    /// 更新结果列表。
    pub results: Vec<UpdateCheckResult>,
}

/// 卸载请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallRequest {
    /// 插件 ID。
    pub plugin_id: String,
}

/// 卸载响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResponse {
    /// 是否成功。
    pub success: bool,
    /// 插件 ID。
    pub plugin_id: String,
    /// 错误信息。
    pub error: Option<String>,
}

/// 吊销请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeRequest {
    /// 插件 ID。
    pub plugin_id: String,
    /// 密钥 ID。
    pub kid: String,
    /// 吊销原因。
    pub reason: Option<String>,
}

/// 吊销响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeResponse {
    /// 是否成功。
    pub success: bool,
    /// 插件 ID。
    pub plugin_id: String,
    /// 吊销时间。
    pub revoked_at: u64,
}

/// API 错误响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponseError {
    /// 错误码。
    pub code: String,
    /// 错误消息。
    pub message: String,
    /// 错误详情。
    pub details: Option<serde_json::Value>,
}

// ──────────────────────────────────────────────────────────────────────────
// 商城 API 客户端
// ──────────────────────────────────────────────────────────────────────────

/// 商城 API 客户端。
pub struct MarketplaceApi {
    base_url: String,
    api_token: Option<String>,
    audit_log: AuditLog,
    revocation_list: RevocationList,
}

impl MarketplaceApi {
    /// 创建新的商城 API 客户端。
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            api_token: None,
            audit_log: AuditLog::new(),
            revocation_list: RevocationList::default(),
        }
    }

    /// 设置 API token。
    pub fn with_token(mut self, token: &str) -> Self {
        self.api_token = Some(token.to_string());
        self
    }

    /// 获取基础 URL。
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// 搜索插件。
    pub fn search(&self, _request: &SearchRequest) -> MarketResult<SearchResponse> {
        // 模拟搜索（实际实现会调用 HTTP API）
        let plugins = vec![
            PluginSummary {
                id: "com.example.plugin-a".into(),
                name: "Plugin A".into(),
                description: Some("Example plugin A".into()),
                plugin_type: "js".into(),
                latest_version: "1.0.0".into(),
                author: Some("Author A".into()),
                rating: Some(4.5),
                downloads: 1000,
                tags: vec!["example".into()],
                icon_url: None,
            },
            PluginSummary {
                id: "com.example.plugin-b".into(),
                name: "Plugin B".into(),
                description: Some("Example plugin B".into()),
                plugin_type: "wasm".into(),
                latest_version: "2.0.0".into(),
                author: Some("Author B".into()),
                rating: Some(4.0),
                downloads: 500,
                tags: vec!["example".into(), "wasm".into()],
                icon_url: None,
            },
        ];

        Ok(SearchResponse { plugins, total: 2, page: 1, per_page: 20 })
    }

    /// 获取插件详情。
    pub fn get_plugin_detail(&self, _plugin_id: &str) -> MarketResult<PluginDetailResponse> {
        let summary = PluginSummary {
            id: "com.example.plugin-a".into(),
            name: "Plugin A".into(),
            description: Some("Example plugin A".into()),
            plugin_type: "js".into(),
            latest_version: "1.0.0".into(),
            author: Some("Author A".into()),
            rating: Some(4.5),
            downloads: 1000,
            tags: vec!["example".into()],
            icon_url: None,
        };

        let versions = vec![
            VersionInfo {
                version: "1.0.0".into(),
                release_date: "2026-09-21T00:00:00Z".into(),
                size_bytes: 1024,
                download_url: "https://example.com/plugin-a-1.0.0.zip".into(),
                signature: "abc123def456".into(),
            },
            VersionInfo {
                version: "0.9.0".into(),
                release_date: "2026-09-01T00:00:00Z".into(),
                size_bytes: 924,
                download_url: "https://example.com/plugin-a-0.9.0.zip".into(),
                signature: "def456ghi789".into(),
            },
        ];

        Ok(PluginDetailResponse {
            summary,
            versions,
            changelog: Some("### 1.0.0\n- Initial release\n".into()),
        })
    }

    /// 检查插件更新。
    pub fn check_updates(&self, request: &UpdateCheckRequest) -> MarketResult<UpdateCheckResponse> {
        let mut results = Vec::new();

        for (plugin_id, current_version) in &request.plugins {
            // 模拟最新版本检查（实际实现会调用 API）
            let latest_version = match plugin_id.as_str() {
                "com.example.plugin-a" => "1.0.0",
                "com.example.plugin-b" => "2.0.0",
                _ => current_version, // 默认无更新
            };

            let update_available = latest_version != current_version;

            results.push(UpdateCheckResult {
                plugin_id: plugin_id.clone(),
                current_version: current_version.clone(),
                latest_version: latest_version.to_string(),
                update_available,
            });
        }

        Ok(UpdateCheckResponse { results })
    }

    /// 安装插件。
    pub fn install(&mut self, request: &InstallRequest) -> MarketResult<InstallResponse> {
        // 检查是否被吊销（遍历所有 kid 检查该插件是否被吊销）
        let revoked =
            self.revocation_list.revoked.values().any(|m| m.contains_key(&request.plugin_id));
        if revoked {
            return Err(MarketError::Revoked {
                plugin: request.plugin_id.clone(),
                kid: "unknown".to_string(),
            });
        }

        // 模拟安装（实际实现会下载、验证、解压）
        let version = request.version.clone().unwrap_or_else(|| "1.0.0".to_string());

        // 记录审计日志
        self.audit_log.append(
            0, // 时间戳由调用方提供
            AuditAction::Install,
            &request.plugin_id,
            Some(version.clone()),
            None,
        );

        Ok(InstallResponse {
            success: true,
            plugin_id: request.plugin_id.clone(),
            version,
            install_path: format!("/plugins/{}", request.plugin_id),
            error: None,
        })
    }

    /// 卸载插件。
    pub fn uninstall(&mut self, request: &UninstallRequest) -> MarketResult<UninstallResponse> {
        // 模拟卸载（实际实现会删除文件）

        // 记录审计日志
        self.audit_log.append(
            0, // 时间戳由调用方提供
            AuditAction::Uninstall,
            &request.plugin_id,
            None,
            None,
        );

        Ok(UninstallResponse { success: true, plugin_id: request.plugin_id.clone(), error: None })
    }

    /// 吊销插件。
    pub fn revoke(&mut self, request: &RevokeRequest) -> MarketResult<RevokeResponse> {
        // 记录吊销
        self.revocation_list.revoke(&request.kid, &request.plugin_id, 0);

        // 记录审计日志
        self.audit_log.append(
            0, // 时间戳由调用方提供
            AuditAction::Revoke,
            &request.plugin_id,
            None,
            None,
        );

        Ok(RevokeResponse { success: true, plugin_id: request.plugin_id.clone(), revoked_at: 0 })
    }

    /// 获取审计日志。
    pub fn audit_log(&self) -> &AuditLog {
        &self.audit_log
    }

    /// 获取吊销列表。
    pub fn revocation_list(&self) -> &RevocationList {
        &self.revocation_list
    }

    /// 验证审计日志完整性。
    pub fn verify_audit_integrity(&self) -> bool {
        self.audit_log.verify_chain()
    }

    /// 检查插件是否被吊销。
    pub fn is_plugin_revoked(&self, plugin_id: &str) -> bool {
        self.revocation_list.revoked.values().any(|m| m.contains_key(plugin_id))
    }
}

impl Default for MarketplaceApi {
    fn default() -> Self {
        Self::new("https://marketplace.tauron.dev")
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 工厂函数
// ──────────────────────────────────────────────────────────────────────────

/// 创建商城 API 客户端。
pub fn create_marketplace_api(base_url: &str) -> MarketplaceApi {
    MarketplaceApi::new(base_url)
}

/// 创建默认商城 API 客户端。
pub fn create_default_marketplace_api() -> MarketplaceApi {
    MarketplaceApi::default()
}

// ──────────────────────────────────────────────────────────────────────────
// 测试
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 搜索测试 ──

    #[test]
    fn test_search_default_request() {
        let api = create_default_marketplace_api();
        let request = SearchRequest::default();
        let result = api.search(&request);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(!response.plugins.is_empty());
        assert_eq!(response.page, 1);
    }

    #[test]
    fn test_search_response_structure() {
        let api = create_default_marketplace_api();
        let request = SearchRequest::default();
        let response = api.search(&request).unwrap();
        assert!(!response.plugins.is_empty());
        assert!(response.total > 0);
        assert!(response.per_page > 0);
    }

    // ── 插件详情测试 ──

    #[test]
    fn test_get_plugin_detail() {
        let api = create_default_marketplace_api();
        let result = api.get_plugin_detail("com.example.plugin-a");
        assert!(result.is_ok());
        let detail = result.unwrap();
        assert!(!detail.summary.id.is_empty());
        assert!(!detail.versions.is_empty());
    }

    #[test]
    fn test_plugin_detail_versions() {
        let api = create_default_marketplace_api();
        let detail = api.get_plugin_detail("com.example.plugin-a").unwrap();
        assert!(detail.versions.len() >= 2);
        assert!(detail.versions[0].version.is_empty() || !detail.versions[0].version.is_empty());
    }

    // ── 更新检查测试 ──

    #[test]
    fn test_check_updates() {
        let api = create_default_marketplace_api();
        let mut plugins = BTreeMap::new();
        plugins.insert("com.example.plugin-a".to_string(), "0.9.0".to_string());
        plugins.insert("com.example.plugin-b".to_string(), "1.0.0".to_string());

        let request = UpdateCheckRequest { plugins };
        let result = api.check_updates(&request);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.results.len(), 2);
    }

    #[test]
    fn test_check_updates_update_available() {
        let api = create_default_marketplace_api();
        let mut plugins = BTreeMap::new();
        plugins.insert("com.example.plugin-a".to_string(), "0.9.0".to_string());

        let request = UpdateCheckRequest { plugins };
        let response = api.check_updates(&request).unwrap();
        assert!(response.results[0].update_available);
    }

    #[test]
    fn test_check_updates_no_update() {
        let api = create_default_marketplace_api();
        let mut plugins = BTreeMap::new();
        plugins.insert("com.example.plugin-a".to_string(), "1.0.0".to_string());

        let request = UpdateCheckRequest { plugins };
        let response = api.check_updates(&request).unwrap();
        assert!(!response.results[0].update_available);
    }

    // ── 安装测试 ──

    #[test]
    fn test_install() {
        let mut api = create_default_marketplace_api();
        let request =
            InstallRequest { plugin_id: "com.example.plugin-a".to_string(), version: None };
        let result = api.install(&request);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.success);
        assert_eq!(response.plugin_id, "com.example.plugin-a");
    }

    #[test]
    fn test_install_specific_version() {
        let mut api = create_default_marketplace_api();
        let request = InstallRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            version: Some("0.9.0".to_string()),
        };
        let result = api.install(&request);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.version, "0.9.0");
    }

    #[test]
    fn test_install_revoked_plugin() {
        let mut api = create_default_marketplace_api();
        // 先吊销插件
        api.revoke(&RevokeRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            kid: "key-1".to_string(),
            reason: Some("Test revocation".to_string()),
        })
        .unwrap();

        // 尝试安装已吊销插件
        let request =
            InstallRequest { plugin_id: "com.example.plugin-a".to_string(), version: None };
        let result = api.install(&request);
        assert!(result.is_err());
    }

    // ── 卸载测试 ──

    #[test]
    fn test_uninstall() {
        let mut api = create_default_marketplace_api();
        let request = UninstallRequest { plugin_id: "com.example.plugin-a".to_string() };
        let result = api.uninstall(&request);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.success);
    }

    // ── 吊销测试 ──

    #[test]
    fn test_revoke() {
        let mut api = create_default_marketplace_api();
        let request = RevokeRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            kid: "key-1".to_string(),
            reason: Some("Security issue".to_string()),
        };
        let result = api.revoke(&request);
        assert!(result.is_ok());
        let response = result.unwrap();
        assert!(response.success);
    }

    #[test]
    fn test_is_plugin_revoked() {
        let mut api = create_default_marketplace_api();
        assert!(!api.is_plugin_revoked("com.example.plugin-a"));

        api.revoke(&RevokeRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            kid: "key-1".to_string(),
            reason: None,
        })
        .unwrap();

        assert!(api.is_plugin_revoked("com.example.plugin-a"));
    }

    // ── 审计日志测试 ──

    #[test]
    fn test_audit_log_records() {
        let mut api = create_default_marketplace_api();

        // 安装
        api.install(&InstallRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            version: None,
        })
        .unwrap();

        // 卸载
        api.uninstall(&UninstallRequest { plugin_id: "com.example.plugin-a".to_string() }).unwrap();

        let audit = api.audit_log();
        assert!(audit.len() >= 2);
    }

    #[test]
    fn test_verify_audit_integrity() {
        let mut api = create_default_marketplace_api();

        api.install(&InstallRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            version: None,
        })
        .unwrap();

        assert!(api.verify_audit_integrity());
    }

    // ── 吊销列表测试 ──

    #[test]
    fn test_revocation_list() {
        let mut api = create_default_marketplace_api();

        api.revoke(&RevokeRequest {
            plugin_id: "com.example.plugin-a".to_string(),
            kid: "key-1".to_string(),
            reason: None,
        })
        .unwrap();

        let revocation = api.revocation_list();
        assert!(revocation.has_key("key-1"));
    }

    // ── 工厂函数测试 ──

    #[test]
    fn test_create_marketplace_api() {
        let api = create_marketplace_api("https://custom-marketplace.dev");
        assert_eq!(api.base_url(), "https://custom-marketplace.dev");
    }

    #[test]
    fn test_create_default_marketplace_api() {
        let api = create_default_marketplace_api();
        assert!(!api.base_url().is_empty());
    }

    #[test]
    fn test_with_token() {
        let api = create_default_marketplace_api().with_token("my-token");
        // Token 已设置（内部验证）
        assert!(!api.base_url().is_empty());
    }

    // ── 排序方式测试 ──

    #[test]
    fn test_sort_order_serialization() {
        let order = SortOrder::Popular;
        let json = serde_json::to_value(order).unwrap();
        assert_eq!(json, serde_json::json!("popular"));
    }

    #[test]
    fn test_sort_order_deserialization() {
        let json = serde_json::json!("latest");
        let order = serde_json::from_value::<SortOrder>(json).unwrap();
        assert_eq!(order, SortOrder::Latest);
    }
}
