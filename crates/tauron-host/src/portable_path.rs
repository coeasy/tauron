//! V4 portable path validation (A85) and scoped filesystem path contract (A95).
//!
//! Validation is deliberately lexical and platform-neutral. A provider must still open files
//! relative to a pre-opened root/handle with no-follow semantics; this module prevents portable
//! manifests from smuggling absolute paths, parent traversal, drive prefixes or NULs.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PortablePathError {
    #[error("path is empty")]
    Empty,
    #[error("absolute/rooted paths are forbidden")]
    Absolute,
    #[error("path traversal is forbidden")]
    Traversal,
    #[error("platform prefix/drive path is forbidden")]
    Prefix,
    #[error("NUL byte is forbidden in paths")]
    Nul,
}

pub fn validate_portable_relative(path: &str) -> Result<PathBuf, PortablePathError> {
    if path.is_empty() {
        return Err(PortablePathError::Empty);
    }
    if path.contains('\0') {
        return Err(PortablePathError::Nul);
    }
    // Reject Windows drive/UNC forms before std::path interprets them on a Unix host.
    let bytes = path.as_bytes();
    if path.starts_with("\\")
        || path.starts_with("//")
        || (bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic())
    {
        return Err(PortablePathError::Prefix);
    }

    let p = Path::new(path);
    if p.is_absolute() {
        return Err(PortablePathError::Absolute);
    }
    for component in p.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Err(PortablePathError::Traversal),
            Component::RootDir => return Err(PortablePathError::Absolute),
            Component::Prefix(_) => return Err(PortablePathError::Prefix),
        }
    }
    Ok(p.to_path_buf())
}

/// Join only after the relative path has passed the portable contract. The result is not a
/// substitute for a no-follow/openat provider; it is useful for package-layout validation.
pub fn join_scoped(root: &Path, relative: &str) -> Result<PathBuf, PortablePathError> {
    Ok(root.join(validate_portable_relative(relative)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_portable_paths() {
        assert_eq!(
            validate_portable_relative("assets/icon.png").unwrap(),
            PathBuf::from("assets/icon.png")
        );
    }

    #[test]
    fn rejects_escape_and_platform_specific_absolute_forms() {
        for bad in [
            "../secret",
            "a/../../secret",
            "/etc/passwd",
            r"C:\Windows\system.ini",
            r"\\server\share",
            "//server/share",
        ] {
            assert!(validate_portable_relative(bad).is_err(), "{bad}");
        }
    }
}
