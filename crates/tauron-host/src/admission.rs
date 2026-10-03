//! V4 credit-based backpressure and hierarchical admission control (A79/A80).
//!
//! 本模块有**两个活的**原语，各自都有生产消费点：
//! - [`AdmissionController`]：分层（global + per-principal）资源预算，由
//!   `Registry::call_begin` 准入、全终态路径释放（A80 的双层限额那一半）。
//! - [`CreditWindow`]：接收方驱动的字节额度，是 `StreamRegistry` **唯一**的额度
//!   算术来源（A79）；流侧不再存裸计数器，也不再有第二套 `saturating_add/min`
//!   可以与之漂移。
//!
//! **不在本模块的东西**：按 owner 的公平调度（A80 的另一半）。曾经的 `FairQueue`
//! 类型零消费者，已按 V4「未接线公开 API 台账」删除登记——饿死风险仍然真实存在，
//! 但它现在记在方案里而不是记在一个没人调用的泛型上。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    Calls,
    Streams,
    Events,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimit {
    pub global_count: u64,
    pub per_principal_count: u64,
    pub global_bytes: u64,
    pub per_principal_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Usage {
    count: u64,
    bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reservation {
    principal: String,
    kind: ResourceKind,
    count: u64,
    bytes: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdmissionError {
    #[error("resource {kind:?} global budget exceeded")]
    GlobalBudgetExceeded { kind: ResourceKind },
    #[error("resource {kind:?} budget exceeded for principal {principal}")]
    PrincipalBudgetExceeded { kind: ResourceKind, principal: String },
    #[error("credit window exhausted: requested {requested}, available {available}")]
    CreditExhausted { requested: u64, available: u64 },
}

#[derive(Debug, Default)]
pub struct AdmissionController {
    limits: HashMap<ResourceKind, ResourceLimit>,
    global: HashMap<ResourceKind, Usage>,
    principals: HashMap<(String, ResourceKind), Usage>,
    reservations: HashMap<String, Reservation>,
}

impl AdmissionController {
    pub fn set_limit(&mut self, kind: ResourceKind, limit: ResourceLimit) {
        self.limits.insert(kind, limit);
    }

    pub fn admit(
        &mut self,
        principal: &str,
        kind: ResourceKind,
        count: u64,
        bytes: u64,
    ) -> Result<String, AdmissionError> {
        let limit = self.limits.get(&kind).copied().unwrap_or(ResourceLimit {
            global_count: u64::MAX,
            per_principal_count: u64::MAX,
            global_bytes: u64::MAX,
            per_principal_bytes: u64::MAX,
        });
        let global = self.global.get(&kind).copied().unwrap_or_default();
        if global.count.saturating_add(count) > limit.global_count
            || global.bytes.saturating_add(bytes) > limit.global_bytes
        {
            return Err(AdmissionError::GlobalBudgetExceeded { kind });
        }
        let key = (principal.to_string(), kind);
        let local = self.principals.get(&key).copied().unwrap_or_default();
        if local.count.saturating_add(count) > limit.per_principal_count
            || local.bytes.saturating_add(bytes) > limit.per_principal_bytes
        {
            return Err(AdmissionError::PrincipalBudgetExceeded {
                kind,
                principal: principal.to_string(),
            });
        }

        self.global
            .insert(kind, Usage { count: global.count + count, bytes: global.bytes + bytes });
        self.principals
            .insert(key, Usage { count: local.count + count, bytes: local.bytes + bytes });
        let token = Uuid::new_v4().to_string();
        self.reservations.insert(
            token.clone(),
            Reservation { principal: principal.to_string(), kind, count, bytes },
        );
        Ok(token)
    }

    pub fn release(&mut self, token: &str) -> bool {
        let Some(r) = self.reservations.remove(token) else {
            return false;
        };
        if let Some(global) = self.global.get_mut(&r.kind) {
            global.count = global.count.saturating_sub(r.count);
            global.bytes = global.bytes.saturating_sub(r.bytes);
        }
        let key = (r.principal, r.kind);
        if let Some(local) = self.principals.get_mut(&key) {
            local.count = local.count.saturating_sub(r.count);
            local.bytes = local.bytes.saturating_sub(r.bytes);
        }
        true
    }

    pub fn release_principal(&mut self, principal: &str) -> usize {
        let tokens: Vec<_> = self
            .reservations
            .iter()
            .filter(|(_, r)| r.principal == principal)
            .map(|(token, _)| token.clone())
            .collect();
        let count = tokens.len();
        for token in tokens {
            self.release(&token);
        }
        count
    }

    /// Internal accounting snapshot used by owning subsystem tests to verify reservation lifecycle.
    #[cfg(test)]
    pub(crate) fn usage(&self, kind: ResourceKind) -> (u64, u64) {
        let usage = self.global.get(&kind).copied().unwrap_or_default();
        (usage.count, usage.bytes)
    }

    #[cfg(test)]
    pub(crate) fn principal_usage(&self, principal: &str, kind: ResourceKind) -> (u64, u64) {
        let usage =
            self.principals.get(&(principal.to_string(), kind)).copied().unwrap_or_default();
        (usage.count, usage.bytes)
    }
}

/// Receiver-driven credits. A sender may not emit more units than the consumer has granted.
///
/// 生产者（仓内唯一）：`StreamRegistry` 的每条流句柄——`open` 装初始额度、
/// `grant` 由接收方补额（封顶 `max`）、`push` 在占 `seq` **之前**扣额，
/// 不足即 `E_STREAM_BACKPRESSURE` 且零副作用。因此额度语义只有一处可改。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditWindow {
    available: u64,
    max: u64,
}

impl CreditWindow {
    /// `initial` 会被夹到 `max`；`max` 至少为 1，避免零额度窗口让首帧永远无法通过。
    pub fn new(initial: u64, max: u64) -> Self {
        let max = max.max(1);
        Self { available: initial.min(max), max }
    }

    pub fn available(&self) -> u64 {
        self.available
    }

    pub fn grant(&mut self, credits: u64) {
        self.available = self.available.saturating_add(credits).min(self.max);
    }

    pub fn consume(&mut self, credits: u64) -> Result<(), AdmissionError> {
        if credits > self.available {
            return Err(AdmissionError::CreditExhausted {
                requested: credits,
                available: self.available,
            });
        }
        self.available -= credits;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_is_bounded_globally_and_per_principal() {
        let mut a = AdmissionController::default();
        a.set_limit(
            ResourceKind::Calls,
            ResourceLimit {
                global_count: 2,
                per_principal_count: 1,
                global_bytes: 100,
                per_principal_bytes: 80,
            },
        );
        let p1 = a.admit("p1", ResourceKind::Calls, 1, 10).unwrap();
        assert!(matches!(
            a.admit("p1", ResourceKind::Calls, 1, 10),
            Err(AdmissionError::PrincipalBudgetExceeded { .. })
        ));
        let p2 = a.admit("p2", ResourceKind::Calls, 1, 10).unwrap();
        assert!(matches!(
            a.admit("p3", ResourceKind::Calls, 1, 10),
            Err(AdmissionError::GlobalBudgetExceeded { .. })
        ));
        assert!(a.release(&p1));
        assert!(a.release(&p2));
    }

    #[test]
    fn usage_tracks_and_releases_reservations() {
        let mut a = AdmissionController::default();
        a.set_limit(
            ResourceKind::Calls,
            ResourceLimit {
                global_count: 4,
                per_principal_count: 2,
                global_bytes: 1_000,
                per_principal_bytes: 500,
            },
        );
        let token = a.admit("p1", ResourceKind::Calls, 1, 123).unwrap();
        assert_eq!(a.usage(ResourceKind::Calls), (1, 123));
        assert_eq!(a.principal_usage("p1", ResourceKind::Calls), (1, 123));
        assert!(a.release(&token));
        assert_eq!(a.usage(ResourceKind::Calls), (0, 0));
        assert_eq!(a.principal_usage("p1", ResourceKind::Calls), (0, 0));
    }

    #[test]
    fn credit_window_prevents_slow_consumer_overrun() {
        let mut c = CreditWindow::new(2, 4);
        c.consume(2).unwrap();
        assert!(matches!(c.consume(1), Err(AdmissionError::CreditExhausted { .. })));
        c.grant(3);
        assert_eq!(c.available(), 3);
    }

    /// `CreditWindow` 是 `StreamRegistry` 的额度原语（A79）。这里锁住它的**边界**语义，
    /// 使流侧那条生产路径与单测共用同一份算术：封顶、夹初始值、min-max=1 的存活下界。
    #[test]
    fn credit_window_bounds_and_ratchet_the_way_streams_depend_on() {
        let c = CreditWindow::new(10, 4);
        assert_eq!(c.available(), 4, "初始额度夹到 max");

        let mut c = CreditWindow::new(1, 0);
        assert_eq!(c.available(), 1, "max=0 也要留下能过一帧的下界");
        c.grant(9);
        assert_eq!(c.available(), 1, "封顶后 grant 不能把窗口抬过 max");
        assert!(c.consume(2).is_err());
        c.consume(1).unwrap();
        assert_eq!(c.available(), 0);
    }
}
