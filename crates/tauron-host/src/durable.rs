//! V4 durable envelope and reversible migration snapshot (A93/A101).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableEnvelope<T> {
    pub schema: String,
    pub generation: u64,
    pub payload: T,
    pub checksum: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DurableError {
    #[error("durable envelope checksum mismatch")]
    ChecksumMismatch,
    #[error("durable envelope JSON is invalid: {0}")]
    InvalidJson(String),
    #[error("durable envelope schema is empty")]
    MissingSchema,
}

fn payload_checksum<T: Serialize>(
    schema: &str,
    generation: u64,
    payload: &T,
) -> Result<String, DurableError> {
    let canonical = serde_json::to_vec(&(schema, generation, payload))
        .map_err(|e| DurableError::InvalidJson(e.to_string()))?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

impl<T: Serialize> DurableEnvelope<T> {
    pub fn seal(
        schema: impl Into<String>,
        generation: u64,
        payload: T,
    ) -> Result<Self, DurableError> {
        let schema = schema.into();
        if schema.trim().is_empty() {
            return Err(DurableError::MissingSchema);
        }
        let checksum = payload_checksum(&schema, generation, &payload)?;
        Ok(Self { schema, generation, payload, checksum })
    }

    pub fn verify(&self) -> Result<(), DurableError> {
        if self.schema.trim().is_empty() {
            return Err(DurableError::MissingSchema);
        }
        let actual = payload_checksum(&self.schema, self.generation, &self.payload)?;
        if actual == self.checksum {
            Ok(())
        } else {
            Err(DurableError::ChecksumMismatch)
        }
    }
}

pub fn encode_durable<T: Serialize>(value: &DurableEnvelope<T>) -> Result<Vec<u8>, DurableError> {
    value.verify()?;
    serde_json::to_vec(value).map_err(|e| DurableError::InvalidJson(e.to_string()))
}

pub fn decode_durable<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
) -> Result<DurableEnvelope<T>, DurableError> {
    let value: DurableEnvelope<T> =
        serde_json::from_slice(bytes).map_err(|e| DurableError::InvalidJson(e.to_string()))?;
    value.verify()?;
    Ok(value)
}

/// Migration carries both before and after snapshots. Persisting the after-image is not allowed
/// to destroy the rollback image until the caller explicitly commits the migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationSnapshot<T> {
    pub from_version: String,
    pub to_version: String,
    pub before: T,
    pub after: T,
    committed: bool,
}

impl<T: Clone> MigrationSnapshot<T> {
    pub fn stage(
        from_version: impl Into<String>,
        to_version: impl Into<String>,
        before: T,
        migrate: impl FnOnce(&T) -> T,
    ) -> Self {
        let after = migrate(&before);
        Self {
            from_version: from_version.into(),
            to_version: to_version.into(),
            before,
            after,
            committed: false,
        }
    }

    pub fn commit(&mut self) {
        self.committed = true;
    }

    pub fn committed(&self) -> bool {
        self.committed
    }

    pub fn rollback_value(&self) -> T {
        self.before.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_detects_torn_or_tampered_payload() {
        let mut envelope = DurableEnvelope::seal("settings/1", 3, vec![1_u8, 2, 3]).unwrap();
        envelope.payload.push(4);
        assert_eq!(envelope.verify(), Err(DurableError::ChecksumMismatch));
    }

    #[test]
    fn durable_round_trip_verifies_before_returning() {
        let envelope = DurableEnvelope::seal("registry/1", 9, serde_json::json!({"a": 1})).unwrap();
        let bytes = encode_durable(&envelope).unwrap();
        let decoded: DurableEnvelope<serde_json::Value> = decode_durable(&bytes).unwrap();
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn migration_keeps_rollback_snapshot_until_commit() {
        let mut m = MigrationSnapshot::stage("1", "2", vec![1], |old| {
            let mut next = old.clone();
            next.push(2);
            next
        });
        assert_eq!(m.rollback_value(), vec![1]);
        assert_eq!(m.after, vec![1, 2]);
        assert!(!m.committed());
        m.commit();
        assert!(m.committed());
    }
}
