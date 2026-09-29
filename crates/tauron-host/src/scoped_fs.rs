//! V4 scoped filesystem capability model (A95).
//!
//! The old adapter path was:
//!   canonicalize -> starts_with(root) -> std::fs open
//! which leaves a check/use race. This module introduces a root-scoped handle model.
//!
//! Unix hard enforcement walks every component with openat + O_NOFOLLOW and opens the final
//! object relative to the already-open parent directory. Windows remains explicitly Partial until
//! the platform adapter validates reparse/junction targets through native handles.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::portable_path::{validate_portable_relative, PortablePathError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FsEnforcement {
    Hard,
    Partial,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedPath {
    root: PathBuf,
    relative: PathBuf,
}

impl ScopedPath {
    pub fn new(root: impl Into<PathBuf>, relative: &str) -> Result<Self, ScopedFsError> {
        let root = root.into();
        if !root.is_absolute() {
            return Err(ScopedFsError::RootNotAbsolute(root));
        }
        let relative = validate_portable_relative(relative).map_err(ScopedFsError::PortablePath)?;
        if relative.components().all(|c| matches!(c, Component::CurDir)) {
            return Err(ScopedFsError::EmptyRelative);
        }
        Ok(Self { root, relative })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn relative(&self) -> &Path {
        &self.relative
    }

    pub fn display_path(&self) -> PathBuf {
        self.root.join(&self.relative)
    }
}

#[derive(Debug, Error)]
pub enum ScopedFsError {
    #[error("scoped filesystem root must be absolute: {0}")]
    RootNotAbsolute(PathBuf),
    #[error("scoped relative path is empty")]
    EmptyRelative,
    #[error("portable path rejected: {0}")]
    PortablePath(PortablePathError),
    #[error("scoped filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("path component is not supported by scoped provider: {0}")]
    InvalidComponent(String),
    #[error("platform does not provide hard scoped filesystem enforcement")]
    HardEnforcementUnavailable,
}

pub const fn platform_enforcement() -> FsEnforcement {
    #[cfg(unix)]
    {
        FsEnforcement::Hard
    }
    #[cfg(windows)]
    {
        FsEnforcement::Partial
    }
    #[cfg(not(any(unix, windows)))]
    {
        FsEnforcement::Unsupported
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::OwnedFd;
    use std::path::{Component, Path};

    use rustix::fs::{openat, Mode, OFlags};

    use super::{ScopedFsError, ScopedPath};

    fn open_root(root: &Path) -> Result<File, ScopedFsError> {
        let file = File::open(root)?;
        if !file.metadata()?.is_dir() {
            return Err(ScopedFsError::InvalidComponent(root.display().to_string()));
        }
        Ok(file)
    }

    fn normal_components(path: &Path) -> Result<Vec<&OsStr>, ScopedFsError> {
        let mut out = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(name) => out.push(name),
                Component::CurDir => {}
                other => {
                    return Err(ScopedFsError::InvalidComponent(
                        other.as_os_str().to_string_lossy().into_owned(),
                    ));
                }
            }
        }
        if out.is_empty() {
            return Err(ScopedFsError::EmptyRelative);
        }
        Ok(out)
    }

    fn open_parent(path: &ScopedPath) -> Result<(OwnedFd, &OsStr), ScopedFsError> {
        let parts = normal_components(path.relative())?;
        let root = open_root(path.root())?;
        let mut dir: OwnedFd = rustix::io::dup(&root).map_err(std::io::Error::from)?;
        for component in &parts[..parts.len() - 1] {
            dir = openat(
                &dir,
                *component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
        }
        Ok((dir, parts[parts.len() - 1]))
    }

    pub fn read(path: &ScopedPath, max_bytes: u64) -> Result<(Vec<u8>, bool), ScopedFsError> {
        let (parent, name) = open_parent(path)?;
        let fd = openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let mut file = File::from(fd);
        let mut bytes = Vec::new();
        let mut limited = std::io::Read::by_ref(&mut file).take(max_bytes.saturating_add(1));
        limited.read_to_end(&mut bytes)?;
        let truncated = bytes.len() as u64 > max_bytes;
        if truncated {
            bytes.truncate(max_bytes as usize);
        }
        Ok((bytes, truncated))
    }

    pub fn write(path: &ScopedPath, bytes: &[u8]) -> Result<u64, ScopedFsError> {
        let (parent, name) = open_parent(path)?;
        let fd = openat(
            &parent,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::TRUNC | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(std::io::Error::from)?;
        let mut file = File::from(fd);
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(bytes.len() as u64)
    }

    pub fn stat(path: &ScopedPath) -> Result<std::fs::Metadata, ScopedFsError> {
        let (parent, name) = open_parent(path)?;
        let fd = openat(
            &parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let file = File::from(fd);
        Ok(file.metadata()?)
    }
}

#[cfg(unix)]
pub fn read_hard(path: &ScopedPath, max_bytes: u64) -> Result<(Vec<u8>, bool), ScopedFsError> {
    unix::read(path, max_bytes)
}

#[cfg(unix)]
pub fn write_hard(path: &ScopedPath, bytes: &[u8]) -> Result<u64, ScopedFsError> {
    unix::write(path, bytes)
}

#[cfg(unix)]
pub fn stat_hard(path: &ScopedPath) -> Result<std::fs::Metadata, ScopedFsError> {
    unix::stat(path)
}

#[cfg(not(unix))]
pub fn read_hard(_path: &ScopedPath, _max_bytes: u64) -> Result<(Vec<u8>, bool), ScopedFsError> {
    Err(ScopedFsError::HardEnforcementUnavailable)
}

#[cfg(not(unix))]
pub fn write_hard(_path: &ScopedPath, _bytes: &[u8]) -> Result<u64, ScopedFsError> {
    Err(ScopedFsError::HardEnforcementUnavailable)
}

#[cfg(not(unix))]
pub fn stat_hard(_path: &ScopedPath) -> Result<std::fs::Metadata, ScopedFsError> {
    Err(ScopedFsError::HardEnforcementUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_path_requires_absolute_root_and_portable_relative_path() {
        assert!(ScopedPath::new("relative-root", "a.txt").is_err());
        let root = if cfg!(windows) { PathBuf::from(r"C:\safe") } else { PathBuf::from("/safe") };
        assert!(ScopedPath::new(root.clone(), "../escape").is_err());
        assert!(ScopedPath::new(root, "nested/file.txt").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn hard_read_rejects_symlink_final_component() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"secret").unwrap();
        symlink(outside.path().join("secret.txt"), root.path().join("link.txt")).unwrap();

        let scoped = ScopedPath::new(root.path().to_path_buf(), "link.txt").unwrap();
        assert!(read_hard(&scoped, 1024).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hard_read_rejects_symlink_intermediate_component() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("dir")).unwrap();
        std::fs::write(outside.path().join("dir/secret.txt"), b"secret").unwrap();
        symlink(outside.path().join("dir"), root.path().join("jump")).unwrap();

        let scoped = ScopedPath::new(root.path().to_path_buf(), "jump/secret.txt").unwrap();
        assert!(read_hard(&scoped, 1024).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hard_write_and_read_round_trip_inside_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("nested")).unwrap();
        let scoped = ScopedPath::new(root.path().to_path_buf(), "nested/data.txt").unwrap();
        assert_eq!(write_hard(&scoped, b"hello").unwrap(), 5);
        let (bytes, truncated) = read_hard(&scoped, 32).unwrap();
        assert_eq!(bytes, b"hello");
        assert!(!truncated);
    }
}
