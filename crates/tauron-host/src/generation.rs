//! V4 generational activation and stale-handle protection (A88/A90), plus
//! cross-host pack/cache GC leases (A89).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(pub u64);

impl Generation {
    pub const INITIAL: Self = Self(1);

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationHandle {
    pub resource: String,
    pub generation: Generation,
    pub token: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum GenerationError {
    #[error("resource {0} has no active generation")]
    NotActive(String),
    #[error("stale handle for {resource}: handle={handle:?}, active={active:?}")]
    StaleHandle { resource: String, handle: Generation, active: Generation },
    #[error("unknown generation lease {0}")]
    UnknownLease(String),
    #[error("pack/cache entry is not tracked: {pack_id}@{version} generation {generation:?}")]
    PackNotTracked {
        pack_id: String,
        version: String,
        generation: Generation,
    },
    #[error("pack/cache lease ttl must be greater than zero")]
    InvalidLeaseTtl,
    #[error("pack/cache lease owner hostInstanceId must not be empty")]
    InvalidHostInstance,
    #[error("pack/cache lease {0} expired")]
    ExpiredPackLease(String),
}

#[derive(Debug, Default)]
pub struct GenerationRegistry {
    active: HashMap<String, Generation>,
    leases: HashMap<String, (String, Generation)>,
}

impl GenerationRegistry {
    pub fn activate(&mut self, resource: &str) -> Generation {
        let next = self.active.get(resource).copied().map_or(Generation::INITIAL, Generation::next);
        self.active.insert(resource.to_string(), next);
        next
    }

    pub fn current(&self, resource: &str) -> Option<Generation> {
        self.active.get(resource).copied()
    }

    pub fn lease(&mut self, resource: &str) -> Result<GenerationHandle, GenerationError> {
        let generation = self
            .current(resource)
            .ok_or_else(|| GenerationError::NotActive(resource.to_string()))?;
        let token = Uuid::new_v4().to_string();
        self.leases.insert(token.clone(), (resource.to_string(), generation));
        Ok(GenerationHandle { resource: resource.to_string(), generation, token })
    }

    pub fn validate(&self, handle: &GenerationHandle) -> Result<(), GenerationError> {
        let active = self
            .current(&handle.resource)
            .ok_or_else(|| GenerationError::NotActive(handle.resource.clone()))?;
        if active != handle.generation {
            return Err(GenerationError::StaleHandle {
                resource: handle.resource.clone(),
                handle: handle.generation,
                active,
            });
        }
        match self.leases.get(&handle.token) {
            Some((resource, generation))
                if resource == &handle.resource && *generation == handle.generation =>
            {
                Ok(())
            }
            _ => Err(GenerationError::UnknownLease(handle.token.clone())),
        }
    }

    pub fn release(&mut self, token: &str) -> bool {
        self.leases.remove(token).is_some()
    }

    pub fn leases_for(&self, resource: &str, generation: Generation) -> usize {
        self.leases.values().filter(|(r, g)| r == resource && *g == generation).count()
    }

    pub fn retire_unleased(
        &self,
        resource: &str,
        generation: Generation,
    ) -> Result<bool, GenerationError> {
        let active = self
            .current(resource)
            .ok_or_else(|| GenerationError::NotActive(resource.to_string()))?;
        Ok(generation != active && self.leases_for(resource, generation) == 0)
    }
}


/* ──────────────────────────────────────────────────────────────────────────
 * V4 A89: cross-host PackLease / CacheLease authority.
 * ──────────────────────────────────────────────────────────────────────── */

/// Identity of one immutable pack/cache generation.
///
/// The version is part of the key on purpose: generation alone is only meaningful inside one
/// resource lineage, while GC needs to distinguish rollback versions that may coexist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackCacheKey {
    pub pack_id: String,
    pub version: String,
    pub generation: Generation,
}

impl PackCacheKey {
    pub fn new(
        pack_id: impl Into<String>,
        version: impl Into<String>,
        generation: Generation,
    ) -> Self {
        Self { pack_id: pack_id.into(), version: version.into(), generation }
    }
}

/// Mutable GC facts that are independent from host leases.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackGcState {
    /// This exact pack/version/generation is currently selected for new work.
    pub active: bool,
    /// Retained as an explicit rollback target.
    pub rollback_pinned: bool,
    /// Owned by an in-flight install/update transaction.
    pub transaction_staged: bool,
}

/// Lease held by one HostInstance while it can still read/execute a pack/cache generation.
///
/// The authority that owns `PackLeaseRegistry` is responsible for using one consistent clock
/// domain for `now_ms`/`expires_at_ms`. A crashed Host does not need to release the lease:
/// expiry makes the generation collectable after all other GC guards are false.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackLease {
    pub token: String,
    pub host_instance_id: String,
    pub pack_id: String,
    pub version: String,
    pub generation: Generation,
    pub expires_at_ms: u64,
}

impl PackLease {
    pub fn key(&self) -> PackCacheKey {
        PackCacheKey::new(self.pack_id.clone(), self.version.clone(), self.generation)
    }

