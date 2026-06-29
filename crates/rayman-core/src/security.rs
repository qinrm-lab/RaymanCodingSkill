use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::dependency_policy::{
    DependencyPolicyManager, DependencyPolicyReport, DependencyPolicyRunner,
    default_dependency_runner,
};
use crate::yaml::Value;
use crate::{display_path, now_iso, read_text, yaml};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityAuditReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub dependency_policy: DependencyPolicyReport,
    pub highest_severity: Option<String>,
    pub finding_count: usize,
    pub findings: Vec<SecurityFinding>,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityFinding {
    pub severity: String,
    pub category: String,
    pub path: String,
    pub line: usize,
    pub message: String,
    pub remediation: String,
}

struct FindingDraft<'a> {
    severity: &'a str,
    category: &'a str,
    path: &'a Path,
    line: usize,
    message: &'a str,
    remediation: &'a str,
}

pub struct SecurityAuditManager {
    workspace: PathBuf,
    dependency_runner: Arc<dyn DependencyPolicyRunner>,
}

impl SecurityAuditManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Self::with_dependency_policy_runner(workspace, default_dependency_runner())
    }

    pub fn with_dependency_policy_runner(
        workspace: impl Into<PathBuf>,
        dependency_runner: Arc<dyn DependencyPolicyRunner>,
    ) -> Result<Self> {
        Ok(Self {
            workspace: workspace
                .into()
                .canonicalize()
                .context("无法解析工作区路径")?,
            dependency_runner,
        })
    }

    pub fn audit(&self) -> Result<SecurityAuditReport> {
        let mut findings = Vec::new();
        self.audit_yaml_config_secrets(&mut findings)?;
        self.audit_auxiliary_ai_policy(&mut findings)?;
        self.audit_research_agent_policy(&mut findings)?;
        let dependency_policy = self.audit_dependency_policy(&mut findings)?;
        self.report(findings, dependency_policy)
    }

    pub fn audit_with_dependency_policy(
        &self,
        dependency_policy: DependencyPolicyReport,
    ) -> Result<SecurityAuditReport> {
        let mut findings = Vec::new();
        self.audit_yaml_config_secrets(&mut findings)?;
        self.audit_auxiliary_ai_policy(&mut findings)?;
        self.audit_research_agent_policy(&mut findings)?;
        if dependency_policy.status == "blocked" {
            push_dependency_policy_finding(&self.workspace, &mut findings, &dependency_policy);
        }
        self.report(findings, dependency_policy)
    }

    fn report(
        &self,
        mut findings: Vec<SecurityFinding>,
        dependency_policy: DependencyPolicyReport,
    ) -> Result<SecurityAuditReport> {
        if is_canonical_rayman_repo(&self.workspace) {
            self.audit_required_agent_controls(&mut findings);
        }
        findings.sort_by(|left, right| {
            severity_rank(&right.severity)
                .cmp(&severity_rank(&left.severity))
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.line.cmp(&right.line))
        });
        let highest_severity = findings.first().map(|finding| finding.severity.clone());
        let blocking = findings
            .iter()
            .filter(|finding| severity_rank(&finding.severity) >= severity_rank("high"))
            .collect::<Vec<_>>();
        let status = if blocking.is_empty() {
            "passed"
        } else {
            "blocked"
        };
        let required_actions = if blocking.is_empty() {
            vec!["No high or critical LLM security blockers detected.".into()]
        } else {
            blocking
                .iter()
                .map(|finding| {
                    format!(
                        "{}:{} {}: {}",
                        finding.path, finding.line, finding.category, finding.remediation
                    )
                })
                .collect()
        };
        Ok(SecurityAuditReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: status.into(),
            dependency_policy,
            highest_severity,
            finding_count: findings.len(),
            findings,
            required_actions,
        })
    }

    pub fn assert_passed(&self) -> Result<SecurityAuditReport> {
        let report = self.audit()?;
        if report.status != "passed" {
            bail!(
                "LLM security audit blocked: {}",
                report.required_actions.join("; ")
            );
        }
        Ok(report)
    }

    fn audit_yaml_config_secrets(&self, findings: &mut Vec<SecurityFinding>) -> Result<()> {
        let config_dir = self.workspace.join("config");
        if !config_dir.exists() {
            return Ok(());
        }
        for entry in WalkDir::new(config_dir)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yaml" | "yml")
            ) {
                continue;
            }
            let text = read_text(path)?;
            let Ok(value) = yaml::from_str::<Value>(&text) else {
                continue;
            };
            audit_plaintext_api_key_values(&self.workspace, path, &text, &value, "root", findings);
        }
        Ok(())
    }

    fn audit_auxiliary_ai_policy(&self, findings: &mut Vec<SecurityFinding>) -> Result<()> {
        let path = self.workspace.join("config").join("auxiliary_ai.yaml");
        if !path.exists() {
            return Ok(());
        }
        let text = read_text(&path)?;
        let value: Value = yaml::from_str(&text)
            .with_context(|| format!("无法解析辅助 AI 配置: {}", path.display()))?;
        if yaml_path_bool(&value, &["auxiliary_ai", "advisory_only"]) != Some(true) {
            push_finding(
                &self.workspace,
                findings,
                FindingDraft {
                    severity: "high",
                    category: "excessive_agency",
                    path: &path,
                    line: line_for_key(&text, "advisory_only").unwrap_or(1),
                    message: "auxiliary AI must be advisory-only",
                    remediation: "set auxiliary_ai.advisory_only: true",
                },
            );
        }
        if let Some(models) = value.get("models").and_then(Value::as_mapping) {
            for (provider_key, provider_value) in models {
                let provider = provider_key.as_str().unwrap_or("<unknown>");
                let auth_required =
                    provider_value.get("auth_required").and_then(Value::as_bool) == Some(true);
                if auth_required
                    && provider_value
                        .get("api_key_env")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .is_empty()
                    && provider_value
                        .get("api_key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim()
                        .is_empty()
                {
                    push_finding(
                        &self.workspace,
                        findings,
                        FindingDraft {
                            severity: "high",
                            category: "missing_secret_reference",
                            path: &path,
                            line: line_for_key(&text, provider).unwrap_or(1),
                            message: &format!(
                                "provider `{provider}` requires auth but has no env key reference"
                            ),
                            remediation: "set api_key_env to a local environment-variable name",
                        },
                    );
                }
                if provider_value
                    .get("data_policy")
                    .and_then(|policy| policy.get("advisory_only"))
                    .and_then(Value::as_bool)
                    != Some(true)
                {
                    push_finding(
                        &self.workspace,
                        findings,
                        FindingDraft {
                            severity: "high",
                            category: "excessive_agency",
                            path: &path,
                            line: line_for_key(&text, provider).unwrap_or(1),
                            message: &format!(
                                "provider `{provider}` lacks advisory-only data policy"
                            ),
                            remediation: "set models.<provider>.data_policy.advisory_only: true",
                        },
                    );
                }
                if provider_value
                    .get("trust_level")
                    .and_then(Value::as_str)
                    .is_some_and(|trust| trust.eq_ignore_ascii_case("untrusted"))
                    && provider_value
                        .get("allow_workspace_data")
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    push_finding(
                        &self.workspace,
                        findings,
                        FindingDraft {
                            severity: "high",
                            category: "data_exfiltration",
                            path: &path,
                            line: line_for_key(&text, provider).unwrap_or(1),
                            message: &format!(
                                "provider `{provider}` is untrusted but may receive workspace data"
                            ),
                            remediation: "set allow_workspace_data: false or raise trust_level only with documented approval",
                        },
                    );
                }
                if provider_value
                    .get("trust_level")
                    .and_then(Value::as_str)
                    .is_some_and(|trust| trust.eq_ignore_ascii_case("trusted_remote"))
                    && provider_value
                        .get("allow_workspace_data")
                        .and_then(Value::as_bool)
                        == Some(true)
                {
                    push_finding(
                        &self.workspace,
                        findings,
                        FindingDraft {
                            severity: "medium",
                            category: "remote_workspace_data",
                            path: &path,
                            line: line_for_key(&text, provider).unwrap_or(1),
                            message: &format!(
                                "provider `{provider}` is remote and may receive workspace data"
                            ),
                            remediation: "keep explicit trust documentation current or set allow_workspace_data: false",
                        },
                    );
                }
            }
        }
        Ok(())
    }

    fn audit_research_agent_policy(&self, findings: &mut Vec<SecurityFinding>) -> Result<()> {
        let path = self.workspace.join("config").join("research_agents.yaml");
        if !path.exists() {
            return Ok(());
        }
        let text = read_text(&path)?;
        let value: Value = yaml::from_str(&text)
            .with_context(|| format!("无法解析 research agent 配置: {}", path.display()))?;
        let research = value.get("research_agents").unwrap_or(&value);
        let scientist = research.get("scientist").unwrap_or(research);
        if scientist.get("can_edit_files").and_then(Value::as_bool) == Some(true) {
            push_finding(
                &self.workspace,
                findings,
                FindingDraft {
                    severity: "critical",
                    category: "excessive_research_agency",
                    path: &path,
                    line: line_for_key(&text, "can_edit_files").unwrap_or(1),
                    message: "scientist agent must not be allowed to edit files",
                    remediation: "set research_agents.scientist.can_edit_files: false",
                },
            );
        }
        if scientist.get("can_close_goals").and_then(Value::as_bool) == Some(true) {
            push_finding(
                &self.workspace,
                findings,
                FindingDraft {
                    severity: "critical",
                    category: "excessive_research_agency",
                    path: &path,
                    line: line_for_key(&text, "can_close_goals").unwrap_or(1),
                    message: "scientist agent must not be allowed to close goals",
                    remediation: "set research_agents.scientist.can_close_goals: false",
                },
            );
        }
        let policy = research.get("command_policy").unwrap_or(research);
        if policy.get("require_workspace_cwd").and_then(Value::as_bool) == Some(false) {
            push_finding(
                &self.workspace,
                findings,
                FindingDraft {
                    severity: "high",
                    category: "unsafe_research_command_policy",
                    path: &path,
                    line: line_for_key(&text, "require_workspace_cwd").unwrap_or(1),
                    message: "scientist experiment cwd boundary must remain enabled",
                    remediation: "set research_agents.command_policy.require_workspace_cwd: true",
                },
            );
        }
        if policy
            .get("diff_check_repo_tracked_files")
            .and_then(Value::as_bool)
            == Some(false)
        {
            push_finding(
                &self.workspace,
                findings,
                FindingDraft {
                    severity: "high",
                    category: "missing_research_diff_gate",
                    path: &path,
                    line: line_for_key(&text, "diff_check_repo_tracked_files").unwrap_or(1),
                    message: "scientist experiments must diff-check protected workspace files",
                    remediation: "set research_agents.command_policy.diff_check_repo_tracked_files: true",
                },
            );
        }
        if policy
            .get("reject_shell_operators")
            .and_then(Value::as_bool)
            == Some(false)
        {
            push_finding(
                &self.workspace,
                findings,
                FindingDraft {
                    severity: "high",
                    category: "unsafe_research_command_policy",
                    path: &path,
                    line: line_for_key(&text, "reject_shell_operators").unwrap_or(1),
                    message: "scientist experiment argv must reject shell operators",
                    remediation: "set research_agents.command_policy.reject_shell_operators: true",
                },
            );
        }
        if let Some(allowed) = policy.get("allowed").and_then(Value::as_sequence) {
            for entry in allowed {
                let Some(args) = entry.as_sequence() else {
                    push_finding(
                        &self.workspace,
                        findings,
                        FindingDraft {
                            severity: "high",
                            category: "unsafe_research_command_policy",
                            path: &path,
                            line: line_for_key(&text, "allowed").unwrap_or(1),
                            message: "scientist command whitelist entries must be argv arrays",
                            remediation: "write each allowed command as a YAML sequence, not a shell string",
                        },
                    );
                    continue;
                };
                for arg in args.iter().filter_map(Value::as_str) {
                    if arg
                        .chars()
                        .any(|ch| matches!(ch, '|' | '&' | ';' | '<' | '>' | '`'))
                    {
                        push_finding(
                            &self.workspace,
                            findings,
                            FindingDraft {
                                severity: "high",
                                category: "unsafe_research_command_policy",
                                path: &path,
                                line: line_for_key(&text, "allowed").unwrap_or(1),
                                message: "scientist command whitelist contains shell operator",
                                remediation: "remove shell operators and represent only direct argv commands",
                            },
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn audit_dependency_policy(
        &self,
        findings: &mut Vec<SecurityFinding>,
    ) -> Result<DependencyPolicyReport> {
        let report = DependencyPolicyManager::with_runner(
            &self.workspace,
            Arc::clone(&self.dependency_runner),
        )?
        .audit()?;
        if report.status == "blocked" {
            push_dependency_policy_finding(&self.workspace, findings, &report);
        }
        Ok(report)
    }

    fn audit_required_agent_controls(&self, findings: &mut Vec<SecurityFinding>) {
        let skill_text = read_skill_and_references(&self.workspace).unwrap_or_default();
        for snippet in [
            "auxiliary AI output are navigation only",
            "never executes, edits, approves, or replaces validation",
            "Ask or block only for missing permissions/credentials, destructive actions",
            "manual/remote validation gaps block `session close --status success`",
        ] {
            if !skill_text.contains(snippet) {
                push_finding(
                    &self.workspace,
                    findings,
                    FindingDraft {
                        severity: "high",
                        category: "missing_agent_control",
                        path: &self.workspace.join("SKILL.md"),
                        line: 1,
                        message: &format!("missing required agent security control: {snippet}"),
                        remediation: "restore the control text in SKILL.md or a referenced skill rule",
                    },
                );
            }
        }
    }
}

fn audit_plaintext_api_key_values(
    workspace: &Path,
    file: &Path,
    text: &str,
    value: &Value,
    yaml_path: &str,
    findings: &mut Vec<SecurityFinding>,
) {
    match value {
        Value::Mapping(mapping) => {
            for (key, nested) in mapping {
                let key_text = key.as_str().unwrap_or("<key>");
                let next_path = format!("{yaml_path}.{key_text}");
                if key_text == "api_key"
                    && let Some(secret) = nested.as_str()
                    && !is_placeholder_secret(secret)
                {
                    push_finding(
                        workspace,
                        findings,
                        FindingDraft {
                            severity: "critical",
                            category: "plaintext_secret",
                            path: file,
                            line: line_for_key(text, "api_key").unwrap_or(1),
                            message: "plaintext API key stored in YAML config",
                            remediation: "replace api_key with api_key_env and keep the secret outside the workspace",
                        },
                    );
                }
                audit_plaintext_api_key_values(workspace, file, text, nested, &next_path, findings);
            }
        }
        Value::Sequence(items) => {
            for (index, nested) in items.iter().enumerate() {
                audit_plaintext_api_key_values(
                    workspace,
                    file,
                    text,
                    nested,
                    &format!("{yaml_path}[{index}]"),
                    findings,
                );
            }
        }
        _ => {}
    }
}

fn is_placeholder_secret(value: &str) -> bool {
    let normalized = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_lowercase();
    normalized.is_empty()
        || normalized == "null"
        || normalized.contains("your_")
        || normalized.contains("replace")
        || normalized.contains("placeholder")
        || normalized.contains("example")
        || normalized.contains("<")
}

fn read_skill_and_references(workspace: &Path) -> Result<String> {
    let mut text = read_text(&workspace.join("SKILL.md"))?;
    let references = workspace.join("references");
    if references.exists() {
        for entry in WalkDir::new(references)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            if entry.file_type().is_file()
                && entry.path().extension().and_then(|ext| ext.to_str()) == Some("md")
                && let Ok(reference_text) = read_text(entry.path())
            {
                text.push('\n');
                text.push_str(&reference_text);
            }
        }
    }
    Ok(text)
}

fn yaml_path_bool(value: &Value, path: &[&str]) -> Option<bool> {
    let mut current = value;
    for item in path {
        current = current.get(*item)?;
    }
    current.as_bool()
}

fn line_for_key(text: &str, key: &str) -> Option<usize> {
    text.lines()
        .position(|line| line.trim_start().starts_with(&format!("{key}:")))
        .map(|index| index + 1)
}

fn push_finding(workspace: &Path, findings: &mut Vec<SecurityFinding>, draft: FindingDraft<'_>) {
    findings.push(SecurityFinding {
        severity: draft.severity.into(),
        category: draft.category.into(),
        path: draft
            .path
            .strip_prefix(workspace)
            .unwrap_or(draft.path)
            .to_string_lossy()
            .replace('\\', "/"),
        line: draft.line,
        message: draft.message.into(),
        remediation: draft.remediation.into(),
    });
}

fn push_dependency_policy_finding(
    workspace: &Path,
    findings: &mut Vec<SecurityFinding>,
    report: &DependencyPolicyReport,
) {
    let category = format!(
        "dependency_policy.{}",
        report.failure_kind.as_deref().unwrap_or("check_failed")
    );
    let path = workspace.join("deny.toml");
    let message = match report.failure_kind.as_deref() {
        Some("tool_missing") => "cargo-deny is required for dependency policy",
        Some("config_missing") => "deny.toml is required for dependency policy",
        Some("tool_error") => "cargo-deny could not run",
        _ => "cargo-deny dependency policy check failed",
    };
    let remediation = if report.required_actions.is_empty() {
        "run cargo deny check and resolve dependency policy failures".into()
    } else {
        report.required_actions.join("; ")
    };
    findings.push(SecurityFinding {
        severity: "high".into(),
        category,
        path: path
            .strip_prefix(workspace)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/"),
        line: 1,
        message: message.into(),
        remediation,
    });
}

fn severity_rank(severity: &str) -> u8 {
    match severity.to_ascii_lowercase().as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn is_canonical_rayman_repo(root: &Path) -> bool {
    root.join("SKILL.md").exists()
        && root.join("crates").join("rayman-core").join("src").exists()
        && root.join("crates").join("rayman-cli").join("src").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dependency_policy::{
        DependencyPolicyCommandOutput, DependencyPolicyRunError, DependencyPolicyRunner,
    };
    use std::fs;

    struct BlockingDependencyRunner;

    impl DependencyPolicyRunner for BlockingDependencyRunner {
        fn run(
            &self,
            _workspace: &Path,
            args: &[&str],
        ) -> std::result::Result<DependencyPolicyCommandOutput, DependencyPolicyRunError> {
            if args == ["deny", "--version"] {
                return Ok(DependencyPolicyCommandOutput {
                    success: true,
                    exit_code: Some(0),
                    stdout: "cargo-deny 0.16.4\n".into(),
                    stderr: String::new(),
                });
            }
            Ok(DependencyPolicyCommandOutput {
                success: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: "GPL-3.0 license denied\n".into(),
            })
        }
    }

    #[test]
    fn security_audit_blocks_plaintext_yaml_api_key() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("auxiliary_ai.yaml"),
            r#"
auxiliary_ai:
  advisory_only: true
models:
  remote:
    auth_required: true
    api_key: "sk-real-secret"
    data_policy:
      advisory_only: true
"#,
        )
        .unwrap();

        let report = SecurityAuditManager::new(temp.path())
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert!(report.findings.iter().any(|finding| {
            finding.severity == "critical" && finding.category == "plaintext_secret"
        }));
    }

    #[test]
    fn security_audit_allows_env_secret_reference() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("auxiliary_ai.yaml"),
            r#"
auxiliary_ai:
  advisory_only: true
models:
  remote:
    auth_required: true
    api_key_env: "REMOTE_API_KEY"
    trust_level: trusted_lan
    allow_workspace_data: true
    data_policy:
      advisory_only: true
"#,
        )
        .unwrap();

        let report = SecurityAuditManager::new(temp.path())
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "passed");
        assert!(
            report
                .findings
                .iter()
                .all(|finding| finding.severity != "critical")
        );
    }

    #[test]
    fn security_audit_blocks_untrusted_workspace_data() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("auxiliary_ai.yaml"),
            r#"
auxiliary_ai:
  advisory_only: true
models:
  remote:
    auth_required: false
    trust_level: untrusted
    allow_workspace_data: true
    data_policy:
      advisory_only: true
"#,
        )
        .unwrap();

        let report = SecurityAuditManager::new(temp.path())
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.category == "data_exfiltration")
        );
    }

    #[test]
    fn security_audit_blocks_scientist_file_edit_authority() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  scientist:
    can_run_experiments: true
    can_edit_files: true
    can_close_goals: false
  command_policy:
    require_workspace_cwd: true
    diff_check_repo_tracked_files: true
    reject_shell_operators: true
    allowed:
      - ["cargo", "test", "--all"]
