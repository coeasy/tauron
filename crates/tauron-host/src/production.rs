//! Production deployment policy and startup readiness checks.
//!
//! V4: development/test compatibility defaults must never silently become production defaults.

use serde::{Deserialize, Serialize};

/// Host deployment intent. Production enables fail-closed readiness validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentMode {
    /// Developer workstation / local integration. Compatibility fallbacks may be enabled.
    #[default]
    Development,
    /// Deterministic tests/fault injection. Test doubles are allowed.
    Test,
    /// Production deployment. Security-critical prerequisites are mandatory.
    Production,
}

/// Inputs that a platform adapter must prove before a production Host can become READY.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProductionReadiness {
    pub caller_identity_policy_enabled: bool,
    pub durable_recovery_available: bool,
    pub recovery_explicitly_unsupported: bool,
    pub install_feature_enabled: bool,
    pub install_trust_configured: bool,
    pub audit_for_admin_operations_available: bool,
    pub writable_data_dir_available: bool,
    pub mock_provider_enabled: bool,
}

/// One production-readiness violation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessViolation {
    pub code: &'static str,
    pub message: &'static str,
}

/// Validate production-only invariants.
///
/// Development and Test deliberately return no violations: callers can still expose
/// `production_safe = false` in diagnostics without breaking local workflows.
pub fn validate(mode: DeploymentMode, input: &ProductionReadiness) -> Vec<ReadinessViolation> {
    if mode != DeploymentMode::Production {
        return Vec::new();
    }

    let mut out = Vec::new();
    if !input.caller_identity_policy_enabled {
        out.push(ReadinessViolation {
            code: "CALLER_IDENTITY_POLICY_REQUIRED",
            message: "production requires a fail-closed caller identity/origin policy",
        });
    }
    if !input.durable_recovery_available && !input.recovery_explicitly_unsupported {
        out.push(ReadinessViolation {
            code: "RECOVERY_DURABILITY_REQUIRED",
            message: "production requires durable recovery or an explicit unsupported declaration",
        });
    }
    if input.install_feature_enabled && !input.install_trust_configured {
        out.push(ReadinessViolation {
            code: "INSTALL_TRUST_REQUIRED",
            message: "plugin install is enabled but trust material is incomplete",
        });
    }
    if !input.audit_for_admin_operations_available {
        out.push(ReadinessViolation {
            code: "ADMIN_AUDIT_REQUIRED",
            message: "production requires an audit sink for privileged administration",
        });
    }
    if !input.writable_data_dir_available {
        out.push(ReadinessViolation {
            code: "DATA_DIR_REQUIRED",
            message: "production requires a writable durable data directory",
        });
    }
    if input.mock_provider_enabled {
        out.push(ReadinessViolation {
            code: "MOCK_PROVIDER_FORBIDDEN",
            message: "mock providers cannot be enabled in production",
        });
    }
    out
}

/// Whether the host may advertise itself as production-safe.
pub fn is_production_safe(mode: DeploymentMode, input: &ProductionReadiness) -> bool {
    mode == DeploymentMode::Production && validate(mode, input).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> ProductionReadiness {
        ProductionReadiness {
            caller_identity_policy_enabled: true,
            durable_recovery_available: true,
            recovery_explicitly_unsupported: false,
            install_feature_enabled: true,
            install_trust_configured: true,
            audit_for_admin_operations_available: true,
            writable_data_dir_available: true,
            mock_provider_enabled: false,
        }
    }

    #[test]
    fn development_keeps_compatibility_without_claiming_production_safety() {
        let input = ProductionReadiness::default();
        assert!(validate(DeploymentMode::Development, &input).is_empty());
        assert!(!is_production_safe(DeploymentMode::Development, &input));
    }

    #[test]
    fn production_is_fail_closed() {
        let violations = validate(DeploymentMode::Production, &ProductionReadiness::default());
        let codes: Vec<_> = violations.iter().map(|v| v.code).collect();
        assert!(codes.contains(&"CALLER_IDENTITY_POLICY_REQUIRED"));
        assert!(codes.contains(&"RECOVERY_DURABILITY_REQUIRED"));
        assert!(codes.contains(&"ADMIN_AUDIT_REQUIRED"));
        assert!(codes.contains(&"DATA_DIR_REQUIRED"));
    }

    #[test]
    fn production_allows_explicit_recovery_unsupported() {
        let mut input = ready();
        input.durable_recovery_available = false;
        input.recovery_explicitly_unsupported = true;
        assert!(validate(DeploymentMode::Production, &input).is_empty());
    }

    #[test]
    fn production_ready_requires_every_security_fact() {
        let input = ready();
        assert!(is_production_safe(DeploymentMode::Production, &input));
    }
}
