use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::ensure_within;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub allow_network: bool,
    pub require_workspace_cwd: bool,
    pub reject_shell_operators: bool,
    #[serde(default)]
    pub allowed_env: Vec<String>,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            allow_network: false,
            require_workspace_cwd: true,
            reject_shell_operators: true,
            allowed_env: Vec::new(),
        }
    }
}

impl ExecutionPolicy {
    pub fn validate_command(
        &self,
        argv: &[String],
        cwd: Option<&Path>,
        workspace: &Path,
    ) -> Result<()> {
        if argv.is_empty() || argv[0].trim().is_empty() {
            bail!("execution command argv must not be empty");
        }
        if self.require_workspace_cwd {
            let cwd = cwd.unwrap_or(workspace);
            ensure_within(cwd, workspace, "execution cwd must stay inside workspace")?;
        }
        if self.reject_shell_operators {
            for arg in argv {
                if contains_shell_operator(arg) {
                    bail!("execution policy rejected shell operator in argv: {arg}");
                }
            }
        }
        if !self.allow_network && command_looks_networked(argv) {
            bail!("execution policy rejected network command while allow_network=false");
        }
        for arg in argv {
            if looks_like_env_secret_assignment(arg)
                && !self
                    .allowed_env
                    .iter()
                    .any(|allowed| arg.starts_with(&format!("{allowed}=")))
            {
                bail!("execution policy rejected undeclared secret env argument: {arg}");
            }
        }
        Ok(())
    }
}

pub fn command_cwd(cwd: Option<&str>, workspace: &Path) -> Result<PathBuf> {
    let cwd = cwd
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.to_path_buf());
    ensure_within(&cwd, workspace, "execution cwd must stay inside workspace")
}

fn contains_shell_operator(arg: &str) -> bool {
    arg.chars()
        .any(|ch| matches!(ch, '|' | '&' | ';' | '<' | '>' | '`'))
}

fn command_looks_networked(argv: &[String]) -> bool {
    let command = argv
        .first()
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let command_name = Path::new(&command)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(&command);
    matches!(
        command_name,
        "curl" | "wget" | "aria2c" | "bitsadmin" | "ssh" | "scp" | "ftp" | "powershell" | "pwsh"
    ) || argv.iter().any(|arg| {
        let arg = arg.to_ascii_lowercase();
        arg.starts_with("http://")
            || arg.starts_with("https://")
            || arg == "invoke-webrequest"
            || arg == "iwr"
            || arg == "invoke-restmethod"
    })
}

fn looks_like_env_secret_assignment(arg: &str) -> bool {
    let Some((key, value)) = arg.split_once('=') else {
        return false;
    };
    !value.is_empty()
        && ["KEY", "TOKEN", "SECRET", "PASSWORD"]
            .iter()
            .any(|marker| key.to_ascii_uppercase().contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_denies_network_commands() {
        let temp = tempfile::tempdir().unwrap();
        let error = ExecutionPolicy::default()
            .validate_command(
                &["curl".into(), "https://example.com".into()],
                None,
                temp.path(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("allow_network=false"));
    }

    #[test]
    fn default_policy_denies_workspace_escape_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let error = ExecutionPolicy::default()
            .validate_command(
                &["cargo".into(), "--version".into()],
                Some(outside.path()),
                temp.path(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("inside workspace"));
    }

    #[test]
    fn default_policy_denies_undeclared_secret_env_arg() {
        let temp = tempfile::tempdir().unwrap();
        let error = ExecutionPolicy::default()
            .validate_command(
                &["cmd".into(), "OPENAI_API_KEY=secret".into()],
                None,
                temp.path(),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("secret env"));
    }
}
