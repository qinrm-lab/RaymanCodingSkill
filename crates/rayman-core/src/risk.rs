use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::assets::AssetRetirementManager;
use crate::audit;
use crate::auxiliary::AuxiliaryTaskStore;
use crate::context::ContextKernel;
use crate::dependency_policy::DependencyPolicyManager;
use crate::docs::{self, DocsMaintainOptions};
use crate::evidence::{EvidenceCheckOptions, check_workspace_evidence, scan_success_claims};
use crate::feature_coverage::{self, FeatureCoverageOptions};
use crate::security::{SecurityAuditManager, SecurityFinding};
use crate::subagent::SubagentLedgerManager;
use crate::temp::{TempCleanupOptions, TempManager};
use crate::{display_path, now_iso, write_text};

const RISK_LEDGER_RELATIVE_PATH: &str = ".RaymanCodingSkill/risk/ledger.jsonl";
const RISK_LEARNED_RELATIVE_PATH: &str = ".RaymanCodingSkill/risk/learned-patterns.json";
const MAX_SAFE_FIX_PASSES: usize = 5;
const SOURCE_POLICY: &str = "Risk findings are current-workspace signals only. Research, cached context, memory, and auxiliary output are advisory until a finding cites current files, command output, or workspace state.";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskScanOptions {
    #[serde(default)]
    pub write_ledger: bool,
    #[serde(default = "default_include_expensive")]
    pub include_expensive: bool,
}

impl Default for RiskScanOptions {
    fn default() -> Self {
        Self {
            write_ledger: true,
            include_expensive: true,
        }
    }
}

