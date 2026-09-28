//! V4 policy epoch, grant version and decision-token contract (A81).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionToken {
    pub principal: String,
    pub operation: String,
    pub policy_epoch: u64,
    pub grant_version: u64,
    pub nonce: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecisionError {
    #[error("decision token principal/operation mismatch")]
    ScopeMismatch,
    #[error("decision token policy epoch is stale: token={token}, current={current}")]
    StalePolicy { token: u64, current: u64 },
    #[error("decision token grant version is stale: token={token}, current={current}")]
    StaleGrant { token: u64, current: u64 },
}

#[derive(Debug, Default)]
pub struct PolicyAuthority {
    policy_epoch: u64,
    grants: HashMap<String, u64>,
}

impl PolicyAuthority {
    pub fn new() -> Self {
        Self {
            policy_epoch: 1,
            grants: HashMap::new(),
        }
    }

    pub fn policy_epoch(&self) -> u64 {
        self.policy_epoch
    }

    pub fn grant_version(&self, principal: &str) -> u64 {
        self.grants.get(principal).copied().unwrap_or(0)
    }

    pub fn bump_policy(&mut self) -> u64 {
        self.policy_epoch = self.policy_epoch.saturating_add(1);
        self.policy_epoch
    }

    /// Grant/revoke both bump the version. Consumers never reuse a decision across either.
    pub fn bump_grant(&mut self, principal: &str) -> u64 {
        let next = self.grant_version(principal).saturating_add(1);
        self.grants.insert(principal.to_string(), next);
        next
    }

    pub fn decide(&self, principal: &str, operation: &str) -> DecisionToken {
        DecisionToken {
            principal: principal.to_string(),
            operation: operation.to_string(),
            policy_epoch: self.policy_epoch,
            grant_version: self.grant_version(principal),
            nonce: Uuid::new_v4().to_string(),
        }
    }

    pub fn validate(
        &self,
        token: &DecisionToken,
        principal: &str,
        operation: &str,
    ) -> Result<(), DecisionError> {
        if token.principal != principal || token.operation != operation {
            return Err(DecisionError::ScopeMismatch);
        }
        if token.policy_epoch != self.policy_epoch {
            return Err(DecisionError::StalePolicy {
                token: token.policy_epoch,
                current: self.policy_epoch,
            });
        }
        let current = self.grant_version(principal);
        if token.grant_version != current {
            return Err(DecisionError::StaleGrant {
                token: token.grant_version,
                current,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoke_or_policy_change_invalidates_old_decision() {
        let mut a = PolicyAuthority::new();
        a.bump_grant("p1");
        let grant_token = a.decide("p1", "fs.read");
        a.validate(&grant_token, "p1", "fs.read").unwrap();

        a.bump_grant("p1");
        assert!(matches!(
            a.validate(&grant_token, "p1", "fs.read"),
            Err(DecisionError::StaleGrant { .. })
        ));

        let policy_token = a.decide("p1", "fs.read");
        a.bump_policy();
        assert!(matches!(
            a.validate(&policy_token, "p1", "fs.read"),
            Err(DecisionError::StalePolicy { .. })
        ));
    }

    #[test]
    fn token_cannot_be_replayed_for_other_operation() {
        let a = PolicyAuthority::new();
        let token = a.decide("p1", "http.request");
        assert_eq!(
            a.validate(&token, "p1", "process.spawn"),
            Err(DecisionError::ScopeMismatch)
        );
    }
}
