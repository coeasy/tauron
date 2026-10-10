// tauron-adapter · HTTP 命令族（T-7 逐字节纯搬移 · 十七片之一）。
//
// 本模块由 crate 根 lib.rs 的命令段整体纯搬移而来（旧 lib 6237–6371 行，一段连续 135 行），
// 未改动任何一行代码——函数体、doc、注释全部逐字节保真，仅把归属文件从 lib.rs 换到 http.rs。
// 内容：host_http_request 的逐跳授权链 cmd_http_request / cmd_http_request_as，连同只服务它们的
// 私有助手 validated_http_method（GET/POST 闭集）、http_policy_denied（NetworkPolicyError→HostError
// 统一映射）、authorize_http_url（首跳授权）。三个助手与调用者同模块可见，故无需任何 `pub(crate)`
// 放宽；私有不外泄、不再导出。
//
// 刻意留在 lib.rs 的是更广的基础设施：`HttpSink` trait 与 `UnavailableHttpSink` 实现、
// `HttpRequestSpec` / `HttpResponseSpec` 线形类型、`with_http_sink` 装配 builder、以及 `guard` /
// `require_main_window` / `unsupported_body` / `ProviderResult` / `HostResult` / `ErrorCode` 等
// crate 根项。本族经 `use super::*;` 原样可见上述类型与 `tauron_host::*`（NetworkPolicy /
// NetworkEnforcement / resolve_redirect_location 等），零可见性放宽、纯归属搬移。

use super::*;

/// HTTP method 的闭集校验（`GET` / `POST`，大小写不敏感）。
fn validated_http_method(method: &str) -> HostResult<String> {
    let m = method.trim().to_ascii_uppercase();
    match m.as_str() {
        "GET" | "POST" => Ok(m),
        other => Err(HostError::new(
            ErrorCode::E_INVALID_MANIFEST,
            format!("HTTP method `{other}` 非法：只接受 GET / POST"),
        )),
    }
}

/// `NetworkPolicy` 拒绝 → `HostError` 的统一映射（轮 48 / A96：URL 授权、逐跳
/// `authorize_redirect` 与 `authorize_resolution` 复检共用同一条判定）。
fn http_policy_denied(error: &tauron_host::NetworkPolicyError) -> HostError {
    let code = match error {
        tauron_host::NetworkPolicyError::EmptyUrl
        | tauron_host::NetworkPolicyError::InvalidUrl(_)
        | tauron_host::NetworkPolicyError::SchemeDenied(_)
        | tauron_host::NetworkPolicyError::HttpDenied
        | tauron_host::NetworkPolicyError::EmbeddedCredentialsDenied
        | tauron_host::NetworkPolicyError::HostMissing
        | tauron_host::NetworkPolicyError::PortMissing(_)
        | tauron_host::NetworkPolicyError::InvalidDomainRule(_)
        | tauron_host::NetworkPolicyError::InvalidRedirectLimit => ErrorCode::E_INVALID_MANIFEST,
        _ => ErrorCode::E_AUTH_DENIED,
    };
    HostError::new(code, format!("HTTP network policy denied request: {error}"))
}

fn authorize_http_url(
    policy: &tauron_host::NetworkPolicy,
    url: &str,
) -> HostResult<tauron_host::AuthorizedUrl> {
    policy.authorize_url(url).map_err(|error| http_policy_denied(&error))
}

/// `host_http_request`：发起一次 HTTP 请求（主窗专属）。
///
/// 命令层做**参数校验 + 逐跳授权流**；能力可用性由 sink 决定：缺省
/// [`UnavailableHttpSink`] 恒返回 `UnsupportedBody`（诚实降级，见其文档）。
///
/// **单跳契约（轮 48 / A96）**：redirect 跟随与解析复检由**宿主**逐跳执行——
/// sink 只发单跳；3xx 的每一跳先经 `authorize_redirect`（含跨源凭据剥离、
/// `max_redirects` 上界）与 `resolve_redirect_location`（相对 Location 归一），
/// 每个响应里 provider 回报的解析地址经 `authorize_resolution` 复检私网/字面 IP。
/// 由此 `authorize_redirect` / `authorize_resolution` 从"只有模块自测"变为
/// 这条生产命令路径上的真实闸门。
pub fn cmd_http_request(
    state: &SubstrateState,
    spec: &HttpRequestSpec,
) -> HostResult<ProviderResult<HttpResponseSpec>> {
    guard("http_request", || {
        validated_http_method(&spec.method)?;
        let mut hop_spec = spec.clone();
        let mut current = authorize_http_url(&state.http_policy, &hop_spec.url)?;
        let mut current_url = hop_spec.url.clone();
        if state.http_sink.native_supported()
            && state.http_sink.network_enforcement()
                != tauron_host::NetworkEnforcement::RedirectAndDns
        {
            return Ok(ProviderResult::Unsupported(unsupported_body(
                "HTTP provider does not enforce V4 redirect/DNS/private-network policy",
                Some("use a provider with NetworkEnforcement::RedirectAndDns"),
            )));
        }
        let mut hop: u8 = 0;
        loop {
            let response = match state.http_sink.request(&hop_spec, &state.http_policy)? {
                ProviderResult::Value(response) => response,
                other => return Ok(other),
            };
            if !response.resolved_addrs.is_empty() {
                let mut resolved = Vec::with_capacity(response.resolved_addrs.len());
                for raw in &response.resolved_addrs {
                    let address = raw.parse::<std::net::IpAddr>().map_err(|_| {
                        HostError::new(
                            ErrorCode::E_AUTH_DENIED,
                            format!(
                                "HTTP network policy denied request: provider reported an \
                                 unparseable resolved address `{raw}`"
                            ),
                        )
                    })?;
                    resolved.push(address);
                }
                state
                    .http_policy
                    .authorize_resolution(&current, &resolved)
                    .map_err(|error| http_policy_denied(&error))?;
            }
            if !(300..=399).contains(&response.status) {
                return Ok(ProviderResult::Value(response));
            }
            let Some(location) = response.headers.get("location") else {
                // 3xx 而无 `location`：没有可授权的下一跳，按最终响应交还调用方。
                return Ok(ProviderResult::Value(response));
            };
            hop = hop.saturating_add(1);
            let has_credentials = hop_spec.headers.keys().any(|key| {
                matches!(
                    key.to_ascii_lowercase().as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                )
            });
            let next_url = tauron_host::resolve_redirect_location(&current_url, location)
                .map_err(|error| http_policy_denied(&error))?;
            let authorization = state
                .http_policy
                .authorize_redirect(&current, &next_url, hop, has_credentials)
                .map_err(|error| http_policy_denied(&error))?;
            if !authorization.forward_credentials {
                hop_spec.headers.retain(|key, _| {
                    !matches!(
                        key.to_ascii_lowercase().as_str(),
                        "authorization" | "cookie" | "proxy-authorization"
                    )
                });
            }
            current = authorization.target;
            current_url = next_url;
            hop_spec.url = current_url.clone();
        }
    })?
}

/// `host_http_request` 的**带身份判定**版本（仅主窗）。
pub fn cmd_http_request_as(
    caller: &Caller,
    state: &SubstrateState,
    spec: &HttpRequestSpec,
) -> HostResult<ProviderResult<HttpResponseSpec>> {
    require_main_window(caller, "host_http_request")?;
    cmd_http_request(state, spec)
}
