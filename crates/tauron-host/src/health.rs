//! V4 liveness/readiness/degradation split (A103).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Liveness {
    Alive,
    Dead,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    Ready,
    NotReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Degradation {
    Full,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthReport {
    pub liveness: Liveness,
    pub readiness: Readiness,
    pub degradation: Degradation,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

impl HealthReport {
    pub fn ready() -> Self {
        Self {
            liveness: Liveness::Alive,
            readiness: Readiness::Ready,
            degradation: Degradation::Full,
            diagnostics: Vec::new(),
        }
    }

    pub fn can_accept_work(&self) -> bool {
        self.liveness == Liveness::Alive && self.readiness == Readiness::Ready
    }

    pub fn degraded(reason: impl Into<String>) -> Self {
        Self {
            liveness: Liveness::Alive,
            readiness: Readiness::Ready,
            degradation: Degradation::Degraded,
            diagnostics: vec![reason.into()],
        }
    }

    pub fn alive_but_not_ready(reason: impl Into<String>) -> Self {
        Self {
            liveness: Liveness::Alive,
            readiness: Readiness::NotReady,
            degradation: Degradation::Degraded,
            diagnostics: vec![reason.into()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alive_is_not_equivalent_to_ready() {
        let report = HealthReport::alive_but_not_ready("credential-expired");
        assert_eq!(report.liveness, Liveness::Alive);
        assert_eq!(report.readiness, Readiness::NotReady);
        assert!(!report.can_accept_work());
    }
}
