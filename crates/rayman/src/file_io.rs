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
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }
    let temp = atomic_temp_path(target);
    if let Err(error) = write_synced(&temp, text) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("无法写入临时文件: {}", temp.display()));
    }
    if let Err(error) = rename_with_retry(&temp, target) {
        let _ = fs::remove_file(&temp);
        bail!(
            "无法原子替换文件 {} -> {}: {error}",
            temp.display(),
            target.display()
        );
    }
    Ok(())
}

/// 写入并 fsync：断电时 rename 元数据可能先于数据落盘，不 sync 目标会变成空文件。
fn write_synced(path: &Path, text: &str) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
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
}