fn default_include_expensive() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskFixOptions {
    #[serde(default = "default_true")]
    pub safe_only: bool,
    #[serde(default)]
    pub guarded: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskFinding {
    pub id: String,
    pub category: String,
    pub severity: String,
    pub title: String,
    pub source: String,
    pub evidence_status: String,
    pub evidence_refs: Vec<String>,
    pub impact_paths: Vec<String>,
    pub remediation: String,
    pub validation_commands: Vec<String>,
    pub action_class: String,
    pub status: String,
    pub detected_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub state_path: String,
    pub status: String,
    pub finding_count: usize,
    pub blocking_count: usize,
    pub unresolved_high_critical_count: usize,
    pub findings: Vec<RiskFinding>,
    pub required_actions: Vec<String>,
    pub source_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskPlan {
    pub workspace_path: String,
    pub generated_at: String,
    pub state_path: String,
    pub status: String,
    pub safe_auto: Vec<RiskFinding>,
    pub guarded_auto: Vec<RiskFinding>,
    pub human_required: Vec<RiskFinding>,
    pub required_actions: Vec<String>,
    pub source_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskFixReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub state_path: String,
    pub mode: String,
    pub status: String,
    pub applied: Vec<RiskFixResult>,
    pub skipped: Vec<RiskFixResult>,
    pub post_scan: RiskReport,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskFixResult {
    pub id: String,
    pub action: String,
    pub status: String,
    pub reason: String,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskVerifyReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub scan: RiskReport,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskLearnReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub state_path: String,
    pub learned_categories: Vec<RiskLearnedCategory>,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskLearnedCategory {
    pub category: String,
    pub action_class: String,
    pub validation_commands: Vec<String>,
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RiskLedgerEntry {
    generated_at: String,
    event: String,
    finding_count: usize,
    blocking_count: usize,
    finding: Option<RiskFinding>,
}

#[derive(Debug, Clone)]
pub struct RiskManager {
    workspace: PathBuf,
    ledger_path: PathBuf,
    learned_path: PathBuf,
}

impl RiskManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        Ok(Self {
            ledger_path: workspace.join(RISK_LEDGER_RELATIVE_PATH),
            learned_path: workspace.join(RISK_LEARNED_RELATIVE_PATH),
            workspace,
        })
    }

    pub fn scan(&self, options: RiskScanOptions) -> Result<RiskReport> {
        let mut findings = Vec::new();
        self.collect_context_risks(&mut findings);
        self.collect_runtime_state_risks(&mut findings);
        self.collect_quality_surface_risks(&mut findings);
        if options.include_expensive {
            self.collect_security_and_dependency_risks(&mut findings);
        }
        self.collect_audit_and_evidence_risks(&mut findings);
        sort_findings(&mut findings);
        let report = self.report_from_findings(findings);
        if options.write_ledger {
            self.append_ledger(&report)?;
        }
        Ok(report)
    }

    pub fn plan(&self) -> Result<RiskPlan> {
        let report = self.scan(RiskScanOptions::default())?;
        let mut safe_auto = Vec::new();
        let mut guarded_auto = Vec::new();
        let mut human_required = Vec::new();
        for finding in report.findings {
            match finding.action_class.as_str() {
                "safe_auto" => safe_auto.push(finding),
                "guarded_auto" => guarded_auto.push(finding),
                _ => human_required.push(finding),
            }
        }
        let required_actions = plan_required_actions(&safe_auto, &guarded_auto, &human_required);
        Ok(RiskPlan {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            state_path: display_path(&self.ledger_path),
            status: if required_actions.is_empty() {
                "passed".into()
            } else {
                "attention".into()
            },
            safe_auto,
            guarded_auto,
            human_required,
            required_actions,
            source_policy: SOURCE_POLICY.into(),
        })
    }

    pub fn fix(&self, options: RiskFixOptions) -> Result<RiskFixReport> {
        if options.safe_only && options.guarded {
            bail!("risk fix accepts either --safe-only or --guarded, not both");
        }
        let mode = if options.guarded {
            "guarded"
        } else {
            "safe_only"
        };
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        let mut applied_ids = BTreeSet::new();
        let mut failed_ids = BTreeSet::new();
        let mut scan = self.scan(RiskScanOptions {
            write_ledger: false,
            include_expensive: true,
        })?;
        for _ in 0..MAX_SAFE_FIX_PASSES {
            let mut applied_this_pass = false;
            let mut pass_categories = BTreeSet::new();
            for finding in &scan.findings {
                if finding.action_class != "safe_auto" {
                    continue;
                }
                if !pass_categories.insert(finding.category.clone())
                    || applied_ids.contains(&finding.id)
                    || failed_ids.contains(&finding.id)
                {
                    continue;
                }
                match self.apply_safe_finding(finding) {
                    Ok(result) => {
                        applied_ids.insert(finding.id.clone());
                        applied.push(result);
                        applied_this_pass = true;
                    }
                    Err(error) => {
                        failed_ids.insert(finding.id.clone());
                        skipped.push(RiskFixResult {
                            id: finding.id.clone(),
                            action: finding.remediation.clone(),
                            status: "failed".into(),
                            reason: error.to_string(),
                            evidence_refs: finding.evidence_refs.clone(),
                        });
                    }
                }
            }
            if !applied_this_pass {
                break;
            }
            scan = self.scan(RiskScanOptions {
                write_ledger: false,
                include_expensive: true,
            })?;
        }
        let post_scan = self.scan(RiskScanOptions::default())?;
        let mut skipped_ids = skipped
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        for finding in &post_scan.findings {
            if skipped_ids.contains(&finding.id) {
                continue;
            }
            if finding.action_class == "safe_auto" {
                if !applied_ids.contains(&finding.id) && !failed_ids.contains(&finding.id) {
                    skipped_ids.insert(finding.id.clone());
                    skipped.push(RiskFixResult {
                        id: finding.id.clone(),
                        action: finding.remediation.clone(),
                        status: "skipped".into(),
                        reason: "safe_auto finding remained after iterative safe fix; inspect detector input or rerun risk fix after the current writer settles".into(),
                        evidence_refs: finding.evidence_refs.clone(),
                    });
                }
            } else {
                skipped_ids.insert(finding.id.clone());
                skipped.push(RiskFixResult {
                    id: finding.id.clone(),
                    action: finding.remediation.clone(),
                    status: "skipped".into(),
                    reason: if options.guarded {
                        "guarded remediation requires a bounded code/edit worker and validation; this command records the plan but does not edit uncertain surfaces".into()
                    } else {
                        "not a safe_auto finding; run risk plan and handle guarded or human-required remediation explicitly".into()
                    },
                    evidence_refs: finding.evidence_refs.clone(),
                });
            }
        }
        let status = if post_scan.unresolved_high_critical_count == 0 {
            "passed"
        } else if applied.is_empty() {
            "blocked"
        } else {
            "partial"
        };
        let required_actions = post_scan.required_actions.clone();
        Ok(RiskFixReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            state_path: display_path(&self.ledger_path),
            mode: mode.into(),
            status: status.into(),
            applied,
            skipped,
            post_scan,
            required_actions,
        })
    }

    pub fn verify(&self) -> Result<RiskVerifyReport> {
        let scan = self.scan(RiskScanOptions::default())?;
        let status = if scan.unresolved_high_critical_count == 0 {
            "passed"
        } else {
            "blocked"
        };
        Ok(RiskVerifyReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: status.into(),
            required_actions: scan.required_actions.clone(),
            scan,
        })
    }

    pub fn learn(&self) -> Result<RiskLearnReport> {
        let scan = self.scan(RiskScanOptions::default())?;
        let learned_categories = learned_categories(&scan.findings);
        if let Some(parent) = self.learned_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建风险学习目录: {}", parent.display()))?;
        }
        let state = json!({
            "workspace_path": display_path(&self.workspace),
            "generated_at": now_iso(),
            "source_policy": SOURCE_POLICY,
            "learned_categories": learned_categories,
        });
        write_text(&self.learned_path, &serde_json::to_string_pretty(&state)?)?;
        Ok(RiskLearnReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: if scan.findings.is_empty() {
                "passed".into()
            } else {
                "learned".into()
            },
            state_path: display_path(&self.learned_path),
            learned_categories,
            required_actions: if scan.findings.is_empty() {
                vec!["No active risk findings to learn from.".into()]
            } else {
                vec![
                    "Review learned risk categories and promote repeated failures to quality incidents when they recur.".into(),
                    "Run rayman risk verify and rayman gate status --check before claiming closure.".into(),
                ]
            },
        })
    }

    fn collect_context_risks(&self, findings: &mut Vec<RiskFinding>) {
        let kernel = match ContextKernel::new(&self.workspace) {
            Ok(kernel) => kernel,
            Err(error) => {
                push_source_error(findings, "context", error);
                return;
            }
        };
        match kernel.status() {
            Ok(status) => {
                let stale = status
                    .get("counts")
                    .and_then(|counts| counts.get("context_index_stale"))
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                if stale > 0 {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "context_freshness",
                            severity: "high",
                            title: "Context Index is stale or missing",
                            source: "rayman context status",
                            evidence_refs: json_refs(&status, &["details", "index_path"]),
                            impact_paths: vec![".RaymanCodingSkill/context/index.json".into()],
                            remediation: "run rayman context refresh",
                            validation_commands: vec![
                                "rayman context status --check".into(),
                                "rayman context os --check".into(),
                            ],
                            action_class: "safe_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "context_freshness", error),
        }
        match kernel.context_os_status() {
            Ok(status) => {
                if status.get("stale").and_then(Value::as_bool).unwrap_or(true) {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "context_os",
                            severity: "high",
                            title: "Context OS state graph is stale or missing",
                            source: "rayman context os --check",
                            evidence_refs: json_refs(&status, &["state_path"]),
                            impact_paths: vec![
                                ".RaymanCodingSkill/context/state.json".into(),
                                ".RaymanCodingSkill/context/events.jsonl".into(),
                            ],
                            remediation: "run rayman context os --write",
                            validation_commands: vec!["rayman context os --check".into()],
                            action_class: "safe_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "context_os", error),
        }
    }

    fn collect_runtime_state_risks(&self, findings: &mut Vec<RiskFinding>) {
        match TempManager::new(&self.workspace).and_then(|manager| manager.success_blockers()) {
            Ok(blockers) => {
                for blocker in blockers {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "managed_temp",
                            severity: "high",
                            title: "Managed temp state blocks success",
                            source: "rayman temp status",
                            evidence_refs: vec![".RaymanCodingSkill/tmp".into()],
                            impact_paths: vec![".RaymanCodingSkill/tmp".into()],
                            remediation: "run rayman temp cleanup --completed --stale --cargo-targets after inspecting failed runs",
                            validation_commands: vec!["rayman temp status".into()],
                            action_class: if blocker.contains("failed")
                                || blocker.contains("Foreign")
                            {
                                "human_required"
                            } else {
                                "safe_auto"
                            },
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "managed_temp", error),
        }
        match AuxiliaryTaskStore::new(&self.workspace)
            .and_then(|manager| manager.success_blockers())
        {
            Ok(blockers) => {
                for blocker in blockers {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "auxiliary_tasks",
                            severity: "high",
                            title: &format!("Auxiliary task blocker: {blocker}"),
                            source: "rayman auxiliary status",
                            evidence_refs: vec![".RaymanCodingSkill/auxiliary/tasks".into()],
                            impact_paths: vec![".RaymanCodingSkill/auxiliary/tasks".into()],
                            remediation: "run rayman auxiliary reconcile or resolve malformed task state",
                            validation_commands: vec!["rayman auxiliary status".into()],
                            action_class: "guarded_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "auxiliary_tasks", error),
        }
        match SubagentLedgerManager::new(&self.workspace)
            .and_then(|manager| manager.success_blockers())
        {
            Ok(blockers) => {
                for blocker in blockers {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "subagent_ledger",
                            severity: "high",
                            title: &format!("Host subagent ledger blocker: {blocker}"),
                            source: "rayman subagent status",
                            evidence_refs: vec![".RaymanCodingSkill/subagents".into()],
                            impact_paths: vec![".RaymanCodingSkill/subagents".into()],
                            remediation: "record subagent result and primary-agent review before success",
                            validation_commands: vec!["rayman subagent status".into()],
                            action_class: "human_required",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "subagent_ledger", error),
        }
    }

    fn collect_quality_surface_risks(&self, findings: &mut Vec<RiskFinding>) {
        match AssetRetirementManager::new(&self.workspace).and_then(|manager| manager.scan()) {
            Ok(report) => {
                for blocker in report.blockers {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "asset_retirement",
                            severity: "high",
                            title: &format!("Obsolete asset blocker: {blocker}"),
                            source: "rayman assets scan",
                            evidence_refs: vec![report.state_path.clone()],
                            impact_paths: vec![report.state_path.clone()],
                            remediation: "retire, delete, rewrite, or explicitly exempt obsolete assets",
                            validation_commands: vec![
                                "rayman assets status".into(),
                                "rayman audit".into(),
                            ],
                            action_class: "guarded_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "asset_retirement", error),
        }
        match feature_coverage::check_feature_coverage_with_options(
            &self.workspace,
            FeatureCoverageOptions { strict: true },
        ) {
            Ok(report) => {
                if report.status != "passed" {
                    for finding in report.findings {
                        let path = finding
                            .path
                            .as_ref()
                            .map(|path| display_path(path))
                            .unwrap_or_else(|| display_path(&report.manifest_path));
                        push_finding(
                            findings,
                            FindingDraft {
                                category: "feature_coverage",
                                severity: "high",
                                title: &format!("Feature coverage gap: {}", finding.message),
                                source: "rayman coverage status --check",
                                evidence_refs: vec![format!("{path}:{}", finding.line)],
                                impact_paths: vec![path],
                                remediation: "update feature coverage docs, implementation anchors, tests, and validation records",
                                validation_commands: vec![
                                    "rayman coverage status --check".into(),
                                    "rayman audit".into(),
                                ],
                                action_class: "guarded_auto",
                            },
                        );
                    }
                }
            }
            Err(error) => push_source_error(findings, "feature_coverage", error),
        }
        match docs::maintain_html_docs(DocsMaintainOptions {
            root: self.workspace.clone(),
            output: None,
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: true,
            apply_prune: false,
        }) {
            Ok(report) => {
                if report.status != "current" {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "docs_maintain",
                            severity: "medium",
                            title: "Generated docs are stale or incomplete",
                            source: "rayman docs maintain --check",
                            evidence_refs: vec![display_path(&report.output)],
                            impact_paths: vec![display_path(&report.output)],
                            remediation: "run rayman docs maintain",
                            validation_commands: vec!["rayman docs maintain --check".into()],
                            action_class: "safe_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "docs_maintain", error),
        }
    }

    fn collect_security_and_dependency_risks(&self, findings: &mut Vec<RiskFinding>) {
        match DependencyPolicyManager::new(&self.workspace).and_then(|manager| manager.audit()) {
            Ok(report) => {
                if report.status == "blocked" {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "dependency_policy",
                            severity: "high",
                            title: "Dependency policy blocks success",
                            source: "cargo deny check",
                            evidence_refs: vec![report.config_path.clone()],
                            impact_paths: vec![report.config_path.clone()],
                            remediation: "repair cargo-deny policy, dependency advisories, bans, licenses, or sources",
                            validation_commands: vec![
                                "cargo deny check".into(),
                                "rayman security audit".into(),
                            ],
                            action_class: "guarded_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "dependency_policy", error),
        }
        match SecurityAuditManager::new(&self.workspace).and_then(|manager| manager.audit()) {
            Ok(report) => {
                for finding in report.findings {
                    push_security_finding(findings, finding);
                }
            }
            Err(error) => push_source_error(findings, "security_audit", error),
        }
    }

    fn collect_audit_and_evidence_risks(&self, findings: &mut Vec<RiskFinding>) {
        match audit::audit_repository(&self.workspace) {
            Ok(audit_findings) => {
                for finding in audit_findings {
                    let path = display_path(&finding.path);
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "repository_audit",
                            severity: "high",
                            title: &finding.message,
                            source: "rayman audit",
                            evidence_refs: vec![format!("{path}:{}", finding.line)],
                            impact_paths: vec![path],
                            remediation: "triage and resolve repository audit finding or close partial/blocked",
                            validation_commands: vec!["rayman audit".into()],
                            action_class: "guarded_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "repository_audit", error),
        }
        match check_workspace_evidence(
            self.workspace.clone(),
            EvidenceCheckOptions {
                scope: "workspace".into(),
                goal_id: None,
                include_advisory: false,
            },
        ) {
            Ok(report) => {
                if !report.blockers.is_empty() {
                    for blocker in report.blockers {
                        push_finding(
                            findings,
                            FindingDraft {
                                category: "evidence_claims",
                                severity: "high",
                                title: &format!("Evidence blocker: {blocker}"),
                                source: "rayman evidence check --scope workspace",
                                evidence_refs: vec![".RaymanCodingSkill/evidence".into()],
                                impact_paths: vec![".RaymanCodingSkill/evidence".into()],
                                remediation: "replace unsupported completion claims with current evidence or blocked status",
                                validation_commands: vec![
                                    "rayman evidence check --scope workspace".into(),
                                    "rayman gate status --check".into(),
                                ],
                                action_class: "guarded_auto",
                            },
                        );
                    }
                }
            }
            Err(error) => push_source_error(findings, "evidence_claims", error),
        }
        match scan_success_claims(self.workspace.clone()) {
            Ok(blockers) => {
                for blocker in blockers {
                    push_finding(
                        findings,
                        FindingDraft {
                            category: "success_claims",
                            severity: "high",
                            title: &format!("Unsupported success claim: {blocker}"),
                            source: "rayman evidence check --scope workspace",
                            evidence_refs: vec![".RaymanCodingSkill/evidence".into()],
                            impact_paths: vec![".RaymanCodingSkill/evidence".into()],
                            remediation: "attach evidence_refs, search_effort, and counterexample challenges",
                            validation_commands: vec![
                                "rayman evidence check --scope workspace".into(),
                                "rayman gate status --check".into(),
                            ],
                            action_class: "guarded_auto",
                        },
                    );
                }
            }
            Err(error) => push_source_error(findings, "success_claims", error),
        }
    }

    fn apply_safe_finding(&self, finding: &RiskFinding) -> Result<RiskFixResult> {
        match finding.category.as_str() {
            "context_freshness" => {
                ContextKernel::new(&self.workspace)?.refresh_index()?;
                Ok(applied_result(
                    finding,
                    "refreshed workspace Context Index and Context OS snapshot",
                ))
            }
            "context_os" => {
                ContextKernel::new(&self.workspace)?.refresh_context_os("risk_fix")?;
                Ok(applied_result(
                    finding,
                    "refreshed Context OS state graph and event log",
                ))
            }
            "managed_temp" => {
                let manager = TempManager::new(&self.workspace)?;
                let report = manager.cleanup(&TempCleanupOptions {
                    completed: true,
                    stale: true,
                    all_failed: false,
                    cargo_targets: true,
                })?;
                Ok(RiskFixResult {
                    id: finding.id.clone(),
                    action: finding.remediation.clone(),
                    status: "applied".into(),
                    reason: format!(
                        "removed={} skipped_active={} skipped_foreign={} failed={}",
                        report.removed.len(),
                        report.skipped_active.len(),
                        report.skipped_foreign.len(),
                        report.failed.len()
                    ),
                    evidence_refs: vec![display_path(manager.root())],
                })
            }
            "docs_maintain" => {
                let report = docs::maintain_html_docs(DocsMaintainOptions {
                    root: self.workspace.clone(),
                    output: None,
                    prompt: None,
                    prompt_file: None,
                    model_output: None,
                    dry_run: false,
                    check: false,
                    apply_prune: false,
                })?;
                Ok(RiskFixResult {
                    id: finding.id.clone(),
                    action: finding.remediation.clone(),
                    status: "applied".into(),
                    reason: format!(
                        "docs status={} updated={} output={}",
                        report.status,
                        report.updated,
                        display_path(&report.output)
                    ),
                    evidence_refs: vec![display_path(&report.output)],
                })
            }
            _ => bail!("no safe-auto handler for category {}", finding.category),
        }
    }

    fn report_from_findings(&self, findings: Vec<RiskFinding>) -> RiskReport {
        let blocking_count = findings
            .iter()
            .filter(|finding| is_blocking(finding))
            .count();
        let required_actions = required_actions_for_findings(&findings);
        let status = if blocking_count > 0 {
            "blocked"
        } else if findings.is_empty() {
            "passed"
        } else {
            "attention"
        };
        RiskReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            state_path: display_path(&self.ledger_path),
            status: status.into(),
            finding_count: findings.len(),
            blocking_count,
            unresolved_high_critical_count: blocking_count,
            findings,
            required_actions,
            source_policy: SOURCE_POLICY.into(),
        }
    }

    fn append_ledger(&self, report: &RiskReport) -> Result<()> {
        if let Some(parent) = self.ledger_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建风险台账目录: {}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.ledger_path)
            .with_context(|| format!("无法打开风险台账: {}", self.ledger_path.display()))?;
        if report.findings.is_empty() {
            let entry = RiskLedgerEntry {
                generated_at: now_iso(),
                event: "scan_clear".into(),
                finding_count: 0,
                blocking_count: 0,
                finding: None,
            };
            writeln!(file, "{}", serde_json::to_string(&entry)?)?;
        } else {
            for finding in &report.findings {
                let entry = RiskLedgerEntry {
                    generated_at: now_iso(),
                    event: "finding_detected".into(),
                    finding_count: report.finding_count,
                    blocking_count: report.blocking_count,
                    finding: Some(finding.clone()),
                };
                writeln!(file, "{}", serde_json::to_string(&entry)?)?;
            }
        }
        Ok(())
    }
}

struct FindingDraft<'a> {
    category: &'a str,
    severity: &'a str,
    title: &'a str,
    source: &'a str,
    evidence_refs: Vec<String>,
    impact_paths: Vec<String>,
    remediation: &'a str,
    validation_commands: Vec<String>,
    action_class: &'a str,
}

fn push_finding(findings: &mut Vec<RiskFinding>, draft: FindingDraft<'_>) {
    let id = risk_id(
        draft.category,
        draft.source,
        draft.title,
        &draft.evidence_refs,
    );
    findings.push(RiskFinding {
        id,
        category: draft.category.into(),
        severity: draft.severity.into(),
        title: draft.title.into(),
        source: draft.source.into(),
        evidence_status: if draft.evidence_refs.is_empty() {
            "unknown".into()
        } else {
            "verified".into()
        },
        evidence_refs: draft.evidence_refs,
        impact_paths: dedup(draft.impact_paths),
        remediation: draft.remediation.into(),
        validation_commands: dedup(draft.validation_commands),
        action_class: draft.action_class.into(),
        status: "open".into(),
        detected_at: now_iso(),
    });
}

fn push_security_finding(findings: &mut Vec<RiskFinding>, finding: SecurityFinding) {
    let action_class = if finding.category.contains("secret")
        || finding.message.to_ascii_lowercase().contains("secret")
        || finding.message.contains("凭证")
    {
        "human_required"
    } else {
        "guarded_auto"
    };
    push_finding(
        findings,
        FindingDraft {
            category: "security_audit",
            severity: &finding.severity,
            title: &format!("{}: {}", finding.category, finding.message),
            source: "rayman security audit",
            evidence_refs: vec![format!("{}:{}", finding.path, finding.line)],
            impact_paths: vec![finding.path],
            remediation: &finding.remediation,
            validation_commands: vec!["rayman security audit".into()],
            action_class,
        },
    );
}

fn push_source_error(findings: &mut Vec<RiskFinding>, category: &str, error: anyhow::Error) {
    push_finding(
        findings,
        FindingDraft {
            category,
            severity: "critical",
            title: &format!("Risk detector failed for {category}: {error:#}"),
            source: "rayman risk scan",
            evidence_refs: Vec::new(),
            impact_paths: Vec::new(),
            remediation: "repair the detector input or command failure before trusting risk closure",
            validation_commands: vec!["rayman risk scan".into()],
            action_class: "human_required",
        },
    );
}

fn applied_result(finding: &RiskFinding, reason: &str) -> RiskFixResult {
    RiskFixResult {
        id: finding.id.clone(),
        action: finding.remediation.clone(),
        status: "applied".into(),
        reason: reason.into(),
        evidence_refs: finding.evidence_refs.clone(),
    }
}

fn risk_id(category: &str, source: &str, title: &str, refs: &[String]) -> String {
    let mut digest = Sha256::new();
    digest.update(category.as_bytes());
    digest.update(b"|");
    digest.update(source.as_bytes());
    digest.update(b"|");
    digest.update(title.as_bytes());
    digest.update(b"|");
    for item in refs {
        digest.update(item.as_bytes());
        digest.update(b"|");
    }
    let hex = format!("{:x}", digest.finalize());
    format!("risk_{}_{}", sanitize_id(category), &hex[..12])
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn is_blocking(finding: &RiskFinding) -> bool {
    severity_rank(&finding.severity) >= severity_rank("high")
        && !matches!(finding.status.as_str(), "resolved" | "verified_clear")
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn sort_findings(findings: &mut [RiskFinding]) {
    findings.sort_by(|left, right| {
        severity_rank(&right.severity)
            .cmp(&severity_rank(&left.severity))
            .then_with(|| left.category.cmp(&right.category))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn required_actions_for_findings(findings: &[RiskFinding]) -> Vec<String> {
    let blocking = findings
        .iter()
        .filter(|finding| is_blocking(finding))
        .map(|finding| {
            format!(
                "{} [{}] {} -> {}",
                finding.id, finding.severity, finding.title, finding.remediation
            )
        })
        .collect::<Vec<_>>();
    if blocking.is_empty() {
        vec!["No unresolved high or critical risks detected.".into()]
    } else {
        blocking
    }
}

fn plan_required_actions(
    safe_auto: &[RiskFinding],
    guarded_auto: &[RiskFinding],
    human_required: &[RiskFinding],
) -> Vec<String> {
    let mut actions = Vec::new();
    if !safe_auto.is_empty() {
        actions.push(format!(
            "Run rayman risk fix --safe-only to apply {} deterministic maintenance fixes.",
            safe_auto.len()
        ));
    }
    if !guarded_auto.is_empty() {
        actions.push(format!(
            "Review {} guarded remediation items and edit only with impact, regression, and validation evidence.",
            guarded_auto.len()
        ));
    }
    if !human_required.is_empty() {
        actions.push(format!(
            "{} risks require primary-agent or user-owned decisions before success.",
            human_required.len()
        ));
    }
    actions
}

fn learned_categories(findings: &[RiskFinding]) -> Vec<RiskLearnedCategory> {
    let mut by_category = std::collections::BTreeMap::<String, RiskLearnedCategory>::new();
    for finding in findings {
        let entry = by_category
            .entry(finding.category.clone())
            .or_insert_with(|| RiskLearnedCategory {
                category: finding.category.clone(),
                action_class: finding.action_class.clone(),
                validation_commands: Vec::new(),
                finding_ids: Vec::new(),
            });
        entry.finding_ids.push(finding.id.clone());
        entry
            .validation_commands
            .extend(finding.validation_commands.clone());
        entry.validation_commands = dedup(std::mem::take(&mut entry.validation_commands));
        if entry.action_class == "safe_auto" && finding.action_class != "safe_auto" {
            entry.action_class = finding.action_class.clone();
        }
    }
    by_category.into_values().collect()
}

fn json_refs(value: &Value, path: &[&str]) -> Vec<String> {
    let mut current = value;
    for segment in path {
        let Some(next) = current.get(segment) else {
            return Vec::new();
        };
        current = next;
    }
    current
        .as_str()
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn dedup(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_finding(severity: &str, action_class: &str) -> RiskFinding {
        let mut findings = Vec::new();
        push_finding(
            &mut findings,
            FindingDraft {
                category: "context_freshness",
                severity,
                title: "sample",
                source: "test",
                evidence_refs: vec!["state.json".into()],
                impact_paths: vec!["state.json".into()],
                remediation: "fix it",
                validation_commands: vec!["rayman risk verify".into()],
                action_class,
            },
        );
        findings.remove(0)
    }

    #[test]
    fn high_and_critical_findings_block_success() {
        assert!(is_blocking(&sample_finding("critical", "human_required")));
        assert!(is_blocking(&sample_finding("high", "guarded_auto")));
        assert!(!is_blocking(&sample_finding("medium", "safe_auto")));
    }

    #[test]
    fn plan_groups_findings_by_action_class() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RiskManager::new(temp.path()).unwrap();
        let plan = RiskPlan {
            workspace_path: display_path(temp.path()),
            generated_at: now_iso(),
            state_path: display_path(&manager.ledger_path),
            status: "attention".into(),
            safe_auto: vec![sample_finding("medium", "safe_auto")],
            guarded_auto: vec![sample_finding("high", "guarded_auto")],
            human_required: vec![sample_finding("critical", "human_required")],
            required_actions: plan_required_actions(
                &[sample_finding("medium", "safe_auto")],
                &[sample_finding("high", "guarded_auto")],
                &[sample_finding("critical", "human_required")],
            ),
            source_policy: SOURCE_POLICY.into(),
        };
        assert_eq!(plan.safe_auto.len(), 1);
        assert_eq!(plan.guarded_auto.len(), 1);
        assert_eq!(plan.human_required.len(), 1);
        assert_eq!(plan.required_actions.len(), 3);
    }

    #[test]
    fn ledger_records_clear_scans() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RiskManager::new(temp.path()).unwrap();
        let report = manager.report_from_findings(Vec::new());

        manager.append_ledger(&report).unwrap();

        let text = fs::read_to_string(manager.ledger_path).unwrap();
        assert!(text.contains("scan_clear"));
    }

    #[test]
    fn learned_categories_merge_validation_commands() {
        let findings = vec![
            sample_finding("high", "guarded_auto"),
            sample_finding("medium", "safe_auto"),
        ];

        let learned = learned_categories(&findings);

        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].category, "context_freshness");
        assert!(
            learned[0]
                .validation_commands
                .contains(&"rayman risk verify".to_string())
        );
    }
}
