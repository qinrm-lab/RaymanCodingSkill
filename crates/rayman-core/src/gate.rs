use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::assets::AssetRetirementManager;
use crate::audit;
use crate::auxiliary::AuxiliaryTaskStore;
use crate::context::ContextKernel;
use crate::dependency_policy::{DependencyPolicyManager, DependencyPolicyReport};
use crate::docs::{self, DocsMaintainOptions};
use crate::evidence::{EvidenceCheckOptions, check_workspace_evidence, scan_success_claims};
use crate::feature_coverage;
use crate::model_catalog::ModelCatalogManager;
use crate::release::{ReleaseEvidenceManager, ReleaseEvidenceOptions};
use crate::risk::{RiskManager, RiskScanOptions};
use crate::security::SecurityAuditManager;
use crate::subagent::SubagentLedgerManager;
use crate::temp::TempManager;
use crate::trace::trace_eval_gate_status;
use crate::workspace::WorkspaceActivationManager;
use crate::{display_path, now_iso};

#[derive(Debug, Clone, Default)]
pub struct GateOptions {
    pub require_provenance: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub check_count: usize,
    pub blocking_count: usize,
    pub warning_count: usize,
    pub checks: Vec<GateCheck>,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GateCheck {
    pub id: String,
    pub title: String,
    pub status: String,
    pub severity: String,
    pub summary: String,
    pub required_actions: Vec<String>,
    pub details: Value,
}

pub struct GateManager {
    workspace: PathBuf,
}

impl GateManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            workspace: workspace
                .into()
                .canonicalize()
                .context("无法解析工作区路径")?,
        })
    }

    pub fn status(&self, options: GateOptions) -> Result<GateReport> {
        self.status_with_progress(options, |_, _| {})
    }

    pub fn status_with_progress<F>(
        &self,
        options: GateOptions,
        mut progress: F,
    ) -> Result<GateReport>
    where
        F: FnMut(&str, &str),
    {
        progress("dependency-policy", "Dependency policy");
        let dependency_policy = DependencyPolicyManager::new(&self.workspace)?.audit()?;
        let mut checks = Vec::new();

        progress("workspace_skill", "Workspace skill activation");
        checks.push(self.workspace_skill_check()?);
        progress("context_freshness", "Fresh Context Index");
        checks.push(self.context_freshness_check()?);
        progress("context_os", "Context OS state graph");
        checks.push(self.context_os_check()?);
        progress("auxiliary_tasks", "Auxiliary AI task state");
        checks.push(self.auxiliary_task_check()?);
        progress("asset_retirement", "Obsolete asset retirement");
        checks.push(self.asset_retirement_check()?);
        progress("subagent_ledger", "Codex host subagent ledger");
        checks.push(self.subagent_ledger_check()?);
        progress("managed_temp", "Managed temp cleanup");
        checks.push(self.temp_check()?);
        progress("feature_coverage", "Feature coverage matrix");
        checks.push(self.coverage_check()?);
        progress("docs_maintain", "Generated developer docs");
        checks.push(self.docs_check()?);
        progress("dependency-policy", "Dependency policy result");
        checks.push(self.dependency_policy_check(&dependency_policy)?);
        progress("security_audit", "LLM security audit");
        checks.push(self.security_check(&dependency_policy)?);
        progress("repository_audit", "Repository policy audit");
        checks.push(self.audit_check()?);
        progress("evidence_claims", "Evidence-backed claims");
        checks.push(self.evidence_check()?);
        progress("risk_ledger", "Proactive risk ledger");
        checks.push(self.risk_check()?);
        progress("model_catalog", "Model catalog governance");
        checks.push(self.model_catalog_check()?);
        progress("trace_eval", "Trace replay and eval reports");
        checks.push(self.trace_eval_check()?);
        progress("release_evidence", "Release evidence");
        checks.push(self.release_check(options.require_provenance, &dependency_policy)?);

        let blocking_count = checks
            .iter()
            .filter(|check| check.severity == "blocker" && check.status != "passed")
            .count();
        let warning_count = checks
            .iter()
            .filter(|check| check.severity == "warning" || check.status == "warning")
            .count();
        let required_actions = checks
            .iter()
            .filter(|check| check.severity == "blocker" && check.status != "passed")
            .flat_map(|check| {
                check
                    .required_actions
                    .iter()
                    .map(|action| format!("{}: {action}", check.id))
            })
            .collect::<Vec<_>>();
        Ok(GateReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: if blocking_count == 0 {
                "passed".into()
            } else {
                "blocked".into()
            },
            check_count: checks.len(),
            blocking_count,
            warning_count,
            checks,
            required_actions,
        })
    }

    pub fn assert_passed(&self, options: GateOptions) -> Result<GateReport> {
        let report = self.status(options)?;
        assert_gate_report_passed(report)
    }

    pub fn assert_passed_with_progress<F>(
        &self,
        options: GateOptions,
        progress: F,
    ) -> Result<GateReport>
    where
        F: FnMut(&str, &str),
    {
        let report = self.status_with_progress(options, progress)?;
        assert_gate_report_passed(report)
    }
}

