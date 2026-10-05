//! V4 portable path validation (A85) and scoped filesystem path contract (A95).
//!
//! Validation is deliberately lexical and platform-neutral. A provider must still open files
//! relative to a pre-opened root/handle with no-follow semantics; this module prevents portable
//! manifests from smuggling absolute paths, parent traversal, drive prefixes or NULs.
//!
//! 轮 42（A85 五维补齐）：除既有形状检查外，逐条校验还覆盖目标平台语义——
//! Windows 保留设备名（`CON`/`NUL`/`COM1`… 含带扩展名形式）、Unicode **NFC 归一**
//! （非 NFC 形式拒绝，`é` 的两种字节写法不再能各占一个条目）、尾点/尾空格
//! （Windows 落盘时被剥离，制造隐形同名片）、便携长度上界（段 ≤ 255 字节，
//! 整条相对路径 ≤ 1023 字节——macOS `PATH_MAX` 是主流平台里最紧的界）。
//! 集合级冲突（同一条目集里 ASCII 大小写折叠后碰撞）由
//! [`validate_portable_entry_set`] 判定：Windows/macOS 默认文件系统大小写不敏感，
//! `A.txt` 与 `a.txt` 必须先拒，而不是等解压到一半才按平台随机失败。

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

/// Windows 设备名保留表：段的主名（第一个 `.` 之前）命中即拒——`NUL.txt` 在
/// Windows 上打开的是设备而不是文件。`COM1`–`COM9` / `LPT1`–`LPT9` 为经典集合。
const WINDOWS_RESERVED_STEMS: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// POSIX `NAME_MAX`：单个段跨平台可移植的字节上界。
const MAX_SEGMENT_BYTES: usize = 255;
/// 整条相对路径的便携上界：macOS `PATH_MAX` 为 1024（含 NUL），取 1023。更深的
/// 布局是宿主私有选择，不属于包清单的可移植承诺。
const MAX_RELATIVE_BYTES: usize = 1023;

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
    #[error("Windows reserved device name is forbidden")]
    ReservedName,
    #[error("path must already be in Unicode NFC form")]
    NotNfc,
    #[error("trailing dot or space is forbidden")]
    TrailingDotOrSpace,
    #[error("path exceeds portable segment/total length limits")]
    TooLong,
    #[error("entries collide after ASCII case folding")]
    CaseCollision,
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

    // ── 五维（轮 42）────────────────────────────────────────────────────
    // 按 `/` 与 `\` 两侧切分：`std::path` 的方言随宿主平台变化（Unix 不把 `\` 当分隔符），
    // 而便携契约要求同一输入在任何宿主上给出同一判定。`.` / `..` 段由上面的
    // components 检查负责（各自平台语义），这里跳过。
    if path.len() > MAX_RELATIVE_BYTES {
        return Err(PortablePathError::TooLong);
    }
    let nfc = icu_normalizer::ComposingNormalizerBorrowed::new_nfc();
    if nfc.normalize(path).as_ref() != path {
        return Err(PortablePathError::NotNfc);
    }
    for segment in path.split(['/', '\\']) {
        if segment == "." || segment == ".." || segment.is_empty() {
            continue;
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return Err(PortablePathError::TrailingDotOrSpace);
        }
        let stem = segment.split('.').next().unwrap_or(segment);
        if WINDOWS_RESERVED_STEMS.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
            return Err(PortablePathError::ReservedName);
        }
        if segment.len() > MAX_SEGMENT_BYTES {
            return Err(PortablePathError::TooLong);
        }
    }

    Ok(p.to_path_buf())
}

/// 轮 42（A85）：包级条目集校验——逐条过 [`validate_portable_relative`]，再做
/// ASCII 大小写折叠冲突检测。Windows/macOS 的默认文件系统大小写不敏感，条目集里的
/// `A.txt` 与 `a.txt` 会在解压/落盘期互相覆盖，且先后取决于平台——必须在签名/解析
/// 阶段整体拒绝，而不是接受一半再报销。**边界成文**：折叠只做 ASCII（Unicode 全表
/// 折叠需要额外数据表）；NFC 归一已由逐条校验保证，非 ASCII 的大小写变体不在本
/// 谓词的覆盖范围。
pub fn validate_portable_entry_set<S: AsRef<str>>(entries: &[S]) -> Result<(), PortablePathError> {
    let mut folded = HashSet::with_capacity(entries.len());
    for entry in entries {
        validate_portable_relative(entry.as_ref())?;
        if !folded.insert(entry.as_ref().to_ascii_lowercase()) {
            return Err(PortablePathError::CaseCollision);
        }
    }
    Ok(())
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

    #[test]
    fn rejects_windows_reserved_device_names_in_any_case_or_extension() {
        for bad in ["NUL", "nul", "CON.txt", "com1.bin", "AUX", "lpt9.tar", "dir/PRN"] {
            assert_eq!(
                validate_portable_relative(bad),
                Err(PortablePathError::ReservedName),
                "{bad}"
            );
        }
        // 前缀包含保留名的普通词不是设备名；COM0/COM10 不在经典保留集合里。
        for ok in ["console.json", "conx", "com0", "com10", "null.md"] {
            assert!(validate_portable_relative(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_trailing_dot_or_space_that_windows_would_strip() {
        for bad in ["a.", "a ", "dir/b. ", "x. /y"] {
            assert_eq!(
                validate_portable_relative(bad),
                Err(PortablePathError::TrailingDotOrSpace),
                "{bad}"
            );
        }
        assert!(validate_portable_relative("a. b/c").is_ok());
    }

    #[test]
    fn rejects_non_nfc_forms_but_accepts_the_precomposed_one() {
        // e + U+0301 组合重音 vs 预组合 é：前者在 macOS 上会以不同字节落盘。
        assert_eq!(validate_portable_relative("e\u{0301}.txt"), Err(PortablePathError::NotNfc));
        assert!(validate_portable_relative("\u{e9}.txt").is_ok(), "预组合 é 是 NFC");
        assert!(validate_portable_relative("中文/图标.png").is_ok(), "CJK 本身是 NFC");
    }

    #[test]
    fn rejects_segments_or_paths_over_the_portable_length_bounds() {
        let long_segment = "a".repeat(MAX_SEGMENT_BYTES + 1);
        assert_eq!(validate_portable_relative(&long_segment), Err(PortablePathError::TooLong));
        assert!(validate_portable_relative(&"a".repeat(MAX_SEGMENT_BYTES)).is_ok());

        let deep = vec!["dir"; 400].join("/");
        assert!(deep.len() > MAX_RELATIVE_BYTES);
        assert_eq!(validate_portable_relative(&deep), Err(PortablePathError::TooLong));
    }

    #[test]
    fn entry_set_rejects_ascii_case_collisions_and_duplicates() {
        assert_eq!(
            validate_portable_entry_set(&["A.txt".to_string(), "a.txt".to_string()]),
            Err(PortablePathError::CaseCollision)
        );
        assert_eq!(
            validate_portable_entry_set(&["dir/x".to_string(), "dir/x".to_string()]),
            Err(PortablePathError::CaseCollision),
            "完全重复是折叠碰撞的退化情形"
        );
        assert!(validate_portable_entry_set(&["A.txt".to_string(), "b.txt".to_string()]).is_ok());
        // 集合级校验同样执行逐条五维：保留名条目在集合入口就被拒。
        assert_eq!(
            validate_portable_entry_set(&["NUL.txt".to_string(), "ok.txt".to_string()]),
            Err(PortablePathError::ReservedName)
        );
    }
}
