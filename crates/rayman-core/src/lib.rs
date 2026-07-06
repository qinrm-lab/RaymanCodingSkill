pub mod agent_skill;
pub mod assets;
pub mod audit;
pub mod auxiliary;
pub mod backup;
pub mod compile;
pub mod config;
pub mod context;
pub mod control;
pub mod customer_deploy;
pub mod dependency_policy;
pub mod docs;
pub mod eval;
pub mod evidence;
pub mod execution;
pub mod feature_coverage;
pub mod gate;
pub mod goal;
pub mod instruction;
pub mod integrations;
pub mod model_catalog;
pub mod models;
pub mod project;
pub mod quality;
pub mod regression_history;
pub mod release;
pub mod research;
pub mod risk;
pub mod security;
pub mod selfcheck;
pub mod semantic;
pub mod session;
pub mod skills;
pub mod stats;
pub mod subagent;
pub mod temp;
pub mod tools;
pub mod trace;
pub mod workflow;
pub mod workspace;
pub mod yaml;

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use sha2::{Digest, Sha256};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn workspace_root() -> Result<PathBuf> {
    std::env::current_dir().context("无法读取当前工作区")
}

pub fn rayman_exe_name() -> &'static str {
    if cfg!(windows) {
        "rayman.exe"
    } else {
        "rayman"
    }
}

pub fn rayman_reminder_exe_name() -> &'static str {
    if cfg!(windows) {
        "提醒.exe"
    } else {
        "提醒"
    }
}

pub fn rayman_cli_source_binary(canonical_root: &Path) -> Result<PathBuf> {
    let release = canonical_root
        .join("target")
        .join("release")
        .join(rayman_exe_name());
    if release.exists() {
        return Ok(release);
    }
    let current = std::env::current_exe().context("无法读取当前 rayman 可执行文件路径")?;
    if current.exists() {
        return Ok(current);
    }
    bail!("未找到 rayman CLI 二进制: {}", release.display());
}

pub fn rayman_reminder_source_binary(canonical_root: &Path) -> Result<PathBuf> {
    let release = canonical_root
        .join("target")
        .join("release")
        .join(rayman_reminder_exe_name());
    if release.exists() {
        return Ok(release);
    }
    bail!("未找到提醒程序二进制: {}", release.display());
}

pub fn rayman_cli_install_target() -> Result<PathBuf> {
    if cfg!(windows) {
        let local_app_data = std::env::var_os("LOCALAPPDATA")
            .or_else(|| {
                std::env::var_os("USERPROFILE").map(|home| {
                    PathBuf::from(home)
                        .join("AppData")
                        .join("Local")
                        .into_os_string()
                })
            })
            .context("无法解析 LOCALAPPDATA 或 USERPROFILE")?;
        Ok(PathBuf::from(local_app_data)
            .join("Rayman")
            .join("bin")
            .join(rayman_exe_name()))
    } else {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("无法解析 HOME")?;
        Ok(home.join(".local").join("bin").join(rayman_exe_name()))
    }
}

pub fn rayman_reminder_install_target(cli_target: &Path) -> PathBuf {
    cli_target.with_file_name(rayman_reminder_exe_name())
}

pub fn display_path(path: &Path) -> String {
    normalize_windows_verbatim_path(&path.display().to_string())
}

pub fn normalize_windows_verbatim_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

pub fn ensure_within(path: &Path, base: &Path, message: &str) -> Result<PathBuf> {
    let base = base
        .canonicalize()
        .with_context(|| format!("无法解析基准路径: {}", base.display()))?;
    let candidate = if path.exists() {
        path.canonicalize()
            .with_context(|| format!("无法解析路径: {}", path.display()))?
    } else {
        normalize_missing_path(if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        })?
    };
    if !candidate.starts_with(&base) {
        bail!("{message}: {}", candidate.display());
    }
    Ok(candidate)
}

fn normalize_missing_path(path: PathBuf) -> Result<PathBuf> {
    let mut ancestor = path.as_path();
    let mut tail = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            bail!("无法解析路径: {}", path.display());
        };
        tail.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            bail!("无法解析父路径: {}", ancestor.display());
        };
        ancestor = parent;
    }
    let mut normalized = ancestor
        .canonicalize()
        .with_context(|| format!("无法解析父路径: {}", ancestor.display()))?;
    for component in tail.iter().rev() {
        if component == OsStr::new(".") {
            continue;
        }
        if component == OsStr::new("..") {
            normalized.pop();
        } else {
            normalized.push(component);
        }
    }
    Ok(normalized)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("无法读取文件: {}", path.display()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

pub fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("无法读取文件: {}", path.display()))
}

pub fn write_text(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }
    fs::write(path, text).with_context(|| format!("无法写入文件: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_verbatim_drive_paths_for_humans() {
        assert_eq!(
            normalize_windows_verbatim_path(r"\\?\E:\rayman\software\AI\RaymanCodingSkill"),
            r"E:\rayman\software\AI\RaymanCodingSkill"
        );
    }

    #[test]
    fn normalizes_windows_verbatim_unc_paths_for_humans() {
        assert_eq!(
            normalize_windows_verbatim_path(r"\\?\UNC\server\share\RaymanCodingSkill"),
            r"\\server\share\RaymanCodingSkill"
        );
    }

    #[test]
    fn leaves_regular_paths_unchanged() {
        assert_eq!(
            normalize_windows_verbatim_path(r"E:\rayman\software\AI\RaymanCodingSkill"),
            r"E:\rayman\software\AI\RaymanCodingSkill"
        );
    }

    #[test]
    fn ensure_within_rejects_missing_path_with_parent_escape() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("workspace");
        fs::create_dir_all(&base).unwrap();
        let escaping = base
            .join("missing")
            .join("..")
            .join("..")
            .join("outside.txt");

        let result = ensure_within(&escaping, &base, "path escaped workspace");

        assert!(result.is_err());
    }
}
