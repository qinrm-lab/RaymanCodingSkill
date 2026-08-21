//! Low-level text and JSON persistence primitives.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

use crate::pathfmt::display_path;

static ATOMIC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Identity and mutable metadata observed through an already-open file handle.
///
/// Ordinary state reads use this together with a second named open so a
/// final-component replacement cannot silently substitute different bytes
/// between validation and parsing.  The timestamps are intentionally part of
/// the identity: readiness seals must detect metadata-only touches as well as
/// content changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
    pub(crate) created: Option<SystemTime>,
    #[cfg(unix)]
    pub(crate) device: u64,
    #[cfg(unix)]
    pub(crate) inode: u64,
    #[cfg(unix)]
    pub(crate) nlink: u64,
    #[cfg(windows)]
    pub(crate) creation_time: u64,
    #[cfg(windows)]
    pub(crate) last_write_time: u64,
    #[cfg(windows)]
    pub(crate) volume_serial_number: Option<u32>,
    #[cfg(windows)]
    pub(crate) file_index: Option<u64>,
    #[cfg(windows)]
    pub(crate) volume_serial_number_64: Option<u64>,
    #[cfg(windows)]
    pub(crate) file_id_128: Option<[u8; 16]>,
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::MetadataExt as _;

    FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(unix)]
        nlink: metadata.nlink(),
        #[cfg(windows)]
        creation_time: metadata.creation_time(),
        #[cfg(windows)]
        last_write_time: metadata.last_write_time(),
        #[cfg(windows)]
        volume_serial_number: None,
        #[cfg(windows)]
        file_index: None,
        #[cfg(windows)]
        volume_serial_number_64: None,
        #[cfg(windows)]
        file_id_128: None,
    }
}

#[cfg(windows)]
pub(crate) fn file_identity_from_handle(
    file: &fs::File,
    metadata: &fs::Metadata,
    path: &Path,
    label: &str,
) -> Result<FileIdentity> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    let read = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) };
    if read == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot read strong {label} identity from handle: {}",
                display_path(path)
            )
        });
    }
    let mut identity = file_identity(metadata);
    identity.volume_serial_number = Some(information.dwVolumeSerialNumber);
    identity.file_index =
        Some((u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow));

    let mut extended = FILE_ID_INFO::default();
    let extended_read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileIdInfo,
            (&raw mut extended).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if extended_read == 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "cannot read 128-bit {label} identity from handle: {}",
                display_path(path)
            )
        });
    }
    identity.volume_serial_number_64 = Some(extended.VolumeSerialNumber);
    identity.file_id_128 = Some(extended.FileId.Identifier);
    Ok(identity)
}

#[cfg(not(windows))]
pub(crate) fn file_identity_from_handle(
    _: &fs::File,
    metadata: &fs::Metadata,
    _: &Path,
    _: &str,
) -> Result<FileIdentity> {
    Ok(file_identity(metadata))
}

pub(crate) fn has_strong_file_identity(identity: &FileIdentity) -> bool {
    #[cfg(windows)]
    {
        identity.volume_serial_number.is_some()
            && identity.file_index.is_some()
            && identity.volume_serial_number_64.is_some()
            && identity.file_id_128.is_some()
    }
    #[cfg(unix)]
    {
        let _ = identity;
        true
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = identity;
        false
    }
}

#[cfg(windows)]
pub(crate) fn open_file_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(path)
}

#[cfg(unix)]
pub(crate) fn open_file_no_follow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn open_file_no_follow(_: &Path) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "handle-bound no-follow reads are unsupported on this platform",
    ))
}

#[cfg(unix)]
pub(crate) fn read_bytes_from_handle(
    file: &fs::File,
    len: u64,
    path: &Path,
    label: &str,
) -> Result<Vec<u8>> {
    use std::os::unix::fs::FileExt as _;

    let len = usize::try_from(len).with_context(|| format!("{label} is too large to read"))?;
    let mut bytes = vec![0_u8; len];
    let mut offset = 0;
    while offset < len {
        let read = file
            .read_at(&mut bytes[offset..], offset as u64)
            .with_context(|| {
                format!(
                    "cannot read {label} from its bound handle: {}",
                    display_path(path)
                )
            })?;
        if read == 0 {
            bail!(
                "{label} handle reached an early EOF: {}",
                display_path(path)
            );
        }
        offset += read;
    }
    Ok(bytes)
}

