//! Path containment checks for destructive or state-restoring operations.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

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
    let _ = base;
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_within_resolves_relative_against_base_not_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let ok = ensure_within(Path::new("sub/file.txt"), &base, "escaped").unwrap();
        assert!(ok.starts_with(&base));
        assert!(ensure_within(Path::new("../outside.txt"), &base, "escaped").is_err());
    }
}
