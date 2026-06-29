use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{display_path, now_iso, sha256_file};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyPolicyReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub failure_kind: Option<String>,
    pub tool_version: Option<String>,
    pub config_path: String,
    pub config_hash: Option<String>,
    pub command: String,
    pub exit_code: Option<i32>,
    pub summary: String,
    pub stdout_tail: Option<String>,
    pub stderr_tail: Option<String>,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyPolicyCommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyPolicyRunError {
    ToolMissing(String),
    Io(String),
}

pub trait DependencyPolicyRunner: Send + Sync {
    fn run(
        &self,
        workspace: &Path,
        args: &[&str],
    ) -> std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError>;
}

#[derive(Debug, Default)]
pub struct CargoDenyRunner;

impl DependencyPolicyRunner for CargoDenyRunner {
    fn run(
        &self,
        workspace: &Path,
        args: &[&str],
    ) -> std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError> {
        let output = Command::new("cargo")
            .args(args)
            .current_dir(workspace)
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    DependencyPolicyRunError::ToolMissing(
                        "cargo is not available on PATH".to_string(),
                    )
                } else {
                    DependencyPolicyRunError::Io(error.to_string())
                }
            })?;
        Ok(DependencyPolicyCommandOutput {
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

pub struct DependencyPolicyManager {
    workspace: PathBuf,
    runner: Arc<dyn DependencyPolicyRunner>,
}

impl DependencyPolicyManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Self::with_runner(workspace, default_dependency_runner())
    }

    pub fn with_runner(
        workspace: impl Into<PathBuf>,
        runner: Arc<dyn DependencyPolicyRunner>,
    ) -> Result<Self> {
        Ok(Self {
            workspace: workspace
                .into()
                .canonicalize()
                .context("无法解析工作区路径")?,
            runner,
        })
    }

    pub fn audit(&self) -> Result<DependencyPolicyReport> {
        let config_path = self.workspace.join("deny.toml");
        let command = "cargo deny check".to_string();
        let config_hash = config_path
            .exists()
            .then(|| sha256_file(&config_path))
            .transpose()?;

        if !self.workspace.join("Cargo.toml").exists() {
            return Ok(self.report(DependencyPolicyReportInput {
                status: "not_applicable",
                failure_kind: None,
                tool_version: None,
                config_path,
                config_hash,
                command,
                exit_code: None,
                summary: "workspace has no Cargo.toml; cargo-deny policy is not applicable".into(),
                stdout_tail: None,
                stderr_tail: None,
                required_actions: Vec::new(),
            }));
        }

        if !config_path.exists() {
            return Ok(self.report(DependencyPolicyReportInput {
                status: "blocked",
                failure_kind: Some("config_missing"),
                tool_version: None,
                config_path,
                config_hash,
                command,
                exit_code: None,
                summary: "deny.toml is missing".into(),
                stdout_tail: None,
                stderr_tail: None,
                required_actions: vec![
                    "add a conservative deny.toml and run cargo deny check".into(),
                ],
            }));
        }

        let version = match self.runner.run(&self.workspace, &["deny", "--version"]) {
            Ok(output) if output.success => first_nonempty_line(&output.stdout)
                .or_else(|| first_nonempty_line(&output.stderr))
                .or_else(|| Some("cargo-deny version unavailable".into())),
            Ok(output) if looks_like_missing_cargo_deny(&output) => {
                return Ok(self.tool_missing_report(config_path, config_hash, command, output));
            }
            Ok(output) => {
                return Ok(self.command_failed_report(DependencyPolicyFailureInput {
                    failure_kind: Some("tool_error"),
                    tool_version: None,
                    config_path,
                    config_hash,
                    command: "cargo deny --version".into(),
                    output,
                    summary: "cargo deny --version failed",
                }));
            }
            Err(DependencyPolicyRunError::ToolMissing(message)) => {
                return Ok(self.tool_missing_error_report(
                    config_path,
                    config_hash,
                    command,
                    message,
                ));
            }
            Err(DependencyPolicyRunError::Io(message)) => {
                return Ok(self.io_error_report(config_path, config_hash, command, message));
            }
        };

        match self.runner.run(&self.workspace, &["deny", "check"]) {
            Ok(output) if output.success => Ok(self.report(DependencyPolicyReportInput {
                status: "passed",
                failure_kind: None,
                tool_version: version,
                config_path,
                config_hash,
                command,
                exit_code: output.exit_code,
                summary: "cargo-deny dependency policy passed".into(),
                stdout_tail: tail(&output.stdout),
                stderr_tail: tail(&output.stderr),
                required_actions: Vec::new(),
            })),
            Ok(output) if looks_like_missing_cargo_deny(&output) => {
                Ok(self.tool_missing_report(config_path, config_hash, command, output))
            }
            Ok(output) => Ok(self.command_failed_report(DependencyPolicyFailureInput {
                failure_kind: Some("check_failed"),
                tool_version: version,
                config_path,
                config_hash,
                command,
                output,
                summary: "cargo-deny dependency policy failed",
            })),
            Err(DependencyPolicyRunError::ToolMissing(message)) => {
                Ok(self.tool_missing_error_report(config_path, config_hash, command, message))
            }
            Err(DependencyPolicyRunError::Io(message)) => {
                Ok(self.io_error_report(config_path, config_hash, command, message))
            }
        }
    }

    fn report(&self, input: DependencyPolicyReportInput) -> DependencyPolicyReport {
        DependencyPolicyReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: input.status.into(),
            failure_kind: input.failure_kind.map(str::to_string),
            tool_version: input.tool_version,
            config_path: display_path(&input.config_path),
            config_hash: input.config_hash,
            command: input.command,
            exit_code: input.exit_code,
            summary: input.summary,
            stdout_tail: input.stdout_tail,
            stderr_tail: input.stderr_tail,
            required_actions: input.required_actions,
        }
    }

    fn tool_missing_report(
        &self,
        config_path: PathBuf,
        config_hash: Option<String>,
        command: String,
        output: DependencyPolicyCommandOutput,
    ) -> DependencyPolicyReport {
        self.report(DependencyPolicyReportInput {
            status: "blocked",
            failure_kind: Some("tool_missing"),
            tool_version: None,
            config_path,
            config_hash,
            command,
            exit_code: output.exit_code,
            summary: "cargo-deny is not installed".into(),
            stdout_tail: tail(&output.stdout),
            stderr_tail: tail(&output.stderr),
            required_actions: vec![
                "install cargo-deny with cargo install cargo-deny and rerun cargo deny check"
                    .into(),
            ],
        })
    }

    fn tool_missing_error_report(
        &self,
        config_path: PathBuf,
        config_hash: Option<String>,
        command: String,
        message: String,
    ) -> DependencyPolicyReport {
        self.report(DependencyPolicyReportInput {
            status: "blocked",
            failure_kind: Some("tool_missing"),
            tool_version: None,
            config_path,
            config_hash,
            command,
            exit_code: None,
            summary: format!("cargo-deny is not available: {message}"),
            stdout_tail: None,
            stderr_tail: None,
            required_actions: vec![
                "install cargo-deny with cargo install cargo-deny and rerun cargo deny check"
                    .into(),
            ],
        })
    }

    fn io_error_report(
        &self,
        config_path: PathBuf,
        config_hash: Option<String>,
        command: String,
        message: String,
    ) -> DependencyPolicyReport {
        self.report(DependencyPolicyReportInput {
            status: "blocked",
            failure_kind: Some("tool_error"),
            tool_version: None,
            config_path,
            config_hash,
            command,
            exit_code: None,
            summary: format!("failed to run cargo-deny: {message}"),
            stdout_tail: None,
            stderr_tail: None,
            required_actions: vec!["fix cargo-deny execution and rerun cargo deny check".into()],
        })
    }

    fn command_failed_report(&self, input: DependencyPolicyFailureInput) -> DependencyPolicyReport {
        self.report(DependencyPolicyReportInput {
            status: "blocked",
            failure_kind: input.failure_kind,
            tool_version: input.tool_version,
            config_path: input.config_path,
            config_hash: input.config_hash,
            command: input.command,
            exit_code: input.output.exit_code,
            summary: input.summary.into(),
            stdout_tail: tail(&input.output.stdout),
            stderr_tail: tail(&input.output.stderr),
            required_actions: vec![
                "resolve cargo-deny advisories, license, bans, or source findings".into(),
            ],
        })
    }
}

