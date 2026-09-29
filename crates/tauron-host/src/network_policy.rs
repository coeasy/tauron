//! V4 HTTP/network scope policy (A96).
//!
//! Policy is transport-neutral and must be applied before the first request, on every redirect,
//! and after DNS resolution. A concrete HTTP provider is not considered fully enforced unless it
//! can apply all of these checks before opening the corresponding connection.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrivateNetworkPolicy {
    Deny,
    Allow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkEnforcement {
    /// Only the initial parsed URL is checked. This is not sufficient for production HTTP.
    UrlOnly,
    /// Initial URL, every redirect and every resolved address are checked.
    RedirectAndDns,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DomainRule {
    /// Exact host (example.com) or subdomain wildcard (*.example.com).
    pub pattern: String,
    /// Empty means the scheme default port only (80 for http, 443 for https).
    #[serde(default)]
    pub ports: Vec<u16>,
}

impl DomainRule {
    pub fn exact(host: impl Into<String>) -> Self {
        Self { pattern: host.into(), ports: Vec::new() }
    }

    pub fn wildcard(suffix: impl Into<String>) -> Self {
        Self { pattern: format!("*.{}", suffix.into()), ports: Vec::new() }
    }

    pub fn with_ports(mut self, ports: impl IntoIterator<Item = u16>) -> Self {
        self.ports = ports.into_iter().collect();
        self
    }

    fn validate(&self) -> Result<(), NetworkPolicyError> {
        let pattern = self.pattern.trim().to_ascii_lowercase();
        if pattern.is_empty()
            || pattern.contains('/')
            || pattern.contains(':')
            || pattern.contains('@')
            || pattern.ends_with('.')
        {
            return Err(NetworkPolicyError::InvalidDomainRule(self.pattern.clone()));
        }
        if pattern.contains('*') && (!pattern.starts_with("*.") || pattern[2..].contains('*')) {
            return Err(NetworkPolicyError::InvalidDomainRule(self.pattern.clone()));
        }
        if pattern == "*." || pattern.contains("..") {
            return Err(NetworkPolicyError::InvalidDomainRule(self.pattern.clone()));
        }
        if self.ports.iter().any(|port| *port == 0) {
            return Err(NetworkPolicyError::InvalidDomainRule(self.pattern.clone()));
        }
        Ok(())
    }

    fn matches_host(&self, host: &str) -> bool {
        let pattern = self.pattern.trim().to_ascii_lowercase();
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if let Some(suffix) = pattern.strip_prefix("*.") {
            host != suffix && host.ends_with(&format!(".{suffix}"))
        } else {
            host == pattern
        }
    }

    fn allows_port(&self, scheme: &str, port: u16) -> bool {
        if self.ports.is_empty() {
            port == default_port(scheme)
        } else {
            self.ports.contains(&port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkPolicy {
    #[serde(default)]
    pub domains: Vec<DomainRule>,
    #[serde(default)]
    pub allow_http: bool,
    #[serde(default = "default_private_policy")]
    pub private_network: PrivateNetworkPolicy,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u8,
}

fn default_private_policy() -> PrivateNetworkPolicy {
    PrivateNetworkPolicy::Deny
}

const fn default_max_redirects() -> u8 {
    8
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self::deny_all()
    }
}

impl NetworkPolicy {
    pub const fn deny_all() -> Self {
        Self {
            domains: Vec::new(),
            allow_http: false,
            private_network: PrivateNetworkPolicy::Deny,
            max_redirects: 8,
        }
    }

    pub fn public_https(domains: Vec<DomainRule>) -> Self {
        Self {
            domains,
            allow_http: false,
            private_network: PrivateNetworkPolicy::Deny,
            max_redirects: 8,
        }
    }

    pub fn validate(&self) -> Result<(), NetworkPolicyError> {
        if self.max_redirects == 0 {
            return Err(NetworkPolicyError::InvalidRedirectLimit);
        }
        for rule in &self.domains {
            rule.validate()?;
        }
        Ok(())
    }

    pub fn authorize_url(&self, raw: &str) -> Result<AuthorizedUrl, NetworkPolicyError> {
        self.validate()?;
        if raw.trim().is_empty() {
            return Err(NetworkPolicyError::EmptyUrl);
        }
        let url = Url::parse(raw).map_err(|error| NetworkPolicyError::InvalidUrl(error.to_string()))?;
        let scheme = url.scheme().to_ascii_lowercase();
        match scheme.as_str() {
            "https" => {}
            "http" if self.allow_http => {}
            "http" => return Err(NetworkPolicyError::HttpDenied),
            other => return Err(NetworkPolicyError::SchemeDenied(other.to_string())),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(NetworkPolicyError::EmbeddedCredentialsDenied);
        }
        let host = url
            .host_str()
            .ok_or(NetworkPolicyError::HostMissing)?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| NetworkPolicyError::PortMissing(scheme.clone()))?;

        let mut host_matched = false;
        let mut allowed = false;
        for rule in &self.domains {
            if rule.matches_host(&host) {
                host_matched = true;
                if rule.allows_port(&scheme, port) {
                    allowed = true;
                    break;
                }
            }
        }
        if !host_matched {
            return Err(NetworkPolicyError::DomainDenied(host));
        }
        if !allowed {
            return Err(NetworkPolicyError::PortDenied { host, port });
        }

        if self.private_network == PrivateNetworkPolicy::Deny {
            if let Ok(ip) = host.parse::<IpAddr>() {
                if !is_public_ip(ip) {
                    return Err(NetworkPolicyError::PrivateAddressDenied(ip));
                }
            }
        }

        Ok(AuthorizedUrl {
            scheme: scheme.clone(),
            host: host.clone(),
            port,
            origin: format!("{scheme}://{host}:{port}"),
        })
    }

    pub fn authorize_resolution(
        &self,
        target: &AuthorizedUrl,
        resolved: &[IpAddr],
    ) -> Result<(), NetworkPolicyError> {
        if resolved.is_empty() {
            return Err(NetworkPolicyError::EmptyResolution(target.host.clone()));
        }
        if self.private_network == PrivateNetworkPolicy::Deny {
            for ip in resolved {
                if !is_public_ip(*ip) {
                    return Err(NetworkPolicyError::PrivateAddressDenied(*ip));
                }
            }
        }
        Ok(())
    }

    pub fn authorize_redirect(
        &self,
        from: &AuthorizedUrl,
        to: &str,
        hop: u8,
        request_has_credentials: bool,
    ) -> Result<RedirectAuthorization, NetworkPolicyError> {
        if hop == 0 || hop > self.max_redirects {
            return Err(NetworkPolicyError::RedirectLimitExceeded {
                hop,
                max: self.max_redirects,
            });
        }
        let target = self.authorize_url(to)?;
        let same_origin = from.origin == target.origin;
        Ok(RedirectAuthorization {
            target,
            forward_credentials: request_has_credentials && same_origin,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedUrl {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectAuthorization {
    pub target: AuthorizedUrl,
    /// Credentials may be forwarded only on same-origin redirects.
    pub forward_credentials: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum NetworkPolicyError {
    #[error("network URL is empty")]
    EmptyUrl,
    #[error("invalid network URL: {0}")]
    InvalidUrl(String),
    #[error("network scheme is denied: {0}")]
    SchemeDenied(String),
    #[error("plain HTTP is denied by policy")]
    HttpDenied,
    #[error("embedded URL credentials are denied")]
    EmbeddedCredentialsDenied,
    #[error("network URL has no host")]
    HostMissing,
    #[error("network URL has no known/default port for scheme {0}")]
    PortMissing(String),
    #[error("network domain is outside scope: {0}")]
    DomainDenied(String),
    #[error("network port is outside scope: {host}:{port}")]
    PortDenied { host: String, port: u16 },
    #[error("invalid network domain rule: {0}")]
    InvalidDomainRule(String),
    #[error("redirect limit must be greater than zero")]
    InvalidRedirectLimit,
    #[error("redirect hop {hop} exceeds policy maximum {max}")]
    RedirectLimitExceeded { hop: u8, max: u8 },
    #[error("DNS resolution returned no addresses for {0}")]
    EmptyResolution(String),
    #[error("private/non-routable address denied by policy: {0}")]
    PrivateAddressDenied(IpAddr),
}

fn default_port(scheme: &str) -> u16 {
    match scheme {
        "http" => 80,
        "https" => 443,
        _ => 0,
    }
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    !matches!(
        (a, b, c, d),
        (0, _, _, _)
            | (10, _, _, _)
            | (100, 64..=127, _, _)
            | (127, _, _, _)
            | (169, 254, _, _)
            | (172, 16..=31, _, _)
            | (192, 0, 0, _)
            | (192, 0, 2, _)
            | (192, 88, 99, _)
            | (192, 168, _, _)
            | (198, 18..=19, _, _)
            | (198, 51, 100, _)
            | (203, 0, 113, _)
            | (224..=255, _, _, _)
    )
}

fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_ipv4(mapped);
    }
    let segments = ip.segments();
    let first = segments[0];
    if ip.is_unspecified() || ip.is_loopback() || (first & 0xff00) == 0xff00 {
        return false;
    }
    if (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 {
        return false;
    }
    if first == 0x2001 && segments[1] == 0x0db8 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> NetworkPolicy {
        NetworkPolicy::public_https(vec![
            DomainRule::exact("api.example.com"),
            DomainRule::wildcard("svc.example.com").with_ports([443, 8443]),
        ])
    }

    #[test]
    fn parses_before_authorizing_and_denies_dangerous_schemes_or_userinfo() {
        let p = policy();
        assert!(matches!(
            p.authorize_url("file:///etc/passwd"),
            Err(NetworkPolicyError::SchemeDenied(_))
        ));
        assert_eq!(
            p.authorize_url("https://user:pass@api.example.com/x"),
            Err(NetworkPolicyError::EmbeddedCredentialsDenied)
        );
    }

    #[test]
    fn exact_and_wildcard_grammar_is_fixed_and_port_scoped() {
        let p = policy();
        p.authorize_url("https://api.example.com/v1").unwrap();
        p.authorize_url("https://a.svc.example.com:8443/v1").unwrap();
        assert!(matches!(
            p.authorize_url("https://svc.example.com/v1"),
            Err(NetworkPolicyError::DomainDenied(_))
        ));
        assert!(matches!(
            p.authorize_url("https://a.svc.example.com:9443/v1"),
            Err(NetworkPolicyError::PortDenied { .. })
        ));
    }

    #[test]
    fn redirect_is_reauthorized_and_credentials_do_not_cross_origin() {
        let p = NetworkPolicy::public_https(vec![
            DomainRule::exact("api.example.com"),
            DomainRule::exact("cdn.example.com"),
        ]);
        let from = p.authorize_url("https://api.example.com/a").unwrap();
        let same = p
            .authorize_redirect(&from, "https://api.example.com/b", 1, true)
            .unwrap();
        assert!(same.forward_credentials);
        let cross = p
            .authorize_redirect(&from, "https://cdn.example.com/b", 2, true)
            .unwrap();
        assert!(!cross.forward_credentials);
        assert!(matches!(
            p.authorize_redirect(&from, "https://evil.example.net/", 3, false),
            Err(NetworkPolicyError::DomainDenied(_))
        ));
    }

    #[test]
    fn dns_and_literal_private_targets_are_denied() {
        let p = NetworkPolicy::public_https(vec![
            DomainRule::exact("api.example.com"),
            DomainRule::exact("127.0.0.1"),
        ]);
        assert!(matches!(
            p.authorize_url("https://127.0.0.1/"),
            Err(NetworkPolicyError::PrivateAddressDenied(_))
        ));
        let target = p.authorize_url("https://api.example.com/").unwrap();
        assert!(matches!(
            p.authorize_resolution(&target, &["10.0.0.2".parse().unwrap()]),
            Err(NetworkPolicyError::PrivateAddressDenied(_))
        ));
        p.authorize_resolution(&target, &["8.8.8.8".parse().unwrap()]).unwrap();
    }

    #[test]
    fn policy_is_fail_closed_by_default() {
        assert!(matches!(
            NetworkPolicy::default().authorize_url("https://example.com/"),
            Err(NetworkPolicyError::DomainDenied(_))
        ));
    }
}
