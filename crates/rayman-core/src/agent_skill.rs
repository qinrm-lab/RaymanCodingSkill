use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::{
    display_path, rayman_cli_install_target, rayman_cli_source_binary,
    rayman_reminder_install_target, rayman_reminder_source_binary, sha256_file,
};

const SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE: &str =
    "references/skill-subagent-auto-start-authorization.md";

#[derive(Debug, Clone, Serialize)]
pub struct AgentSkillResult {
    pub agent: String,
    pub path: PathBuf,
    pub exists: bool,
    pub sha256: Option<String>,
    pub error: Option<String>,
}

impl AgentSkillResult {
    pub fn ok(&self) -> bool {
        self.exists && self.error.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct AgentSkillInstallManager {
    pub canonical_root: PathBuf,
    pub skill_name: String,
    cli_install_target: Option<PathBuf>,
    codex_home: Option<PathBuf>,
    claude_home: Option<PathBuf>,
}

impl AgentSkillInstallManager {
    pub fn new(canonical_root: impl Into<PathBuf>) -> Result<Self> {
        let canonical_root = canonical_root
            .into()
            .canonicalize()
            .context("无法解析规范仓库根目录")?;
        Ok(Self {
            canonical_root,
            skill_name: "raymancodingskill".into(),
            cli_install_target: None,
            codex_home: None,
            claude_home: None,
        })
    }

    pub fn sync(&self, targets: &[String]) -> Result<Vec<AgentSkillResult>> {
        let content = self.render_entry()?;
        let mut results = Vec::new();
        for (agent, path) in self.resolve_targets(targets)? {
            match write_entry(&path, &content) {
                Ok(()) => results.push(AgentSkillResult {
                    agent,
                    sha256: sha256_file(&path).ok(),
                    exists: true,
                    path,
                    error: None,
                }),
                Err(error) => {
                    let exists = path.exists();
                    results.push(AgentSkillResult {
                        agent,
                        path,
                        exists,
                        sha256: None,
                        error: Some(error.to_string()),
                    });
                }
            }
        }
        results.push(self.sync_cli_binary());
        results.push(self.sync_reminder_binary());
        Ok(results)
    }

    pub fn status(&self, targets: &[String]) -> Result<Vec<AgentSkillResult>> {
        let mut results = self
            .resolve_targets(targets)?
            .into_iter()
            .map(|(agent, path)| AgentSkillResult {
                agent,
                exists: path.exists(),
                sha256: if path.exists() {
                    sha256_file(&path).ok()
                } else {
                    None
                },
                path,
                error: None,
            })
            .collect::<Vec<_>>();
        results.push(self.cli_binary_status());
        results.push(self.reminder_binary_status());
        Ok(results)
    }

    pub fn with_cli_install_target(mut self, target: impl Into<PathBuf>) -> Self {
        self.cli_install_target = Some(target.into());
        self
    }

    pub fn with_agent_homes(
        mut self,
        codex_home: impl Into<PathBuf>,
        claude_home: impl Into<PathBuf>,
    ) -> Self {
        self.codex_home = Some(codex_home.into());
        self.claude_home = Some(claude_home.into());
        self
    }

    pub fn render_entry(&self) -> Result<String> {
        let skill = self.canonical_root.join("SKILL.md");
        let cargo = self.canonical_root.join("Cargo.toml");
        let subagent_authorization = self
            .canonical_root
            .join(SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE);
        if !skill.exists() {
            bail!("未找到规范 SKILL.md: {}", skill.display());
        }
        if !cargo.exists() {
            bail!("未找到 Rust workspace Cargo.toml: {}", cargo.display());
        }
        if !subagent_authorization.exists() {
            bail!(
                "未找到 host subagent 自动授权规则: {} ({})",
                SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE,
                subagent_authorization.display()
            );
        }
        let cli = rayman_cli_source_binary(&self.canonical_root)?;
        Ok(format!(
            r#"---
name: {name}
description: Use RaymanCodingSkill for programming workflows in Codex or Claude Code when explicitly invoked or when a workspace opts in through `.RaymanCodingSkill/workspace_skill.yaml`. Delegates to the canonical Rust implementation at `{root}` and its `rayman` CLI. Covers code generation, review, obsolete-code pruning, refactoring, tests, docs sync, regression/conflict/instruction governance, backups, model routing, customer-project scope guards, and session pending-work continuity.
metadata:
  short-description: RaymanCodingSkill Rust coding workflow
---

# RaymanCodingSkill Installed Entry

Read and follow the latest canonical skill file:

`{skill}`

## Host Subagent Auto-Start Authorization

Before deciding whether to spawn Codex host subagents, read and follow this standalone authorization file:

`{subagent_authorization_reference}`

Resolved path:

`{subagent_authorization}`

Use the canonical CLI when needed:

`rayman <command>`

If `rayman` reports an unknown current command, use the canonical release binary directly and run `agent-skill sync` to refresh the installed CLI:

`{cli} <command>`

`rayman agent-skill sync` also installs the companion `提醒` background reminder next to `rayman`, so goal/session stop notifications run without a prompt-token step.

If the canonical path is unavailable, report that the installed entry points to a missing RaymanCodingSkill source before making project changes.

Workspace opt-in is controlled only by `.RaymanCodingSkill/workspace_skill.yaml` with `enabled: true`. Workspace opt-out with `enabled: false` must be honored unless the user explicitly asks for one-off RaymanCodingSkill use.
"#,
            name = self.skill_name,
            root = display_path(&self.canonical_root),
            skill = display_path(&skill),
            subagent_authorization_reference = SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE,
            subagent_authorization = display_path(&subagent_authorization),
            cli = display_path(&cli)
        ))
    }

    fn cli_target(&self) -> Result<PathBuf> {
        self.cli_install_target
            .clone()
            .map(Ok)
            .unwrap_or_else(rayman_cli_install_target)
    }

    fn sync_cli_binary(&self) -> AgentSkillResult {
        let target = match self.cli_target() {
            Ok(target) => target,
            Err(error) => {
                return AgentSkillResult {
                    agent: "rayman-cli".into(),
                    path: PathBuf::new(),
                    exists: false,
                    sha256: None,
                    error: Some(error.to_string()),
                };
            }
        };
        match install_cli_binary(&self.canonical_root, &target) {
            Ok(()) => AgentSkillResult {
                agent: "rayman-cli".into(),
                sha256: sha256_file(&target).ok(),
                exists: true,
                path: target,
                error: None,
            },
            Err(error) => AgentSkillResult {
                agent: "rayman-cli".into(),
                exists: target.exists(),
                path: target,
                sha256: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn sync_reminder_binary(&self) -> AgentSkillResult {
        let target = match self.cli_target() {
            Ok(target) => rayman_reminder_install_target(&target),
            Err(error) => {
                return AgentSkillResult {
                    agent: "rayman-reminder".into(),
                    path: PathBuf::new(),
                    exists: false,
                    sha256: None,
                    error: Some(error.to_string()),
                };
            }
        };
        match install_reminder_binary(&self.canonical_root, &target) {
            Ok(()) => AgentSkillResult {
                agent: "rayman-reminder".into(),
                sha256: sha256_file(&target).ok(),
                exists: true,
                path: target,
                error: None,
            },
            Err(error) => AgentSkillResult {
                agent: "rayman-reminder".into(),
                exists: target.exists(),
                path: target,
                sha256: None,
                error: Some(error.to_string()),
            },
        }
    }

    fn cli_binary_status(&self) -> AgentSkillResult {
        let target = match self.cli_target() {
            Ok(target) => target,
            Err(error) => {
                return AgentSkillResult {
                    agent: "rayman-cli".into(),
                    path: PathBuf::new(),
                    exists: false,
                    sha256: None,
                    error: Some(error.to_string()),
                };
            }
        };
        let source_hash = rayman_cli_source_binary(&self.canonical_root)
            .and_then(|source| sha256_file(&source))
            .ok();
        let target_hash = if target.exists() {
            sha256_file(&target).ok()
        } else {
            None
        };
        let error = if target.exists() && source_hash.is_some() && source_hash != target_hash {
            Some("installed rayman CLI is stale; run rayman agent-skill sync".into())
        } else {
            None
        };
        AgentSkillResult {
            agent: "rayman-cli".into(),
            exists: target.exists(),
            path: target,
            sha256: target_hash,
            error,
        }
    }

    fn reminder_binary_status(&self) -> AgentSkillResult {
        let target = match self.cli_target() {
            Ok(target) => rayman_reminder_install_target(&target),
            Err(error) => {
                return AgentSkillResult {
                    agent: "rayman-reminder".into(),
                    path: PathBuf::new(),
                    exists: false,
                    sha256: None,
                    error: Some(error.to_string()),
                };
            }
        };
        let source_hash = rayman_reminder_source_binary(&self.canonical_root)
            .and_then(|source| sha256_file(&source))
            .ok();
        let target_hash = if target.exists() {
            sha256_file(&target).ok()
        } else {
            None
        };
        let error = if source_hash.is_none() {
            Some("canonical reminder binary is missing; build release target `提醒`".into())
        } else if target.exists() && source_hash != target_hash {
            Some("installed reminder binary is stale; run rayman agent-skill sync".into())
        } else {
            None
        };
        AgentSkillResult {
            agent: "rayman-reminder".into(),
            exists: target.exists(),
            path: target,
            sha256: target_hash,
            error,
        }
    }

    fn resolve_targets(&self, requested: &[String]) -> Result<Vec<(String, PathBuf)>> {
        let requested = if requested.is_empty() {
            vec!["all".to_string()]
        } else {
            requested.to_vec()
        };
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .context("无法解析用户主目录")?;
        let codex_home = self
            .codex_home
            .clone()
            .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".codex"));
        let claude_home = self
            .claude_home
            .clone()
            .or_else(|| std::env::var_os("CLAUDE_HOME").map(PathBuf::from))
            .unwrap_or_else(|| home.join(".claude"));
        let mut out = Vec::new();
        for target in requested {
            match target.as_str() {
                "all" => {
                    push_unique(
                        &mut out,
                        "codex",
                        codex_home
                            .join("skills")
                            .join(&self.skill_name)
                            .join("SKILL.md"),
                    );
                    push_unique(
                        &mut out,
                        "claude",
                        claude_home
                            .join("skills")
                            .join(&self.skill_name)
                            .join("SKILL.md"),
                    );
                }
                "codex" => push_unique(
                    &mut out,
                    "codex",
                    codex_home
                        .join("skills")
                        .join(&self.skill_name)
                        .join("SKILL.md"),
                ),
                "claude" | "claude-code" => push_unique(
                    &mut out,
                    "claude",
                    claude_home
                        .join("skills")
                        .join(&self.skill_name)
                        .join("SKILL.md"),
                ),
                other => bail!("未知 agent skill 目标: {other}"),
            }
        }
        Ok(out)
    }
}

fn push_unique(out: &mut Vec<(String, PathBuf)>, agent: &str, path: PathBuf) {
    if !out.iter().any(|(existing, _)| existing == agent) {
        out.push((agent.to_string(), path));
    }
}

fn write_entry(path: &PathBuf, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn install_cli_binary(canonical_root: &std::path::Path, target: &std::path::Path) -> Result<()> {
    let source = rayman_cli_source_binary(canonical_root)?;
    install_binary(&source, target, "rayman CLI")
}

fn install_reminder_binary(
    canonical_root: &std::path::Path,
    target: &std::path::Path,
) -> Result<()> {
    let source = rayman_reminder_source_binary(canonical_root)?;
    install_binary(&source, target, "提醒程序")
}

fn install_binary(source: &std::path::Path, target: &std::path::Path, label: &str) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建 {label} 安装目录: {}", parent.display()))?;
    }
    fs::copy(source, target).with_context(|| {
        format!(
            "无法同步 {label}: {} -> {}",
            source.display(),
            target.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_minimal_root(root: &std::path::Path) -> Result<()> {
        fs::create_dir_all(root.join("target").join("release"))?;
        fs::create_dir_all(root.join("references"))?;
        fs::write(root.join("Cargo.toml"), "[workspace]\n")?;
        fs::write(root.join("SKILL.md"), "# skill\n")?;
        fs::write(
            root.join(SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE),
            "# Host Subagent Auto-Start Authorization\n",
        )?;
        Ok(())
    }

    fn write_release_cli(root: &std::path::Path) -> Result<PathBuf> {
        let source = root.join("target").join("release").join(if cfg!(windows) {
            "rayman.exe"
        } else {
            "rayman"
        });
        let mut source_file = fs::File::create(&source)?;
        source_file.write_all(b"new-cli")?;
        Ok(source)
    }

    #[test]
    fn sync_installs_agent_entries_and_cli_binary_together() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("RaymanCodingSkill");
        write_minimal_root(&root)?;
        write_release_cli(&root)?;
        let reminder_source = root
            .join("target")
            .join("release")
            .join(crate::rayman_reminder_exe_name());
        fs::write(&reminder_source, b"new-reminder")?;

        let cli_target = temp.path().join("install").join("rayman-test");
        let manager = AgentSkillInstallManager::new(&root)?
            .with_cli_install_target(&cli_target)
            .with_agent_homes(
                temp.path().join("codex-home"),
                temp.path().join("claude-home"),
            );
        let results = manager.sync(&["codex".into()])?;

        assert!(results.iter().any(|result| result.agent == "codex"));
        assert!(results.iter().any(|result| result.agent == "rayman-cli"));
        assert!(
            results
                .iter()
                .any(|result| result.agent == "rayman-reminder")
        );
        assert_eq!(fs::read(&cli_target)?, b"new-cli");
        assert_eq!(
            fs::read(rayman_reminder_install_target(&cli_target))?,
            b"new-reminder"
        );
        assert!(results.iter().all(AgentSkillResult::ok));
        Ok(())
    }

    #[test]
    fn rendered_entry_points_to_release_cli_fallback() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("RaymanCodingSkill");
        write_minimal_root(&root)?;
        write_release_cli(&root)?;

        let entry = AgentSkillInstallManager::new(&root)?.render_entry()?;

        assert!(entry.contains("unknown current command"));
        assert!(entry.contains("target"));
        assert!(entry.contains("release"));
        assert!(entry.contains("agent-skill sync"));
        assert!(entry.contains("提醒"));
        Ok(())
    }

    #[test]
    fn rendered_entry_points_to_subagent_auto_start_authorization() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("RaymanCodingSkill");
        write_minimal_root(&root)?;
        write_release_cli(&root)?;

        let entry = AgentSkillInstallManager::new(&root)?.render_entry()?;

        assert!(entry.contains("Host Subagent Auto-Start Authorization"));
        assert!(entry.contains(SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE));
        assert!(entry.contains("Before deciding whether to spawn Codex host subagents"));
        Ok(())
    }

    #[test]
    fn render_entry_fails_without_subagent_auto_start_authorization() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("RaymanCodingSkill");
        fs::create_dir_all(root.join("target").join("release"))?;
        fs::write(root.join("Cargo.toml"), "[workspace]\n")?;
        fs::write(root.join("SKILL.md"), "# skill\n")?;

        let error = AgentSkillInstallManager::new(&root)?
            .render_entry()
            .unwrap_err()
            .to_string();

        assert!(error.contains("host subagent 自动授权规则"));
        assert!(error.contains(SUBAGENT_AUTO_START_AUTHORIZATION_REFERENCE));
        Ok(())
    }
}
