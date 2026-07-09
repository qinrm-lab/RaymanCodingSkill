//! Low-level text and JSON persistence primitives.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
    fs::write(&temp, text).with_context(|| format!("无法写入临时文件: {}", temp.display()))?;
    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        bail!(
            "无法原子替换文件 {} -> {}: {error}",
            temp.display(),
            target.display()
        );
    }
    Ok(())
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
    if !path.exists() {
        return Ok(None);
    }
    let text = read_text(path)?;
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