pub(crate) fn default_dependency_runner() -> Arc<dyn DependencyPolicyRunner> {
    #[cfg(test)]
    {
        if test_dependency_policy_forced_passed() {
            return Arc::new(TestPassingCargoDenyRunner);
        }
    }
    Arc::new(CargoDenyRunner)
}

struct DependencyPolicyReportInput {
    status: &'static str,
    failure_kind: Option<&'static str>,
    tool_version: Option<String>,
    config_path: PathBuf,
    config_hash: Option<String>,
    command: String,
    exit_code: Option<i32>,
    summary: String,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
    required_actions: Vec<String>,
}

struct DependencyPolicyFailureInput {
    failure_kind: Option<&'static str>,
    tool_version: Option<String>,
    config_path: PathBuf,
    config_hash: Option<String>,
    command: String,
    output: DependencyPolicyCommandOutput,
    summary: &'static str,
}

fn first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn looks_like_missing_cargo_deny(output: &DependencyPolicyCommandOutput) -> bool {
    let text = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    text.contains("no such command")
        || text.contains("no such subcommand")
        || text.contains("a command with a similar name exists")
        || text.contains("could not find `deny`")
}

fn tail(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut chars = trimmed.chars().rev().take(4000).collect::<Vec<_>>();
    chars.reverse();
    Some(chars.into_iter().collect())
}

