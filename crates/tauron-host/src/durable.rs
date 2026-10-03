//! V4 durable envelope (A93).
//!
//! A101 的「迁移回滚快照」不在这一层：它是宿主设置的具体协议，落盘/消费都在
//! `tauron-adapter`（`stage_settings_rollback_image` / `load_settings_rollback_image`）。
//! 这里曾经有一个通用的 `MigrationSnapshot<T>`（before/after + `committed`），零消费者，
//! 且与 `tauron-settings` 的 `MigrationReceipt.before` 是同一个事实的两份表达——已删除并
//! 登记进 V4「未接线公开 API 台账」轮 11 小节。

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
}
