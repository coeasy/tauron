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
    /// Whether the runtime origin gate is **armed**: a non-empty allowlist is actually
    /// enforced on every host command. `caller_identity_policy_enabled` may be satisfied
    /// by a transport-level declaration, which does not arm the gate — hence a separate fact.
    pub origin_gate_armed: bool,
    pub durable_recovery_available: bool,
    pub recovery_explicitly_unsupported: bool,
    pub install_feature_enabled: bool,
    pub install_trust_configured: bool,
    /// A100: trusted time is required for supply-chain expiry decisions when install is enabled.
    pub trusted_time_available: bool,
    /// Privileged administration has a working audit sink.
    ///
    /// Batch 0-3 (F3): adapters must derive this from the live
    /// [`crate::admin_audit::AdminAuditFacts::healthy`] snapshot, not from a host-set flag.
    pub audit_for_admin_operations_available: bool,
    pub writable_data_dir_available: bool,
    /// Whether this host instance actually installs the process-plugin runtime.
    pub process_runtime_enabled: bool,
    /// True only when the exact process runtime uses an A97 hard sandbox provider.
    pub hard_process_sandbox_available: bool,
    pub mock_provider_enabled: bool,
}

/// One production-readiness violation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessViolation {
    pub code: &'static str,
    pub message: &'static str,
}

/// Machine-readable production self-test result (V4 A109).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductionDoctorReport {
    pub deployment_mode: DeploymentMode,
    pub production_safe: bool,
    pub checks: Vec<ProductionDoctorCheck>,
    /// Live audit facts of the privileged-operation audit sink, or `None` when the host
    /// configured no sink at all.
    ///
    /// Batch 0-3 (F3): the `admin-audit` check used to read a host-set boolean. It is now
    /// derived from this sink, so a report can no longer claim an audit trail that records
    /// nothing. `doctor()` itself stays pure (mode + readiness only); the platform adapter
    /// attaches the real snapshot before the report leaves the host.
    pub admin_audit: Option<crate::admin_audit::AdminAuditFacts>,
}

/// One production self-test check. A failed check is release-blocking in Production.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProductionDoctorCheck {
    pub id: &'static str,
    pub pass: bool,
    pub required_in_production: bool,
    pub message: &'static str,
}

