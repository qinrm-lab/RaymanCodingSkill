use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    display_path, now_iso, rayman_cli_install_target, rayman_cli_source_binary,
    rayman_reminder_install_target, rayman_reminder_source_binary, sha256_file,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelfStatus {
    pub generated_at: String,
    pub workspace_path: String,
    pub current_exe: String,
    pub current_exe_sha256: Option<String>,
    pub release_binary: String,
    pub release_binary_sha256: Option<String>,
    pub install_source: String,
    pub install_source_sha256: Option<String>,
    pub install_target: String,
    pub install_target_sha256: Option<String>,
    pub installed_matches_source: bool,
    pub reminder_source: String,
    pub reminder_source_sha256: Option<String>,
    pub reminder_target: String,
    pub reminder_target_sha256: Option<String>,
    pub reminder_installed_matches_source: bool,
    pub source_skill: String,
    pub source_skill_sha256: Option<String>,
    pub status: String,
}

pub struct SelfManager {
    workspace: PathBuf,
}

impl SelfManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            workspace: workspace
                .into()
                .canonicalize()
                .context("无法解析工作区路径")?,
        })
    }

    pub fn status(&self) -> Result<SelfStatus> {
        let current_exe = std::env::current_exe().context("无法读取当前可执行文件路径")?;
        let release_binary = self
            .workspace
            .join("target")
            .join("release")
            .join(crate::rayman_exe_name());
        let reminder_release_binary = self
            .workspace
            .join("target")
            .join("release")
            .join(crate::rayman_reminder_exe_name());
        let install_source = rayman_cli_source_binary(&self.workspace)?;
        let install_target = rayman_cli_install_target()?;
        let reminder_source = rayman_reminder_source_binary(&self.workspace)
            .unwrap_or_else(|_| reminder_release_binary.clone());
        let reminder_target = rayman_reminder_install_target(&install_target);
        let skill = self.workspace.join("SKILL.md");
        let current_hash = file_hash(&current_exe)?;
        let source_hash = file_hash(&install_source)?;
        let install_hash = file_hash(&install_target)?;
        let reminder_source_hash = file_hash(&reminder_source)?;
        let reminder_target_hash = file_hash(&reminder_target)?;
        let installed_matches_source = source_hash.is_some() && source_hash == install_hash;
        let reminder_installed_matches_source =
            reminder_source_hash.is_some() && reminder_source_hash == reminder_target_hash;
        let status = if (installed_matches_source || !install_target.exists())
            && (reminder_installed_matches_source || !reminder_target.exists())
        {
            "ready"
        } else {
            "stale"
        };
        Ok(SelfStatus {
            generated_at: now_iso(),
            workspace_path: display_path(&self.workspace),
            current_exe: display_path(&current_exe),
            current_exe_sha256: current_hash,
            release_binary: display_path(&release_binary),
            release_binary_sha256: file_hash(&release_binary)?,
            install_source: display_path(&install_source),
            install_source_sha256: source_hash,
            install_target: display_path(&install_target),
            install_target_sha256: install_hash,
            installed_matches_source,
            reminder_source: display_path(&reminder_source),
            reminder_source_sha256: reminder_source_hash,
            reminder_target: display_path(&reminder_target),
            reminder_target_sha256: reminder_target_hash,
            reminder_installed_matches_source,
            source_skill: display_path(&skill),
            source_skill_sha256: file_hash(&skill)?,
            status: status.into(),
        })
    }

    pub fn install(&self) -> Result<SelfStatus> {
        let install_source = rayman_cli_source_binary(&self.workspace)?;
        let install_target = rayman_cli_install_target()?;
        if let Some(parent) = install_target.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建安装目录: {}", parent.display()))?;
        }
        fs::copy(&install_source, &install_target).with_context(|| {
            format!(
                "无法安装 {} 到 {}",
                install_source.display(),
                install_target.display()
            )
        })?;
        let reminder_source = rayman_reminder_source_binary(&self.workspace)?;
        let reminder_target = rayman_reminder_install_target(&install_target);
        fs::copy(&reminder_source, &reminder_target).with_context(|| {
            format!(
                "无法安装 {} 到 {}",
                reminder_source.display(),
                reminder_target.display()
            )
        })?;
        self.status()
    }
}

fn file_hash(path: &Path) -> Result<Option<String>> {
    path.exists().then(|| sha256_file(path)).transpose()
}