"#,
        )
        .unwrap();

        let report = SecurityAuditManager::new(temp.path())
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert!(report.findings.iter().any(|finding| {
            finding.category == "excessive_research_agency" && finding.severity == "critical"
        }));
    }

    #[test]
    fn security_audit_blocks_disabled_research_workspace_cwd_boundary() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  scientist:
    can_run_experiments: true
    can_edit_files: false
    can_close_goals: false
  command_policy:
    require_workspace_cwd: false
    diff_check_repo_tracked_files: true
    reject_shell_operators: true
    allowed:
      - ["cargo", "test", "--all"]
"#,
        )
        .unwrap();

        let report = SecurityAuditManager::new(temp.path())
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert!(report.findings.iter().any(|finding| {
            finding.category == "unsafe_research_command_policy"
                && finding.message.contains("cwd boundary")
        }));
    }

    #[test]
    fn security_audit_allows_bounded_research_agent_policy() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("config")).unwrap();
        fs::write(
            temp.path().join("config").join("research_agents.yaml"),
            r#"
research_agents:
  enabled: true
  scientist:
    can_run_experiments: true
    can_edit_files: false
    can_close_goals: false
  command_policy:
    require_workspace_cwd: true
    diff_check_repo_tracked_files: true
    reject_shell_operators: true
    allowed:
      - ["cargo", "test", "--all"]
"#,
        )
        .unwrap();

        let report = SecurityAuditManager::new(temp.path())
            .unwrap()
            .audit()
            .unwrap();

        assert_eq!(report.status, "passed");
    }

    #[test]
    fn security_audit_blocks_dependency_policy_failure() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(temp.path().join("deny.toml"), "[licenses]\n").unwrap();

        let report = SecurityAuditManager::with_dependency_policy_runner(
            temp.path(),
            Arc::new(BlockingDependencyRunner),
        )
        .unwrap()
        .audit()
        .unwrap();

        assert_eq!(report.status, "blocked");
        assert_eq!(report.dependency_policy.status, "blocked");
        assert!(report.findings.iter().any(|finding| {
            finding.category == "dependency_policy.check_failed" && finding.severity == "high"
        }));
    }
}