/// Run the production security/readiness self-test without mutating host state.
///
/// Development/Test can expose warnings while remaining runnable; Production must have every
/// required check pass before the Host advertises READY.
pub fn doctor(mode: DeploymentMode, input: &ProductionReadiness) -> ProductionDoctorReport {
    let checks = vec![
        ProductionDoctorCheck {
            id: "caller-identity",
            pass: input.caller_identity_policy_enabled,
            required_in_production: true,
            message: "caller identity/origin policy is fail-closed",
        },
        ProductionDoctorCheck {
            id: "origin-gate",
            pass: input.origin_gate_armed,
            required_in_production: true,
            message: "the runtime origin gate enforces a non-empty allowlist",
        },
        ProductionDoctorCheck {
            id: "recovery-durability",
            pass: input.durable_recovery_available || input.recovery_explicitly_unsupported,
            required_in_production: true,
            message: "durable recovery is configured or explicitly unsupported",
        },
        ProductionDoctorCheck {
            id: "install-trust",
            pass: !input.install_feature_enabled || input.install_trust_configured,
            required_in_production: true,
            message: "plugin installation trust material is complete when install is enabled",
        },
        ProductionDoctorCheck {
            id: "trusted-time",
            pass: !input.install_feature_enabled || input.trusted_time_available,
            required_in_production: true,
            message: "plugin installation has a currently trusted time source for expiry decisions",
        },
        ProductionDoctorCheck {
            id: "admin-audit",
            pass: input.audit_for_admin_operations_available,
            required_in_production: true,
            message: "privileged administration records into a durable audit sink",
        },
        ProductionDoctorCheck {
            id: "durable-data-dir",
            pass: input.writable_data_dir_available,
            required_in_production: true,
            message: "durable writable data directory is available",
        },
        ProductionDoctorCheck {
            id: "process-sandbox",
            pass: !input.process_runtime_enabled || input.hard_process_sandbox_available,
            required_in_production: true,
            message: "process runtime uses a hard OS sandbox when process plugins are enabled",
        },
        ProductionDoctorCheck {
            id: "no-mock-provider",
            pass: !input.mock_provider_enabled,
            required_in_production: true,
            message: "mock/test providers are absent",
        },
    ];
    let production_safe =
        mode == DeploymentMode::Production && checks.iter().all(|check| check.pass);
    ProductionDoctorReport { deployment_mode: mode, production_safe, checks, admin_audit: None }
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
    if !input.origin_gate_armed {
        out.push(ReadinessViolation {
            code: "ORIGIN_GATE_ARMED_REQUIRED",
            message: "production requires an armed origin gate (empty allowlist = gate disabled)",
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
    if input.install_feature_enabled && !input.trusted_time_available {
        out.push(ReadinessViolation {
            code: "TRUSTED_TIME_REQUIRED",
            message:
                "plugin install is enabled but no currently trusted time provider is available",
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
    if input.process_runtime_enabled && !input.hard_process_sandbox_available {
        out.push(ReadinessViolation {
            code: "PROCESS_SANDBOX_HARD_REQUIRED",
            message: "production process runtime requires a hard ProcessSandboxProvider",
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
            origin_gate_armed: true,
            durable_recovery_available: true,
            recovery_explicitly_unsupported: false,
            install_feature_enabled: true,
            install_trust_configured: true,
            trusted_time_available: true,
            audit_for_admin_operations_available: true,
            writable_data_dir_available: true,
            process_runtime_enabled: true,
            hard_process_sandbox_available: true,
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

    /// V4 轮 10（F2）：一条**已声明**的身份策略不得替 runtime origin 门装弹。
    ///
    /// 旧行为是 `caller_identity_policy_enabled` 单真即通过，于是「配了 flag、
    /// 允许清单为空」的宿主能在 origin 门实际放行一切的情况下自称 production-safe。
    #[test]
    fn declared_identity_policy_does_not_arm_the_origin_gate() {
        let mut input = ready();
        input.origin_gate_armed = false;
        let violations = validate(DeploymentMode::Production, &input);
        assert!(
            violations.iter().any(|v| v.code == "ORIGIN_GATE_ARMED_REQUIRED"),
            "empty allowlist must not read as an enforced origin policy"
        );
        assert!(!is_production_safe(DeploymentMode::Production, &input));
        let report = doctor(DeploymentMode::Production, &input);
        assert!(!report.production_safe);
        assert!(report.checks.iter().any(|c| c.id == "origin-gate" && !c.pass));
    }

    #[test]
    fn production_install_requires_trusted_time_only_when_install_is_enabled() {
        let mut input = ready();
        input.trusted_time_available = false;
        let violations = validate(DeploymentMode::Production, &input);
        assert!(
            violations.iter().any(|v| v.code == "TRUSTED_TIME_REQUIRED"),
            "production install must fail closed without trusted time"
        );

        input.install_feature_enabled = false;
        assert!(
            validate(DeploymentMode::Production, &input)
                .iter()
                .all(|v| v.code != "TRUSTED_TIME_REQUIRED"),
            "hosts without installation capability must not require a time source they never use"
        );
    }

    #[test]
    fn production_requires_hard_process_sandbox_only_when_process_runtime_is_enabled() {
        let mut input = ready();
        input.hard_process_sandbox_available = false;
        let violations = validate(DeploymentMode::Production, &input);
        assert!(violations.iter().any(|v| v.code == "PROCESS_SANDBOX_HARD_REQUIRED"));

        input.process_runtime_enabled = false;
        assert!(
            validate(DeploymentMode::Production, &input)
                .iter()
                .all(|v| v.code != "PROCESS_SANDBOX_HARD_REQUIRED"),
            "headless/substrate-only production must not require a process sandbox it does not use"
        );
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
        let report = doctor(DeploymentMode::Production, &input);
        assert!(report.production_safe);
        assert!(report.checks.iter().all(|check| check.pass));
    }

    #[test]
    fn doctor_reports_failed_production_facts_without_mutating_policy() {
        let report = doctor(DeploymentMode::Production, &ProductionReadiness::default());
        assert!(!report.production_safe);
        assert!(report.checks.iter().any(|check| check.id == "caller-identity" && !check.pass));
        assert!(report.checks.iter().any(|check| check.id == "admin-audit" && !check.pass));
    }

    #[test]
    fn development_doctor_never_claims_production_safe() {
        let report = doctor(DeploymentMode::Development, &ready());
        assert!(!report.production_safe);
    }
}
