//! V4 storage namespace and single-writer lease (A87).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StorageNamespace {
    pub tenant: String,
    pub application: String,
    pub principal: String,
}

impl StorageNamespace {
    pub fn key(&self) -> String {
        format!("{}:{}:{}", self.tenant, self.application, self.principal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SingleWriterLease {
    pub namespace: StorageNamespace,
    pub owner: String,
    pub epoch: u64,
    pub token: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WriterLeaseError {
    #[error("storage namespace already has a writer: {owner}")]
    Busy { owner: String },
    #[error("writer lease is stale or unknown")]
    Stale,
}

#[derive(Debug, Default)]
pub struct WriterLeaseTable {
    active: HashMap<String, SingleWriterLease>,
    epochs: HashMap<String, u64>,
}

impl WriterLeaseTable {
    pub fn acquire(
        &mut self,
        namespace: StorageNamespace,
        owner: &str,
    ) -> Result<SingleWriterLease, WriterLeaseError> {
        let key = namespace.key();
        if let Some(existing) = self.active.get(&key) {
            return Err(WriterLeaseError::Busy {
                owner: existing.owner.clone(),
            });
        }
        let epoch = self.epochs.get(&key).copied().unwrap_or(0).saturating_add(1);
        self.epochs.insert(key.clone(), epoch);
        let lease = SingleWriterLease {
            namespace,
            owner: owner.to_string(),
            epoch,
            token: Uuid::new_v4().to_string(),
        };
        self.active.insert(key, lease.clone());
        Ok(lease)
    }

    pub fn validate(&self, lease: &SingleWriterLease) -> Result<(), WriterLeaseError> {
        let key = lease.namespace.key();
        if self.active.get(&key) == Some(lease) {
            Ok(())
        } else {
            Err(WriterLeaseError::Stale)
        }
    }

    pub fn release(&mut self, lease: &SingleWriterLease) -> Result<(), WriterLeaseError> {
        self.validate(lease)?;
        self.active.remove(&lease.namespace.key());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns() -> StorageNamespace {
        StorageNamespace {
            tenant: "default".into(),
            application: "app".into(),
            principal: "plugin.p".into(),
        }
    }

    #[test]
    fn only_one_writer_exists_and_epoch_advances_after_release() {
        let mut table = WriterLeaseTable::default();
        let first = table.acquire(ns(), "host-a").unwrap();
        assert!(matches!(
            table.acquire(ns(), "host-b"),
            Err(WriterLeaseError::Busy { .. })
        ));
        table.release(&first).unwrap();
        let second = table.acquire(ns(), "host-b").unwrap();
        assert!(second.epoch > first.epoch);
        assert_eq!(table.validate(&first), Err(WriterLeaseError::Stale));
    }
}