#[cfg(windows)]
pub(crate) fn read_bytes_from_handle(
    file: &fs::File,
    len: u64,
    path: &Path,
    label: &str,
) -> Result<Vec<u8>> {
    use std::os::windows::fs::FileExt as _;

    let len = usize::try_from(len).with_context(|| format!("{label} is too large to read"))?;
    let mut bytes = vec![0_u8; len];
    let mut offset = 0;
    while offset < len {
        let read = file
            .seek_read(&mut bytes[offset..], offset as u64)
            .with_context(|| {
                format!(
                    "cannot read {label} from its bound handle: {}",
                    display_path(path)
                )
            })?;
        if read == 0 {
            bail!(
                "{label} handle reached an early EOF: {}",
                display_path(path)
            );
        }
        offset += read;
    }
    Ok(bytes)
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn read_bytes_from_handle(
    _: &fs::File,
    _: u64,
    path: &Path,
    label: &str,
) -> Result<Vec<u8>> {
    bail!(
        "handle-bound {label} reads are unsupported on this platform: {}",
        display_path(path)
    )
}

/// Read an ordinary file twice from one no-follow handle, then reopen the
/// named path without following its final component and require the same
/// strong object identity. This detects ordinary replacement and mutation
/// races; it deliberately does not claim to prevent an exact A -> B -> A cycle.
pub(crate) fn read_handle_bound_file(path: &Path, label: &str) -> Result<(Vec<u8>, FileIdentity)> {
    let file = open_file_no_follow(path).with_context(|| {
        format!(
            "cannot open {label} without following links: {}",
            display_path(path)
        )
    })?;
    read_open_handle_bound_file_with_hook(path, label, file, None, || {})
}

fn read_open_handle_bound_file_with_hook(
    path: &Path,
    label: &str,
    file: fs::File,
    maximum_bytes: Option<u64>,
    before_named_reopen: impl FnOnce(),
) -> Result<(Vec<u8>, FileIdentity)> {
    let before = file.metadata().with_context(|| {
        format!(
            "cannot read {label} handle metadata: {}",
            display_path(path)
        )
    })?;
    if is_link_or_reparse(&before) || !before.file_type().is_file() {
        bail!(
            "{label} must be an ordinary non-link file: {}",
            display_path(path)
        );
    }
    let before_identity = file_identity_from_handle(&file, &before, path, label)?;
    ensure_bounded_identity(&before_identity, maximum_bytes, path, label)?;
    if !has_strong_file_identity(&before_identity) {
        bail!(
            "{label} lacks the strong file identity required for stable reads: {}",
            display_path(path)
        );
    }

    let first = read_bytes_from_handle(&file, before_identity.len, path, label)?;
    let middle = file.metadata().with_context(|| {
        format!(
            "cannot recheck {label} handle metadata: {}",
            display_path(path)
        )
    })?;
    let middle_identity = file_identity_from_handle(&file, &middle, path, label)?;
    ensure_bounded_identity(&middle_identity, maximum_bytes, path, label)?;
    let second = read_bytes_from_handle(&file, middle_identity.len, path, label)?;
    let after = file.metadata().with_context(|| {
        format!(
            "cannot finalize {label} handle metadata: {}",
            display_path(path)
        )
    })?;
    let after_identity = file_identity_from_handle(&file, &after, path, label)?;
    ensure_bounded_identity(&after_identity, maximum_bytes, path, label)?;
    if first != second
        || before_identity != middle_identity
        || middle_identity != after_identity
        || after_identity.len != second.len() as u64
    {
        bail!(
            "{label} changed during handle-bound read: {}",
            display_path(path)
        );
    }

    before_named_reopen();
    let named_file = open_file_no_follow(path).with_context(|| {
        format!(
            "{label} path changed after handle-bound read: {}",
            display_path(path)
        )
    })?;
    let named_metadata = named_file.metadata().with_context(|| {
        format!(
            "cannot recheck named {label} handle: {}",
            display_path(path)
        )
    })?;
    if is_link_or_reparse(&named_metadata) || !named_metadata.file_type().is_file() {
        bail!(
            "{label} path became a link or non-file: {}",
            display_path(path)
        );
    }
    let named_identity = file_identity_from_handle(&named_file, &named_metadata, path, label)?;
    if named_identity != after_identity {
        bail!(
            "{label} path changed identity during handle-bound read: {}",
            display_path(path)
        );
    }
    Ok((second, after_identity))
}

fn ensure_bounded_identity(
    identity: &FileIdentity,
    maximum_bytes: Option<u64>,
    path: &Path,
    label: &str,
) -> Result<()> {
    if maximum_bytes.is_some_and(|maximum| identity.len > maximum) {
        bail!(
            "{label} exceeds the bounded read limit: {}",
            display_path(path)
        );
    }
    Ok(())
}

pub(crate) fn read_optional_handle_bound_file(
    path: &Path,
    label: &str,
) -> Result<Option<(Vec<u8>, FileIdentity)>> {
    read_optional_handle_bound_file_with_hook(path, label, || {})
}

pub(crate) fn read_optional_handle_bound_file_bounded(
    path: &Path,
    label: &str,
    maximum_bytes: u64,
) -> Result<Option<(Vec<u8>, FileIdentity)>> {
    let file = match open_file_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot open {label} without following links: {}",
                    display_path(path)
                )
            });
        }
    };
    read_open_handle_bound_file_with_hook(path, label, file, Some(maximum_bytes), || {}).map(Some)
}

