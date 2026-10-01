//! V4 scoped filesystem capability model (A95).
//!
//! The old adapter path was:
//!   canonicalize -> starts_with(root) -> std::fs open
//! which leaves a check/use race. This module introduces a root-scoped handle model.
//!
//! Unix hard enforcement walks every component with openat + O_NOFOLLOW and opens the final
//! object relative to the already-open parent directory. Windows remains explicitly Partial until
//! the platform adapter validates reparse/junction targets through native handles.

use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedDirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
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

    use rustix::fs::{mkdirat, openat, statat, unlinkat, AtFlags, Dir, FileType, Mode, OFlags};
    use rustix::io::Errno;

    use super::{ScopedDirEntry, ScopedFsError, ScopedPath};

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
        Ok(out)
    }

    fn root_fd(path: &ScopedPath) -> Result<OwnedFd, ScopedFsError> {
        let root = open_root(path.root())?;
        rustix::io::dup(&root).map_err(std::io::Error::from).map_err(Into::into)
    }

    fn open_target_dir(path: &ScopedPath) -> Result<OwnedFd, ScopedFsError> {
        let parts = normal_components(path.relative())?;
        let mut dir = root_fd(path)?;
        for component in parts {
            dir = openat(
                &dir,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?;
        }
        Ok(dir)
    }

    fn open_parent(path: &ScopedPath) -> Result<(OwnedFd, &OsStr), ScopedFsError> {
        let parts = normal_components(path.relative())?;
        if parts.is_empty() {
            return Err(ScopedFsError::EmptyRelative);
        }
        let mut dir = root_fd(path)?;
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

    pub fn list(path: &ScopedPath) -> Result<Vec<ScopedDirEntry>, ScopedFsError> {
        let fd = open_target_dir(path)?;
        let mut dir = Dir::new(fd).map_err(std::io::Error::from)?;
        let mut out = Vec::new();
        while let Some(entry) = dir.read() {
            let entry = entry.map_err(std::io::Error::from)?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let dirfd = dir.fd().map_err(std::io::Error::from)?;
            let stat =
                statat(dirfd, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
            let kind = FileType::from_raw_mode(stat.st_mode);
            out.push(ScopedDirEntry {
                name: String::from_utf8_lossy(name.to_bytes()).into_owned(),
                is_dir: kind.is_dir(),
                size: if kind.is_file() { stat.st_size.max(0) as u64 } else { 0 },
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn mkdir(path: &ScopedPath, recursive: bool) -> Result<(), ScopedFsError> {
        let parts = normal_components(path.relative())?;
        if parts.is_empty() {
            return Err(ScopedFsError::EmptyRelative);
        }

        if !recursive {
            let (parent, name) = open_parent(path)?;
            mkdirat(&parent, name, Mode::from_bits_truncate(0o700))
                .map_err(std::io::Error::from)?;
            return Ok(());
        }

        let mut dir = root_fd(path)?;
        for component in parts {
            match openat(
                &dir,
                component,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(next) => dir = next,
                Err(error) if error == Errno::NOENT => {
                    mkdirat(&dir, component, Mode::from_bits_truncate(0o700))
                        .map_err(std::io::Error::from)?;
                    dir = openat(
                        &dir,
                        component,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(std::io::Error::from)?;
                }
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        Ok(())
    }

    pub fn remove(path: &ScopedPath) -> Result<(), ScopedFsError> {
        let (parent, name) = open_parent(path)?;
        let stat =
            statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(std::io::Error::from)?;
        let kind = FileType::from_raw_mode(stat.st_mode);
        let flags = if kind.is_dir() { AtFlags::REMOVEDIR } else { AtFlags::empty() };
        unlinkat(&parent, name, flags).map_err(std::io::Error::from)?;
        Ok(())
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

#[cfg(unix)]
pub fn list_hard(path: &ScopedPath) -> Result<Vec<ScopedDirEntry>, ScopedFsError> {
    unix::list(path)
}

#[cfg(unix)]
pub fn mkdir_hard(path: &ScopedPath, recursive: bool) -> Result<(), ScopedFsError> {
    unix::mkdir(path, recursive)
}

#[cfg(unix)]
pub fn remove_hard(path: &ScopedPath) -> Result<(), ScopedFsError> {
    unix::remove(path)
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

#[cfg(not(unix))]
pub fn list_hard(_path: &ScopedPath) -> Result<Vec<ScopedDirEntry>, ScopedFsError> {
    Err(ScopedFsError::HardEnforcementUnavailable)
}

#[cfg(not(unix))]
pub fn mkdir_hard(_path: &ScopedPath, _recursive: bool) -> Result<(), ScopedFsError> {
    Err(ScopedFsError::HardEnforcementUnavailable)
}

#[cfg(not(unix))]
pub fn remove_hard(_path: &ScopedPath) -> Result<(), ScopedFsError> {
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
    fn hard_list_mkdir_remove_stay_beneath_root() {
        let root = tempfile::tempdir().unwrap();
        let nested = ScopedPath::new(root.path().to_path_buf(), "a/b").unwrap();
        mkdir_hard(&nested, true).unwrap();

        let file = ScopedPath::new(root.path().to_path_buf(), "a/b/data.txt").unwrap();
        write_hard(&file, b"x").unwrap();

        let dir = ScopedPath::new(root.path().to_path_buf(), "a/b").unwrap();
        let entries = list_hard(&dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "data.txt");
        assert_eq!(entries[0].size, 1);

        remove_hard(&file).unwrap();
        remove_hard(&nested).unwrap();
        let parent = ScopedPath::new(root.path().to_path_buf(), "a").unwrap();
        remove_hard(&parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn root_itself_can_be_listed_without_path_reinterpretation() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("root.txt"), b"x").unwrap();
        let scoped = ScopedPath::new(root.path().to_path_buf(), ".").unwrap();
        let entries = list_hard(&scoped).unwrap();
        assert!(entries.iter().any(|entry| entry.name == "root.txt"));
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
