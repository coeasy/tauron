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
    /// Concrete resource/scope the decision applies to. "*" is the legacy unscoped form.
    pub scope: String,
    pub policy_epoch: u64,
    pub grant_version: u64,
    pub nonce: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DecisionError {
    #[error("decision token principal/operation/scope mismatch")]
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
    /// 全局单调发号器（轮 17）：`grants` 的值取自它，不再按主体自增。
    ///
    /// 为什么不能按主体自增：`forget` 删行后 `grant_version` 回落 0，而
    /// `validate_scoped` **不看 nonce**——于是「dispose 前用 v1 签的 token」会在
    /// 「dispose + 重新 approve（又回到 v1）」那一刻重新有效。今天走不到那条路径
    /// （token 在同一次 `subscribe` 里铸完就用），但它把「回收 = fail-closed」
    /// 变成了一句要看调用点才成立的话。全局发号让版本号永不复用，性质由结构保证。
    grant_sequence: u64,
}

impl PolicyAuthority {
    pub fn new() -> Self {
        Self { policy_epoch: 1, grants: HashMap::new(), grant_sequence: 0 }
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
        self.grant_sequence = self.grant_sequence.saturating_add(1);
        let next = self.grant_sequence;
        self.grants.insert(principal.to_string(), next);
        next
    }

    /// 卸载/清除级回收（轮 16 R2）：删掉该主体的版本行。
    ///
    /// `grants` 此前只有插入点（approve/revoke 各一）而没有任何删除点——
    /// dispose 清了 approvals/queues/subs/ordering，却给它留了一张按主体
    /// 无界增长的表。删除后 `grant_version` 回落到 0，该主体此前签发的
    /// DecisionToken 会因版本不等被判 `StaleGrant`——**fail-closed**，
    /// 不是「没版本 = 没人管」。
    ///
    /// 返回「是否真删了一行」。**没有**配套的 `grant_count()` 探针：轮 17 复查
    /// 指出那样的公开 API 只有测试在调，属于「声明了没人读」的表面积（本仓曾专门
    /// 清过一类）。判据用 `grant_version` 就够——版本取自下面的全局发号器、
    /// 最小值是 1，所以「版本 0」与「行不存在」是同一件事。
    pub fn forget(&mut self, principal: &str) -> bool {
        self.grants.remove(principal).is_some()
    }

    pub fn decide(&self, principal: &str, operation: &str) -> DecisionToken {
        self.decide_scoped(principal, operation, "*")
    }

    pub fn decide_scoped(&self, principal: &str, operation: &str, scope: &str) -> DecisionToken {
        DecisionToken {
            principal: principal.to_string(),
            operation: operation.to_string(),
            scope: scope.to_string(),
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
        self.validate_scoped(token, principal, operation, &token.scope)
    }

    pub fn validate_scoped(
        &self,
        token: &DecisionToken,
        principal: &str,
        operation: &str,
        scope: &str,
    ) -> Result<(), DecisionError> {
        if token.principal != principal || token.operation != operation || token.scope != scope {
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
            return Err(DecisionError::StaleGrant { token: token.grant_version, current });
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

    /// 轮 17：`forget` 之后重新授权，**不得**让 dispose 前的 token 复活。
    ///
    /// 旧实现按主体自增，删行即回落到 0，下一次 approve 又回到 1——而
    /// `validate_scoped` 不比对 nonce，于是版本号相同的旧 token 重新校验通过。
    /// 现在版本取自全局单调发号器，这条性质由结构保证，不靠调用点自觉。
    #[test]
    fn grant_version_is_never_reused_after_forget() {
        let mut a = PolicyAuthority::new();
        let first = a.bump_grant("p1");
        let token = a.decide("p1", "fs.read");
        a.validate(&token, "p1", "fs.read").unwrap();

        assert!(a.forget("p1"), "回收必须真的删行");
        assert!(matches!(
            a.validate(&token, "p1", "fs.read"),
            Err(DecisionError::StaleGrant { .. })
        ));

        let second = a.bump_grant("p1");
        assert_ne!(first, second, "版本号不得跨回收复用");
        assert_eq!(a.grant_version("p1"), second, "重新授权必须把新行写回");
        assert!(
            matches!(a.validate(&token, "p1", "fs.read"), Err(DecisionError::StaleGrant { .. })),
            "重新授权不得让 dispose 前的 token 复活"
        );
        let fresh = a.decide("p1", "fs.read");
        a.validate(&fresh, "p1", "fs.read").unwrap();
    }

    #[test]
    fn scoped_token_cannot_be_replayed_for_other_resource() {
        let a = PolicyAuthority::new();
        let token = a.decide_scoped("p1", "events.subscribe.private", "topic:a");
        assert_eq!(
            a.validate_scoped(&token, "p1", "events.subscribe.private", "topic:b"),
            Err(DecisionError::ScopeMismatch)
        );
        a.validate_scoped(&token, "p1", "events.subscribe.private", "topic:a").unwrap();
    }

    #[test]
    fn token_cannot_be_replayed_for_other_operation() {
        let a = PolicyAuthority::new();
        let token = a.decide("p1", "http.request");
        assert_eq!(a.validate(&token, "p1", "process.spawn"), Err(DecisionError::ScopeMismatch));
    }
}