fn read_optional_handle_bound_file_with_hook(
    path: &Path,
    label: &str,
    before_named_reopen: impl FnOnce(),
) -> Result<Option<(Vec<u8>, FileIdentity)>> {
    let file = match open_file_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot open {label} without following links: {}",
                    display_path(path)
                )
            });
        }
    };
    read_open_handle_bound_file_with_hook(path, label, file, None, before_named_reopen).map(Some)
}

/// 该 metadata 是否指向符号链接或 Windows reparse point。
///
/// 路径安全检查的共同前提：凡是"必须是工作区内真实文件/目录"的判定都要先排除
/// 链接，否则校验的是链接本身、写入的却是链接目标。全仓只允许这一份实现——
/// 它曾在 7 个模块里各有一份拷贝，可以互相独立漂移。
pub(crate) fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

/// 校验 `path` 是一个真实目录（不是链接/reparse point）。
///
/// `label` 只影响诊断文案。各模块保留一行同名委托以维持各自的措辞，但判定逻辑
/// 全仓只有这一份——它曾在 3 个模块里各写了一遍。
pub(crate) fn ensure_real_directory_labeled(path: &Path, label: &str) -> Result<()> {
    // display_path 而非 Path::display：Windows 上后者会漏出 `\\?\` 扩展长度前缀，
    // 收敛这三份实现时曾把 checkpoint 的诊断退化成裸路径。
    let shown = crate::pathfmt::display_path(path);
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("无法读取{label}元数据: {shown}"))?;
    if is_link_or_reparse(&metadata) {
        bail!("拒绝链接/reparse {label}: {shown}");
    }
    if !metadata.file_type().is_dir() {
        bail!("{label}不是目录: {shown}");
    }
    Ok(())
}

pub fn write_atomic(target: &Path, text: &str) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("原子写入目标没有父目录: {}", display_path(target)))?;
    // The callers that own state creation must establish their parent through
    // `state_paths` first.  Re-creating it here would follow a parent that was
    // swapped to a symlink/reparse point after that validation.
    crate::state_paths::ensure_real_directory(parent)
        .with_context(|| format!("原子写入父目录不安全或不存在: {}", display_path(parent)))?;
    let temp = create_synced_temp(target, text)?;
    // Detect a parent swap that happened while the temporary file was being
    // written before attempting to publish it.  Handle-relative no-follow I/O
    // would be needed to eliminate the remaining kernel-level TOCTOU window.
    crate::state_paths::ensure_real_directory(parent)
        .with_context(|| format!("原子发布前父目录不安全: {}", display_path(parent)))?;
    if let Err(error) = rename_with_retry(&temp, target) {
        let _ = fs::remove_file(&temp);
        bail!(
            "无法原子替换文件 {} -> {}: {error}",
            display_path(temp.as_ref()),
            display_path(target)
        );
    }
    sync_parent_directory(parent)?;
    Ok(())
}

