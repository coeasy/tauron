//! V4 provider lifecycle and capability epoch (A74).
//!
//! Capability snapshots are invalidated whenever a provider changes lifecycle state. Consumers
//! carry the epoch they observed; stale decisions must be re-evaluated instead of being reused.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderState {
    Registered,
    Starting,
    Ready,
    Degraded,
    Stopping,
    Stopped,
    Quarantined,
}

impl ProviderState {
    pub const fn can_serve(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProviderLifecycleError {
    #[error("invalid provider lifecycle transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: ProviderState,
        to: ProviderState,
    },
}

#[derive(Debug)]
pub struct CapabilityEpoch {
    value: AtomicU64,
}

impl Default for CapabilityEpoch {
    fn default() -> Self {
        Self {
            value: AtomicU64::new(1),
        }
    }
}

impl CapabilityEpoch {
    pub fn current(&self) -> u64 {
        self.value.load(Ordering::Acquire)
    }

    pub fn bump(&self) -> u64 {
        self.value.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn is_current(&self, observed: u64) -> bool {
        observed == self.current()
    }
}

#[derive(Debug)]
pub struct ProviderLifecycle {
    state: ProviderState,
    epoch: CapabilityEpoch,
}

impl Default for ProviderLifecycle {
    fn default() -> Self {
        Self {
            state: ProviderState::Registered,
            epoch: CapabilityEpoch::default(),
        }
    }
}

impl ProviderLifecycle {
    pub fn state(&self) -> ProviderState {
        self.state
    }

    pub fn capability_epoch(&self) -> u64 {
        self.epoch.current()
    }

    pub fn transition(&mut self, to: ProviderState) -> Result<u64, ProviderLifecycleError> {
        if self.state == to {
            return Ok(self.epoch.current());
        }
        if !allowed(self.state, to) {
            return Err(ProviderLifecycleError::InvalidTransition {
                from: self.state,
                to,
            });
        }
        self.state = to;
        Ok(self.epoch.bump())
    }
}

const fn allowed(from: ProviderState, to: ProviderState) -> bool {
    use ProviderState::*;
    matches!(
        (from, to),
        (Registered, Starting)
            | (Starting, Ready)
            | (Starting, Degraded)
            | (Starting, Quarantined)
            | (Ready, Degraded)
            | (Ready, Stopping)
            | (Ready, Quarantined)
            | (Degraded, Ready)
            | (Degraded, Stopping)
            | (Degraded, Quarantined)
            | (Quarantined, Starting)
            | (Quarantined, Stopping)
            | (Stopping, Stopped)
            | (Stopped, Starting)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_changes_invalidate_capability_epoch() {
        let mut lifecycle = ProviderLifecycle::default();
        let e1 = lifecycle.capability_epoch();
        let e2 = lifecycle.transition(ProviderState::Starting).unwrap();
        let e3 = lifecycle.transition(ProviderState::Ready).unwrap();
        assert!(e2 > e1 && e3 > e2);
        assert!(!lifecycle.epoch.is_current(e1));
        assert!(lifecycle.state().can_serve());
    }

    #[test]
    fn illegal_transition_fails_closed_without_epoch_change() {
        let mut lifecycle = ProviderLifecycle::default();
        let before = lifecycle.capability_epoch();
        assert!(lifecycle.transition(ProviderState::Ready).is_err());
        assert_eq!(lifecycle.capability_epoch(), before);
        assert_eq!(lifecycle.state(), ProviderState::Registered);
    }
}