#[cfg(test)]
thread_local! {
    static FORCE_TEST_DEPENDENCY_POLICY_PASSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub struct TestDependencyPolicyGuard;

#[cfg(test)]
impl Drop for TestDependencyPolicyGuard {
    fn drop(&mut self) {
        FORCE_TEST_DEPENDENCY_POLICY_PASSED.with(|value| value.set(false));
    }
}

#[cfg(test)]
pub fn force_test_dependency_policy_passed() -> TestDependencyPolicyGuard {
    FORCE_TEST_DEPENDENCY_POLICY_PASSED.with(|value| value.set(true));
    TestDependencyPolicyGuard
}

#[cfg(test)]
fn test_dependency_policy_forced_passed() -> bool {
    FORCE_TEST_DEPENDENCY_POLICY_PASSED.with(|value| value.get())
}

#[cfg(test)]
struct TestPassingCargoDenyRunner;

#[cfg(test)]
impl DependencyPolicyRunner for TestPassingCargoDenyRunner {
    fn run(
        &self,
        _workspace: &Path,
        args: &[&str],
    ) -> std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError> {
        if args == ["deny", "--version"] {
            return Ok(DependencyPolicyCommandOutput {
                success: true,
                exit_code: Some(0),
                stdout: "cargo-deny test\n".into(),
                stderr: String::new(),
            });
        }
        Ok(DependencyPolicyCommandOutput {
            success: true,
            exit_code: Some(0),
            stdout: "dependency policy passed in test\n".into(),
            stderr: String::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockRunner {
        outputs: Mutex<
            Vec<std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError>>,
        >,
    }

    impl MockRunner {
        fn new(
            outputs: Vec<
                std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError>,
            >,
        ) -> Self {
            Self {
                outputs: Mutex::new(outputs),
            }
        }
    }

    impl DependencyPolicyRunner for MockRunner {
        fn run(
            &self,
            _workspace: &Path,
            _args: &[&str],
        ) -> std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError> {
            self.outputs.lock().unwrap().remove(0)
        }
    }

    fn output(
        success: bool,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
    ) -> DependencyPolicyCommandOutput {
        DependencyPolicyCommandOutput {
            success,
            exit_code: Some(exit_code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn dependency_policy_is_not_applicable_without_cargo_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let report =
            DependencyPolicyManager::with_runner(temp.path(), Arc::new(MockRunner::default()))
                .unwrap()
                .audit()
                .unwrap();

        assert_eq!(report.status, "not_applicable");
        assert!(report.required_actions.is_empty());
    }

    #[test]
    fn dependency_policy_blocks_missing_config() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();

        let report =
            DependencyPolicyManager::with_runner(temp.path(), Arc::new(MockRunner::default()))
                .unwrap()
                .audit()
                .unwrap();

        assert_eq!(report.status, "blocked");
        assert_eq!(report.failure_kind.as_deref(), Some("config_missing"));
    }

    #[test]
    fn dependency_policy_blocks_missing_tool() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();
        let runner = MockRunner::new(vec![Ok(output(
            false,
            101,
            "",
            "error: no such command: `deny`",
        ))]);

        let report = DependencyPolicyManager::with_runner(temp.path(), Arc::new(runner))
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert_eq!(report.failure_kind.as_deref(), Some("tool_missing"));
        assert!(report.summary.contains("not installed"));
    }

    #[test]
    fn dependency_policy_passes_when_cargo_deny_passes() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();
        let runner = MockRunner::new(vec![
            Ok(output(true, 0, "cargo-deny 0.16.4\n", "")),
            Ok(output(true, 0, "all ok\n", "")),
        ]);

        let report = DependencyPolicyManager::with_runner(temp.path(), Arc::new(runner))
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "passed");
        assert_eq!(report.tool_version.as_deref(), Some("cargo-deny 0.16.4"));
        assert!(report.config_hash.is_some());
    }

    #[test]
    fn dependency_policy_blocks_check_failure() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();
        let runner = MockRunner::new(vec![
            Ok(output(true, 0, "cargo-deny 0.16.4\n", "")),
            Ok(output(false, 1, "", "GPL-3.0 license denied\n")),
        ]);

        let report = DependencyPolicyManager::with_runner(temp.path(), Arc::new(runner))
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert_eq!(report.failure_kind.as_deref(), Some("check_failed"));
        assert!(report.stderr_tail.as_deref().unwrap().contains("GPL-3.0"));
    }
}
