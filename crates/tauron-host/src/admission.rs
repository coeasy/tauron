//! V4 credit-based backpressure and hierarchical admission control (A79/A80).

use std::collections::{HashMap, VecDeque};

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
    PrincipalBudgetExceeded {
        kind: ResourceKind,
        principal: String,
    },
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

        self.global.insert(
            kind,
            Usage {
                count: global.count + count,
                bytes: global.bytes + bytes,
            },
        );
        self.principals.insert(
            key,
            Usage {
                count: local.count + count,
                bytes: local.bytes + bytes,
            },
        );
        let token = Uuid::new_v4().to_string();
        self.reservations.insert(
            token.clone(),
            Reservation {
                principal: principal.to_string(),
                kind,
                count,
                bytes,
            },
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
}

/// Receiver-driven credits. A sender may not emit more units than the consumer has granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditWindow {
    available: u64,
    max: u64,
}

impl CreditWindow {
    pub fn new(initial: u64, max: u64) -> Self {
        let max = max.max(1);
        Self {
            available: initial.min(max),
            max,
        }
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

/// Small deterministic round-robin queue used by adapters to avoid one principal monopolizing
/// dispatch. It is intentionally transport/runtime neutral.
#[derive(Debug, Default)]
pub struct FairQueue<T> {
    order: VecDeque<String>,
    queues: HashMap<String, VecDeque<T>>,
}

impl<T> FairQueue<T> {
    pub fn push(&mut self, principal: impl Into<String>, item: T) {
        let principal = principal.into();
        let queue = self.queues.entry(principal.clone()).or_default();
        let was_empty = queue.is_empty();
        queue.push_back(item);
        if was_empty {
            self.order.push_back(principal);
        }
    }

    pub fn pop(&mut self) -> Option<(String, T)> {
        let principal = self.order.pop_front()?;
        let queue = self.queues.get_mut(&principal)?;
        let item = queue.pop_front()?;
        if !queue.is_empty() {
            self.order.push_back(principal.clone());
        } else {
            self.queues.remove(&principal);
        }
        Some((principal, item))
    }

    pub fn len(&self) -> usize {
        self.queues.values().map(VecDeque::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.queues.is_empty()
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
    fn credit_window_prevents_slow_consumer_overrun() {
        let mut c = CreditWindow::new(2, 4);
        c.consume(2).unwrap();
        assert!(matches!(
            c.consume(1),
            Err(AdmissionError::CreditExhausted { .. })
        ));
        c.grant(3);
        assert_eq!(c.available(), 3);
    }

    #[test]
    fn fair_queue_round_robins_principals() {
        let mut q = FairQueue::default();
        q.push("a", 1);
        q.push("a", 2);
        q.push("b", 3);
        assert_eq!(q.pop(), Some(("a".into(), 1)));
        assert_eq!(q.pop(), Some(("b".into(), 3)));
        assert_eq!(q.pop(), Some(("a".into(), 2)));
        assert!(q.is_empty());
    }
}
