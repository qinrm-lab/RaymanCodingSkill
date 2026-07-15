//! Low-level text and JSON persistence primitives.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};

static ATOMIC_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("无法读取文件: {}", path.display()))
}

pub fn write_atomic(target: &Path, text: &str) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("原子写入目标没有父目录: {}", target.display()))?;
    // The callers that own state creation must establish their parent through
    // `state_paths` first.  Re-creating it here would follow a parent that was
    // swapped to a symlink/reparse point after that validation.
    crate::state_paths::ensure_real_directory(parent)
        .with_context(|| format!("原子写入父目录不安全或不存在: {}", parent.display()))?;
    let temp = create_synced_temp(target, text)?;
    // Detect a parent swap that happened while the temporary file was being
    // written before attempting to publish it.  Handle-relative no-follow I/O
    // would be needed to eliminate the remaining kernel-level TOCTOU window.
    crate::state_paths::ensure_real_directory(parent)
        .with_context(|| format!("原子发布前父目录不安全: {}", parent.display()))?;
    if let Err(error) = rename_with_retry(&temp, target) {
        let _ = fs::remove_file(&temp);
        bail!(
            "无法原子替换文件 {} -> {}: {error}",
            temp.display(),
            target.display()
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
        .ok_or_else(|| anyhow::anyhow!("原子复制目标没有父目录: {}", target.display()))?;
    crate::state_paths::ensure_real_directory(parent)
        .with_context(|| format!("原子复制父目录不安全或不存在: {}", parent.display()))?;
    let permissions = fs::metadata(source)
        .with_context(|| format!("无法读取原子复制源元数据: {}", source.display()))?
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
                return Err(error)
                    .with_context(|| format!("无法创建原子复制临时文件: {}", temp.display()));
            }
        };
        let copied = (|| -> Result<u64> {
            let mut input = fs::File::open(source)
                .with_context(|| format!("无法读取原子复制源: {}", source.display()))?;
            let copied = io::copy(&mut input, &mut output)
                .with_context(|| format!("无法复制到临时文件: {}", temp.display()))?;
            output
                .set_permissions(permissions.clone())
                .with_context(|| format!("无法保留复制权限: {}", temp.display()))?;
            output
                .sync_all()
                .with_context(|| format!("无法同步原子复制临时文件: {}", temp.display()))?;
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
                return Err(error)
                    .with_context(|| format!("无法校验原子复制临时文件: {}", temp.display()));
            }
        };
        if copied != expected_size || staged_hash != expected_sha256 {
            let _ = fs::remove_file(&temp);
            bail!(
                "原子复制临时文件完整性不匹配: {} (size {} != {} or sha256 {} != {})",
                temp.display(),
                copied,
                expected_size,
                staged_hash,
                expected_sha256
            );
        }
        crate::state_paths::ensure_real_directory(parent)
            .with_context(|| format!("原子复制发布前父目录不安全: {}", parent.display()))?;
        if let Err(error) = rename_with_retry(&temp, target) {
            let _ = fs::remove_file(&temp);
            bail!(
                "无法原子替换复制目标 {} -> {}: {error}",
                temp.display(),
                target.display()
            );
        }
        sync_parent_directory(parent)?;
        return Ok(copied);
    }
    bail!(
        "无法为原子复制创建独占临时文件（连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在）: {}",
        target.display()
    )
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path) -> Result<()> {
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("无法同步父目录: {}", parent.display()))
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
                return Err(error).with_context(|| format!("无法写入临时文件: {}", temp.display()));
            }
        }
    }
    bail!(
        "无法为原子写入创建独占临时文件（连续 {MAX_TEMP_NAME_ATTEMPTS} 个名称已存在）: {}",
        target.display()
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
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取文件: {}", path.display()));
        }
    };
    let value = serde_json::from_str(&text)
        .with_context(|| format!("无法解析 JSON: {}", path.display()))?;
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
        assert_eq!(read_text(&target).unwrap(), "two");
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