fn assert_gate_report_passed(report: GateReport) -> Result<GateReport> {
    if report.status != "passed" {
        bail!(
            "readiness gate blocked:\n{}",
            report.required_actions.join("\n")
        );
    }
    Ok(report)
}

impl GateManager {
    fn workspace_skill_check(&self) -> Result<GateCheck> {
        let status = WorkspaceActivationManager::new(&self.workspace)?.status()?;
        let enabled = status
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let latest = status
            .get("uses_latest_skill_file")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let passed = enabled && latest;
        Ok(GateCheck {
            id: "workspace_skill".into(),
            title: "Workspace skill activation".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "workspace skill is enabled and current".into()
            } else {
                "workspace skill is disabled or stale".into()
            },
            required_actions: if passed {
                Vec::new()
            } else {
                vec!["run rayman workspace-skill mark-used or sync the skill state".into()]
            },
            details: yaml_to_json(status),
        })
    }

    fn context_freshness_check(&self) -> Result<GateCheck> {
        let status = ContextKernel::new(&self.workspace)?.status()?;
        let stale = status
            .get("counts")
            .and_then(|counts| counts.get("context_index_stale"))
            .and_then(Value::as_u64)
            .unwrap_or(1);
        let passed = stale == 0;
        Ok(GateCheck {
            id: "context_freshness".into(),
            title: "Fresh Context Index".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "Context Index hashes match current files".into()
            } else {
                "Context Index is missing or stale".into()
            },
            required_actions: status
                .get("required_actions")
                .and_then(Value::as_array)
                .map(|items| strings_from_json_array(items))
                .unwrap_or_else(|| vec!["run rayman context refresh".into()]),
            details: status,
        })
    }

    fn context_os_check(&self) -> Result<GateCheck> {
        let status = ContextKernel::new(&self.workspace)?.context_os_status()?;
        let stale = status.get("stale").and_then(Value::as_bool).unwrap_or(true);
        let passed = !stale;
        Ok(GateCheck {
            id: "context_os".into(),
            title: "Context OS state graph".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "Context OS state graph and event log match current workspace evidence".into()
            } else {
                "Context OS state graph is missing or stale".into()
            },
            required_actions: status
                .get("required_actions")
                .and_then(Value::as_array)
                .map(|items| strings_from_json_array(items))
                .unwrap_or_else(|| vec!["run rayman context os --write".into()]),
            details: status,
        })
    }

    fn temp_check(&self) -> Result<GateCheck> {
        let manager = TempManager::new(&self.workspace)?;
        let status = manager.status()?;
        let blockers = manager.success_blockers()?;
        let passed = blockers.is_empty();
        Ok(GateCheck {
            id: "managed_temp".into(),
            title: "Managed temp cleanup".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "managed temp state is clean".into()
            } else {
                "managed temp state has cleanup blockers".into()
            },
            required_actions: if passed { Vec::new() } else { blockers },
            details: serde_json::to_value(status)?,
        })
    }

    fn asset_retirement_check(&self) -> Result<GateCheck> {
        let report = AssetRetirementManager::new(&self.workspace)?.scan()?;
        let passed = !report.has_blockers();
        Ok(GateCheck {
            id: "asset_retirement".into(),
            title: "Obsolete asset retirement".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                format!(
                    "obsolete asset state is clean ({})",
                    report.controller_scope
                )
            } else {
                format!(
                    "obsolete asset retirement blockers={} ({})",
                    report.blockers.len(),
                    report.controller_scope
                )
            },
            required_actions: if passed {
                Vec::new()
            } else {
                report.required_actions.clone()
            },
            details: serde_json::to_value(report)?,
        })
    }

    fn subagent_ledger_check(&self) -> Result<GateCheck> {
        let manager = SubagentLedgerManager::new(&self.workspace)?;
        let status = manager.status()?;
        let blockers = manager.success_blockers()?;
        let passed = blockers.is_empty();
        Ok(GateCheck {
            id: "subagent_ledger".into(),
            title: "Codex host subagent ledger".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "subagent ledger has no unresolved host-agent blockers".into()
            } else {
                "subagent ledger has unresolved host-agent blockers".into()
            },
            required_actions: if passed { Vec::new() } else { blockers },
            details: status,
        })
    }

    fn auxiliary_task_check(&self) -> Result<GateCheck> {
        let manager = AuxiliaryTaskStore::new(&self.workspace)?;
        let blockers = match manager.success_blockers() {
            Ok(blockers) => blockers,
            Err(error) => vec![error.to_string()],
        };
        let passed = blockers.is_empty();
        let details = manager.status_json().unwrap_or_else(|error| {
            json!({
                "status": "blocked",
                "error": error.to_string(),
            })
        });
        Ok(GateCheck {
            id: "auxiliary_tasks".into(),
            title: "Auxiliary AI task state".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "auxiliary AI task state has no unresolved blockers".into()
            } else {
                "auxiliary AI task state has unresolved blockers".into()
            },
            required_actions: if passed { Vec::new() } else { blockers },
            details,
        })
    }

    fn coverage_check(&self) -> Result<GateCheck> {
        let report = feature_coverage::check_feature_coverage_with_options(
            &self.workspace,
            feature_coverage::FeatureCoverageOptions { strict: true },
        )?;
        let passed = report.status == "passed";
        Ok(GateCheck {
            id: "feature_coverage".into(),
            title: "Feature coverage matrix".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!(
                "feature coverage status={} findings={}",
                report.status, report.finding_count
            ),
            required_actions: report.required_actions.clone(),
            details: serde_json::to_value(report)?,
        })
    }

    fn docs_check(&self) -> Result<GateCheck> {
        let report = docs::maintain_html_docs(DocsMaintainOptions {
            root: self.workspace.clone(),
            output: None,
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: true,
            apply_prune: false,
        })?;
        let passed = report.status == "current";
        Ok(GateCheck {
            id: "docs_maintain".into(),
            title: "Generated developer docs".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!("docs maintain status={}", report.status),
            required_actions: report.required_actions.clone(),
            details: serde_json::to_value(report)?,
        })
    }

    fn security_check(&self, dependency_policy: &DependencyPolicyReport) -> Result<GateCheck> {
        let report = SecurityAuditManager::new(&self.workspace)?
            .audit_with_dependency_policy(dependency_policy.clone())?;
        let passed = report.status == "passed";
        Ok(GateCheck {
            id: "security_audit".into(),
            title: "LLM security audit".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!(
                "security status={} findings={}",
                report.status, report.finding_count
            ),
            required_actions: report.required_actions.clone(),
            details: serde_json::to_value(report)?,
        })
    }

    fn dependency_policy_check(&self, report: &DependencyPolicyReport) -> Result<GateCheck> {
        let passed = matches!(report.status.as_str(), "passed" | "not_applicable");
        Ok(GateCheck {
            id: "dependency-policy".into(),
            title: "Cargo dependency policy".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!(
                "dependency policy status={} kind={}",
                report.status,
                report.failure_kind.as_deref().unwrap_or("none")
            ),
            required_actions: report.required_actions.clone(),
            details: serde_json::to_value(report)?,
        })
    }

    fn audit_check(&self) -> Result<GateCheck> {
        let findings = audit::audit_repository(&self.workspace)?;
        let passed = findings.is_empty();
        Ok(GateCheck {
            id: "repository_audit".into(),
            title: "Repository policy audit".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                "repository audit has no findings".into()
            } else {
                format!("repository audit findings={}", findings.len())
            },
            required_actions: findings
                .iter()
                .map(|finding| {
                    format!(
                        "{}:{} {}",
                        display_path(&finding.path),
                        finding.line,
                        finding.message
                    )
                })
                .collect(),
            details: json!({ "findings": findings }),
        })
    }

    fn release_check(
        &self,
        require_provenance: bool,
        dependency_policy: &DependencyPolicyReport,
    ) -> Result<GateCheck> {
        let report = ReleaseEvidenceManager::new(&self.workspace)?
            .generate_with_options_and_dependency_policy(
                ReleaseEvidenceOptions {
                    label: "gate".into(),
                    write_default: false,
                    require_provenance,
                    ..ReleaseEvidenceOptions::default()
                },
                dependency_policy.clone(),
            )?;
        let (status, severity) = release_gate_classification(
            &report.status,
            &report.provenance_status,
            require_provenance,
        );
        Ok(GateCheck {
            id: "release_evidence".into(),
            title: "Release evidence".into(),
            status,
            severity,
            summary: format!(
                "release status={} provenance={}",
                report.status, report.provenance_status
            ),
            required_actions: report.required_actions.clone(),
            details: serde_json::to_value(report)?,
        })
    }

    fn evidence_check(&self) -> Result<GateCheck> {
        let report = check_workspace_evidence(
            self.workspace.clone(),
            EvidenceCheckOptions {
                scope: "workspace".into(),
                goal_id: None,
                include_advisory: false,
            },
        )?;
        let blockers = scan_success_claims(self.workspace.clone())?;
        let passed = blockers.is_empty();
        Ok(GateCheck {
            id: "evidence_claims".into(),
            title: "Evidence-backed claims".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: if passed {
                format!(
                    "success claims checked; workspace file-presence status={}",
                    report.status.as_str()
                )
            } else {
                format!("unverified success claims={}", blockers.len())
            },
            required_actions: if passed { Vec::new() } else { blockers },
            details: serde_json::to_value(report)?,
        })
    }

    fn risk_check(&self) -> Result<GateCheck> {
        let report = RiskManager::new(&self.workspace)?.scan(RiskScanOptions {
            write_ledger: false,
            include_expensive: false,
        })?;
        let passed = report.unresolved_high_critical_count == 0;
        Ok(GateCheck {
            id: "risk_ledger".into(),
            title: "Proactive risk ledger".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!(
                "risk status={} findings={} unresolved_high_critical={}",
                report.status, report.finding_count, report.unresolved_high_critical_count
            ),
            required_actions: if passed {
                Vec::new()
            } else {
                report.required_actions.clone()
            },
            details: serde_json::to_value(report)?,
        })
    }

    fn trace_eval_check(&self) -> Result<GateCheck> {
        let report = trace_eval_gate_status(&self.workspace)?;
        let passed = report.get("status").and_then(Value::as_str) == Some("passed");
        Ok(GateCheck {
            id: "trace_eval".into(),
            title: "Trace replay and eval reports".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!(
                "trace_eval status={} passed_eval_reports={}",
                report
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                report
                    .get("passed_eval_reports")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            ),
            required_actions: report
                .get("required_actions")
                .and_then(Value::as_array)
                .map(|items| strings_from_json_array(items))
                .unwrap_or_default(),
            details: report,
        })
    }

    fn model_catalog_check(&self) -> Result<GateCheck> {
        let report = ModelCatalogManager::new(&self.workspace)?.status()?;
        let passed = report.status == "passed";
        Ok(GateCheck {
            id: "model_catalog".into(),
            title: "Model catalog governance".into(),
            status: gate_status(passed),
            severity: "blocker".into(),
            summary: format!(
                "models status={} cache_present={} unknown_routes={} deprecated_routes={}",
                report.status,
                report.cache_present,
                report.unknown_routes.len(),
                report.deprecated_routes.len()
            ),
            required_actions: if passed {
                Vec::new()
            } else {
                report.required_actions.clone()
            },
            details: serde_json::to_value(report)?,
        })
    }
}

