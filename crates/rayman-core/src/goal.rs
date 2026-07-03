use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use walkdir::WalkDir;

use crate::assets::AssetRetirementManager;
use crate::auxiliary::AuxiliaryTaskStore;
use crate::context::ContextKernel;
use crate::customer_deploy::CustomerDeployManager;
use crate::evidence::{
    EvidenceResolver, EvidenceStatus, counterexample_blockers_for_success_evidence_with_resolver,
    scan_success_claims, validation_records_from_steps,
};
use crate::models::AgentManager;
use crate::project::ProjectAnalyzer;
use crate::quality::{QualityGateReport, QualityManager};
use crate::research::ResearchManager;
use crate::session::{SessionManager, manual_remote_validation_gap_blockers};
use crate::subagent::{SubagentLedgerManager, SubagentPlanRequest};
use crate::temp::TempManager;
use crate::{display_path, ensure_within, now_iso};

const STAGES: &[&str] = &[
    "intake",
    "plan",
    "impact",
    "implement",
    "validate",
    "repair",
    "doc_sync",
    "regression",
    "summary",
    "complete",
];
const ACTIVE_STATUSES: &[&str] = &["active", "in_progress"];
const TERMINAL_STATUSES: &[&str] = &["success", "blocked", "failed", "partial"];
const RECOVERABLE_STATUSES: &[&str] = &["active", "in_progress", "blocked", "partial"];
const DEFAULT_CHECKPOINT_INTERVAL_MINUTES: u64 = 10;
const DEFAULT_MAX_REPAIR_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalRequirement {
    pub id: String,
    pub priority: String,
    pub text: String,
    pub status: String,
    pub evidence: Option<String>,
    #[serde(default)]
    pub validation_commands: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationOption {
    pub label: String,
    pub value: String,
    pub recommended: bool,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClarificationChoice {
    pub id: String,
    pub title: String,
    pub default_option: String,
    pub options: Vec<ClarificationOption>,
    pub requires_customer_confirmation: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalClarification {
    pub goal_summary: String,
    pub default_choices: Vec<ClarificationChoice>,
    pub inferred_requirements: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub verification_suggestions: Vec<String>,
    pub out_of_scope: Vec<String>,
    pub customer_questions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalContract {
    pub goal: String,
    pub workflow_name: String,
    pub requirements: Vec<GoalRequirement>,
    pub acceptance_criteria: Vec<String>,
    pub verification: Vec<String>,
    pub assumptions: Vec<String>,
    #[serde(default)]
    pub clarification: GoalClarification,
    pub risks: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoalStep {
    pub stage: String,
    pub status: String,
    pub evidence: Option<String>,
    pub auxiliary_ai: Value,
    pub error: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub stderr_summary: Option<String>,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preconditions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_dispatch: Option<Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoalRecord {
    pub id: String,
    pub contract: GoalContract,
    pub status: String,
    pub current_stage: String,
    pub next_action: String,
    pub blocked_reason: Option<String>,
    pub intervention_policy: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    #[serde(default = "empty_metadata")]
    pub metadata: Value,
    pub steps: Vec<GoalStep>,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalRunUntil {
    #[default]
    NextStep,
    Blocked,
    Summary,
    Complete,
}

impl GoalRunUntil {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NextStep => "next_step",
            Self::Blocked => "blocked",
            Self::Summary => "summary",
            Self::Complete => "complete",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "next_step" => Ok(Self::NextStep),
            "blocked" => Ok(Self::Blocked),
            "summary" => Ok(Self::Summary),
            "complete" => Ok(Self::Complete),
            other => bail!("goal run until 必须是 next-step/blocked/summary/complete: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalRunOptions {
    #[serde(default)]
    pub until: GoalRunUntil,
    #[serde(default = "default_checkpoint_interval_minutes")]
    pub checkpoint_interval_minutes: u64,
    #[serde(default = "default_max_repair_attempts")]
    pub max_repair_attempts: u32,
}

impl Default for GoalRunOptions {
    fn default() -> Self {
        Self {
            until: GoalRunUntil::NextStep,
            checkpoint_interval_minutes: DEFAULT_CHECKPOINT_INTERVAL_MINUTES,
            max_repair_attempts: DEFAULT_MAX_REPAIR_ATTEMPTS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GoalRunReport {
    pub goal: GoalRecord,
    pub status: String,
    pub until: GoalRunUntil,
    pub iterations: u32,
    pub stopped_reason: String,
    pub checkpoints: Vec<Value>,
    pub resume_command: String,
    pub blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_dispatch: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct GoalManager {
    pub workspace: PathBuf,
    pub goals_dir: PathBuf,
}

impl GoalManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let goals_dir = workspace.join(".RaymanCodingSkill").join("goals");
        let goals_dir = ensure_within(&goals_dir, &workspace, "目标状态目录必须位于工作区内")?;
        Ok(Self {
            workspace,
            goals_dir,
        })
    }

    pub fn start(
        &self,
        goal: &str,
        workflow_name: &str,
        requirements: &[String],
        acceptance: &[String],
        verification: &[String],
        assumptions: &[String],
    ) -> Result<GoalRecord> {
        if goal.trim().is_empty() {
            bail!("目标不能为空");
        }
        let created_at = now_iso();
        let id = goal_id(goal, &created_at);
        let requirements = goal_requirements_or_default(goal, requirements);
        let clarification =
            build_goal_clarification(goal, &requirements, acceptance, verification, assumptions);
        let deploy_metadata =
            CustomerDeployManager::new(self.workspace.clone())?.goal_metadata(goal)?;
        let mut record = GoalRecord {
            id,
            contract: GoalContract {
                goal: goal.trim().to_string(),
                workflow_name: workflow_name.trim().to_string(),
                requirements: requirements
                    .into_iter()
                    .enumerate()
                    .map(|(index, text)| GoalRequirement {
                        id: format!("req_{}", index + 1),
                        priority: "must".into(),
                        text,
                        status: "pending".into(),
                        evidence: None,
                        validation_commands: verification.to_vec(),
                    })
                    .collect(),
                acceptance_criteria: acceptance.to_vec(),
                verification: verification.to_vec(),
                assumptions: assumptions.to_vec(),
                clarification,
                risks: high_intervention_policy(),
                created_at: created_at.clone(),
            },
            status: "active".into(),
            current_stage: "intake".into(),
            next_action:
                "run intake: confirm goal contract, must requirements, and default choices".into(),
            blocked_reason: None,
            intervention_policy: high_intervention_policy(),
            created_at: created_at.clone(),
            updated_at: created_at,
            closed_at: None,
            metadata: deploy_metadata.unwrap_or_else(|| json!({})),
            steps: Vec::new(),
        };
        apply_customer_deploy_next_action(&mut record);
        self.write_goal(&record)?;
        Ok(record)
    }

    pub fn run_next(
        &self,
        id: Option<&str>,
        manager: Option<&mut AgentManager>,
    ) -> Result<GoalRecord> {
        let mut record = self.resolve_goal(id)?;
        if TERMINAL_STATUSES.contains(&record.status.as_str()) {
            return Ok(record);
        }
        let stage = record.current_stage.clone();
        refresh_customer_deploy_metadata(&self.workspace, &mut record)?;
        let mut auxiliary_ai = Value::Null;
        let mut evidence = stage_evidence(&stage).to_string();
        let mut subagent_dispatch = None;
        if stage == "plan" || stage == "summary" {
            let task = if stage == "plan" {
                "planning"
            } else {
                "workflow_summary"
            };
            let prompt = format!(
                "Goal autonomy stage {stage} for RaymanCodingSkill.\nGoal: {}\nWorkflow: {}\nRequirements: {}",
                record.contract.goal,
                record.contract.workflow_name,
                serde_json::to_string(&record.contract.requirements)?
            );
            if let Some(agent) = manager {
                let advice = agent.auxiliary_advice(&prompt, Some(task))?;
                auxiliary_ai = agent.auxiliary_usage_json();
                if let Some(advice) = advice {
                    attach_auxiliary_advice(&mut auxiliary_ai, advice);
                }
            } else {
                auxiliary_ai = json!({
                    "status": "skipped_unavailable",
                    "task": task,
                    "skip_reason": "AgentManager unavailable; goal-runner remains fail-open"
                });
            }
        }
        if stage == "plan" {
            subagent_dispatch = self.ensure_subagent_dispatch_request(&mut record)?;
            if let Some(dispatch) = &subagent_dispatch {
                let request_id = dispatch
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                let dispatch_status = dispatch
                    .get("dispatch_status")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let auto_start_ready = dispatch
                    .get("auto_start_ready")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                evidence = format!(
                    "{evidence}; subagent_dispatch_request={request_id}; dispatch_status={dispatch_status}; auto_start_ready={auto_start_ready}"
                );
            }
        }
        if stage == "impact" {
            let impact = ProjectAnalyzer::new(&self.workspace)?.impact(&[])?;
            evidence = format!(
                "{evidence}; languages={}, tests={}, confidence={}",
                impact.project_adapters.len(),
                impact.likely_tests.len(),
                impact.confidence
            );
        }
        if ["plan", "impact", "summary"].contains(&stage.as_str())
            && let Some(deploy) = record.metadata.get("customer_deploy")
        {
            let status = deploy
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let missing = deploy
                .get("missing_required")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            evidence = if missing.is_empty() {
                format!("{evidence}; customer_deploy_config={status}")
            } else {
                format!("{evidence}; customer_deploy_config={status}; missing={missing}")
            };
        }
        record.steps.push(GoalStep {
            stage: stage.clone(),
            status: "succeeded".into(),
            evidence: Some(evidence),
            auxiliary_ai,
            error: None,
            command: None,
            exit_code: None,
            stderr_summary: None,
            attempt: next_attempt(&record, &stage),
            preconditions: stage_preconditions(&stage),
            success_criteria: stage_success_criteria(&stage),
            failure_policy: Some(stage_failure_policy(&stage).into()),
            next_step: Some(stage_next_action(&stage).into()),
            checkpoint: Some(goal_checkpoint(&record, "stage_recorded", 0)),
            resume_command: Some(Self::resume_command(&record.id)),
            blocked_kind: None,
            subagent_dispatch: subagent_dispatch.clone(),
            created_at: now_iso(),
        });
        if stage == "summary" && self.assert_success_gate(&record, None, &[]).is_err() {
            record.status = "in_progress".into();
            record.current_stage = "summary".into();
            record.next_action =
                "close goal with explicit req_id-mapped completion evidence".into();
            record.updated_at = now_iso();
            self.write_goal(&record)?;
            return Ok(record);
        }
        advance_stage(&mut record);
        apply_customer_deploy_next_action(&mut record);
        self.write_goal(&record)?;
        Ok(record)
    }

    pub fn run_layered(
        &self,
        id: Option<&str>,
        mut manager: Option<&mut AgentManager>,
        options: GoalRunOptions,
    ) -> Result<GoalRunReport> {
        let mut record = self.resolve_goal(id)?;
        let goal_id = record.id.clone();
        let mut iterations = 0u32;
        let mut checkpoints = Vec::new();
        let mut blockers = Vec::new();
        let mut subagent_dispatch = None;
        let max_iterations = STAGES.len() as u32 + options.max_repair_attempts + 8;

        let stopped_reason = loop {
            if TERMINAL_STATUSES.contains(&record.status.as_str()) {
                break format!("terminal_status:{}", record.status);
            }
            if let Some(dispatch) = self.pending_subagent_dispatch_request(&record)? {
                let request_id = dispatch
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>")
                    .to_string();
                blockers.push(format!(
                    "host_subagent_dispatch_requested: request_id={request_id}; call host spawn_agent or record unavailable/failed closeout"
                ));
                subagent_dispatch = Some(dispatch);
                break "host_subagent_dispatch_requested".into();
            }
            if let Some(blocker) = human_intervention_blocker(&record) {
                record = self.close_goal(
                    Some(&goal_id),
                    "blocked",
                    &blocker.reason,
                    std::slice::from_ref(&blocker.resume_command),
                )?;
                blockers.push(blocker.reason);
                break blocker.kind;
            }
            let failed_repairs = failed_validation_attempts(&record);
            if record.current_stage == "repair" && failed_repairs > options.max_repair_attempts {
                let reason = format!(
                    "max_repair_attempts_exceeded: failed_validation_attempts={failed_repairs}; limit={}",
                    options.max_repair_attempts
                );
                let resume = Self::resume_command(&goal_id);
                record = self.close_goal(Some(&goal_id), "blocked", &reason, &[resume])?;
                blockers.push(reason);
                break "blocked_max_repair_attempts".into();
            }
            if options.until == GoalRunUntil::Summary && record.current_stage == "summary" {
                break "reached_summary".into();
            }
            if record.current_stage == "summary"
                && record
                    .next_action
                    .contains("explicit req_id-mapped completion evidence")
            {
                break "summary_requires_completion_evidence".into();
            }
            if iterations >= max_iterations {
                let reason = format!(
                    "max_layer_iterations_exceeded: iterations={iterations}; limit={max_iterations}"
                );
                let resume = Self::resume_command(&goal_id);
                record = self.close_goal(Some(&goal_id), "blocked", &reason, &[resume])?;
                blockers.push(reason);
                break "blocked_max_iterations".into();
            }

            let agent = manager.as_deref_mut();
            record = self.run_next(Some(&goal_id), agent)?;
            iterations += 1;
            let checkpoint = self.append_long_run_checkpoint(
                &mut record,
                "layer_completed",
                iterations,
                &options,
            )?;
            checkpoints.push(checkpoint);

            if let Some(dispatch) = self.pending_subagent_dispatch_request(&record)? {
                let request_id = dispatch
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>")
                    .to_string();
                blockers.push(format!(
                    "host_subagent_dispatch_requested: request_id={request_id}; call host spawn_agent or record unavailable/failed closeout"
                ));
                subagent_dispatch = Some(dispatch);
                break "host_subagent_dispatch_requested".into();
            }

            if options.until == GoalRunUntil::NextStep {
                break "next_step_completed".into();
            }
        };

        Ok(GoalRunReport {
            status: record.status.clone(),
            until: options.until,
            iterations,
            stopped_reason,
            checkpoints,
            resume_command: Self::resume_command(&record.id),
            blockers,
            subagent_dispatch,
            goal: record,
        })
    }

    pub fn resume(
        &self,
        id: Option<&str>,
        manager: Option<&mut AgentManager>,
        options: GoalRunOptions,
    ) -> Result<GoalRunReport> {
        let mut record = if let Some(id) = id {
            self.read_goal(id)?
        } else {
            self.next_recoverable_goal()?
                .context("没有可恢复目标；请提供 --id 或先运行 rayman goal start")?
        };
        if record.status == "success" {
            bail!("目标已经 success，不能 resume: {}", record.id);
        }
        let prior_status = record.status.clone();
        if !ACTIVE_STATUSES.contains(&record.status.as_str()) {
            record.status = "in_progress".into();
            record.closed_at = None;
        }
        record.blocked_reason = None;
        record.next_action = if record.current_stage == "summary" {
            "close goal with explicit req_id-mapped completion evidence".into()
        } else {
            stage_next_action(&record.current_stage).into()
        };
        record.updated_at = now_iso();
        record.steps.push(GoalStep {
            stage: "resume".into(),
            status: "succeeded".into(),
            evidence: Some(format!(
                "goal resumed from status={prior_status}; {}",
                Self::resume_command(&record.id)
            )),
            auxiliary_ai: Value::Null,
            error: None,
            command: Some(Self::resume_command(&record.id)),
            exit_code: Some(0),
            stderr_summary: None,
            attempt: next_attempt(&record, "resume"),
            preconditions: vec![
                "goal state exists in current workspace".into(),
                "resume command names the goal id explicitly".into(),
            ],
            success_criteria: vec![
                "blocked or partial state is reopened without claiming success".into(),
                "remaining stages continue through normal evidence gates".into(),
            ],
            failure_policy: Some(
                "If resume cannot prove current goal state, keep the goal blocked and report unknown."
                    .into(),
            ),
            next_step: Some(record.next_action.clone()),
            checkpoint: Some(goal_checkpoint(&record, "goal_resumed", 0)),
            resume_command: Some(Self::resume_command(&record.id)),
            blocked_kind: None,
            subagent_dispatch: None,
            created_at: now_iso(),
        });
        self.write_goal(&record)?;
        SessionManager::new(self.workspace.clone())?.complete_goal_resume_items(
            &record.id,
            &format!("resumed with {}", Self::resume_command(&record.id)),
        )?;
        self.run_layered(Some(&record.id), manager, options)
    }

    fn ensure_subagent_dispatch_request(&self, record: &mut GoalRecord) -> Result<Option<Value>> {
        if let Some(dispatch) = self.pending_subagent_dispatch_request(record)? {
            return Ok(Some(dispatch));
        }

        let ledger = SubagentLedgerManager::new(self.workspace.clone())?;
        let mut request = ledger.plan(SubagentPlanRequest {
            task: record.contract.goal.clone(),
            paths: Vec::new(),
            read_only: false,
            max_lanes: 4,
        })?;
        let created_at = now_iso();
        let request_id = subagent_dispatch_request_id(&record.id, &created_at);
        let auto_start_ready = request
            .get("auto_start_ready")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(object) = request.as_object_mut() {
            object.insert("request_id".into(), json!(request_id));
            object.insert("goal_id".into(), json!(record.id.clone()));
            object.insert("created_at".into(), json!(created_at));
            object.insert(
                "closeout_status".into(),
                json!(if auto_start_ready {
                    "missing"
                } else {
                    "not_required"
                }),
            );
            object.insert(
                "host_thread_action".into(),
                json!(if auto_start_ready {
                    "Call multi_agent_v1.spawn_agent for recommended lanes, then record result/review; if host subagents are unavailable, record failed or unavailable closeout and continue the primary path."
                } else {
                    "No host subagent dispatch recommended for this goal stage."
                }),
            );
            add_dispatch_ids_to_lane_commands(object, &record.id, &request_id);
        }
        append_subagent_dispatch_request(record, request.clone())?;
        Ok(Some(request))
    }

    fn pending_subagent_dispatch_request(&self, record: &GoalRecord) -> Result<Option<Value>> {
        let ledger = SubagentLedgerManager::new(self.workspace.clone())?;
        for request in subagent_dispatch_requests(record).into_iter().rev() {
            if !request
                .get("auto_start_ready")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let Some(request_id) = request.get("request_id").and_then(Value::as_str) else {
                continue;
            };
            if !ledger.dispatch_request_has_closeout(&record.id, request_id)? {
                let mut request = request.clone();
                if let Some(object) = request.as_object_mut() {
                    object.insert("goal_id".into(), json!(record.id.clone()));
                    object.insert("closeout_status".into(), json!("missing"));
                }
                return Ok(Some(request));
            }
        }
        Ok(None)
    }

    pub fn record_validation_result(
        &self,
        id: Option<&str>,
        passed: bool,
        evidence: &str,
    ) -> Result<GoalRecord> {
        let mut record = self.resolve_goal(id)?;
        let status = if passed { "succeeded" } else { "failed" };
        record.steps.push(GoalStep {
            stage: "validate".into(),
            status: status.into(),
            evidence: Some(evidence.trim().to_string()),
            auxiliary_ai: Value::Null,
            error: if passed {
                None
            } else {
                Some("validation failed; entering repair loop".into())
            },
            command: first_line(evidence),
            exit_code: if passed { Some(0) } else { Some(1) },
            stderr_summary: (!passed).then(|| evidence.chars().take(500).collect()),
            attempt: next_attempt(&record, "validate"),
            preconditions: stage_preconditions("validate"),
            success_criteria: stage_success_criteria("validate"),
            failure_policy: Some(stage_failure_policy("validate").into()),
            next_step: Some(if passed {
                "continue toward documentation and regression evidence".into()
            } else {
                "run repair: fix validation failure, then validate again".into()
            }),
            checkpoint: Some(goal_checkpoint(&record, "validation_recorded", 0)),
            resume_command: Some(Self::resume_command(&record.id)),
            blocked_kind: (!passed).then(|| "repair_loop".into()),
            subagent_dispatch: None,
            created_at: now_iso(),
        });
        if passed {
            let next_stage = first_missing_pre_validation_stage(&record).unwrap_or("doc_sync");
            record.current_stage = next_stage.into();
            record.next_action = stage_next_action(next_stage).into();
        } else {
            record.current_stage = "repair".into();
            record.next_action = "run repair: fix validation failure, then validate again".into();
        }
        record.status = "active".into();
        record.updated_at = now_iso();
        apply_customer_deploy_next_action(&mut record);
        self.write_goal(&record)?;
        Ok(record)
    }

    pub fn close_goal(
        &self,
        id: Option<&str>,
        status: &str,
        evidence: &str,
        next_steps: &[String],
    ) -> Result<GoalRecord> {
        if !["success", "blocked", "failed", "partial"].contains(&status) {
            bail!("目标关闭状态必须是 success/blocked/failed/partial");
        }
        let mut record = self.resolve_goal(id)?;
        if status == "success" {
            refresh_customer_deploy_metadata(&self.workspace, &mut record)?;
        }
        let quality_report = if status == "success" {
            Some(self.assert_success_gate(&record, Some(evidence), next_steps)?)
        } else {
            None
        };
        if let Some(report) = &quality_report {
            QualityManager::new(self.workspace.clone())?.record_gate_hits(report)?;
        }
        record.status = status.to_string();
        record.current_stage = if status == "success" {
            "complete".into()
        } else {
            record.current_stage
        };
        record.next_action = if status == "success" {
            "goal complete".into()
        } else {
            next_steps
                .first()
                .cloned()
                .unwrap_or_else(|| "resume goal and complete remaining must requirements".into())
        };
        record.blocked_reason = if status == "blocked" {
            Some(evidence.to_string())
        } else {
            None
        };
        record.closed_at = Some(now_iso());
        record.updated_at = now_iso();
        for requirement in &mut record.contract.requirements {
            if status == "success" {
                requirement.status = "satisfied".into();
                requirement.evidence = Some(evidence.to_string());
            }
        }
        record.steps.push(GoalStep {
            stage: "close".into(),
            status: status.to_string(),
            evidence: Some(evidence.to_string()),
            auxiliary_ai: Value::Null,
            error: record.blocked_reason.clone(),
            command: None,
            exit_code: None,
            stderr_summary: None,
            attempt: next_attempt(&record, "close"),
            preconditions: vec![
                "goal close status is explicit".into(),
                "success close must pass evidence gates".into(),
            ],
            success_criteria: vec![
                "success has req_id-mapped evidence".into(),
                "non-success records pending recovery work".into(),
            ],
            failure_policy: Some(
                "If completion evidence is missing, close partial/blocked and keep a resume command."
                    .into(),
            ),
            next_step: Some(record.next_action.clone()),
            checkpoint: Some(goal_checkpoint(&record, "goal_closed", 0)),
            resume_command: (status != "success").then(|| Self::resume_command(&record.id)),
            blocked_kind: (status == "blocked").then(|| classify_blocker_kind(evidence).into()),
            subagent_dispatch: None,
            created_at: now_iso(),
        });
        apply_customer_deploy_next_action(&mut record);
        self.write_goal(&record)?;
        if status != "success" {
            let pending = SessionManager::new(self.workspace.clone())?;
            let resume_command = Self::resume_command(&record.id);
            let blocker_kind = classify_blocker_kind(evidence);
            let recovery_contract =
                blocker_recovery_contract(&record, status, blocker_kind, evidence, next_steps);
            pending.add_pending(
                &format!("resume goal {}", record.id),
                evidence,
                "workflow",
                "goal_close",
                "must",
                json!({
                    "goal_id": record.id,
                    "goal_status": status,
                    "blocker_kind": blocker_kind,
                    "minimum_input": recovery_contract.minimum_input,
                    "evidence_path": recovery_contract.evidence_path,
                    "resume_command": resume_command,
                    "auto_resume_strategy": recovery_contract.auto_resume_strategy,
                    "next_steps": next_steps,
                }),
            )?;
        }
        Ok(record)
    }

    fn assert_success_gate(
        &self,
        record: &GoalRecord,
        closing_evidence: Option<&str>,
        next_steps: &[String],
    ) -> Result<QualityGateReport> {
        if !record
            .steps
            .iter()
            .any(|step| step.stage == "impact" && step.status == "succeeded")
        {
            bail!("目标成功门禁未通过: 缺少 impact 阶段证据");
        }
        let gap_blockers =
            manual_remote_validation_gap_blockers(closing_evidence.unwrap_or_default(), next_steps);
        if !gap_blockers.is_empty() {
            bail!(
                "目标成功门禁未通过: manual/remote validation gap remains:\n{}",
                gap_blockers.join("\n")
            );
        }
        if let Some(blocker) = customer_deploy_success_blocker(record) {
            bail!("目标成功门禁未通过: {blocker}");
        }
        let missing_requirements =
            missing_must_requirement_evidence(&self.workspace, record, closing_evidence);
        if !missing_requirements.is_empty() {
            bail!(
                "目标成功门禁未通过: must requirement 缺少逐条完成证据: {}。完成证据必须显式引用每个 req_id，并包含当前文件路径、成功验证命令或实际 evidence artifact",
                missing_requirements.join(", ")
            );
        }
        if !record.contract.verification.is_empty()
            && !record
                .steps
                .iter()
                .any(|step| step.stage == "validate" && step.status == "succeeded")
        {
            bail!("目标成功门禁未通过: 缺少成功验证证据");
        }
        let auxiliary_blockers =
            AuxiliaryTaskStore::new(self.workspace.clone())?.success_blockers()?;
        if !auxiliary_blockers.is_empty() {
            bail!(
                "目标成功门禁未通过: 存在未解决的辅助 AI 纠偏冲突:\n{}",
                auxiliary_blockers.join("\n")
            );
        }
        if !ResearchManager::new(self.workspace.clone())?
            .unresolved_blockers()?
            .is_empty()
        {
            bail!("目标成功门禁未通过: 存在未解决的 research agent 冲突或策略违规");
        }
        let pending = SessionManager::new(self.workspace.clone())?.list_pending()?;
        if !pending.is_empty() {
            bail!("目标成功门禁未通过: 存在待完成项");
        }
        AssetRetirementManager::new(self.workspace.clone())?.assert_no_blockers()?;
        let temp_blockers = TempManager::new(self.workspace.clone())?.success_blockers()?;
        if !temp_blockers.is_empty() {
            bail!(
                "目标成功门禁未通过: 临时资产清理未完成:\n{}",
                temp_blockers.join("\n")
            );
        }
        let subagent_blockers =
            SubagentLedgerManager::new(self.workspace.clone())?.success_blockers()?;
        if !subagent_blockers.is_empty() {
            bail!(
                "目标成功门禁未通过: Codex host subagent ledger 未复核或存在冲突:\n{}",
                subagent_blockers.join("\n")
            );
        }
        let evidence_claim_blockers = scan_success_claims(self.workspace.clone())?;
        if !evidence_claim_blockers.is_empty() {
            bail!(
                "目标成功门禁未通过: 存在无证据成功声明:\n{}",
                evidence_claim_blockers.join("\n")
            );
        }
        let context = ContextKernel::new(self.workspace.clone())?.status()?;
        if context["counts"]["review_blockers"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            bail!("目标成功门禁未通过: 存在审查阻断");
        }
        if context["counts"]["audit_findings"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            bail!("目标成功门禁未通过: 存在审计发现");
        }
        if context["counts"]["context_index_stale"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            bail!(
                "目标成功门禁未通过: Context Index 过期或缺失: {}",
                context_stale_gate_detail(&context)
            );
        }
        if context["counts"]["context_os_stale"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            bail!(
                "目标成功门禁未通过: Context OS state 过期或缺失: {}",
                context_stale_gate_detail(&context)
            );
        }
        let counterexample_blockers =
            success_evidence_counterexample_blockers(&self.workspace, record, closing_evidence);
        if !counterexample_blockers.is_empty() {
            bail!(
                "目标成功门禁未通过: success evidence 缺少反例质证或搜索努力:\n{}",
                counterexample_blockers.join("\n")
            );
        }
        QualityManager::new(self.workspace.clone())?.assert_goal_gate(record, closing_evidence)
    }

    pub fn get_goal(&self, id: Option<&str>) -> Result<GoalRecord> {
        self.resolve_goal(id)
    }

    pub fn status(&self, id: Option<&str>) -> Result<Value> {
        if let Some(id) = id {
            return Ok(serde_json::to_value(self.read_goal(id)?)?);
        }
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "goals_dir": display_path(&self.goals_dir),
            "stats": self.stats()?,
            "next_goal": self.next_active_goal()?.map(serde_json::to_value).transpose()?.unwrap_or(Value::Null),
        }))
    }

    pub fn stats(&self) -> Result<Value> {
        let mut total = 0u64;
        let mut active = 0u64;
        let mut completed = 0u64;
        let mut blocked = 0u64;
        let mut failed = 0u64;
        let mut partial = 0u64;
        for goal in self.list_goals()? {
            total += 1;
            match goal.status.as_str() {
                "success" => completed += 1,
                "blocked" => blocked += 1,
                "failed" => failed += 1,
                "partial" => partial += 1,
                status if ACTIVE_STATUSES.contains(&status) => active += 1,
                _ => {}
            }
        }
        Ok(json!({
            "total": total,
            "active": active,
            "completed": completed,
            "blocked": blocked,
            "failed": failed,
            "partial": partial,
            "state_dir": display_path(&self.goals_dir),
        }))
    }

    pub fn next_active_goal(&self) -> Result<Option<GoalRecord>> {
        let mut goals = self
            .list_goals()?
            .into_iter()
            .filter(|goal| ACTIVE_STATUSES.contains(&goal.status.as_str()))
            .collect::<Vec<_>>();
        goals.sort_by_key(|goal| {
            (
                goal.contract
                    .requirements
                    .iter()
                    .any(|req| req.priority == "must" && req.status != "satisfied"),
                goal.created_at.clone(),
            )
        });
        Ok(goals.into_iter().next())
    }

    pub fn next_recoverable_goal(&self) -> Result<Option<GoalRecord>> {
        let mut goals = self
            .list_goals()?
            .into_iter()
            .filter(|goal| RECOVERABLE_STATUSES.contains(&goal.status.as_str()))
            .collect::<Vec<_>>();
        goals.sort_by_key(|goal| {
            (
                match goal.status.as_str() {
                    "active" | "in_progress" => 0,
                    "blocked" => 1,
                    "partial" => 2,
                    _ => 99,
                },
                goal.created_at.clone(),
            )
        });
        Ok(goals.into_iter().next())
    }

    pub fn resume_command(id: &str) -> String {
        format!("rayman goal resume --id {id} --until blocked")
    }

    fn append_long_run_checkpoint(
        &self,
        record: &mut GoalRecord,
        reason: &str,
        iteration: u32,
        options: &GoalRunOptions,
    ) -> Result<Value> {
        let checkpoint = goal_checkpoint(record, reason, iteration);
        let metadata = record
            .metadata
            .as_object_mut()
            .context("goal metadata must be a JSON object")?;
        let long_run = metadata
            .entry("long_run")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("goal metadata.long_run must be a JSON object")?;
        long_run.insert("until".into(), Value::String(options.until.as_str().into()));
        long_run.insert(
            "checkpoint_interval_minutes".into(),
            Value::Number(options.checkpoint_interval_minutes.into()),
        );
        long_run.insert(
            "max_repair_attempts".into(),
            Value::Number((options.max_repair_attempts as u64).into()),
        );
        long_run
            .entry("checkpoints")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .context("goal metadata.long_run.checkpoints must be an array")?
            .push(checkpoint.clone());
        record.updated_at = now_iso();
        self.write_goal(record)?;
        Ok(checkpoint)
    }

    fn resolve_goal(&self, id: Option<&str>) -> Result<GoalRecord> {
        if let Some(id) = id {
            return self.read_goal(id);
        }
        self.next_active_goal()?
            .context("没有 active/in_progress 目标；请先运行 rayman goal start")
    }

    fn list_goals(&self) -> Result<Vec<GoalRecord>> {
        if !self.goals_dir.exists() {
            return Ok(Vec::new());
        }
        WalkDir::new(&self.goals_dir)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .map(|entry| read_goal_file(entry.path()))
            .collect()
    }

    fn read_goal(&self, id: &str) -> Result<GoalRecord> {
        read_goal_file(&self.goal_path(id)?)
    }

    fn write_goal(&self, record: &GoalRecord) -> Result<()> {
        if let Some(parent) = self.goal_path(&record.id)?.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目标状态目录: {}", parent.display()))?;
        }
        fs::write(
            self.goal_path(&record.id)?,
            serde_json::to_string_pretty(record)?,
        )
        .with_context(|| format!("无法写入目标状态: {}", record.id))
    }

    fn goal_path(&self, id: &str) -> Result<PathBuf> {
        if id.trim().is_empty() || id.contains(['/', '\\', ':']) {
            bail!("无效目标 id: {id}");
        }
        ensure_within(
            &self.goals_dir.join(format!("{id}.json")),
            &self.workspace,
            "目标状态文件必须位于工作区内",
        )
    }
}

fn read_goal_file(path: &Path) -> Result<GoalRecord> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("无法读取目标状态: {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("无法解析目标状态: {}", path.display()))
}

pub fn build_goal_clarification(
    goal: &str,
    requirements: &[String],
    acceptance: &[String],
    verification: &[String],
    assumptions: &[String],
) -> GoalClarification {
    let requirements = goal_requirements_or_default(goal, requirements);
    let chinese = contains_cjk(goal)
        || requirements.iter().any(|item| contains_cjk(item))
        || acceptance.iter().any(|item| contains_cjk(item))
        || assumptions.iter().any(|item| contains_cjk(item));
    if chinese {
        build_chinese_goal_clarification(goal, &requirements, acceptance, verification, assumptions)
    } else {
        build_english_goal_clarification(goal, &requirements, acceptance, verification, assumptions)
    }
}

pub fn render_goal_clarification_text(clarification: &GoalClarification) -> String {
    let chinese = contains_cjk(&clarification.goal_summary)
        || clarification
            .default_choices
            .iter()
            .any(|choice| contains_cjk(&choice.title));
    let mut lines = Vec::new();
    lines.push(if chinese {
        "客户隐性需求澄清".to_string()
    } else {
        "Customer Requirement Clarification".to_string()
    });
    lines.push(format!(
        "{}: {}",
        if chinese {
            "目标摘要"
        } else {
            "Goal summary"
        },
        clarification.goal_summary
    ));
    lines.push(String::new());
    lines.push(if chinese {
        "默认选项".to_string()
    } else {
        "Default choices".to_string()
    });
    for choice in &clarification.default_choices {
        lines.push(format!(
            "- {} [{}]",
            choice.title,
            if choice.requires_customer_confirmation {
                if chinese {
                    "需客户确认"
                } else {
                    "requires customer confirmation"
                }
            } else if chinese {
                "默认可推进"
            } else {
                "default can proceed"
            }
        ));
        lines.push(format!(
            "  {}: {}",
            if chinese {
                "推荐默认值"
            } else {
                "Recommended default"
            },
            choice.default_option
        ));
        for option in &choice.options {
            let marker = if option.recommended { "*" } else { "-" };
            lines.push(format!(
                "  {marker} {}: {}",
                option.label, option.description
            ));
        }
    }
    append_text_section(
        &mut lines,
        if chinese {
            "推导需求"
        } else {
            "Inferred requirements"
        },
        &clarification.inferred_requirements,
    );
    append_text_section(
        &mut lines,
        if chinese {
            "验收标准"
        } else {
            "Acceptance criteria"
        },
        &clarification.acceptance_criteria,
    );
    append_text_section(
        &mut lines,
        if chinese {
            "验证建议"
        } else {
            "Verification suggestions"
        },
        &clarification.verification_suggestions,
    );
    append_text_section(
        &mut lines,
        if chinese {
            "不在范围内"
        } else {
            "Out of scope"
        },
        &clarification.out_of_scope,
    );
    append_text_section(
        &mut lines,
        if chinese {
            "需要客户确认"
        } else {
            "Questions for customer confirmation"
        },
        &clarification.customer_questions,
    );
    lines.join("\n")
}

fn append_text_section(lines: &mut Vec<String>, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(title.to_string());
    for item in items {
        lines.push(format!("- {item}"));
    }
}

fn build_chinese_goal_clarification(
    goal: &str,
    requirements: &[String],
    acceptance: &[String],
    verification: &[String],
    assumptions: &[String],
) -> GoalClarification {
    let trimmed_goal = goal.trim();
    let mut inferred_requirements = cleaned_items(requirements);
    push_unique(
        &mut inferred_requirements,
        "保留客户原始目标，并把未明确说明的业务规则记录为可确认默认值。",
    );
    push_unique(
        &mut inferred_requirements,
        "遵循现有项目权限、数据可见范围、错误处理和兼容性约束，不默认扩大行为边界。",
    );
    push_unique(
        &mut inferred_requirements,
        "异常、空数据、无权限、重复操作和部分失败场景需要给出可诊断反馈。",
    );
    if !assumptions.is_empty() {
        push_unique(
            &mut inferred_requirements,
            "显式假设作为默认值记录，后续实现和验收必须引用这些假设。",
        );
    }

    let mut acceptance_criteria = cleaned_items(acceptance);
    push_unique(
        &mut acceptance_criteria,
        "客户能看到或确认默认选项，且实现结果与确认后的目标合同一致。",
    );
    push_unique(
        &mut acceptance_criteria,
        "每个 must requirement 都能映射到当前文件、成功验证命令或实际证据 artifact。",
    );

    let mut verification_suggestions = cleaned_items(verification);
    if verification_suggestions.is_empty() {
        push_unique(
            &mut verification_suggestions,
            "运行与变更相关的最小单元测试，并在风险较高时运行项目既有门禁。",
        );
    } else {
        push_unique(
            &mut verification_suggestions,
            "保留显式验证命令；新增默认验证只能补充，不能替代客户指定验证。",
        );
    }

    let mut customer_questions = Vec::new();
    if is_high_risk_goal(trimmed_goal) {
        push_unique(
            &mut customer_questions,
            "该需求可能涉及发布、部署、删除、真实下单、付款或业务承诺，请确认是否允许进入对应外部流程。",
        );
    }
    push_unique(
        &mut customer_questions,
        "是否接受推荐默认值；如不接受，请指出需要改成的业务规则。",
    );

    GoalClarification {
        goal_summary: format!("客户希望完成：{trimmed_goal}"),
        default_choices: vec![
            clarification_choice(
                "user_scope",
                "使用人范围",
                "当前业务用户或操作人员",
                vec![
                    option(
                        "当前业务用户",
                        "current_business_user",
                        true,
                        "按现有业务角色理解，优先满足当前使用者的主要流程。",
                    ),
                    option(
                        "管理员/运营",
                        "admin_operator",
                        false,
                        "按后台管理或运营复核场景设计，需要更强权限与审计。",
                    ),
                    option(
                        "外部客户/供应商",
                        "external_party",
                        false,
                        "按外部交付或协同场景设计，需要更严格输入校验和输出说明。",
                    ),
                ],
                false,
            ),
            clarification_choice(
                "data_scope",
                "数据范围",
                "当前请求相关数据",
                vec![
                    option(
                        "当前请求相关数据",
                        "current_relevant_data",
                        true,
                        "只处理本次需求直接关联的数据，避免误碰历史或全量数据。",
                    ),
                    option(
                        "用户可见全量数据",
                        "visible_all_data",
                        false,
                        "处理当前用户可见的全部数据，适合报表、导出或批处理。",
                    ),
                    option(
                        "历史与归档数据",
                        "historical_archived_data",
                        false,
                        "纳入历史/归档记录，需要额外兼容性和性能验证。",
                    ),
                ],
                false,
            ),
            clarification_choice(
                "permission_policy",
                "权限与可见性",
                "沿用现有权限与可见范围",
                vec![
                    option(
                        "沿用现有权限",
                        "existing_permissions",
                        true,
                        "不新增越权访问，不绕过当前认证、授权和数据隔离规则。",
                    ),
                    option(
                        "管理员限定",
                        "admin_only",
                        false,
                        "只允许管理员或高权限角色使用，降低误操作风险。",
                    ),
                    option(
                        "公开可用",
                        "public_access",
                        false,
                        "对所有用户开放，需要客户明确确认安全和业务风险。",
                    ),
                ],
                false,
            ),
            clarification_choice(
                "failure_policy",
                "异常与失败处理",
                "明确报错并保留可重试状态",
                vec![
                    option(
                        "明确报错可重试",
                        "diagnostic_retryable_error",
                        true,
                        "失败时告诉用户原因和下一步，不把失败伪装成成功。",
                    ),
                    option(
                        "部分成功可追踪",
                        "partial_success_tracked",
                        false,
                        "适合批量任务，需记录哪些项目成功、失败和待重试。",
                    ),
                    option(
                        "失败即阻断",
                        "fail_closed",
                        false,
                        "适合订单、发布、删除等高风险流程，任何不确定都不继续。",
                    ),
                ],
                false,
            ),
            clarification_choice(
                "compatibility_policy",
                "兼容性",
                "保持现有接口和数据向后兼容",
                vec![
                    option(
                        "向后兼容",
                        "backward_compatible",
                        true,
                        "新增字段或行为不破坏旧数据、旧 API、旧 CLI 和既有测试。",
                    ),
                    option(
                        "允许迁移",
                        "migration_allowed",
                        false,
                        "可修改旧结构，但必须提供迁移和回滚说明。",
                    ),
                    option(
                        "允许破坏性变更",
                        "breaking_change_allowed",
                        false,
                        "只在客户明确接受影响面时使用。",
                    ),
                ],
                false,
            ),
        ],
        inferred_requirements,
        acceptance_criteria,
        verification_suggestions,
        out_of_scope: vec![
            "未明确授权的破坏性删除、真实付款、真实下单、生产发布和凭证变更。".into(),
            "未命名外部项目或客户仓库时，不把需求实现到相邻或记忆中的其他目录。".into(),
        ],
        customer_questions,
    }
}

fn build_english_goal_clarification(
    goal: &str,
    requirements: &[String],
    acceptance: &[String],
    verification: &[String],
    assumptions: &[String],
) -> GoalClarification {
    let trimmed_goal = goal.trim();
    let mut inferred_requirements = cleaned_items(requirements);
    push_unique(
        &mut inferred_requirements,
        "Preserve the original customer goal and record unstated business rules as confirmable defaults.",
    );
    push_unique(
        &mut inferred_requirements,
        "Reuse existing permission, visibility, error-handling, and compatibility boundaries unless the customer explicitly changes them.",
    );
    push_unique(
        &mut inferred_requirements,
        "Handle empty data, unauthorized access, duplicate actions, partial failures, and retryable errors with diagnosable feedback.",
    );
    if !assumptions.is_empty() {
        push_unique(
            &mut inferred_requirements,
            "Record explicit assumptions as defaults that implementation and acceptance evidence must reference.",
        );
    }

    let mut acceptance_criteria = cleaned_items(acceptance);
    push_unique(
        &mut acceptance_criteria,
        "The customer can review default choices, and implementation matches the confirmed goal contract.",
    );
    push_unique(
        &mut acceptance_criteria,
        "Every must requirement maps to current-file evidence, a successful validation command, or an evidence artifact.",
    );

    let mut verification_suggestions = cleaned_items(verification);
    if verification_suggestions.is_empty() {
        push_unique(
            &mut verification_suggestions,
            "Run the smallest relevant tests first, then existing project gates when risk is higher.",
        );
    } else {
        push_unique(
            &mut verification_suggestions,
            "Keep explicit validation commands; default verification can only add coverage, not replace customer-specified validation.",
        );
    }

    let mut customer_questions = Vec::new();
    if is_high_risk_goal(trimmed_goal) {
        push_unique(
            &mut customer_questions,
            "This request may involve release, deployment, deletion, orders, payment, or business commitments; confirm before touching that external process.",
        );
    }
    push_unique(
        &mut customer_questions,
        "Confirm whether the recommended defaults are acceptable; if not, provide the business rule to use instead.",
    );

    GoalClarification {
        goal_summary: format!("Customer wants to complete: {trimmed_goal}"),
        default_choices: vec![
            clarification_choice(
                "user_scope",
                "User scope",
                "Current business user or operator",
                vec![
                    option("Current user", "current_business_user", true, "Optimize for the current user's main workflow."),
                    option("Admin/operator", "admin_operator", false, "Treat as an admin or operations workflow that needs stronger auditability."),
                    option("External party", "external_party", false, "Treat as an external collaboration or delivery workflow with stricter validation."),
                ],
                false,
            ),
            clarification_choice(
                "data_scope",
                "Data scope",
                "Current request data",
                vec![
                    option("Current request data", "current_relevant_data", true, "Only process data directly related to this request."),
                    option("All visible data", "visible_all_data", false, "Process all data visible to the current user."),
                    option("Historical data", "historical_archived_data", false, "Include historical or archived records, requiring extra compatibility checks."),
                ],
                false,
            ),
            clarification_choice(
                "permission_policy",
                "Permission policy",
                "Reuse existing permissions",
                vec![
                    option("Existing permissions", "existing_permissions", true, "Do not bypass current authentication, authorization, or data isolation rules."),
                    option("Admin only", "admin_only", false, "Restrict the capability to admin or high-privilege users."),
                    option("Public access", "public_access", false, "Expose broadly only with explicit customer security approval."),
                ],
                false,
            ),
            clarification_choice(
                "failure_policy",
                "Failure handling",
                "Diagnostic retryable errors",
                vec![
                    option("Retryable error", "diagnostic_retryable_error", true, "Explain failure causes and next steps instead of treating failure as success."),
                    option("Tracked partial success", "partial_success_tracked", false, "For batch work, record which items succeeded, failed, or need retry."),
                    option("Fail closed", "fail_closed", false, "For high-risk release, order, deletion, or payment flows, stop on uncertainty."),
                ],
                false,
            ),
            clarification_choice(
                "compatibility_policy",
                "Compatibility",
                "Backward compatible",
                vec![
                    option("Backward compatible", "backward_compatible", true, "Do not break old data, APIs, CLI behavior, or tests."),
                    option("Migration allowed", "migration_allowed", false, "Allow old shapes to change only with migration and rollback notes."),
                    option("Breaking change allowed", "breaking_change_allowed", false, "Use only when the customer explicitly accepts the impact."),
                ],
                false,
            ),
        ],
        inferred_requirements,
        acceptance_criteria,
        verification_suggestions,
        out_of_scope: vec![
            "Destructive deletion, real payment, real ordering, production release, or credential changes without explicit approval.".into(),
            "Implementation in adjacent or remembered external repositories unless the customer names that project path.".into(),
        ],
        customer_questions,
    }
}

fn clarification_choice(
    id: &str,
    title: &str,
    default_option: &str,
    options: Vec<ClarificationOption>,
    requires_customer_confirmation: bool,
) -> ClarificationChoice {
    ClarificationChoice {
        id: id.into(),
        title: title.into(),
        default_option: default_option.into(),
        options,
        requires_customer_confirmation,
    }
}

fn option(label: &str, value: &str, recommended: bool, description: &str) -> ClarificationOption {
    ClarificationOption {
        label: label.into(),
        value: value.into(),
        recommended,
        description: description.into(),
    }
}

fn goal_requirements_or_default(goal: &str, requirements: &[String]) -> Vec<String> {
    if requirements.is_empty() {
        vec![goal.to_string()]
    } else {
        requirements.to_vec()
    }
}

fn cleaned_items(items: &[String]) -> Vec<String> {
    let mut cleaned = Vec::new();
    for item in items {
        push_unique(&mut cleaned, item.trim());
    }
    cleaned
}

fn push_unique(items: &mut Vec<String>, item: &str) {
    if item.is_empty() {
        return;
    }
    if !items.iter().any(|existing| existing == item) {
        items.push(item.to_string());
    }
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        ('\u{4e00}'..='\u{9fff}').contains(&ch)
            || ('\u{3400}'..='\u{4dbf}').contains(&ch)
            || ('\u{f900}'..='\u{faff}').contains(&ch)
    })
}

fn is_high_risk_goal(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    [
        "deploy",
        "release",
        "delete",
        "remove",
        "payment",
        "credential",
        "secret",
        "production",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
        || [
            "place order",
            "submit order",
            "real order",
            "production order",
        ]
        .iter()
        .any(|keyword| lower.contains(keyword))
        || [
            "发布",
            "上线",
            "删除",
            "移除",
            "付款",
            "支付",
            "下单",
            "真实订单",
            "提交订单",
            "生产",
            "凭证",
            "密钥",
        ]
        .iter()
        .any(|keyword| goal.contains(keyword))
}

fn empty_metadata() -> Value {
    json!({})
}

fn default_checkpoint_interval_minutes() -> u64 {
    DEFAULT_CHECKPOINT_INTERVAL_MINUTES
}

fn default_max_repair_attempts() -> u32 {
    DEFAULT_MAX_REPAIR_ATTEMPTS
}

#[derive(Debug, Clone)]
struct InterventionBlocker {
    kind: String,
    reason: String,
    resume_command: String,
}

fn human_intervention_blocker(record: &GoalRecord) -> Option<InterventionBlocker> {
    let blocker = customer_deploy_success_blocker(record)?;
    Some(InterventionBlocker {
        kind: "wait_user".into(),
        reason: format!("{blocker}; pause_long_run_until_customer_deploy_config_is_ready"),
        resume_command: GoalManager::resume_command(&record.id),
    })
}

fn attach_auxiliary_advice(auxiliary_ai: &mut Value, advice: String) {
    let metadata = json!({
        "advice": advice,
        "advisory_only": true,
        "cannot_execute_or_validate": true,
        "primary_ai_must_validate_against_files": true,
    });
    match auxiliary_ai {
        Value::Object(map) => {
            if let Value::Object(metadata_map) = metadata {
                map.extend(metadata_map);
            }
        }
        _ => *auxiliary_ai = metadata,
    }
}

fn refresh_customer_deploy_metadata(workspace: &Path, record: &mut GoalRecord) -> Result<()> {
    let Some(metadata) =
        CustomerDeployManager::new(workspace)?.goal_metadata(&record.contract.goal)?
    else {
        return Ok(());
    };
    match (&mut record.metadata, metadata) {
        (Value::Object(record_map), Value::Object(new_map)) => {
            record_map.extend(new_map);
        }
        (_, metadata) => {
            record.metadata = metadata;
        }
    }
    apply_customer_deploy_next_action(record);
    Ok(())
}

fn apply_customer_deploy_next_action(record: &mut GoalRecord) {
    let Some(deploy) = record.metadata.get("customer_deploy") else {
        return;
    };
    let missing = deploy
        .get("missing_required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if missing.is_empty() || record.status == "success" {
        return;
    }
    record.next_action = format!(
        "complete customer deploy config before release/deploy goal: {}",
        missing.join(", ")
    );
}

fn append_subagent_dispatch_request(record: &mut GoalRecord, request: Value) -> Result<()> {
    let metadata = metadata_object_mut(record);
    let dispatch = metadata
        .entry("subagent_dispatch")
        .or_insert_with(|| json!({"requests": []}));
    if !dispatch.is_object() {
        *dispatch = json!({"requests": []});
    }
    let dispatch_object = dispatch
        .as_object_mut()
        .context("subagent_dispatch metadata must be an object")?;
    let requests = dispatch_object
        .entry("requests")
        .or_insert_with(|| json!([]));
    if !requests.is_array() {
        *requests = json!([]);
    }
    requests
        .as_array_mut()
        .context("subagent_dispatch.requests must be an array")?
        .push(request);
    Ok(())
}

fn subagent_dispatch_requests(record: &GoalRecord) -> Vec<Value> {
    record
        .metadata
        .get("subagent_dispatch")
        .and_then(|value| value.get("requests"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn metadata_object_mut(record: &mut GoalRecord) -> &mut Map<String, Value> {
    if !record.metadata.is_object() {
        record.metadata = json!({});
    }
    record
        .metadata
        .as_object_mut()
        .expect("metadata was normalized to object")
}

fn add_dispatch_ids_to_lane_commands(
    request: &mut Map<String, Value>,
    goal_id: &str,
    dispatch_request_id: &str,
) {
    let Some(lanes) = request
        .get_mut("recommended_lanes")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for lane in lanes {
        let Some(object) = lane.as_object_mut() else {
            continue;
        };
        if let Some(template) = object
            .get("record_command_template")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            object.insert(
                "record_command_template".into(),
                json!(format!(
                    "{} --goal-id {} --dispatch-request-id {}",
                    template,
                    quote_cli_arg(goal_id),
                    quote_cli_arg(dispatch_request_id)
                )),
            );
        }
    }
}

fn quote_cli_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn subagent_dispatch_request_id(goal_id: &str, created_at: &str) -> String {
    let goal = goal_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(16)
        .collect::<String>();
    let suffix = created_at
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .rev()
        .take(10)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!(
        "subagent_dispatch_{}_{}",
        if goal.is_empty() { "goal" } else { &goal },
        suffix
    )
}

fn customer_deploy_success_blocker(record: &GoalRecord) -> Option<String> {
    let deploy = record.metadata.get("customer_deploy")?;
    if deploy
        .get("detected")
        .and_then(Value::as_bool)
        .is_some_and(|detected| !detected)
    {
        return None;
    }
    let status = deploy
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let missing = deploy
        .get("missing_required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if status == "ready" && missing.is_empty() {
        return None;
    }
    let config_path = deploy
        .get("config_path")
        .and_then(Value::as_str)
        .unwrap_or(".RaymanCodingSkill/customer_deploy.yaml");
    let missing_detail = if missing.is_empty() {
        "none listed".into()
    } else {
        missing.join(", ")
    };
    Some(format!(
        "customer_deploy_config status={status} blocks release/deploy goal success; missing_required={missing_detail}; configure {config_path} with rayman customer-deploy set"
    ))
}

fn missing_must_requirement_evidence(
    workspace: &Path,
    record: &GoalRecord,
    closing_evidence: Option<&str>,
) -> Vec<String> {
    record
        .contract
        .requirements
        .iter()
        .filter(|requirement| {
            requirement.priority == "must"
                && !requirement_has_completion_evidence(
                    workspace,
                    record,
                    requirement,
                    closing_evidence,
                )
        })
        .map(|requirement| requirement.id.clone())
        .collect()
}

fn requirement_has_completion_evidence(
    workspace: &Path,
    record: &GoalRecord,
    requirement: &GoalRequirement,
    closing_evidence: Option<&str>,
) -> bool {
    let stored = requirement
        .evidence
        .as_deref()
        .map(str::trim)
        .filter(|evidence| !evidence.is_empty())
        .is_some_and(|evidence| evidence_has_substantive_proof(workspace, record, evidence));
    let closing = closing_evidence
        .and_then(|evidence| requirement_evidence_segment(evidence, &requirement.id))
        .is_some_and(|evidence| evidence_has_substantive_proof(workspace, record, evidence));
    requirement.status == "satisfied" && stored || closing
}

fn requirement_evidence_segment<'a>(evidence: &'a str, req_id: &str) -> Option<&'a str> {
    let mut segment_start = None;
    let mut line_start = 0usize;
    for line in evidence.split_inclusive('\n') {
        let line_end = line_start + line.len();
        if let Some(start) = segment_start
            && evidence_req_header(line).is_some()
        {
            return Some(&evidence[start..line_start]);
        }
        if segment_start.is_none() && evidence_req_header(line) == Some(req_id) {
            segment_start = Some(line_start);
        }
        line_start = line_end;
    }
    segment_start.map(|start| &evidence[start..])
}

fn evidence_req_header(line: &str) -> Option<&str> {
    let mut trimmed = line.trim_start();
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            trimmed = rest.trim_start();
            break;
        }
    }
    let rest = trimmed.strip_prefix("req_")?;
    let digit_len = rest
        .bytes()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digit_len == 0 {
        return None;
    }
    let id_len = "req_".len() + digit_len;
    let after = &trimmed[id_len..];
    if after.is_empty()
        || after
            .chars()
            .next()
            .is_some_and(|ch| matches!(ch, ':' | '-' | ')' | ']' | ' ' | '\t' | '\r' | '\n'))
    {
        Some(&trimmed[..id_len])
    } else {
        None
    }
}

fn evidence_has_substantive_proof(workspace: &Path, record: &GoalRecord, evidence: &str) -> bool {
    let validation_records = validation_records_from_steps(record.steps.iter().map(|step| {
        (
            step.stage.as_str(),
            step.status.as_str(),
            step.exit_code,
            step.evidence.as_deref(),
        )
    }));
    EvidenceResolver::with_validation_records(workspace, validation_records)
        .map(|resolver| {
            resolver
                .claim("completion_evidence", "completion evidence", evidence)
                .status
                == EvidenceStatus::Verified
        })
        .unwrap_or(false)
}

fn success_evidence_counterexample_blockers(
    workspace: &Path,
    record: &GoalRecord,
    closing_evidence: Option<&str>,
) -> Vec<String> {
    let validation_records = validation_records_from_steps(record.steps.iter().map(|step| {
        (
            step.stage.as_str(),
            step.status.as_str(),
            step.exit_code,
            step.evidence.as_deref(),
        )
    }));
    let resolver = match EvidenceResolver::with_validation_records(workspace, validation_records) {
        Ok(resolver) => resolver,
        Err(error) => {
            return vec![format!("counterexample_evidence_resolver_error: {error}")];
        }
    };
    let mut blockers = Vec::new();
    let Some(evidence) = closing_evidence else {
        return vec![
            "missing_success_evidence: success requires current evidence plus counterexample challenge"
                .into(),
        ];
    };
    for requirement in record
        .contract
        .requirements
        .iter()
        .filter(|requirement| requirement.priority == "must")
    {
        let Some(segment) = requirement_evidence_segment(evidence, &requirement.id) else {
            blockers.push(format!(
                "{}: missing_counterexample_evidence_segment",
                requirement.id
            ));
            continue;
        };
        for blocker in
            counterexample_blockers_for_success_evidence_with_resolver(segment, &resolver)
        {
            blockers.push(format!("{}: {blocker}", requirement.id));
        }
    }
    blockers
}

fn advance_stage(record: &mut GoalRecord) {
    if record.current_stage == "repair" {
        record.current_stage = "validate".into();
    } else if record.current_stage == "validate" {
        record.current_stage = "doc_sync".into();
    } else {
        let index = STAGES
            .iter()
            .position(|stage| *stage == record.current_stage)
            .unwrap_or(0);
        record.current_stage = STAGES.get(index + 1).unwrap_or(&"complete").to_string();
    }
    if record.current_stage == "complete" {
        record.status = "active".into();
        record.closed_at = None;
        record.next_action = "close goal with explicit req_id-mapped completion evidence".into();
        for requirement in &mut record.contract.requirements {
            requirement.status = "pending_evidence".into();
            requirement.evidence = None;
        }
    } else {
        record.status = "in_progress".into();
        record.next_action = stage_next_action(&record.current_stage).into();
    }
    record.updated_at = now_iso();
}

fn stage_evidence(stage: &str) -> &'static str {
    match stage {
        "intake" => "goal contract captured with must requirements and default choices",
        "plan" => "execution plan prepared",
        "impact" => "project impact summary generated",
        "implement" => {
            "implementation stage recorded; agent should continue concrete edits when applicable"
        }
        "validate" => {
            "validation stage recorded; attach command evidence with goal run --validation"
        }
        "repair" => "repair loop recorded; rerun validation after fixes",
        "doc_sync" => "documentation synchronization stage recorded",
        "regression" => "regression planning/check stage recorded",
        "summary" => "delivery summary and unmet requirement check recorded",
        _ => "stage recorded",
    }
}

fn stage_preconditions(stage: &str) -> Vec<String> {
    match stage {
        "intake" => vec![
            "customer goal is captured as a goal contract".into(),
            "must requirements are explicit before implementation claims".into(),
        ],
        "plan" => vec![
            "current goal contract is available".into(),
            "auxiliary advice is advisory and cannot prove completion".into(),
        ],
        "impact" => vec!["current workspace files are authoritative".into()],
        "implement" => vec![
            "implementation scope is constrained to the current workspace".into(),
            "risky changes are broken into atomic operations".into(),
        ],
        "validate" => vec![
            "validation command output must be recorded with exit status".into(),
            "confidence or advisory agreement is not validation proof".into(),
        ],
        "repair" => vec![
            "a concrete validation failure or blocker exists".into(),
            "repair must be followed by validation evidence".into(),
        ],
        "doc_sync" => vec!["public docs and runtime behavior must stay aligned".into()],
        "regression" => vec!["impacted behavior has a regression plan".into()],
        "summary" => vec![
            "success claims require req_id-mapped evidence".into(),
            "unknown or blocked items must stay visible".into(),
        ],
        _ => vec!["current goal state is readable".into()],
    }
}

fn stage_success_criteria(stage: &str) -> Vec<String> {
    match stage {
        "intake" => vec!["goal contract is stored".into()],
        "plan" => vec!["next executable steps are known".into()],
        "impact" => vec!["affected modules and likely tests are recorded".into()],
        "implement" => vec!["changed paths are ready for validation".into()],
        "validate" => vec!["successful commands are recorded as evidence".into()],
        "repair" => vec!["failed validation has a targeted repair path".into()],
        "doc_sync" => vec!["docs/config/feature coverage reflect public behavior".into()],
        "regression" => vec!["old behavior and gates are checked".into()],
        "summary" => vec!["remaining unknowns, blockers, and evidence are explicit".into()],
        "close" => vec!["terminal status reflects available evidence".into()],
        _ => vec!["stage is durably recorded".into()],
    }
}

fn stage_failure_policy(stage: &str) -> &'static str {
    match stage {
        "validate" => "failed validation enters repair and cannot be reported as success",
        "repair" => "repeated repair failures block with a resume command",
        "summary" => "missing req_id evidence keeps the goal in progress or blocked",
        _ => "if evidence is missing, keep status unknown/blocked instead of success",
    }
}

fn stage_next_action(stage: &str) -> &'static str {
    match stage {
        "plan" => "run plan: call auxiliary planning and prepare steps",
        "impact" => "run impact: generate project impact summary and test selection",
        "implement" => "run implement: make concrete code/docs/config changes",
        "validate" => "run validate: execute focused and required broad gates",
        "repair" => "run repair: fix validation failures and retry",
        "doc_sync" => "run doc_sync: synchronize docs and usage notes",
        "regression" => "run regression: check old behavior and compatibility",
        "summary" => "run summary: call workflow_summary and record delivery evidence",
        "complete" => "goal complete",
        _ => "run next stage",
    }
}

