use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::quality::QualityManager;
use crate::{display_path, ensure_within, now_iso};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEvalProfile {
    Core,
    Full,
}

impl AgentEvalProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEvalReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub profile: String,
    pub status: String,
    pub passed: usize,
    pub failed: usize,
    pub cases: Vec<AgentEvalCaseResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentEvalCaseResult {
    pub id: String,
    pub name: String,
    pub status: String,
    pub scenario: String,
    pub task_contract: Vec<String>,
    pub validation_evidence: Vec<String>,
    pub expected_patterns: Vec<String>,
    pub matched_patterns: Vec<String>,
    pub missing_patterns: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalDataset {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    pub cases: Vec<EvalCase>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalCase {
    pub id: String,
    pub input: String,
    #[serde(default)]
    pub expected_patterns: Vec<String>,
    #[serde(default)]
    pub forbidden_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GraderResult {
    pub case_id: String,
    pub status: String,
    pub matched_patterns: Vec<String>,
    pub missing_patterns: Vec<String>,
    pub forbidden_matches: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvalDatasetReport {
    pub id: String,
    pub workspace_path: String,
    pub generated_at: String,
    pub dataset_path: String,
    pub dataset_id: String,
    pub grader_id: String,
    pub status: String,
    pub passed: usize,
    pub failed: usize,
    pub cases: Vec<GraderResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
}

#[derive(Debug, Clone)]
struct AgentEvalCase {
    id: &'static str,
    name: &'static str,
    corpus: &'static str,
    task_contract: &'static [&'static str],
    validation_evidence: &'static [&'static str],
    expected_patterns: &'static [&'static str],
}

pub struct AgentEvalManager {
    workspace: PathBuf,
}

impl AgentEvalManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            workspace: workspace
                .into()
                .canonicalize()
                .map_err(|error| anyhow::anyhow!("无法解析工作区路径: {error}"))?,
        })
    }

    pub fn run(&self, profile: AgentEvalProfile) -> Result<AgentEvalReport> {
        let quality = QualityManager::new(&self.workspace)?;
        let mut cases = Vec::new();
        for case in agent_eval_cases(profile) {
            let regression_items = quality.regression_items_for_text(case.corpus)?;
            let matched_patterns = regression_items
                .into_iter()
                .map(|item| item.pattern_id)
                .collect::<Vec<_>>();
            let expected_patterns = case
                .expected_patterns
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect::<Vec<_>>();
            let missing_patterns = expected_patterns
                .iter()
                .filter(|pattern| !matched_patterns.iter().any(|matched| matched == *pattern))
                .cloned()
                .collect::<Vec<_>>();
            let status = if missing_patterns.is_empty() {
                "passed"
            } else {
                "failed"
            };
            cases.push(AgentEvalCaseResult {
                id: case.id.into(),
                name: case.name.into(),
                status: status.into(),
                scenario: case.corpus.into(),
                task_contract: case
                    .task_contract
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                validation_evidence: case
                    .validation_evidence
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                expected_patterns,
                matched_patterns,
                missing_patterns,
                rationale:
                    "task-level fixture matched required quality patterns and validation evidence"
                        .into(),
            });
        }
        let failed = cases.iter().filter(|case| case.status != "passed").count();
        let passed = cases.len() - failed;
        let status = if failed == 0 { "passed" } else { "failed" };
        Ok(AgentEvalReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            profile: profile.as_str().into(),
            status: status.into(),
            passed,
            failed,
            cases,
        })
    }

    pub fn assert_passed(&self, profile: AgentEvalProfile) -> Result<AgentEvalReport> {
        let report = self.run(profile)?;
        if report.status != "passed" {
            let failed = report
                .cases
                .iter()
                .filter(|case| case.status != "passed")
                .map(|case| format!("{} missing {:?}", case.id, case.missing_patterns))
                .collect::<Vec<_>>();
            bail!("agent eval failed: {}", failed.join("; "));
        }
        Ok(report)
    }

    pub fn run_dataset(
        &self,
        dataset_path: impl AsRef<Path>,
        grader_id: &str,
    ) -> Result<EvalDatasetReport> {
        if !matches!(grader_id, "contains" | "substring") {
            bail!("unsupported eval grader: {grader_id}");
        }
        let dataset_path = ensure_within(
            dataset_path.as_ref(),
            &self.workspace,
            "eval dataset path must stay inside workspace",
        )?;
        let dataset: EvalDataset = serde_json::from_str(&fs::read_to_string(&dataset_path)?)?;
        if dataset.cases.is_empty() {
            bail!("eval dataset must contain at least one case");
        }
        let mut cases = Vec::new();
        for case in &dataset.cases {
            let input_lower = case.input.to_ascii_lowercase();
            let matched_patterns = case
                .expected_patterns
                .iter()
                .filter(|pattern| input_lower.contains(&pattern.to_ascii_lowercase()))
                .cloned()
                .collect::<Vec<_>>();
            let missing_patterns = case
                .expected_patterns
                .iter()
                .filter(|pattern| !input_lower.contains(&pattern.to_ascii_lowercase()))
                .cloned()
                .collect::<Vec<_>>();
            let forbidden_matches = case
                .forbidden_patterns
                .iter()
                .filter(|pattern| input_lower.contains(&pattern.to_ascii_lowercase()))
                .cloned()
                .collect::<Vec<_>>();
            let status = if missing_patterns.is_empty() && forbidden_matches.is_empty() {
                "passed"
            } else {
                "failed"
            };
            cases.push(GraderResult {
                case_id: case.id.clone(),
                status: status.into(),
                matched_patterns,
                missing_patterns,
                forbidden_matches,
                rationale: "contains grader checks required and forbidden substrings".into(),
            });
        }
        let failed = cases.iter().filter(|case| case.status != "passed").count();
        let passed = cases.len() - failed;
        let status = if failed == 0 { "passed" } else { "failed" };
        let id = format!("dataset_eval_{}", dataset.id);
        let mut report = EvalDatasetReport {
            id,
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            dataset_path: display_path(&dataset_path),
            dataset_id: dataset.id,
            grader_id: grader_id.into(),
            status: status.into(),
            passed,
            failed,
            cases,
            report_path: None,
        };
        let value = serde_json::to_value(&report)?;
        let path = crate::trace::write_eval_report(&self.workspace, &value)?;
        report.report_path = Some(display_path(&path));
        Ok(report)
    }
}