    pub fn is_live_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms > now_ms
    }
}

/// Shared-owner decision table for pack/cache lifetime.
///
/// It is deliberately independent from filesystem deletion. Callers first ask this authority
/// whether a generation is GC-eligible and only then remove bytes. This keeps the four V4 A89
/// predicates in one place instead of duplicating them across installers/runtimes.
#[derive(Debug, Default)]
pub struct PackLeaseRegistry {
    entries: HashMap<PackCacheKey, PackGcState>,
    leases: HashMap<String, PackLease>,
}

impl PackLeaseRegistry {
    /// Register/update GC facts for an immutable pack generation.
    pub fn track(&mut self, key: PackCacheKey, state: PackGcState) {
        self.entries.insert(key, state);
    }

    pub fn state(&self, key: &PackCacheKey) -> Option<PackGcState> {
        self.entries.get(key).copied()
    }

    pub fn set_state(
        &mut self,
        key: &PackCacheKey,
        state: PackGcState,
    ) -> Result<(), GenerationError> {
        let slot = self.entries.get_mut(key).ok_or_else(|| GenerationError::PackNotTracked {
            pack_id: key.pack_id.clone(),
            version: key.version.clone(),
            generation: key.generation,
        })?;
        *slot = state;
        Ok(())
    }

    /// Acquire a TTL lease for one HostInstance.
    pub fn acquire(
        &mut self,
        host_instance_id: &str,
        key: &PackCacheKey,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<PackLease, GenerationError> {
        if host_instance_id.trim().is_empty() {
            return Err(GenerationError::InvalidHostInstance);
        }
        if ttl_ms == 0 {
            return Err(GenerationError::InvalidLeaseTtl);
        }
        if !self.entries.contains_key(key) {
            return Err(GenerationError::PackNotTracked {
                pack_id: key.pack_id.clone(),
                version: key.version.clone(),
                generation: key.generation,
            });
        }

        let lease = PackLease {
            token: Uuid::new_v4().to_string(),
            host_instance_id: host_instance_id.to_string(),
            pack_id: key.pack_id.clone(),
            version: key.version.clone(),
            generation: key.generation,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
        };
        self.leases.insert(lease.token.clone(), lease.clone());
        Ok(lease)
    }

    /// Extend a live lease. Expired leases cannot be resurrected by heartbeat.
    pub fn heartbeat(
        &mut self,
        token: &str,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<PackLease, GenerationError> {
        if ttl_ms == 0 {
            return Err(GenerationError::InvalidLeaseTtl);
        }
        let expired = self
            .leases
            .get(token)
            .map(|lease| !lease.is_live_at(now_ms))
            .ok_or_else(|| GenerationError::UnknownLease(token.to_string()))?;
        if expired {
            self.leases.remove(token);
            return Err(GenerationError::ExpiredPackLease(token.to_string()));
        }

        let lease = self.leases.get_mut(token).expect("lease checked above");
        lease.expires_at_ms = now_ms.saturating_add(ttl_ms);
        Ok(lease.clone())
    }

    pub fn release(&mut self, token: &str) -> bool {
        self.leases.remove(token).is_some()
    }

    /// Crash recovery: drop lease facts whose owners stopped heartbeating.
    pub fn expire_dead_leases(&mut self, now_ms: u64) -> usize {
        let before = self.leases.len();
        self.leases.retain(|_, lease| lease.is_live_at(now_ms));
        before - self.leases.len()
    }

    pub fn live_lease_count(&self, key: &PackCacheKey, now_ms: u64) -> usize {
        self.leases
            .values()
            .filter(|lease| lease.is_live_at(now_ms) && lease.key() == *key)
            .count()
    }

    /// V4 A89 single-source GC decision:
    ///
    /// `not active && not rollback-pinned && no live lease && not transaction-staged`.
    pub fn gc_eligible(
        &self,
        key: &PackCacheKey,
        now_ms: u64,
    ) -> Result<bool, GenerationError> {
        let state = self.entries.get(key).ok_or_else(|| GenerationError::PackNotTracked {
            pack_id: key.pack_id.clone(),
            version: key.version.clone(),
            generation: key.generation,
        })?;
        Ok(!state.active
            && !state.rollback_pinned
            && !state.transaction_staged
            && self.live_lease_count(key, now_ms) == 0)
    }

    /// Snapshot of all currently collectable keys. Stale lease rows are ignored by time check,
    /// so a crashed host cannot block GC forever even before the explicit expiry sweep runs.
    pub fn gc_candidates(&self, now_ms: u64) -> Vec<PackCacheKey> {
        self.entries
            .keys()
            .filter(|key| self.gc_eligible(key, now_ms).unwrap_or(false))
            .cloned()
            .collect()
    }

    /// Remove metadata only when the same four-condition gate permits byte deletion.
    ///
    /// Filesystem/cache owners should perform actual deletion only after this returns `true`.
    pub fn retire_if_gc_eligible(
        &mut self,
        key: &PackCacheKey,
        now_ms: u64,
    ) -> Result<bool, GenerationError> {
        if !self.gc_eligible(key, now_ms)? {
            return Ok(false);
        }
        self.entries.remove(key);
        self.leases.retain(|_, lease| lease.key() != *key);
        Ok(true)
    }

    pub fn lease_count(&self) -> usize {
        self.leases.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_invalidates_old_handles_without_destroying_their_lease_fact() {
        let mut r = GenerationRegistry::default();
        assert_eq!(r.activate("plugin:p"), Generation(1));
        let old = r.lease("plugin:p").unwrap();
        assert_eq!(r.activate("plugin:p"), Generation(2));
        assert!(matches!(r.validate(&old), Err(GenerationError::StaleHandle { .. })));
        assert_eq!(r.leases_for("plugin:p", Generation(1)), 1);
        assert!(!r.retire_unleased("plugin:p", Generation(1)).unwrap());
        assert!(r.release(&old.token));
        assert!(r.retire_unleased("plugin:p", Generation(1)).unwrap());
    }

    fn cache_key() -> PackCacheKey {
        PackCacheKey::new("wasm-engine", "1.2.3", Generation(7))
    }

    #[test]
    fn pack_gc_requires_all_four_guards_to_be_clear() {
        let key = cache_key();
        let mut leases = PackLeaseRegistry::default();

        for state in [
            PackGcState { active: true, rollback_pinned: false, transaction_staged: false },
            PackGcState { active: false, rollback_pinned: true, transaction_staged: false },
            PackGcState { active: false, rollback_pinned: false, transaction_staged: true },
        ] {
            leases.track(key.clone(), state);
            assert!(!leases.gc_eligible(&key, 100).unwrap());
        }

        leases.track(key.clone(), PackGcState::default());
        assert!(leases.gc_eligible(&key, 100).unwrap());
    }

    #[test]
    fn live_cross_host_lease_blocks_gc_until_release_or_expiry() {
        let key = cache_key();
        let mut leases = PackLeaseRegistry::default();
        leases.track(key.clone(), PackGcState::default());

        let a = leases.acquire("host-a", &key, 1_000, 500).unwrap();
        let b = leases.acquire("host-b", &key, 1_000, 1_000).unwrap();
        assert_eq!(leases.live_lease_count(&key, 1_100), 2);
        assert!(!leases.gc_eligible(&key, 1_100).unwrap());

        assert!(leases.release(&a.token));
        assert_eq!(leases.live_lease_count(&key, 1_100), 1);
        assert!(!leases.gc_eligible(&key, 1_100).unwrap());

        assert_eq!(leases.live_lease_count(&key, 2_000), 0);
        assert!(leases.gc_eligible(&key, 2_000).unwrap());
        assert_eq!(leases.expire_dead_leases(2_000), 1);
        assert_eq!(leases.lease_count(), 0);
        assert_eq!(b.host_instance_id, "host-b");
    }

    #[test]
    fn heartbeat_extends_only_a_live_lease_and_cannot_resurrect_expired_owner() {
        let key = cache_key();
        let mut leases = PackLeaseRegistry::default();
        leases.track(key.clone(), PackGcState::default());

        let lease = leases.acquire("host-a", &key, 100, 100).unwrap();
        let renewed = leases.heartbeat(&lease.token, 150, 500).unwrap();
        assert_eq!(renewed.expires_at_ms, 650);
        assert!(!leases.gc_eligible(&key, 649).unwrap());

        let err = leases.heartbeat(&lease.token, 650, 500).unwrap_err();
        assert!(matches!(err, GenerationError::ExpiredPackLease(_)));
        assert!(leases.gc_eligible(&key, 650).unwrap());
    }

    #[test]
    fn gc_retirement_is_atomic_with_the_authority_metadata() {
        let key = cache_key();
        let mut leases = PackLeaseRegistry::default();
        leases.track(key.clone(), PackGcState::default());
        let lease = leases.acquire("host-a", &key, 0, 10).unwrap();

        assert!(!leases.retire_if_gc_eligible(&key, 5).unwrap());
        assert!(leases.release(&lease.token));
        assert!(leases.retire_if_gc_eligible(&key, 5).unwrap());
        assert!(leases.state(&key).is_none());
        assert!(matches!(
            leases.gc_eligible(&key, 5),
            Err(GenerationError::PackNotTracked { .. })
        ));
    }

    #[test]
    fn pack_lease_input_is_fail_closed() {
        let key = cache_key();
        let mut leases = PackLeaseRegistry::default();
        leases.track(key.clone(), PackGcState::default());
        assert_eq!(
            leases.acquire("", &key, 0, 10).unwrap_err(),
            GenerationError::InvalidHostInstance
        );
        assert_eq!(
            leases.acquire("host-a", &key, 0, 0).unwrap_err(),
            GenerationError::InvalidLeaseTtl
        );

        let missing = PackCacheKey::new("missing", "1", Generation(1));
        assert!(matches!(
            leases.acquire("host-a", &missing, 0, 10),
            Err(GenerationError::PackNotTracked { .. })
        ));
    }
}
