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
        Self::from_reader(bytes).expect("读内存切片不会失败")
    }

    /// 从任意 `Read` 流式构造（固定 64 KiB 缓冲）：内容身份只由摘要与长度决定，
    /// 安装路径的大文件不必整读进内存（A94）。
    pub fn from_reader(mut reader: impl std::io::Read) -> std::io::Result<Self> {
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            size += read as u64;
        }
        Ok(Self { sha256: hex::encode(hasher.finalize()), size })
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