fn gate_status(passed: bool) -> String {
    if passed { "passed" } else { "blocked" }.into()
}

fn strings_from_json_array(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn yaml_to_json(value: crate::yaml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn release_gate_classification(
    release_status: &str,
    provenance_status: &str,
    require_provenance: bool,
) -> (String, String) {
    let passed = release_status == "ready";
    if passed && provenance_status == "not_git" && require_provenance {
        return ("blocked".into(), "blocker".into());
    }
    if passed && provenance_status == "not_git" && !require_provenance {
        return ("warning".into(), "warning".into());
    }
    (gate_status(passed), "blocker".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_gate_only_downgrades_not_git_when_release_is_ready() {
        assert_eq!(
            release_gate_classification("ready", "not_git", false),
            ("warning".into(), "warning".into())
        );
        assert_eq!(
            release_gate_classification("partial", "not_git", false),
            ("blocked".into(), "blocker".into())
        );
        assert_eq!(
            release_gate_classification("ready", "not_git", true),
            ("blocked".into(), "blocker".into())
        );
    }

    #[test]
    fn readiness_gate_progress_reports_each_stage() {
        let temp = tempfile::tempdir().unwrap();
        let mut stages = Vec::new();

        let _ = GateManager::new(temp.path())
            .unwrap()
            .status_with_progress(GateOptions::default(), |id, _title| {
                stages.push(id.to_string());
            })
            .unwrap();

        assert!(stages.contains(&"workspace_skill".to_string()));
        assert!(stages.contains(&"feature_coverage".to_string()));
        assert!(stages.contains(&"model_catalog".to_string()));
        assert!(stages.contains(&"release_evidence".to_string()));
    }

    #[test]
    fn readiness_gate_blocks_unreviewed_subagent_ledger() {
        let temp = tempfile::tempdir().unwrap();
        crate::subagent::SubagentLedgerManager::new(temp.path())
            .unwrap()
            .record(crate::subagent::SubagentRecordRequest {
                host_agent_id: "agent-1".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "inspect gate".into(),
                boundary: "read-only".into(),
                read_only: true,
                write_paths: Vec::new(),
            })
            .unwrap();

        let report = GateManager::new(temp.path())
            .unwrap()
            .status(GateOptions::default())
            .unwrap();
        let subagent = report
            .checks
            .iter()
            .find(|check| check.id == "subagent_ledger")
            .expect("subagent ledger check");

        assert_eq!(subagent.status, "blocked");
        assert!(
            subagent
                .required_actions
                .iter()
                .any(|action| action.contains("subagent_ledger_unreviewed"))
        );
    }

    #[test]
    fn readiness_gate_blocks_malformed_auxiliary_task_json() {
        let temp = tempfile::tempdir().unwrap();
        let task_dir = temp
            .path()
            .join(".RaymanCodingSkill")
            .join("auxiliary")
            .join("tasks");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("bad.json"), "{not json").unwrap();

        let report = GateManager::new(temp.path())
            .unwrap()
            .status(GateOptions::default())
            .unwrap();
        let auxiliary = report
            .checks
            .iter()
            .find(|check| check.id == "auxiliary_tasks")
            .expect("auxiliary task check");

        assert_eq!(auxiliary.status, "blocked");
        assert!(
            auxiliary
                .required_actions
                .iter()
                .any(|action| { action.contains("auxiliary_task_parse_error") })
        );
    }

    #[test]
    fn readiness_gate_has_first_class_asset_retirement_check() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        crate::assets::AssetRetirementManager::new(temp.path())
            .unwrap()
            .retire(crate::assets::AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "new.md".into(),
                deletion_reason: "replaced".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let report = GateManager::new(temp.path())
            .unwrap()
            .status(GateOptions::default())
            .unwrap();
        let assets = report
            .checks
            .iter()
            .find(|check| check.id == "asset_retirement")
            .expect("asset retirement check");

        assert_eq!(assets.status, "blocked");
        assert!(
            assets
                .summary
                .contains("obsolete asset retirement blockers")
        );
    }

    #[test]
    fn readiness_gate_has_first_class_context_os_check() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("README.md"), "# project").unwrap();
        crate::context::ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        std::fs::write(temp.path().join("README.md"), "# changed").unwrap();

        let report = GateManager::new(temp.path())
            .unwrap()
            .status(GateOptions::default())
            .unwrap();
        let context_os = report
            .checks
            .iter()
            .find(|check| check.id == "context_os")
            .expect("context os check");

        assert_eq!(context_os.status, "blocked");
        assert!(
            context_os
                .required_actions
                .iter()
                .any(|action| action.contains("context os --write"))
        );
    }

    #[test]
    fn readiness_gate_has_first_class_risk_check() {
        let temp = tempfile::tempdir().unwrap();

        let report = GateManager::new(temp.path())
            .unwrap()
            .status(GateOptions::default())
            .unwrap();

        let risk = report
            .checks
            .iter()
            .find(|check| check.id == "risk_ledger")
            .expect("risk ledger check");

        assert_eq!(risk.severity, "blocker");
        assert!(risk.summary.contains("unresolved_high_critical"));
    }

    #[test]
    fn readiness_gate_has_first_class_model_catalog_check() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            "default_model:\n  type: openai\n  name: missing\n",
        )
        .unwrap();

        let report = GateManager::new(temp.path())
            .unwrap()
            .status(GateOptions::default())
            .unwrap();
        let models = report
            .checks
            .iter()
            .find(|check| check.id == "model_catalog")
            .expect("model catalog check");

        assert_eq!(models.status, "blocked");
        assert!(models.summary.contains("models status=blocked"));
    }

    #[test]
    fn readiness_gate_feature_coverage_uses_strict_validation_records() {
        let temp = tempfile::tempdir().unwrap();
        write_strict_feature_coverage_repo(temp.path());

        let check = GateManager::new(temp.path())
            .unwrap()
            .coverage_check()
            .unwrap();

        assert_eq!(check.status, "blocked");
        let details = check.details;
        let findings = details["findings"].as_array().unwrap();
        assert!(
            findings.iter().any(|finding| {
                finding["kind"].as_str() == Some("claim_validation_records_missing")
                    && finding["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("strict_gate_claim")
            }),
            "findings: {findings:#?}"
        );
    }

    #[test]
    fn evidence_gate_blocks_unverified_success_claim() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("claim-report.json"),
            serde_json::json!({
                "claim_ledger": {
                    "claims": [{
                        "id": "claim_1",
                        "text": "feature completed successfully",
                        "status": "unknown",
                        "evidence_refs": [],
                        "blockers": ["missing current evidence"],
                        "checked_at": "2026-06-15T00:00:00Z"
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let check = GateManager::new(temp.path())
            .unwrap()
            .evidence_check()
            .unwrap();

        assert_eq!(check.status, "blocked");
        assert!(
            check
                .required_actions
                .iter()
                .any(|action| action.contains("unverified_success_claim"))
        );
    }

    fn write_strict_feature_coverage_repo(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("crates").join("rayman-cli").join("src")).unwrap();
        std::fs::create_dir_all(root.join("crates").join("rayman-cli").join("tests")).unwrap();
        std::fs::create_dir_all(root.join("crates").join("rayman-api").join("src")).unwrap();
        std::fs::write(
            root.join("docs").join("CLI.md"),
            "# CLI\nrayman gate status\n",
        )
        .unwrap();
        std::fs::write(root.join("docs").join("API.md"), "# API\n").unwrap();
        std::fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            "enum Command {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates")
                .join("rayman-cli")
                .join("tests")
                .join("ui_contract.rs"),
            "// @ui:cli\nfn cli_help() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates")
                .join("rayman-api")
                .join("src")
                .join("lib.rs"),
            "pub fn app() {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join(feature_coverage::FEATURE_COVERAGE_MANIFEST),
            r##"
features:
  - id: gate
    title: Gate
    doc_anchors:
      - path: docs/CLI.md
        contains: "# CLI"
    implementation_anchors:
      - path: crates/rayman-cli/src/main.rs
        contains: enum Command
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: fn cli_help
        proves:
          - rayman gate status
          - strict_gate_claim
    validation_commands:
      - "cargo test -p rayman-core gate::"
    ui_surfaces:
      - cli
    public_commands:
      - rayman gate status
    claim_checks:
      - id: strict_gate_claim
        claim: Gate uses strict validation records.
        strict_validation: true
        implementation_anchors:
          - path: crates/rayman-cli/src/main.rs
            contains: enum Command
        test_anchors:
          - path: crates/rayman-cli/tests/ui_contract.rs
            contains: fn cli_help
            proves:
              - strict_gate_claim
        validation_commands:
          - "cargo test -p rayman-core gate::"
  - id: api
    title: API
    doc_anchors:
      - path: docs/API.md
        contains: "# API"
    implementation_anchors:
      - path: crates/rayman-api/src/lib.rs
        contains: pub fn app
    test_anchors:
      - path: crates/rayman-cli/tests/ui_contract.rs
        contains: "@ui:cli"
    validation_commands:
      - cargo test -p rayman-api
"##,
        )
        .unwrap();
    }
}
