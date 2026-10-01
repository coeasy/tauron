//! V4 immutable installed content and activation integrity (A84).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::generation::Generation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentIdentity {
    pub sha256: String,
    pub size: u64,
}

impl ContentIdentity {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self { sha256: hex::encode(Sha256::digest(bytes)), size: bytes.len() as u64 }
    }

    pub fn verify(&self, bytes: &[u8]) -> bool {
        self.size == bytes.len() as u64 && self.sha256 == hex::encode(Sha256::digest(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationRecord {
    pub resource: String,
    pub generation: Generation,
    pub content: ContentIdentity,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ActivationError {
    #[error("installed content does not match activation record")]
    IntegrityMismatch,
}

impl ActivationRecord {
    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<(), ActivationError> {
        if self.content.verify(bytes) {
            Ok(())
        } else {
            Err(ActivationError::IntegrityMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_is_bound_to_immutable_content_digest() {
        let bytes = b"plugin payload";
        let record = ActivationRecord {
            resource: "plugin:p".into(),
            generation: Generation(4),
            content: ContentIdentity::from_bytes(bytes),
        };
        record.verify_bytes(bytes).unwrap();
        assert_eq!(
            record.verify_bytes(b"plugin payload modified"),
            Err(ActivationError::IntegrityMismatch)
        );
    }
}
