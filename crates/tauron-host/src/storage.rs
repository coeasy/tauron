//! V4 storage namespace and single-writer lease (A87).

use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    #[error("writer lease I/O failed during {action}: {detail}")]
    Io { action: String, detail: String },
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
            return Err(WriterLeaseError::Busy { owner: existing.owner.clone() });
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

/// Cross-process single-writer lease backed by the OS file-lock primitive.
///
/// `WriterLeaseTable` is intentionally useful for one Kernel instance, but V4 A87 also requires
/// two independent processes to be unable to become writers for the same persistent namespace.
/// This type keeps one file handle exclusively locked for the lifetime of the lease. Rust's
/// `File::try_lock` maps to `flock` on Unix and `LockFileEx` on Windows, and the operating
/// system releases the lock automatically if the process dies.
///
/// The lock file itself is persistent and only carries diagnostic/fencing metadata. A stale file
/// after a crash is therefore harmless: once the dead process' OS lock disappears, the next
/// writer acquires the same file and overwrites the record while holding the exclusive lock.
#[derive(Debug)]
pub struct PersistentWriterLease {
    lease: SingleWriterLease,
    file: File,
    lock_path: PathBuf,
    released: bool,
}

impl PersistentWriterLease {
    pub fn acquire(
        root: &Path,
        namespace: StorageNamespace,
        owner: &str,
    ) -> Result<Self, WriterLeaseError> {
        std::fs::create_dir_all(root).map_err(|error| WriterLeaseError::Io {
            action: "create lock directory".into(),
            detail: error.to_string(),
        })?;
        let lock_path = persistent_lock_path(root, &namespace);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&lock_path).map_err(|error| WriterLeaseError::Io {
            action: "open lock file".into(),
            detail: error.to_string(),
        })?;

        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let owner = read_busy_owner(&lock_path).unwrap_or_else(|| "active-writer".into());
                return Err(WriterLeaseError::Busy { owner });
            }
            Err(TryLockError::Error(error)) => {
                return Err(WriterLeaseError::Io {
                    action: "acquire OS file lock".into(),
                    detail: error.to_string(),
                });
            }
        }

        let previous_epoch =
            read_locked_lease(&mut file).ok().flatten().map(|lease| lease.epoch).unwrap_or(0);
        let lease = SingleWriterLease {
            namespace,
            owner: owner.to_string(),
            epoch: previous_epoch.saturating_add(1),
            token: Uuid::new_v4().to_string(),
        };
        write_locked_lease(&mut file, &lease)?;

        Ok(Self { lease, file, lock_path, released: false })
    }

    pub fn lease(&self) -> &SingleWriterLease {
        &self.lease
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Explicit release is optional; dropping the file handle also releases the OS lock.
    pub fn release(&mut self) -> Result<(), WriterLeaseError> {
        if self.released {
            return Ok(());
        }
        self.file.unlock().map_err(|error| WriterLeaseError::Io {
            action: "release OS file lock".into(),
            detail: error.to_string(),
        })?;
        self.released = true;
        Ok(())
    }
}

impl Drop for PersistentWriterLease {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.file.unlock();
            self.released = true;
        }
    }
}

fn persistent_lock_path(root: &Path, namespace: &StorageNamespace) -> PathBuf {
    let digest = Sha256::digest(namespace.key().as_bytes());
    root.join(format!("{}.writer.lock", hex::encode(digest)))
}

fn read_busy_owner(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<SingleWriterLease>(&bytes).ok().map(|lease| lease.owner)
}

fn read_locked_lease(file: &mut File) -> Result<Option<SingleWriterLease>, WriterLeaseError> {
    file.seek(SeekFrom::Start(0)).map_err(|error| WriterLeaseError::Io {
        action: "seek lock record".into(),
        detail: error.to_string(),
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| WriterLeaseError::Io {
        action: "read lock record".into(),
        detail: error.to_string(),
    })?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    // The OS lock is the security fact. Metadata can be torn by a crashed previous writer, so a
    // malformed diagnostic record is recovered by the next exclusive owner instead of creating a
    // permanent stale-lock brick.
    Ok(serde_json::from_slice::<SingleWriterLease>(&bytes).ok())
}

fn write_locked_lease(file: &mut File, lease: &SingleWriterLease) -> Result<(), WriterLeaseError> {
    let bytes = serde_json::to_vec(lease).map_err(|error| WriterLeaseError::Io {
        action: "encode lock record".into(),
        detail: error.to_string(),
    })?;
    file.set_len(0).map_err(|error| WriterLeaseError::Io {
        action: "truncate lock record".into(),
        detail: error.to_string(),
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| WriterLeaseError::Io {
        action: "seek lock record".into(),
        detail: error.to_string(),
    })?;
    file.write_all(&bytes).map_err(|error| WriterLeaseError::Io {
        action: "write lock record".into(),
        detail: error.to_string(),
    })?;
    file.sync_data().map_err(|error| WriterLeaseError::Io {
        action: "sync lock record".into(),
        detail: error.to_string(),
    })
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
        assert!(matches!(table.acquire(ns(), "host-b"), Err(WriterLeaseError::Busy { .. })));
        table.release(&first).unwrap();
        let second = table.acquire(ns(), "host-b").unwrap();
        assert!(second.epoch > first.epoch);
        assert_eq!(table.validate(&first), Err(WriterLeaseError::Stale));
    }

    #[test]
    fn persistent_writer_lease_is_os_scoped_and_recovers_after_drop() {
        let root = tempfile::tempdir().unwrap();
        let first = PersistentWriterLease::acquire(root.path(), ns(), "host-a").unwrap();
        let first_epoch = first.lease().epoch;
        assert!(matches!(
            PersistentWriterLease::acquire(root.path(), ns(), "host-b"),
            Err(WriterLeaseError::Busy { .. })
        ));
        drop(first);

        let second = PersistentWriterLease::acquire(root.path(), ns(), "host-b").unwrap();
        assert!(second.lease().epoch > first_epoch);
        assert_eq!(second.lease().owner, "host-b");
    }
}