/// Atomic copy variant used by checkpoint restore.  The staged sibling must
/// match the manifest before it is published, so a source mutation during the
/// copy cannot replace a good workspace file with unverified bytes.
pub fn copy_atomic_verified(
    source: &Path,
    target: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<u64> {
    copy_atomic_impl(source, target, expected_size, expected_sha256)
}

fn copy_atomic_impl(
    source: &Path,
    target: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<u64> {
    const MAX_TEMP_NAME_ATTEMPTS: usize = 32;
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("原子复制目标没有父目录: {}", display_path(target)))?;
    crate::state_paths::ensure_real_directory(parent)
        .with_context(|| format!("原子复制父目录不安全或不存在: {}", display_path(parent)))?;
    let permissions = fs::metadata(source)
        .with_context(|| format!("无法读取原子复制源元数据: {}", display_path(source)))?
        .permissions();

    for _ in 0..MAX_TEMP_NAME_ATTEMPTS {
        let temp = atomic_temp_path(target);
        let mut output = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法创建原子复制临时文件: {}", display_path(temp.as_ref()))
                });
            }
        };
        let copied = (|| -> Result<u64> {
            let mut input = fs::File::open(source)
                .with_context(|| format!("无法读取原子复制源: {}", display_path(source)))?;
            let copied = io::copy(&mut input, &mut output)
                .with_context(|| format!("无法复制到临时文件: {}", display_path(temp.as_ref())))?;
            output
                .set_permissions(permissions.clone())
                .with_context(|| format!("无法保留复制权限: {}", display_path(temp.as_ref())))?;
            output.sync_all().with_context(|| {
                format!("无法同步原子复制临时文件: {}", display_path(temp.as_ref()))
            })?;
            Ok(copied)
        })();
        drop(output);
        let copied = match copied {
            Ok(copied) => copied,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error);
            }
        };
        let staged_hash = match crate::hash::sha256_file(&temp) {
            Ok(hash) => hash,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error).with_context(|| {
                    format!("无法校验原子复制临时文件: {}", display_path(temp.as_ref()))
                });
            }
        };
        if copied != expected_size || staged_hash != expected_sha256 {
            let _ = fs::remove_file(&temp);
            bail!(
                "原子复制临时文件完整性不匹配: {} (size {} != {} or sha256 {} != {})",
                display_path(temp.as_ref()),
                copied,
                expected_size,
                staged_hash,
                expected_sha256
            );
        }
        crate::state_paths::ensure_real_directory(parent)
            .with_context(|| format!("原子复制发布前父目录不安全: {}", display_path(parent)))?;
        if let Err(error) = rename_with_retry(&temp, target) {
            let _ = fs::remove_file(&temp);
            bail!(
                "无法原子替换复制目标 {} -> {}: {error}",
                display_path(temp.as_ref()),
                display_path(target)
            );
        }
        sync_parent_directory(parent)?;
        return Ok(copied);
    }
    bail!(
        "无法为原子复制创建独占临时文件（连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在）: {}",
        display_path(target)
    )
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("无法同步父目录: {}", display_path(parent)))
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path) -> Result<()> {
    Ok(())
}

/// Create the temporary sibling exclusively.  A predictable old temporary name
/// must never be opened with truncation: an untrusted pre-existing file (or
/// symlink) is not ours to overwrite.  Retrying also makes a stale temp from a
/// previous crashed process a harmless availability issue rather than a write
/// target.
fn create_synced_temp(target: &Path, text: &str) -> Result<PathBuf> {
    const MAX_TEMP_NAME_ATTEMPTS: usize = 32;
    for _ in 0..MAX_TEMP_NAME_ATTEMPTS {
        let temp = atomic_temp_path(target);
        match write_synced(&temp, text) {
            Ok(()) => return Ok(temp),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                let _ = fs::remove_file(&temp);
                return Err(error)
                    .with_context(|| format!("无法写入临时文件: {}", display_path(temp.as_ref())));
            }
        }
    }
    bail!(
        "无法为原子写入创建独占临时文件（连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在）: {}",
        display_path(target)
    )
}

