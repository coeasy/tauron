//! V4 generational activation, leases and stale-handle protection (A88/A89/A90).

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
    StaleHandle {
        resource: String,
        handle: Generation,
        active: Generation,
    },
    #[error("unknown generation lease {0}")]
    UnknownLease(String),
}

#[derive(Debug, Default)]
pub struct GenerationRegistry {
    active: HashMap<String, Generation>,
    leases: HashMap<String, (String, Generation)>,
}

impl GenerationRegistry {
    pub fn activate(&mut self, resource: &str) -> Generation {
        let next = self
            .active
            .get(resource)
            .copied()
            .map_or(Generation::INITIAL, Generation::next);
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
        self.leases
            .insert(token.clone(), (resource.to_string(), generation));
        Ok(GenerationHandle {
            resource: resource.to_string(),
            generation,
            token,
        })
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
        self.leases
            .values()
            .filter(|(r, g)| r == resource && *g == generation)
            .count()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_invalidates_old_handles_without_destroying_their_lease_fact() {
        let mut r = GenerationRegistry::default();
        assert_eq!(r.activate("plugin:p"), Generation(1));
        let old = r.lease("plugin:p").unwrap();
        assert_eq!(r.activate("plugin:p"), Generation(2));
        assert!(matches!(
            r.validate(&old),
            Err(GenerationError::StaleHandle { .. })
        ));
        assert_eq!(r.leases_for("plugin:p", Generation(1)), 1);
        assert!(!r.retire_unleased("plugin:p", Generation(1)).unwrap());
        assert!(r.release(&old.token));
        assert!(r.retire_unleased("plugin:p", Generation(1)).unwrap());
    }
}
