// T-7 拆分：本文件是 `crates/tauron-adapter/src/lib.rs` 的 fs 命令族纯搬移落点。
// 逐字节原样搬迁、零语义改动；`mod fs;` + 按原名 `pub use` 追加在 lib.rs 文件末尾
// （不在顶部插行，把行号漂移面压到最小）。
// 子模块经 `use super::*;` 可见父 crate-root 的私有助手（guard/unsupported_body/
// require_main_window/unsupported_body 等）与再导出，故搬来的命令照常编译。
// 说明：`FS_MAX_READ_BYTES` 是 pub const（公共路径不变，经 crate-root 再导出保留），
// 三个私有助手 fs_unavailable / canonicalize_existing_prefix / scoped_within_roots
// 只在本模块内被 cmd_fs_* 使用，故不导出。

use super::*;

/// 单次 `host_fs_read` 的字节上限（4 MiB）。
pub const FS_MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

fn fs_unavailable() -> UnsupportedBody {
    unsupported_body(
        "fs provider is not configured",
        Some("在 AdapterConfig.fs_allowed_roots 中配置允许根目录（空 = 不可用）"),
    )
}

/// 规范化**已存在的最近祖先**并把剩余组件原样接回：允许对尚不存在的目标
/// （如 `host_fs_write` 的新文件）做 root 前缀判定，而不用整路径 `canonicalize`
/// 失败即拒绝。
fn canonicalize_existing_prefix(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let mut pending: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cursor = path;
    loop {
        match cursor.canonicalize() {
            Ok(mut base) => {
                for component in pending.iter().rev() {
                    base.push(component);
                }
                return Ok(base);
            }
            Err(error) => {
                let Some(parent) = cursor.parent() else {
                    return Err(error);
                };
                let Some(name) = cursor.file_name() else {
                    return Err(error);
                };
                pending.push(name);
                cursor = parent;
            }
        }
    }
}

/// V4 A95：把授权结果转换为“可信 root + portable relative path”。
///
/// 预检查只用于选择 root；真正 read/write/stat 在 Unix 通过 openat/O_NOFOLLOW
/// 沿同一 root handle 执行，因此 check/use 期间替换 symlink 仍会被拒绝。
///
/// 候选路径先按**最近的已存在祖先**规范化再比对前缀：宿主装配期的 roots 已经
/// `canonicalize`（Windows 上会带 `\\?\` verbatim 前缀），直接用原始字符串做
/// `starts_with` 会把 root 内的合法路径误判为越界。
fn scoped_within_roots(roots: &[PathBuf], raw: &str) -> HostResult<tauron_host::ScopedPath> {
    let candidate = PathBuf::from(raw);
    if !candidate.is_absolute() {
        return Err(HostError::new(
            ErrorCode::E_AUTH_DENIED,
            format!("fs 路径 `{raw}` 必须是允许 root 下的绝对路径"),
        ));
    }
    let candidate = canonicalize_existing_prefix(&candidate).map_err(|error| {
        HostError::new(ErrorCode::E_AUTH_DENIED, format!("fs 路径 `{raw}` 无法规范化：{error}"))
    })?;

    for root in roots {
        if !candidate.starts_with(root) {
            continue;
        }
        let relative = candidate.strip_prefix(root).map_err(|_| {
            HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("fs 路径 `{raw}` 无法转换为 root-relative path"),
            )
        })?;
        let relative = if relative.as_os_str().is_empty() {
            "."
        } else {
            relative.to_str().ok_or_else(|| {
                HostError::new(
                    ErrorCode::E_INVALID_MANIFEST,
                    format!("fs 路径 `{raw}` 含无法在线协议表达的非 UTF-8 组件"),
                )
            })?
        };
        return tauron_host::ScopedPath::new(root.clone(), relative).map_err(|error| {
            HostError::new(
                ErrorCode::E_AUTH_DENIED,
                format!("fs scoped path 拒绝 `{raw}`: {error}"),
            )
        });
    }

    Err(HostError::new(ErrorCode::E_AUTH_DENIED, format!("fs 路径 `{raw}` 不在宿主允许 root 内")))
}