/// 写入并 fsync：断电时 rename 元数据可能先于数据落盘，不 sync 目标会变成空文件。
fn write_synced(path: &Path, text: &str) -> io::Result<()> {
    // `create_new` maps to O_EXCL / CREATE_NEW.  It refuses a pre-existing
    // symlink instead of following it, which keeps temporary-file creation from
    // escaping the verified parent directory.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// 杀软/索引器可能短暂持有目标文件，Windows 上 rename 会瞬时报拒绝访问/共享冲突。
fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut delay = Duration::from_millis(10);
    for _ in 0..5 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if is_transient_share_conflict(&error) => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(error) => return Err(error),
        }
    }
    fs::rename(from, to)
}

fn is_transient_share_conflict(error: &io::Error) -> bool {
    // 5 = ERROR_ACCESS_DENIED, 32 = ERROR_SHARING_VIOLATION
    error.kind() == io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(32)
}

fn atomic_temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("rayman");
    let counter = ATOMIC_COUNTER.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(
        ".{name}.rayman-{}-{counter}.tmp",
        std::process::id()
    ))
}

pub fn write_json<T: serde::Serialize>(target: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("无法序列化 JSON")?;
    write_atomic(target, &text)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    // 只有"不存在"才算无状态；权限/IO 错误必须报错，否则调用方会当首次运行并覆盖原数据。
    let Some((bytes, _identity)) = read_optional_handle_bound_file(path, "JSON file")? else {
        return Ok(None);
    };
    let value = serde_json::from_slice(&bytes)
        .with_context(|| format!("无法解析 JSON: {}", display_path(path)))?;
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_without_deleting_first() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.json");
        write_atomic(&target, "one").unwrap();
        write_atomic(&target, "two").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "two");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("rayman-"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件残留: {leftovers:?}");
    }

    #[test]
    fn read_json_missing_is_none_corrupt_is_err() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        assert!(read_json::<serde_json::Value>(&path).unwrap().is_none());
        write_atomic(&path, "{ not json").unwrap();
        assert!(read_json::<serde_json::Value>(&path).is_err());
    }

    #[test]
    fn optional_read_reports_deletion_after_initial_open_as_drift() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("optional.json");
        fs::write(&path, br#"{"ready":true}"#).unwrap();

        let error = read_optional_handle_bound_file_with_hook(&path, "optional JSON file", || {
            fs::remove_file(&path).unwrap();
        })
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("optional JSON file path changed after handle-bound read"),
            "error={error:#}"
        );
    }

    #[test]
    fn write_atomic_does_not_create_an_unverified_parent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("missing").join("state.json");
        assert!(write_atomic(&target, "state").is_err());
        assert!(!target.parent().unwrap().exists());
    }

    #[test]
    fn verified_atomic_copy_replaces_complete_contents_without_temp_leaks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let target = dir.path().join("target.bin");
        fs::write(&source, [0_u8, 1, 2, 3, 255]).unwrap();
        fs::write(&target, b"old").unwrap();

        let expected_hash = crate::hash::sha256_file(&source).unwrap();
        assert_eq!(
            copy_atomic_verified(&source, &target, 5, &expected_hash).unwrap(),
            5
        );
        assert_eq!(fs::read(&target).unwrap(), [0_u8, 1, 2, 3, 255]);
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("rayman-"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件残留: {leftovers:?}");
    }

    #[test]
    fn verified_atomic_copy_preserves_target_on_integrity_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let target = dir.path().join("target.bin");
        fs::write(&source, b"new").unwrap();
        fs::write(&target, b"old").unwrap();

        assert!(copy_atomic_verified(&source, &target, 3, &"0".repeat(64)).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
    }

    #[test]
    fn temporary_write_refuses_to_truncate_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join("existing.tmp");
        fs::write(&temp, "preserve").unwrap();

        let error = write_synced(&temp, "new contents").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&temp).unwrap(), "preserve");
    }

    #[cfg(unix)]
    #[test]
    fn temporary_write_refuses_to_follow_a_preexisting_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.txt");
        fs::write(&outside_file, "preserve").unwrap();
        let temp = dir.path().join("existing.tmp");
        symlink(&outside_file, &temp).unwrap();

        assert!(write_synced(&temp, "new contents").is_err());
        assert_eq!(fs::read_to_string(&outside_file).unwrap(), "preserve");
    }
}