fn first_missing_pre_validation_stage(record: &GoalRecord) -> Option<&'static str> {
    ["plan", "impact", "implement"]
        .iter()
        .copied()
        .find(|stage| !stage_succeeded(record, stage))
}

fn stage_succeeded(record: &GoalRecord, stage: &str) -> bool {
    record
        .steps
        .iter()
        .any(|step| step.stage == stage && step.status == "succeeded")
}

fn context_stale_gate_detail(context: &Value) -> String {
    let details = &context["next_record"]["details"];
    let groups = [
        ("changed", "changed_files"),
        ("missing", "missing_files"),
        ("new", "new_files"),
    ];
    let stale_files = groups
        .iter()
        .filter_map(|(label, key)| {
            let files = details
                .get(*key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .take(8)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (!files.is_empty()).then(|| format!("{label}={}", files.join(",")))
        })
        .collect::<Vec<_>>()
        .join("; ");
    if !stale_files.is_empty() {
        return format!("{stale_files}; run rayman context refresh and reread current files");
    }
    if let Some(reason) = details.get("reason").and_then(Value::as_str) {
        return format!("{reason}; run rayman context refresh and reread current files");
    }
    "run rayman context refresh and reread current files".into()
}

fn next_attempt(record: &GoalRecord, stage: &str) -> u32 {
    record
        .steps
        .iter()
        .filter(|step| step.stage == stage)
        .count() as u32
        + 1
}

fn failed_validation_attempts(record: &GoalRecord) -> u32 {
    record
        .steps
        .iter()
        .filter(|step| step.stage == "validate" && step.status == "failed")
        .count() as u32
}

fn goal_checkpoint(record: &GoalRecord, reason: &str, iteration: u32) -> Value {
    json!({
        "goal_id": record.id.clone(),
        "status": record.status.clone(),
        "current_stage": record.current_stage.clone(),
        "next_action": record.next_action.clone(),
        "reason": reason,
        "iteration": iteration,
        "resume_command": GoalManager::resume_command(&record.id),
        "created_at": now_iso(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockerRecoveryContract {
    minimum_input: String,
    evidence_path: String,
    auto_resume_strategy: String,
}

fn blocker_recovery_contract(
    record: &GoalRecord,
    status: &str,
    blocker_kind: &str,
    evidence: &str,
    next_steps: &[String],
) -> BlockerRecoveryContract {
    let resume_command = GoalManager::resume_command(&record.id);
    let minimum_input = minimum_input_for_blocker(blocker_kind, evidence, next_steps);
    let evidence_path = format!(".RaymanCodingSkill/goals/{}.json", record.id);
    let auto_resume_strategy = match blocker_kind {
        "wait_user" => format!(
            "After the user-owned input is provided, run `{resume_command}`; continue non-human executable stages until the next true blocker or evidence boundary."
        ),
        "wait_external" => format!(
            "After the external service or party is available, run `{resume_command}`; continue non-human executable stages until the next true blocker or evidence boundary."
        ),
        _ => format!(
            "Repair the hard gate with local executable actions, record validation evidence, then run `{resume_command}`; if no local repair remains, keep status={status} with updated evidence."
        ),
    };
    BlockerRecoveryContract {
        minimum_input,
        evidence_path,
        auto_resume_strategy,
    }
}

fn minimum_input_for_blocker(blocker_kind: &str, evidence: &str, next_steps: &[String]) -> String {
    if let Some(next_step) = next_steps
        .iter()
        .map(|step| step.trim())
        .find(|step| !step.is_empty())
    {
        return next_step.to_string();
    }
    let summary = first_line(evidence).unwrap_or_else(|| "blocked goal".into());
    match blocker_kind {
        "wait_user" => format!("Provide the user-owned input or confirmation for: {summary}"),
        "wait_external" => {
            format!("Provide evidence that the external dependency is available for: {summary}")
        }
        _ => format!("Provide passing gate evidence or a narrower failing command for: {summary}"),
    }
}

fn classify_blocker_kind(evidence: &str) -> &'static str {
    let lower = evidence.to_ascii_lowercase();
    if lower.contains("credential")
        || lower.contains("api key")
        || lower.contains("token")
        || lower.contains("customer")
        || lower.contains("permission")
        || lower.contains("confirm")
        || lower.contains("approval")
        || lower.contains("business decision")
        || lower.contains("captcha")
        || lower.contains("recaptcha")
        || lower.contains("login")
        || lower.contains("eula")
        || evidence.contains("凭证")
        || evidence.contains("密钥")
        || evidence.contains("确认")
        || evidence.contains("权限")
        || evidence.contains("登录")
        || evidence.contains("验证码")
        || evidence.contains("人工")
    {
        "wait_user"
    } else if lower.contains("service")
        || lower.contains("network")
        || lower.contains("rate limit")
        || lower.contains("external")
        || lower.contains("remote")
        || lower.contains("vendor")
        || lower.contains("portal")
        || evidence.contains("外部")
        || evidence.contains("网络")
        || evidence.contains("服务")
    {
        "wait_external"
    } else {
        "hard_gate"
    }
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

fn goal_id(goal: &str, created_at: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(goal.as_bytes());
    hasher.update(created_at.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("goal_{}", &digest[..12])
}

fn high_intervention_policy() -> Vec<String> {
    vec![
        "missing permission or credentials".into(),
        "destructive operation requires approval".into(),
        "conflicting customer requirements".into(),
        "unclear project scope risks modifying the wrong workspace".into(),
        "external service unavailable with no local fallback".into(),
        "same failure reached retry limit".into(),
        "business decision requires customer judgment".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn success_evidence(req_evidence: &str) -> String {
        format!(
            "{req_evidence}\nchecked: {req_evidence}\nnegative check: stale success evidence not found; evidence: {req_evidence}"
        )
    }

    #[test]
    fn goal_contract_round_trips_and_closes() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "ship feature",
                "standard_development",
                &[],
                &["tests pass".into()],
                &["cargo test".into()],
                &[],
            )
            .unwrap();

        let saved = manager.status(Some(&goal.id)).unwrap();
        assert_eq!(saved["contract"]["goal"], "ship feature");
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager
            .record_validation_result(Some(&goal.id), true, "cargo test passed")
            .unwrap();

        let closed = manager
            .close_goal(
                Some(&goal.id),
                "success",
                &success_evidence(
                    "req_1: crates/rayman-core/src/goal.rs updated and cargo test passed",
                ),
                &[],
            )
            .unwrap();
        assert_eq!(closed.status, "success");
        assert_eq!(closed.current_stage, "complete");
    }

    #[test]
    fn goal_close_success_rejects_req_id_only_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("must requirement"));
        assert!(error.contains("req_1"));
        assert!(error.contains("当前文件路径"));
    }

    #[test]
    fn goal_close_success_accepts_req_evidence_with_path_or_validation() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let closed = manager
            .close_goal(
                Some(&goal.id),
                "success",
                &success_evidence("req_1: README.md updated"),
                &[],
            )
            .unwrap();

        assert_eq!(closed.status, "success");
    }

    #[test]
    fn goal_close_success_blocks_missing_counterexample_challenge() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1: README.md updated", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("success evidence 缺少反例质证或搜索努力"));
        assert!(error.contains("missing_counterexample_challenge"));
    }

    #[test]
    fn goal_close_success_requires_counterexample_for_each_must_requirement() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "ship feature",
                "standard_development",
                &["first must".into(), "second must".into()],
                &[],
                &[],
                &[],
            )
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let evidence = [
            success_evidence("req_1: README.md updated"),
            "req_2: README.md updated".into(),
        ]
        .join("\n");
        let error = manager
            .close_goal(Some(&goal.id), "success", &evidence, &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("req_2: missing_search_effort"));
        assert!(error.contains("req_2: missing_counterexample_challenge"));
    }

    #[test]
    fn goal_close_success_rejects_nonexistent_path_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: docs/MISSING.md updated",
                &[],
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("req_1"));
        assert!(error.contains("当前文件路径"));
    }

    #[test]
    fn goal_close_success_rejects_negated_validation_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager
            .record_validation_result(Some(&goal.id), true, "cargo test passed")
            .unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1: cargo test not run", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("req_1"));
    }

    #[test]
    fn goal_close_success_blocks_unverified_success_claim_ledger() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        fs::write(
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

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1: README.md updated", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("无证据成功声明"));
        assert!(error.contains("unverified_success_claim"));
    }

    #[test]
    fn goal_close_success_rejects_req_id_prefix_collision() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let requirements = (1..=10)
            .map(|index| format!("must requirement {index}"))
            .collect::<Vec<_>>();
        let goal = manager
            .start(
                "ship feature",
                "standard_development",
                &requirements,
                &[],
                &[],
                &[],
            )
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let evidence = std::iter::once("req_10: README.md updated".to_string())
            .chain((2..10).map(|index| format!("req_{index}: README.md updated")))
            .collect::<Vec<_>>()
            .join("\n");

        let error = manager
            .close_goal(Some(&goal.id), "success", &evidence, &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("req_1"));
        assert!(!error.contains("req_10。"));
    }

    #[test]
    fn goal_close_success_blocks_manual_remote_validation_gap() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: docs/CLI.md updated, but remote validation gap remains",
                &[],
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("summary_validation_gap"));

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: docs/CLI.md updated and cargo test passed",
                &["需要远端验证后才能确认".into()],
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("next_step_validation_gap"));
    }

    #[test]
    fn release_goal_injects_customer_deploy_metadata_and_missing_next_action() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();

        let goal = manager
            .start("发布客户项目", "standard_development", &[], &[], &[], &[])
            .unwrap();

        assert_eq!(goal.metadata["customer_deploy"]["detected"], true);
        assert_eq!(goal.metadata["customer_deploy"]["status"], "missing");
        assert!(goal.next_action.contains("customer deploy config"));
        assert!(goal.next_action.contains("build_command"));
    }

    #[test]
    fn release_goal_uses_ready_customer_deploy_config() {
        let temp = tempfile::tempdir().unwrap();
        crate::customer_deploy::CustomerDeployManager::new(temp.path())
            .unwrap()
            .set(crate::customer_deploy::CustomerDeployUpdate {
                environment: Some("prod".into()),
                build_command: Some("cargo build --release".into()),
                test_commands: vec!["cargo test".into()],
                deploy_command: Some("scripts/deploy.ps1".into()),
                credential_refs: vec![crate::customer_deploy::CredentialRef {
                    env: Some("PROD_TOKEN".into()),
                    credential_ref: None,
                }],
                ..Default::default()
            })
            .unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();

        let goal = manager
            .start(
                "deploy customer project",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();

        assert_eq!(goal.metadata["customer_deploy"]["status"], "ready");
        assert_eq!(
            goal.next_action,
            "run intake: confirm goal contract, must requirements, and default choices"
        );
    }

    #[test]
    fn minimal_chinese_goal_builds_utf8_default_clarification() {
        let clarification = build_goal_clarification("支持导出客户订单", &[], &[], &[], &[]);

        assert!(clarification.goal_summary.contains("支持导出客户订单"));
        assert_eq!(
            clarification
                .inferred_requirements
                .first()
                .map(String::as_str),
            Some("支持导出客户订单")
        );
        assert!(
            clarification
                .default_choices
                .iter()
                .any(|choice| choice.title == "权限与可见性"
                    && choice.default_option == "沿用现有权限与可见范围")
        );
        assert!(render_goal_clarification_text(&clarification).contains("客户隐性需求澄清"));
        assert!(
            !clarification
                .customer_questions
                .iter()
                .any(|question| question.contains("付款") || question.contains("下单"))
        );
    }

    #[test]
    fn high_risk_goal_requests_explicit_customer_confirmation() {
        let clarification = build_goal_clarification("上线并允许真实下单", &[], &[], &[], &[]);

        assert!(
            clarification
                .customer_questions
                .iter()
                .any(|question| question.contains("发布") && question.contains("下单"))
        );
    }

    #[test]
    fn explicit_contract_items_are_preserved_in_clarification() {
        let requirements = vec!["必须导出 XLSX".to_string()];
        let acceptance = vec!["导出文件包含订单号".to_string()];
        let verification = vec!["cargo test -p exporter".to_string()];
        let clarification = build_goal_clarification(
            "导出订单",
            &requirements,
            &acceptance,
            &verification,
            &["默认按当前用户权限".into()],
        );

        assert_eq!(
            clarification
                .inferred_requirements
                .first()
                .map(String::as_str),
            Some("必须导出 XLSX")
        );
        assert_eq!(
            clarification
                .acceptance_criteria
                .first()
                .map(String::as_str),
            Some("导出文件包含订单号")
        );
        assert_eq!(
            clarification
                .verification_suggestions
                .first()
                .map(String::as_str),
            Some("cargo test -p exporter")
        );
    }

    #[test]
    fn goal_start_persists_clarification_and_default_choice_next_action() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("导出客户订单", "standard_development", &[], &[], &[], &[])
            .unwrap();

        assert!(
            goal.contract
                .clarification
                .goal_summary
                .contains("导出客户订单")
        );
        assert_eq!(
            goal.next_action,
            "run intake: confirm goal contract, must requirements, and default choices"
        );

        let loaded = manager.get_goal(Some(&goal.id)).unwrap();
        assert_eq!(loaded.contract.clarification, goal.contract.clarification);
    }

    #[test]
    fn goal_clarify_preview_matches_goal_start_default_clarification() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();

        let preview = build_goal_clarification("支持导出客户订单", &[], &[], &[], &[]);
        let started = manager
            .start(
                "支持导出客户订单",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();

        assert_eq!(preview, started.contract.clarification);
    }

    #[test]
    fn release_goal_success_close_blocks_missing_customer_deploy_config() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "deploy customer project",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1: README.md updated", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("customer_deploy_config"));
        assert!(error.contains("build_command"));
    }

    #[test]
    fn release_goal_success_close_allows_ready_customer_deploy_config() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        crate::customer_deploy::CustomerDeployManager::new(temp.path())
            .unwrap()
            .set(crate::customer_deploy::CustomerDeployUpdate {
                environment: Some("prod".into()),
                build_command: Some("cargo build --release".into()),
                test_commands: vec!["cargo test".into()],
                deploy_command: Some("scripts/deploy.ps1".into()),
                credential_refs: vec![crate::customer_deploy::CredentialRef {
                    env: Some("PROD_TOKEN".into()),
                    credential_ref: None,
                }],
                ..Default::default()
            })
            .unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "deploy customer project",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();

        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let closed = manager
            .close_goal(
                Some(&goal.id),
                "success",
                &success_evidence("req_1: README.md updated"),
                &[],
            )
            .unwrap();

        assert_eq!(closed.status, "success");
    }

    #[test]
    fn non_release_goal_does_not_inject_customer_deploy_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();

        let goal = manager
            .start("fix parser", "standard_development", &[], &[], &[], &[])
            .unwrap();

        assert!(goal.metadata.get("customer_deploy").is_none());
    }

    #[test]
    fn old_goal_json_defaults_metadata_and_clarification() {
        let text = r#"{
  "id": "goal_old",
  "contract": {
    "goal": "old",
    "workflow_name": "standard_development",
    "requirements": [],
    "acceptance_criteria": [],
    "verification": [],
    "assumptions": [],
    "risks": [],
    "created_at": "2026-01-01T00:00:00.000000Z"
  },
  "status": "active",
  "current_stage": "intake",
  "next_action": "run",
  "blocked_reason": null,
  "intervention_policy": [],
  "created_at": "2026-01-01T00:00:00.000000Z",
  "updated_at": "2026-01-01T00:00:00.000000Z",
  "closed_at": null,
  "steps": []
}"#;

        let goal: GoalRecord = serde_json::from_str(text).unwrap();

        assert_eq!(goal.metadata, json!({}));
        assert_eq!(goal.contract.clarification, GoalClarification::default());
    }

    #[test]
    fn success_close_blocks_on_unresolved_auxiliary_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "ship feature",
                "standard_development",
                &[],
                &["tests pass".into()],
                &["cargo test".into()],
                &[],
            )
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager
            .record_validation_result(Some(&goal.id), true, "cargo test passed")
            .unwrap();

        let store = AuxiliaryTaskStore::new(temp.path()).unwrap();
        let task = store
            .enqueue(
                "code_generation",
                "prompt",
                Some("primary"),
                0,
                Some("aux".into()),
            )
            .unwrap();
        store
            .complete(
                task,
                Vec::new(),
                crate::auxiliary::AuxiliaryReconciliation {
                    primary_correct: false,
                    correction_required: true,
                    risk_level: "high".into(),
                    rationale: "primary missed a must requirement".into(),
                    suggested_fix: Some("repair the output".into()),
                    tests: vec!["cargo test".into()],
                    raw_response: None,
                },
            )
            .unwrap();
        store.reconcile_ready().unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: crates/rayman-core/src/goal.rs updated and cargo test passed",
                &[],
            )
            .unwrap_err();

        assert!(error.to_string().contains("辅助 AI 纠偏冲突"));
    }

    #[test]
    fn success_close_blocks_on_malformed_auxiliary_task_json() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "ship feature",
                "standard_development",
                &[],
                &["tests pass".into()],
                &["cargo test".into()],
                &[],
            )
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager
            .record_validation_result(Some(&goal.id), true, "cargo test passed")
            .unwrap();
        let task_dir = temp
            .path()
            .join(".RaymanCodingSkill")
            .join("auxiliary")
            .join("tasks");
        std::fs::create_dir_all(&task_dir).unwrap();
        std::fs::write(task_dir.join("bad.json"), "{not json").unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: crates/rayman-core/src/goal.rs updated and cargo test passed",
                &[],
            )
            .unwrap_err();

        assert!(error.to_string().contains("auxiliary_task_parse_error"));
    }

    #[test]
    fn success_close_blocks_on_unreviewed_subagent_ledger() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "ship feature",
                "standard_development",
                &[],
                &["tests pass".into()],
                &["cargo test".into()],
                &[],
            )
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager
            .record_validation_result(Some(&goal.id), true, "cargo test passed")
            .unwrap();
        crate::subagent::SubagentLedgerManager::new(temp.path())
            .unwrap()
            .record(crate::subagent::SubagentRecordRequest {
                host_agent_id: "agent-1".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "inspect goal close".into(),
                boundary: "read-only".into(),
                read_only: true,
                write_paths: Vec::new(),
            })
            .unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: crates/rayman-core/src/goal.rs updated and cargo test passed",
                &[],
            )
            .unwrap_err();

        assert!(error.to_string().contains("subagent ledger"));
    }

    #[test]
    fn success_close_requires_req_id_mapped_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "verified", &[])
            .unwrap_err();

        assert!(error.to_string().contains("req_1"));
    }

    #[test]
    fn success_close_reports_stale_context_files() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        fs::write(temp.path().join("README.md"), "# changed").unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: README.md updated and cargo test passed",
                &[],
            )
            .unwrap_err();

        let text = error.to_string();
        assert!(text.contains("Context Index"));
        assert!(text.contains("changed=README.md"));
        assert!(text.contains("rayman context refresh"));
    }

    #[test]
    fn success_close_blocks_on_obsolete_asset_candidate() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("old.md"), "old behavior\n").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        crate::assets::AssetRetirementManager::new(temp.path())
            .unwrap()
            .retire(crate::assets::AssetRetireRequest {
                path: PathBuf::from("old.md"),
                replacement_behavior: "new.md".into(),
                deletion_reason: "replaced by current behavior".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                "req_1: old.md retired and cargo test passed",
                &[],
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("obsolete asset retirement blockers")
        );
    }

    #[test]
    fn success_close_blocks_on_completed_managed_temp() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        let temp_manager = TempManager::new(temp.path()).unwrap();
        let run = temp_manager.run_dir("finished validation").unwrap();
        run.complete().unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1: README.md updated", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("临时资产清理未完成"));
        assert!(error.contains("rayman temp cleanup --completed"));
    }

    #[test]
    fn success_close_blocks_on_all_managed_temp_cleanup_states() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        let temp_manager = TempManager::new(temp.path()).unwrap();
        let _active = temp_manager.run_dir("active validation").unwrap();
        let completed = temp_manager.run_dir("completed validation").unwrap();
        completed.complete().unwrap();
        let stale = temp_manager.run_dir("stale validation").unwrap();
        stale.complete().unwrap();
        make_temp_run_stale(&stale);
        let failed = temp_manager.run_dir("failed validation").unwrap();
        failed.fail().unwrap();
        fs::create_dir_all(temp_manager.root().join("runs").join("foreign")).unwrap();

        let error = manager
            .close_goal(Some(&goal.id), "success", "req_1: README.md updated", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("临时资产清理未完成"));
        assert!(error.contains("managed_temp_active"));
        assert!(error.contains("managed_temp_completed"));
        assert!(error.contains("managed_temp_stale"));
        assert!(error.contains("managed_temp_failed"));
        assert!(error.contains("managed_temp_foreign"));
    }

    #[test]
    fn success_close_blocks_on_quality_gate_missing_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(temp.path().join("docs").join("CLI.md"), "# CLI").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "answer latest president news",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let error = manager
            .close_goal(
                Some(&goal.id),
                "success",
                &success_evidence("req_1: docs/CLI.md updated and cargo test passed"),
                &[],
            )
            .unwrap_err();

        assert!(error.to_string().contains("质量模式硬门禁"));
    }

    #[test]
    fn next_active_goal_prefers_oldest_must_goal() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let first = manager
            .start("first", "standard_development", &[], &[], &[], &[])
            .unwrap();
        let _second = manager
            .start("second", "standard_development", &[], &[], &[], &[])
            .unwrap();

        assert_eq!(manager.next_active_goal().unwrap().unwrap().id, first.id);
    }

    #[test]
    fn blocked_goal_creates_pending_resume_record() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("needs key", "standard_development", &[], &[], &[], &[])
            .unwrap();

        let blocked = manager
            .close_goal(
                Some(&goal.id),
                "blocked",
                "missing credentials",
                &["provide API key".into()],
            )
            .unwrap();

        assert_eq!(blocked.status, "blocked");
        let pending = SessionManager::new(temp.path())
            .unwrap()
            .list_pending()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["metadata"]["goal_id"], goal.id);
        assert_eq!(
            pending[0]["metadata"]["resume_command"],
            GoalManager::resume_command(&goal.id)
        );
        assert_eq!(pending[0]["metadata"]["blocker_kind"], "wait_user");
        assert_eq!(pending[0]["metadata"]["minimum_input"], "provide API key");
        assert_eq!(
            pending[0]["metadata"]["evidence_path"],
            format!(".RaymanCodingSkill/goals/{}.json", goal.id)
        );
        assert!(
            pending[0]["metadata"]["auto_resume_strategy"]
                .as_str()
                .unwrap()
                .contains("continue non-human executable stages")
        );
    }

    #[test]
    fn blocker_kind_classification_keeps_wait_ownership_explicit() {
        assert_eq!(
            classify_blocker_kind("missing API key for provider"),
            "wait_user"
        );
        assert_eq!(
            classify_blocker_kind("external service unavailable with no local fallback"),
            "wait_external"
        );
        assert_eq!(
            classify_blocker_kind("cargo test failed: validation gate is still red"),
            "hard_gate"
        );
    }

    #[test]
    fn hard_gate_pending_record_has_full_recovery_contract() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "fix failing gate",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();

        let blocked = manager
            .close_goal(
                Some(&goal.id),
                "blocked",
                "cargo test failed: validation gate is still red",
                &[],
            )
            .unwrap();

        assert_eq!(blocked.status, "blocked");
        let pending = SessionManager::new(temp.path())
            .unwrap()
            .list_pending()
            .unwrap();
        let metadata = &pending[0]["metadata"];
        assert_eq!(metadata["blocker_kind"], "hard_gate");
        assert!(
            metadata["minimum_input"]
                .as_str()
                .unwrap()
                .contains("passing gate evidence")
        );
        assert_eq!(
            metadata["evidence_path"],
            format!(".RaymanCodingSkill/goals/{}.json", goal.id)
        );
        assert_eq!(
            metadata["resume_command"],
            GoalManager::resume_command(&goal.id)
        );
        assert!(
            metadata["auto_resume_strategy"]
                .as_str()
                .unwrap()
                .contains("Repair the hard gate")
        );
    }

    #[test]
    fn layered_run_stops_at_summary_without_claiming_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("build cli", "standard_development", &[], &[], &[], &[])
            .unwrap();

        let report = manager
            .run_layered(
                Some(&goal.id),
                None,
                GoalRunOptions {
                    until: GoalRunUntil::Blocked,
                    checkpoint_interval_minutes: 0,
                    max_repair_attempts: 3,
                },
            )
            .unwrap();

        assert_eq!(report.goal.status, "in_progress");
        assert_eq!(report.goal.current_stage, "summary");
        assert_eq!(
            report.stopped_reason,
            "summary_requires_completion_evidence"
        );
        assert!(report.iterations > 1);
        assert!(!report.checkpoints.is_empty());
        assert_eq!(report.resume_command, GoalManager::resume_command(&goal.id));
    }

    #[test]
    fn layered_run_requests_host_subagent_dispatch_for_broad_goal() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "全仓审计修复并闭环 gate",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();

        let report = manager
            .run_layered(
                Some(&goal.id),
                None,
                GoalRunOptions {
                    until: GoalRunUntil::Blocked,
                    checkpoint_interval_minutes: 0,
                    max_repair_attempts: 3,
                },
            )
            .unwrap();

        assert_eq!(report.stopped_reason, "host_subagent_dispatch_requested");
        let dispatch = report.subagent_dispatch.as_ref().expect("dispatch request");
        assert_eq!(dispatch["auto_start_ready"].as_bool(), Some(true));
        assert_eq!(
            dispatch["auto_start_contract"]["host_tool"].as_str(),
            Some("multi_agent_v1.spawn_agent")
        );
        assert!(
            dispatch["recommended_lanes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|lane| lane["record_command_template"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("--dispatch-request-id"))
        );
        assert_eq!(report.goal.current_stage, "impact");
        assert_eq!(
            report.goal.steps.last().unwrap().subagent_dispatch.as_ref(),
            Some(dispatch)
        );
    }

    #[test]
    fn tiny_goal_does_not_request_host_subagent_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("note.md"), "# note\n").unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("fix typo", "standard_development", &[], &[], &[], &[])
            .unwrap();

        let report = manager
            .run_layered(
                Some(&goal.id),
                None,
                GoalRunOptions {
                    until: GoalRunUntil::Summary,
                    checkpoint_interval_minutes: 0,
                    max_repair_attempts: 3,
                },
            )
            .unwrap();

        assert_ne!(report.stopped_reason, "host_subagent_dispatch_requested");
        assert!(report.subagent_dispatch.is_none());
        let requests = subagent_dispatch_requests(&report.goal);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["auto_start_ready"].as_bool(), Some(false));
    }

    #[test]
    fn host_subagent_unavailable_closeout_allows_goal_to_continue() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "全仓审计修复并闭环 gate",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let first = manager
            .run_layered(
                Some(&goal.id),
                None,
                GoalRunOptions {
                    until: GoalRunUntil::Blocked,
                    checkpoint_interval_minutes: 0,
                    max_repair_attempts: 3,
                },
            )
            .unwrap();
        let request_id = first.subagent_dispatch.as_ref().unwrap()["request_id"]
            .as_str()
            .unwrap()
            .to_string();

        let ledger = SubagentLedgerManager::new(temp.path()).unwrap();
        let record = ledger
            .record(crate::subagent::SubagentRecordRequest {
                host_agent_id: "agent-unavailable".into(),
                goal_id: Some(goal.id.clone()),
                dispatch_request_id: Some(request_id),
                nickname: Some("unavailable".into()),
                task: "host subagent unavailable".into(),
                boundary: "record unavailable host-subagent lane".into(),
                read_only: true,
                write_paths: Vec::new(),
            })
            .unwrap();
        ledger
            .record_result(
                &record.id,
                crate::subagent::SubagentResultRequest {
                    status: "failed".into(),
                    summary: "host subagent unavailable; primary path continued".into(),
                    evidence_refs: vec!["host_tool=unavailable".into()],
                    changed_paths: Vec::new(),
                },
            )
            .unwrap();
        ledger
            .record_review(
                &record.id,
                crate::subagent::SubagentReviewRequest {
                    verdict: "not_used".into(),
                    summary: "primary reviewed unavailable closeout".into(),
                    overlap_resolution: None,
                },
            )
            .unwrap();

        let resumed = manager
            .run_layered(
                Some(&goal.id),
                None,
                GoalRunOptions {
                    until: GoalRunUntil::Summary,
                    checkpoint_interval_minutes: 0,
                    max_repair_attempts: 3,
                },
            )
            .unwrap();

        assert_ne!(resumed.stopped_reason, "host_subagent_dispatch_requested");
        assert!(resumed.subagent_dispatch.is_none());
    }

    #[test]
    fn resume_reopens_blocked_goal_and_completes_resume_pending() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "needs customer answer",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        manager
            .close_goal(
                Some(&goal.id),
                "blocked",
                "wait_user: missing customer confirmation",
                &["customer confirms scope".into()],
            )
            .unwrap();
        assert_eq!(
            SessionManager::new(temp.path())
                .unwrap()
                .list_pending()
                .unwrap()
                .len(),
            1
        );

        let report = manager
            .resume(Some(&goal.id), None, GoalRunOptions::default())
            .unwrap();

        assert_ne!(report.goal.status, "blocked");
        assert_eq!(
            SessionManager::new(temp.path())
                .unwrap()
                .list_pending()
                .unwrap()
                .len(),
            0
        );
        assert!(report.goal.steps.iter().any(|step| step.stage == "resume"));
    }

    #[test]
    fn layered_run_blocks_after_max_repair_attempts() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start(
                "fix failing tests",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        manager
            .record_validation_result(Some(&goal.id), false, "cargo test failed once")
            .unwrap();
        manager
            .record_validation_result(Some(&goal.id), false, "cargo test failed twice")
            .unwrap();

        let report = manager
            .run_layered(
                Some(&goal.id),
                None,
                GoalRunOptions {
                    until: GoalRunUntil::Blocked,
                    checkpoint_interval_minutes: 0,
                    max_repair_attempts: 1,
                },
            )
            .unwrap();

        assert_eq!(report.goal.status, "blocked");
        assert_eq!(report.stopped_reason, "blocked_max_repair_attempts");
        assert!(report.blockers[0].contains("max_repair_attempts_exceeded"));
        assert_eq!(
            SessionManager::new(temp.path())
                .unwrap()
                .list_pending()
                .unwrap()[0]["metadata"]["resume_command"],
            GoalManager::resume_command(&goal.id)
        );
    }

    #[test]
    fn validation_failure_enters_repair_loop() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("fix test", "standard_development", &[], &[], &[], &[])
            .unwrap();

        let updated = manager
            .record_validation_result(Some(&goal.id), false, "cargo test failed")
            .unwrap();

        assert_eq!(updated.current_stage, "repair");
        assert_eq!(updated.steps.last().unwrap().stage, "validate");
        assert_eq!(updated.steps.last().unwrap().status, "failed");
    }

    #[test]
    fn validation_pass_does_not_skip_pre_validation_stages() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("ship feature", "standard_development", &[], &[], &[], &[])
            .unwrap();

        let updated = manager
            .record_validation_result(Some(&goal.id), true, "cargo test passed")
            .unwrap();
        assert_eq!(updated.current_stage, "plan");

        let planned = manager.run_next(Some(&goal.id), None).unwrap();
        assert_eq!(planned.current_stage, "impact");
    }

    #[test]
    fn goal_run_records_auxiliary_skip_without_blocking() {
        let temp = tempfile::tempdir().unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("plan safely", "standard_development", &[], &[], &[], &[])
            .unwrap();
        manager.run_next(Some(&goal.id), None).unwrap();

        let planned = manager.run_next(Some(&goal.id), None).unwrap();

        assert_eq!(planned.current_stage, "impact");
        let plan_step = planned
            .steps
            .iter()
            .find(|step| step.stage == "plan")
            .unwrap();
        assert_eq!(plan_step.auxiliary_ai["status"], "skipped_unavailable");
    }

    #[test]
    fn auxiliary_advice_is_metadata_not_completion_evidence() {
        let mut auxiliary_ai = json!({"status": "success", "task": "workflow_summary"});

        attach_auxiliary_advice(&mut auxiliary_ai, "advisory note".into());

        assert_eq!(auxiliary_ai["advice"], "advisory note");
        assert_eq!(auxiliary_ai["advisory_only"], true);
        assert_eq!(auxiliary_ai["cannot_execute_or_validate"], true);
        assert_eq!(auxiliary_ai["primary_ai_must_validate_against_files"], true);
    }

    #[test]
    fn standard_development_reaches_summary_and_completion() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("crates").join("rayman-cli").join("src")).unwrap();
        fs::write(
            temp.path()
                .join("crates")
                .join("rayman-cli")
                .join("src")
                .join("main.rs"),
            "fn main() {}\n",
        )
        .unwrap();
        crate::docs::maintain_html_docs(crate::docs::DocsMaintainOptions {
            root: temp.path().to_path_buf(),
            output: Some(temp.path().join("docs").join("project-docs.html")),
            prompt: None,
            prompt_file: None,
            model_output: None,
            dry_run: false,
            check: false,
            apply_prune: false,
        })
        .unwrap();
        let manager = GoalManager::new(temp.path()).unwrap();
        let goal = manager
            .start("build cli", "standard_development", &[], &[], &[], &[])
            .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let mut record = goal;
        for _ in 0..STAGES.len() {
            record = manager.run_next(Some(&record.id), None).unwrap();
            if record.status == "success" {
                break;
            }
        }

        assert_eq!(record.status, "in_progress");
        assert_eq!(record.current_stage, "summary");
        assert_eq!(
            record.next_action,
            "close goal with explicit req_id-mapped completion evidence"
        );
        let record = manager
            .close_goal(
                Some(&record.id),
                "success",
                &success_evidence(
                    "req_1: crates/rayman-cli/src/main.rs implemented and cargo test passed",
                ),
                &[],
            )
            .unwrap();
        assert_eq!(record.status, "success");
        assert_eq!(record.current_stage, "complete");
        assert!(record.steps.iter().any(|step| step.stage == "summary"));
    }

    fn make_temp_run_stale(run: &crate::temp::TempRun) {
        let text = fs::read_to_string(&run.metadata_path).unwrap();
        let mut metadata: crate::temp::TempRunMetadata = serde_json::from_str(&text).unwrap();
        let old = (chrono::Utc::now() - chrono::Duration::hours(48)).to_rfc3339();
        metadata.created_at = old.clone();
        metadata.updated_at = old;
        fs::write(
            &run.metadata_path,
            serde_json::to_string_pretty(&serde_json::json!(metadata)).unwrap(),
        )
        .unwrap();
    }
}