/// `host_fs_read`：读取文本文件（允许根目录内；超限截断并如实标注）。
pub fn cmd_fs_read(
    state: &SubstrateState,
    path: &str,
    max_bytes: Option<u64>,
) -> HostResult<ProviderResult<FsReadResult>> {
    guard("fs_read", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        let limit = max_bytes.unwrap_or(FS_MAX_READ_BYTES).min(FS_MAX_READ_BYTES);
        let (bytes, truncated) = state.fs_sink.read(&scoped, limit)?;
        Ok(ProviderResult::Value(FsReadResult {
            path: scoped.display_path().to_string_lossy().into_owned(),
            text: String::from_utf8_lossy(&bytes).into_owned(),
            bytes: bytes.len() as u64,
            truncated,
        }))
    })?
}

/// `host_fs_read` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_read_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
    max_bytes: Option<u64>,
) -> HostResult<ProviderResult<FsReadResult>> {
    require_main_window(caller, "host_fs_read")?;
    cmd_fs_read(state, path, max_bytes)
}

/// `host_fs_write`：写入文本文件（覆盖；允许根目录内）。
pub fn cmd_fs_write(
    state: &SubstrateState,
    path: &str,
    text: &str,
) -> HostResult<ProviderResult<FsWriteResult>> {
    guard("fs_write", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        let bytes = state.fs_sink.write(&scoped, text.as_bytes())?;
        Ok(ProviderResult::Value(FsWriteResult {
            path: scoped.display_path().to_string_lossy().into_owned(),
            bytes,
        }))
    })?
}

/// `host_fs_write` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_write_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
    text: &str,
) -> HostResult<ProviderResult<FsWriteResult>> {
    require_main_window(caller, "host_fs_write")?;
    cmd_fs_write(state, path, text)
}

/// `host_fs_list`：列目录（允许根目录内）。
pub fn cmd_fs_list(state: &SubstrateState, path: &str) -> HostResult<ProviderResult<Vec<FsEntry>>> {
    guard("fs_list", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.list(&scoped).map(ProviderResult::Value)
    })?
}

/// `host_fs_list` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_list_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
) -> HostResult<ProviderResult<Vec<FsEntry>>> {
    require_main_window(caller, "host_fs_list")?;
    cmd_fs_list(state, path)
}

/// `host_fs_stat`：取元数据（允许根目录内）。
pub fn cmd_fs_stat(state: &SubstrateState, path: &str) -> HostResult<ProviderResult<FsStat>> {
    guard("fs_stat", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.stat(&scoped).map(ProviderResult::Value)
    })?
}

/// `host_fs_stat` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_stat_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
) -> HostResult<ProviderResult<FsStat>> {
    require_main_window(caller, "host_fs_stat")?;
    cmd_fs_stat(state, path)
}

/// `host_fs_mkdir`：建目录（允许根目录内）。
pub fn cmd_fs_mkdir(
    state: &SubstrateState,
    path: &str,
    recursive: bool,
) -> HostResult<ProviderResult<()>> {
    guard("fs_mkdir", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.mkdir(&scoped, recursive)?;
        Ok(ProviderResult::Value(()))
    })?
}

/// `host_fs_mkdir` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_mkdir_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
    recursive: bool,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_fs_mkdir")?;
    cmd_fs_mkdir(state, path, recursive)
}

/// `host_fs_remove`：删除文件或空目录（允许根目录内；**不递归**）。
pub fn cmd_fs_remove(state: &SubstrateState, path: &str) -> HostResult<ProviderResult<()>> {
    guard("fs_remove", || {
        let roots: &[PathBuf] = state.fs_allowed_roots.as_ref();
        if roots.is_empty() {
            return Ok(ProviderResult::Unsupported(fs_unavailable()));
        }
        let scoped = scoped_within_roots(roots, path)?;
        state.fs_sink.remove(&scoped)?;
        Ok(ProviderResult::Value(()))
    })?
}

/// `host_fs_remove` 的**带身份判定**版本（仅主窗）。
pub fn cmd_fs_remove_as(
    caller: &Caller,
    state: &SubstrateState,
    path: &str,
) -> HostResult<ProviderResult<()>> {
    require_main_window(caller, "host_fs_remove")?;
    cmd_fs_remove(state, path)
}