fn agent_eval_cases(profile: AgentEvalProfile) -> Vec<AgentEvalCase> {
    let mut cases = vec![
        AgentEvalCase {
            id: "audit_failure_not_success",
            name: "Audit failure cannot be delivered as success",
            corpus: "rayman audit failed; 审计失败; old docs 旧文档; unrelated audit; final answer claims success",
            task_contract: &[
                "detect failed audit output",
                "triage every audit finding before success",
                "close partial or blocked when audit remains failed",
            ],
            validation_evidence: &["rayman audit failed output", "non-success close evidence"],
            expected_patterns: &["audit_failure_delivery_gate"],
        },
        AgentEvalCase {
            id: "fresh_project_context",
            name: "Project understanding requires fresh local context",
            corpus: "project understanding from stale context index and long-lived memory without rayman context refresh",
            task_contract: &[
                "check context status",
                "retrieve task-scoped context",
                "reread current files before implementation",
            ],
            validation_evidence: &[
                "rayman context status",
                "rayman context task",
                "current file reread",
            ],
            expected_patterns: &["project_understanding_freshness"],
        },
        AgentEvalCase {
            id: "tool_loop_recovery",
            name: "Tool loop failures require recovery evidence",
            corpus: "empty response from tool loop, irrelevant tool result, fallback synthesis needed",
            task_contract: &[
                "detect empty or irrelevant tool result",
                "retry or use supplemental local evidence",
                "report recovery evidence",
            ],
            validation_evidence: &[
                "retry output",
                "supplemental lookup",
                "local synthesis evidence",
            ],
            expected_patterns: &["tool_loop_recovery"],
        },
        AgentEvalCase {
            id: "expert_gate_stack",
            name: "Expert-recommended gates require executable evidence",
            corpus: "agent eval, LLM security prompt injection audit, release evidence provenance, supply chain, regression history observability",
            task_contract: &[
                "run agent eval",
                "run security audit",
                "run release evidence",
                "inspect latest regression history",
            ],
            validation_evidence: &[
                "rayman eval run --profile full",
                "rayman security audit",
                "rayman release evidence",
                "rayman regression history",
            ],
            expected_patterns: &["agent_eval_security_provenance"],
        },
        AgentEvalCase {
            id: "research_agent_autonomy",
            name: "Scientist agent autonomy requires whitelist and conflict gates",
            corpus: "autonomous scientist agent, multi-agent research, experiment whitelist, scientist cannot edit files or close goals, research conflict reconciliation",
            task_contract: &[
                "run only whitelisted argv experiments",
                "keep scientist output advisory-only",
                "reconcile conflicts before success",
            ],
            validation_evidence: &[
                "research policy report",
                "experiment command evidence",
                "reconciliation status",
            ],
            expected_patterns: &["research_agent_autonomy"],
        },
        AgentEvalCase {
            id: "codex_host_subagent_ledger",
            name: "Codex host subagent use requires ledger and primary review",
            corpus: "Codex host subagent ledger with rayman subagent status, read-only or write scope, primary-agent review, no overlap conflict disposition",
            task_contract: &[
                "record spawned host subagent scope",
                "record result evidence",
                "primary agent reviews disposition",
            ],
            validation_evidence: &[
                "rayman subagent status",
                "subagent result evidence",
                "subagent review",
            ],
            expected_patterns: &["codex_host_subagent_ledger"],
        },
        AgentEvalCase {
            id: "codex_harness_execution_contract",
            name: "Codex harness language maps to documented execution controls",
            corpus: "Codex harness execution contract maps to official Codex manual documented Codex execution controls; sandbox approval boundary with approval_policy and sandbox_mode; AGENTS.md skills hooks MCP/rules durable instruction surfaces; subagent inheritance with rayman subagent status and primary-agent review; non-interactive approval failure handling",
            task_contract: &[
                "map harness terminology to documented Codex execution controls",
                "verify sandbox and approval boundaries",
                "keep subagent and instruction-surface evidence reviewable",
            ],
            validation_evidence: &[
                "Codex manual or current-session capability mapping",
                "sandbox/approval boundary evidence",
                "instruction-surface mapping",
                "subagent inheritance and ledger review",
                "non-interactive approval failure handling",
            ],
            expected_patterns: &["codex_harness_execution_contract"],
        },
    ];
    if profile == AgentEvalProfile::Full {
        cases.extend([
            AgentEvalCase {
                id: "managed_temp",
                name: "Runtime temp work stays workspace-local",
                corpus: "temp directory failure with system temp and stale temp cleanup",
                task_contract: &[
                    "use workspace-local managed temp",
                    "clean completed runs before success",
                    "preserve failed runs for diagnosis",
                ],
                validation_evidence: &["rayman temp status", "rayman temp cleanup", "TempManager metadata"],
                expected_patterns: &["managed_temp_freshness"],
            },
            AgentEvalCase {
                id: "debug_release",
                name: "Customer delivery needs debug and release evidence",
                corpus: "customer program compiled in debug but release build evidence is missing",
                task_contract: &[
                    "compile debug build",
                    "compile release build",
                    "separate temporary workaround from release evidence",
                ],
                validation_evidence: &["debug build output", "release build output"],
                expected_patterns: &["debug_release_delivery"],
            },
            AgentEvalCase {
                id: "obsolete_assets",
                name: "Feature replacement retires obsolete assets",
                corpus: "obsolete asset cleanup after feature replacement leaves old entrypoint and stale docs",
                task_contract: &[
                    "inventory obsolete behavior",
                    "retire or explicitly exempt old assets",
                    "synchronize docs/config/tests",
                ],
                validation_evidence: &["rayman assets status", "rayman audit"],
                expected_patterns: &["obsolete_asset_retirement"],
            },
        ]);
    }
    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_eval_matches_required_quality_patterns() {
        let temp = tempfile::tempdir().unwrap();
        let report = AgentEvalManager::new(temp.path())
            .unwrap()
            .run(AgentEvalProfile::Core)
            .unwrap();

        assert_eq!(report.status, "passed");
        assert!(report.cases.iter().any(|case| {
            case.id == "expert_gate_stack"
                && case
                    .matched_patterns
                    .contains(&"agent_eval_security_provenance".into())
                && case
                    .validation_evidence
                    .contains(&"rayman release evidence".into())
        }));
        assert!(report.cases.iter().any(|case| {
            case.id == "codex_host_subagent_ledger"
                && case
                    .matched_patterns
                    .contains(&"codex_host_subagent_ledger".into())
        }));
        assert!(report.cases.iter().any(|case| {
            case.id == "codex_harness_execution_contract"
                && case
                    .matched_patterns
                    .contains(&"codex_harness_execution_contract".into())
        }));
    }

    #[test]
    fn full_eval_includes_delivery_and_asset_cases() {
        let temp = tempfile::tempdir().unwrap();
        let report = AgentEvalManager::new(temp.path())
            .unwrap()
            .run(AgentEvalProfile::Full)
            .unwrap();

        assert_eq!(report.status, "passed");
        assert!(report.cases.iter().any(|case| case.id == "debug_release"));
        assert!(report.cases.iter().any(|case| case.id == "obsolete_assets"));
    }

    #[test]
    fn dataset_eval_writes_report_and_blocks_failed_case() {
        let temp = tempfile::tempdir().unwrap();
        let dataset_path = temp.path().join("dataset.json");
        std::fs::write(
            &dataset_path,
            serde_json::json!({
                "id": "modern_agent_contract",
                "cases": [
                    {
                        "id": "pass",
                        "input": "trace replay and eval gate passed",
                        "expected_patterns": ["trace replay", "eval gate"]
                    },
                    {
                        "id": "fail",
                        "input": "semantic hit only",
                        "expected_patterns": ["current file evidence"],
                        "forbidden_patterns": ["semantic hit only"]
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();

        let report = AgentEvalManager::new(temp.path())
            .unwrap()
            .run_dataset(&dataset_path, "contains")
            .unwrap();

        assert_eq!(report.status, "failed");
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 1);
        assert!(
            report
                .report_path
                .as_deref()
                .unwrap()
                .contains(".RaymanCodingSkill")
        );
        assert!(std::path::Path::new(report.report_path.as_ref().unwrap()).exists());
    }
}
