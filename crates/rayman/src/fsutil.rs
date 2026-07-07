//! 文件系统与持久化基础设施：原子写、路径安全、哈希、时间戳。
//! 所有状态写入都走 `write_atomic`（同目录临时文件 + rename），杜绝写到一半崩溃留下的截断文件。

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};

static ATOMIC_COUNTER: AtomicU64 = AtomicU64::new(0);

/// RFC3339 微秒 UTC 时间戳。
pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// 读取文本文件。
pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("无法读取文件: {}", path.display()))
}

/// 原子写入：同目录临时文件 + rename，保证同卷、崩溃安全地替换目标。
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

/// 序列化为漂亮打印的 JSON 并原子写入。
pub fn write_json<T: serde::Serialize>(target: &Path, value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value).context("无法序列化 JSON")?;
    write_atomic(target, &text)
}

/// 读取并解析 JSON；文件缺失时返回 `Ok(None)`，损坏时返回 `Err`（调用方决定是否降级）。
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = read_text(path)?;
    let value = serde_json::from_str(&text)
        .with_context(|| format!("无法解析 JSON: {}", path.display()))?;
    Ok(Some(value))
}

/// 文件内容的 SHA-256（流式读取，不整体载入超大文件）。
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("无法打开文件: {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("无法读取文件: {}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// 把 Windows verbatim 前缀（`\\?\`）规整为人类可读路径。
pub fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        text
    }
}

/// 校验 `path` 归一化后仍在 `base` 内，防止越权写/删。
/// 相对路径一律先按 `base` 解析（不再吃进程 CWD），修复原实现的 CWD 依赖问题。
pub fn ensure_within(path: &Path, base: &Path, message: &str) -> Result<PathBuf> {
    let base = base
        .canonicalize()
        .with_context(|| format!("无法解析基准路径: {}", base.display()))?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let candidate = if absolute.exists() {
        absolute
            .canonicalize()
            .with_context(|| format!("无法解析路径: {}", absolute.display()))?
    } else {
        normalize_missing(&base, &absolute)?
    };
    if !candidate.starts_with(&base) {
        bail!("{message}: {}", candidate.display());
    }
    Ok(candidate)
}

fn normalize_missing(base: &Path, path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            bail!("无法解析路径: {}", path.display());
        };
        tail.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            bail!("无法解析父路径: {}", path.display());
        };
        ancestor = parent;
    }
    let mut normalized = ancestor
        .canonicalize()
        .with_context(|| format!("无法解析父路径: {}", ancestor.display()))?;
    for component in tail.iter().rev() {
        if component == OsStr::new(".") {
            continue;
        } else if component == OsStr::new("..") {
            normalized.pop();
        } else {
            normalized.push(component);
        }
    }
    // 保底：missing 的正常路径也应落在 base 下。
    let _ = base;
    Ok(normalized)
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
        // 临时文件不残留。
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
    fn ensure_within_resolves_relative_against_base_not_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        // 相对路径按 base 解析。
        let ok = ensure_within(Path::new("sub/file.txt"), &base, "escaped").unwrap();
        assert!(ok.starts_with(&base));
        // 越权被拒。
        assert!(ensure_within(Path::new("../outside.txt"), &base, "escaped").is_err());
    }
}
