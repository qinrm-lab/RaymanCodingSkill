use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{display_path, ensure_within, now_iso, read_text, write_text};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentRecord {
    pub id: String,
    pub host_agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_request_id: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
    pub task: String,
    pub boundary: String,
    pub read_only: bool,
    #[serde(default)]
    pub write_paths: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub result_summary: Option<String>,
    #[serde(default)]
    pub result_evidence_refs: Vec<String>,
    #[serde(default)]
    pub changed_paths: Vec<String>,
    #[serde(default)]
    pub primary_review: Option<SubagentReview>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentReview {
    pub verdict: String,
    pub summary: String,
    #[serde(default)]
    pub overlap_resolution: Option<String>,
    pub reviewed_at: String,
}

#[derive(Debug, Clone)]
pub struct SubagentRecordRequest {
    pub host_agent_id: String,
    pub goal_id: Option<String>,
    pub dispatch_request_id: Option<String>,
    pub nickname: Option<String>,
    pub task: String,
    pub boundary: String,
    pub read_only: bool,
    pub write_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SubagentResultRequest {
    pub status: String,
    pub summary: String,
    pub evidence_refs: Vec<String>,
    pub changed_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SubagentReviewRequest {
    pub verdict: String,
    pub summary: String,
    pub overlap_resolution: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SubagentPlanRequest {
    pub task: String,
    pub paths: Vec<PathBuf>,
    pub read_only: bool,
    pub max_lanes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SubagentLedgerState {
    version: u32,
    workspace_path: String,
    updated_at: String,
    records: Vec<SubagentRecord>,
}

pub struct SubagentLedgerManager {
    workspace: PathBuf,
    state_path: PathBuf,
    lock_path: PathBuf,
}

impl SubagentLedgerManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let state_path = workspace
            .join(".RaymanCodingSkill")
            .join("subagents")
            .join("ledger.json");
        let state_path =
            ensure_within(&state_path, &workspace, "subagent ledger 必须位于工作区内")?;
        let lock_path = state_path.with_extension("lock");
        Ok(Self {
            workspace,
            state_path,
            lock_path,
        })
    }

    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    pub fn record(&self, request: SubagentRecordRequest) -> Result<SubagentRecord> {
        validate_required("host_agent_id", &request.host_agent_id)?;
        validate_required("task", &request.task)?;
        validate_required("boundary", &request.boundary)?;
        if request.read_only && !request.write_paths.is_empty() {
            bail!("read-only subagent 不能声明 write_paths");
        }
        if !request.read_only && request.write_paths.is_empty() {
            bail!("可写 subagent 必须声明至少一个 write-path，或使用 --read-only");
        }

        let goal_id = request
            .goal_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let dispatch_request_id = request
            .dispatch_request_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if dispatch_request_id.is_some() && goal_id.is_none() {
            bail!("dispatch_request_id requires goal_id");
        }

        let write_paths = request
            .write_paths
            .iter()
            .map(|path| normalize_workspace_path(path, &self.workspace))
            .collect::<Result<Vec<_>>>()?;
        self.with_locked_state(|state| {
            let created_at = now_iso();
            let record = SubagentRecord {
                id: subagent_record_id(&request.host_agent_id, &created_at),
                host_agent_id: request.host_agent_id.trim().to_string(),
                goal_id,
                dispatch_request_id,
                nickname: request
                    .nickname
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                task: request.task.trim().to_string(),
                boundary: request.boundary.trim().to_string(),
                read_only: request.read_only,
                write_paths,
                status: "open".into(),
                result_summary: None,
                result_evidence_refs: Vec::new(),
                changed_paths: Vec::new(),
                primary_review: None,
                created_at: created_at.clone(),
                updated_at: created_at,
            };
            state.records.push(record.clone());
            Ok(record)
        })
    }

    pub fn record_result(
        &self,
        id: &str,
        request: SubagentResultRequest,
    ) -> Result<SubagentRecord> {
        validate_required("id", id)?;
        validate_required("summary", &request.summary)?;
        if !["completed", "failed", "conflict"].contains(&request.status.as_str()) {
            bail!("subagent result status 必须是 completed/failed/conflict");
        }
        self.with_locked_state(|state| {
            let record = find_record_mut(&mut state.records, id)?;
            if record.read_only && !request.changed_paths.is_empty() {
                bail!("read-only subagent 不能声明 changed_paths");
            }
            let changed_paths = request
                .changed_paths
                .iter()
                .map(|path| normalize_workspace_path(path, &self.workspace))
                .collect::<Result<Vec<_>>>()?;
            for changed_path in &changed_paths {
                if !changed_path_is_within_declared_scope(changed_path, &record.write_paths) {
                    bail!(
                        "subagent changed path 超出声明 write scope: {} not in {:?}",
                        changed_path,
                        record.write_paths
                    );
                }
            }
            record.status = request.status;
            record.result_summary = Some(request.summary.trim().to_string());
            record.result_evidence_refs = request
                .evidence_refs
                .into_iter()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .collect();
            record.changed_paths = changed_paths;
            record.updated_at = now_iso();
            Ok(record.clone())
        })
    }

    pub fn record_review(
        &self,
        id: &str,
        request: SubagentReviewRequest,
    ) -> Result<SubagentRecord> {
        validate_required("id", id)?;
        validate_required("summary", &request.summary)?;
        if !["accepted", "not_used", "conflict"].contains(&request.verdict.as_str()) {
            bail!("subagent review verdict 必须是 accepted/not_used/conflict");
        }
        self.with_locked_state(|state| {
            let record = find_record_mut(&mut state.records, id)?;
            if record.status == "open" || record.result_summary.is_none() {
                bail!("subagent review requires a recorded result before primary review");
            }
            if matches!(record.status.as_str(), "failed" | "conflict")
                && request.verdict == "accepted"
            {
                bail!(
                    "failed or conflicted subagent results cannot be accepted; use not_used or conflict"
                );
            }
            let reviewed_at = now_iso();
            record.primary_review = Some(SubagentReview {
                verdict: request.verdict.clone(),
                summary: request.summary.trim().to_string(),
                overlap_resolution: request
                    .overlap_resolution
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty()),
                reviewed_at,
            });
            record.status = if request.verdict == "conflict" {
                "conflict".into()
            } else {
                "reviewed".into()
            };
            record.updated_at = now_iso();
            Ok(record.clone())
        })
    }

    pub fn status(&self) -> Result<Value> {
        let state = match self.state() {
            Ok(state) => state,
            Err(error) => {
                let blocker = self.parse_error_blocker(&error);
                return Ok(json!({
                    "workspace_path": display_path(&self.workspace),
                    "state_path": display_path(&self.state_path),
                    "record_count": 0,
                    "blocking_count": 1,
                    "status": "blocked",
                    "blockers": [blocker],
                    "records": [],
                }));
            }
        };
        let blockers = self.success_blockers_from_records(&state.records)?;
        let dispatch_requests = self.goal_dispatch_requests_for_records(&state.records)?;
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "state_path": display_path(&self.state_path),
            "record_count": state.records.len(),
            "dispatch_request_count": dispatch_requests.len(),
            "blocking_count": blockers.len(),
            "status": if blockers.is_empty() { "passed" } else { "blocked" },
            "blockers": blockers,
            "dispatch_requests": dispatch_requests,
            "records": state.records,
        }))
    }

    pub fn plan(&self, request: SubagentPlanRequest) -> Result<Value> {
        validate_required("task", &request.task)?;
        let task = request.task.trim().to_string();
        let task_lower = task.to_lowercase();
        let read_only_intent = request.read_only
            || task_has_any(
                &task_lower,
                &[
                    "read-only",
                    "readonly",
                    "audit-only",
                    "review-only",
                    "no edits",
                    "no source edits",
                    "只读",
                    "不改",
                    "不编辑",
                    "仅审计",
                ],
            );
        let lane_limit = if request.max_lanes == 0 {
            4
        } else {
            request.max_lanes.clamp(1, 6)
        };
        let paths = request
            .paths
            .iter()
            .map(|path| normalize_workspace_path(path, &self.workspace))
            .collect::<Result<Vec<_>>>()?;

        let broad_or_complex = task_has_any(
            &task_lower,
            &[
                "audit",
                "review",
                "refactor",
                "regression",
                "performance",
                "subagent",
                "workflow",
                "审计",
                "全仓",
                "全面",
                "重构",
                "修复",
                "实现",
                "性能",
                "提速",
                "并行",
            ],
        ) || paths.len() >= 2;
        let tiny_fast_path = !broad_or_complex && paths.len() <= 1 && task.chars().count() < 80;
        let write_scopes = disjoint_write_scopes(&paths);
        let dispatch_status = if tiny_fast_path {
            "dispatch_not_recommended"
        } else if paths.is_empty() {
            "dispatch_recommended_after_scope_split"
        } else {
            "dispatch_recommended"
        };
        let expected_time_saved = if tiny_fast_path {
            "low"
        } else if write_scopes.len() >= 2
            || paths.is_empty()
                && task_has_any(&task_lower, &["全仓", "全面", "audit", "regression"])
        {
            "high"
        } else {
            "medium"
        };

        let mut lanes = Vec::new();
        let mut push_lane = |lane_id: &str,
                             agent_type: &str,
                             read_only: bool,
                             write_paths: Vec<String>,
                             purpose: &str,
                             boundary: &str,
                             prompt_focus: Vec<&str>| {
            if lanes.len() >= lane_limit {
                return;
            }
            lanes.push(json!({
                "lane_id": lane_id,
                "agent_type": agent_type,
                "read_only": read_only,
                "write_paths": write_paths,
                "purpose": purpose,
                "boundary": boundary,
                "prompt_focus": prompt_focus,
            }));
        };

        if !tiny_fast_path {
            push_lane(
                "impact_scan",
                "explorer",
                true,
                Vec::new(),
                "Map affected modules, existing patterns, risky contracts, and the smallest safe validation set.",
                "Read-only impact scan; no edits; return file/line evidence and validation recommendations.",
                vec![
                    "affected files and ownership boundaries",
                    "existing local patterns to reuse",
                    "focused tests or gates likely to catch regressions",
                ],
            );

            if paths.is_empty() {
                push_lane(
                    "scope_slicer",
                    "explorer",
                    true,
                    Vec::new(),
                    "Split the task into independent write scopes before any worker edits are delegated.",
                    "Read-only scope slicing; no edits; propose non-overlapping write scopes and merge order.",
                    vec![
                        "candidate write scopes",
                        "which scopes can run in parallel",
                        "which scope must stay on the primary-agent critical path",
                    ],
                );
            } else if read_only_intent {
                push_lane(
                    "read_only_scope_review",
                    "explorer",
                    true,
                    Vec::new(),
                    "Review the supplied paths without editing and report bounded findings for primary-agent action.",
                    "Read-only path review; no edits; return file/line evidence, risks, and validation recommendations.",
                    vec![
                        "current-file evidence only",
                        "findings that materially affect the parent task",
                        "tests or gates the primary agent should run",
                    ],
                );
            } else {
                for (index, scope) in write_scopes.iter().take(2).enumerate() {
                    push_lane(
                        &format!("worker_scope_{}", index + 1),
                        "worker",
                        false,
                        vec![scope.clone()],
                        "Implement one bounded non-overlapping code or document slice.",
                        "Writable only inside the declared write scope; do not revert other edits; report changed paths and validation evidence.",
                        vec![
                            "make the smallest coherent patch inside the write scope",
                            "do not edit outside the declared scope",
                            "list changed paths and focused validation",
                        ],
                    );
                }
            }

            if paths.iter().any(|path| {
                path.starts_with("docs")
                    || path.starts_with("references")
                    || path.starts_with("SKILL.md")
                    || path.starts_with("config")
            }) || task_has_any(
                &task_lower,
                &["docs", "documentation", "skill", "contract", "文档", "契约"],
            ) {
                push_lane(
                    "contract_sync",
                    "explorer",
                    true,
                    Vec::new(),
                    "Check docs, skill rules, feature coverage, and fixtures for terminology drift.",
                    "Read-only contract sync review; no edits; flag stale wording and missing validation anchors.",
                    vec![
                        "subagent terminology must mean main-model/strong-model host subagents",
                        "auxiliary AI must remain a separate low-authority critic path",
                        "feature coverage and tests that must move with docs",
                    ],
                );
            }

            push_lane(
                "validation_lane",
                "explorer",
                true,
                Vec::new(),
                "Run or identify focused validation independently while the primary agent integrates edits.",
                "Read-only validation lane; no source edits; report command output, failure signatures, and retained evidence paths.",
                vec![
                    "focused tests first",
                    "gate blockers that can be repaired in parallel",
                    "whether failures are new, stale, or environment-owned",
                ],
            );
        }

        let auto_start_ready =
            dispatch_status.starts_with("dispatch_recommended") && !lanes.is_empty();
        let workspace_path = display_path(&self.workspace);
        for lane in &mut lanes {
            if let Some(object) = lane.as_object_mut() {
                let lane_id = object
                    .get("lane_id")
                    .and_then(Value::as_str)
                    .unwrap_or("subagent_lane")
                    .to_string();
                let agent_type = object
                    .get("agent_type")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .to_string();
                let write_paths = object
                    .get("write_paths")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let purpose = object
                    .get("purpose")
                    .and_then(Value::as_str)
                    .unwrap_or("subagent lane")
                    .to_string();
                let boundary = object
                    .get("boundary")
                    .and_then(Value::as_str)
                    .unwrap_or("bounded subagent lane")
                    .to_string();
                let read_only = object
                    .get("read_only")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                let prompt_focus = object
                    .get("prompt_focus")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                object.insert(
                    "spawn_agent_request".into(),
                    subagent_spawn_agent_request(SubagentSpawnRequestSpec {
                        workspace_path: &workspace_path,
                        parent_task: &task,
                        lane_id: &lane_id,
                        agent_type: &agent_type,
                        read_only,
                        write_paths: &write_paths,
                        purpose: &purpose,
                        boundary: &boundary,
                        prompt_focus: &prompt_focus,
                    }),
                );
                object.insert(
                    "record_command_template".into(),
                    json!(subagent_record_command(
                        &purpose,
                        &boundary,
                        read_only,
                        &write_paths
                    )),
                );
            }
        }

        Ok(json!({
            "task": task,
            "workspace_path": display_path(&self.workspace),
            "input_paths": paths,
            "read_only_intent": read_only_intent,
            "dispatch_status": dispatch_status,
            "expected_time_saved": expected_time_saved,
            "auto_start_ready": auto_start_ready,
            "auto_start_contract": subagent_auto_start_contract(auto_start_ready),
            "decision_authority": "main_agent",
            "runtime_model_policy": "Codex host subagents are main-model/strong-model child agents; omit model overrides so they inherit the parent/main model unless the user explicitly asks otherwise.",
            "auxiliary_ai_boundary": "This is not auxiliary AI. ai_ubuntu_8888/local auxiliary AI remains a low-authority critic/advisory path and is not the speed runtime.",
            "primary_agent_actions": [
                "spawn independent lanes in the same round when the host exposes subagents",
                "continue the critical-path implementation while subagents run",
                "record every spawned host subagent with rayman subagent record/result/review",
                "review, integrate, validate, and own final status in the primary agent"
            ],
            "do_not_dispatch_when": [
                "the task is a tiny single-file edit faster than delegation overhead",
                "write scopes overlap and cannot be split cleanly",
                "missing credentials, destructive approval, or business input blocks the task",
                "a subagent would only duplicate work already on the primary-agent critical path"
            ],
            "recommended_lanes": lanes,
        }))
    }

    pub fn success_blockers(&self) -> Result<Vec<String>> {
        match self.state() {
            Ok(state) => self.success_blockers_from_records(&state.records),
            Err(error) => Ok(vec![self.parse_error_blocker(&error)]),
        }
    }

    pub fn dispatch_request_has_closeout(
        &self,
        goal_id: &str,
        dispatch_request_id: &str,
    ) -> Result<bool> {
        validate_required("goal_id", goal_id)?;
        validate_required("dispatch_request_id", dispatch_request_id)?;
        let state = self.state()?;
        Ok(dispatch_closeout_record(&state.records, goal_id, dispatch_request_id).is_some())
    }

    pub fn review_blockers(&self) -> Result<Vec<Value>> {
        let state = match self.state() {
            Ok(state) => state,
            Err(error) => {
                return Ok(vec![json!({
                    "type": "subagent_ledger",
                    "reason": self.parse_error_blocker(&error),
                    "state_path": display_path(&self.state_path),
                })]);
            }
        };
        let mut blockers = Vec::new();
        for blocker in self.success_blockers_from_records(&state.records)? {
            blockers.push(json!({
                "type": "subagent_ledger",
                "reason": blocker,
                "state_path": display_path(&self.state_path),
            }));
        }
        Ok(blockers)
    }

    fn success_blockers_from_records(&self, records: &[SubagentRecord]) -> Result<Vec<String>> {
        let mut blockers = Vec::new();
        for record in records {
            if record.status == "open" || record.result_summary.is_none() {
                blockers.push(format!(
                    "subagent_ledger_unreviewed subagent_ledger_unresolved {}: task={} status={} missing recorded result",
                    record.id, record.task, record.status
                ));
                continue;
            }
            if record.primary_review.is_none() {
                blockers.push(format!(
                    "subagent_ledger_unreviewed {}: task={} status={}",
                    record.id, record.task, record.status
                ));
                continue;
            }
            if record
                .primary_review
                .as_ref()
                .map(|review| review.verdict.as_str())
                == Some("conflict")
            {
                blockers.push(format!(
                    "subagent_ledger_conflict {}: primary review reported conflict",
                    record.id
                ));
            }
        }

        for (left_index, left) in records.iter().enumerate() {
            if left.read_only {
                continue;
            }
            for right in records.iter().skip(left_index + 1) {
                if right.read_only {
                    continue;
                }
                if self.records_overlap(left, right)? && !overlap_is_resolved(left, right) {
                    blockers.push(format!(
                        "subagent_ledger_overlap {} <-> {}: overlapping write paths require primary-agent overlap_resolution",
                        left.id, right.id
                    ));
                }
            }
        }
        for blocker in self.dispatch_request_blockers_from_records(records)? {
            blockers.push(blocker);
        }
        Ok(blockers)
    }

    fn dispatch_request_blockers_from_records(
        &self,
        records: &[SubagentRecord],
    ) -> Result<Vec<String>> {
        let mut blockers = Vec::new();
        for request in self.goal_dispatch_requests_for_records(records)? {
            if request
                .get("auto_start_ready")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && request
                    .get("closeout_status")
                    .and_then(Value::as_str)
                    .unwrap_or("missing")
                    != "closed"
            {
                let goal_id = request
                    .get("goal_id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                let request_id = request
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                blockers.push(format!(
                    "subagent_dispatch_unclosed goal_id={goal_id} dispatch_request_id={request_id}: call host spawn_agent for recommended lanes, or record failed/unavailable closeout with rayman subagent record/result/review"
                ));
            }
        }
        Ok(blockers)
    }

    fn goal_dispatch_requests_for_records(&self, records: &[SubagentRecord]) -> Result<Vec<Value>> {
        let goals_dir = self.workspace.join(".RaymanCodingSkill").join("goals");
        if !goals_dir.exists() {
            return Ok(Vec::new());
        }
        let mut requests = Vec::new();
        for entry in fs::read_dir(&goals_dir)
            .with_context(|| format!("无法读取目标状态目录: {}", goals_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let text = read_text(&path)?;
            let goal: Value = serde_json::from_str(&text)
                .with_context(|| format!("无法解析目标状态: {}", path.display()))?;
            let goal_id = goal
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let goal_status = goal
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let Some(request_items) = goal
                .get("metadata")
                .and_then(|value| value.get("subagent_dispatch"))
                .and_then(|value| value.get("requests"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            for request in request_items {
                let mut request = request.clone();
                let Some(object) = request.as_object_mut() else {
                    continue;
                };
                object.insert("goal_id".into(), json!(goal_id));
                object.insert("goal_status".into(), json!(goal_status));
                let request_id = object
                    .get("request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if request_id.is_empty() {
                    object.insert("closeout_status".into(), json!("missing_request_id"));
                    requests.push(request);
                    continue;
                }
                if let Some(record) = dispatch_closeout_record(records, goal_id, &request_id) {
                    object.insert("closeout_status".into(), json!("closed"));
                    object.insert("closeout_record_id".into(), json!(record.id));
                    object.insert("closeout_record_status".into(), json!(record.status));
                } else {
                    object.insert("closeout_status".into(), json!("missing"));
                }
                requests.push(request);
            }
        }
        Ok(requests)
    }

    fn records_overlap(&self, left: &SubagentRecord, right: &SubagentRecord) -> Result<bool> {
        for left_path in &left.write_paths {
            let left_path = normalize_workspace_path(Path::new(left_path), &self.workspace)?;
            for right_path in &right.write_paths {
                let right_path = normalize_workspace_path(Path::new(right_path), &self.workspace)?;
                if path_overlaps(&left_path, &right_path) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn state(&self) -> Result<SubagentLedgerState> {
        if !self.state_path.exists() {
            return Ok(SubagentLedgerState {
                version: 1,
                workspace_path: display_path(&self.workspace),
                updated_at: now_iso(),
                records: Vec::new(),
            });
        }
        let text = read_text(&self.state_path)?;
        serde_json::from_str(&text)
            .with_context(|| format!("无法解析 subagent ledger: {}", self.state_path.display()))
    }

    fn write_state(&self, mut state: SubagentLedgerState) -> Result<()> {
        state.updated_at = now_iso();
        state.workspace_path = display_path(&self.workspace);
        let text = serde_json::to_string_pretty(&state)?;
        let _: SubagentLedgerState =
            serde_json::from_str(&text).context("subagent ledger round-trip validation failed")?;
        write_text(&self.state_path, &text)?;
        Ok(())
    }

    fn with_locked_state<T>(
        &self,
        operation: impl FnOnce(&mut SubagentLedgerState) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.acquire_lock()?;
        let mut state = self.state()?;
        let output = operation(&mut state)?;
        self.write_state(state)?;
        Ok(output)
    }

    fn acquire_lock(&self) -> Result<LedgerLock> {
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录: {}", parent.display()))?;
        }
        for _ in 0..50 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.lock_path)
            {
                Ok(_) => {
                    return Ok(LedgerLock {
                        path: self.lock_path.clone(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => {
                    bail!(
                        "无法创建 subagent ledger lock {}: {}",
                        self.lock_path.display(),
                        error
                    );
                }
            }
        }
        bail!(
            "subagent ledger lock timeout: {}",
            display_path(&self.lock_path)
        )
    }

    fn parse_error_blocker(&self, error: &anyhow::Error) -> String {
        format!(
            "subagent_ledger_parse_error: state_path={} error={} recovery=repair or restore the JSON ledger before recording/reviewing host subagents",
            display_path(&self.state_path),
            error
        )
    }
}

struct LedgerLock {
    path: PathBuf,
}

impl Drop for LedgerLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn validate_required(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{name} 不能为空");
    }
    Ok(())
}

fn find_record_mut<'a>(
    records: &'a mut [SubagentRecord],
    id: &str,
) -> Result<&'a mut SubagentRecord> {
    records
        .iter_mut()
        .find(|record| record.id == id)
        .with_context(|| format!("subagent ledger record 不存在: {id}"))
}

fn normalize_workspace_path(path: &Path, workspace: &Path) -> Result<String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    let normalized = ensure_within(
        &candidate,
        workspace,
        "subagent write path 必须位于工作区内",
    )?;
    Ok(relative_path(workspace, &normalized))
}

fn relative_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .ok()
        .map(display_path)
        .unwrap_or_else(|| display_path(path))
}

fn path_overlaps(left: &str, right: &str) -> bool {
    let left = Path::new(left);
    let right = Path::new(right);
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn changed_path_is_within_declared_scope(changed_path: &str, scopes: &[String]) -> bool {
    scopes.iter().any(|scope| {
        path_overlaps(changed_path, scope) && Path::new(changed_path).starts_with(scope)
    })
}

fn task_has_any(task_lower: &str, signals: &[&str]) -> bool {
    signals.iter().any(|signal| task_lower.contains(signal))
}

fn disjoint_write_scopes(paths: &[String]) -> Vec<String> {
    let mut scopes = Vec::new();
    for path in paths {
        let normalized = path.replace('\\', "/");
        let parts = normalized
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let scope = if parts.first() == Some(&"crates") && parts.len() >= 2 {
            if parts.len() >= 4 && Path::new(&normalized).extension().is_some() {
                normalized.clone()
            } else {
                format!("{}/{}", parts[0], parts[1])
            }
        } else if parts.len() >= 2 && ["docs", "references", "config"].contains(&parts[0]) {
            parts[0].to_string()
        } else {
            normalized
        };
        if !scopes.iter().any(|existing| existing == &scope) {
            scopes.push(scope);
        }
    }
    scopes
}

fn subagent_record_command(
    task: &str,
    boundary: &str,
    read_only: bool,
    write_paths: &[String],
) -> String {
    let mut command = format!(
        "rayman subagent record --agent-id <host-agent-id> --nickname <nickname> --task {} --boundary {}",
        quote_arg(task),
        quote_arg(boundary)
    );
    if read_only {
        command.push_str(" --read-only");
    } else {
        for path in write_paths {
            command.push_str(" --write-path ");
            command.push_str(&quote_arg(path));
        }
    }
    command
}

struct SubagentSpawnRequestSpec<'a> {
    workspace_path: &'a str,
    parent_task: &'a str,
    lane_id: &'a str,
    agent_type: &'a str,
    read_only: bool,
    write_paths: &'a [String],
    purpose: &'a str,
    boundary: &'a str,
    prompt_focus: &'a [String],
}

fn subagent_spawn_agent_request(spec: SubagentSpawnRequestSpec<'_>) -> Value {
    let mut message = vec![
        format!(
            "RaymanCodingSkill host subagent lane `{}` for parent task: {}",
            spec.lane_id, spec.parent_task
        ),
        format!("Workspace: {}", spec.workspace_path),
        format!("Purpose: {}", spec.purpose),
        format!("Boundary: {}", spec.boundary),
    ];
    if spec.read_only {
        message.push("Read-only lane: do not edit files.".into());
    } else {
        message.push(format!(
            "Writable lane: edit only these declared write paths: {}",
            spec.write_paths.join(", ")
        ));
    }
    if !spec.prompt_focus.is_empty() {
        message.push(format!("Prompt focus: {}", spec.prompt_focus.join("; ")));
    }
    message.push(
        "You are not alone in the workspace; do not revert or overwrite unrelated edits.".into(),
    );
    message.push(
        "Return concise file/line evidence, changed paths if any, validation evidence, blockers, and a final status.".into(),
    );
    message.push(
        "Codex host subagents are main-model/strong-model child agents, not ai_ubuntu_8888 auxiliary AI; the primary agent owns integration, validation, and final status.".into(),
    );

    json!({
        "agent_type": spec.agent_type,
        "fork_context": false,
        "message": message.join("\n"),
    })
}

fn subagent_auto_start_contract(auto_start_ready: bool) -> Value {
    json!({
        "host_tool": "multi_agent_v1.spawn_agent",
        "host_tool_available": "runtime_probe_required",
        "auto_start": auto_start_ready,
        "authorization_mode": "standing_workspace_authorization",
        "per_use_prompt_required": false,
        "explicit_subagent_phrase_required": false,
        "start_when": "auto_start_ready is true and the host exposes a subagent spawn tool; in an enabled workspace, no additional '开启subagent' phrase is required.",
        "spawn_strategy": "spawn independent recommended_lanes in the same round when possible, then continue primary-agent critical-path work while they run.",
        "model_override_policy": "omit model overrides so host subagents inherit the parent/main model unless the user explicitly requests a different model.",
        "ledger_sequence": [
            "call the host spawn tool with lane.spawn_agent_request",
            "fill lane.record_command_template with the returned host-agent-id and nickname",
            "after completion, record rayman subagent result with status, evidence, and changed paths",
            "record rayman subagent review after primary-agent review and integration"
        ],
        "fallback_when_unavailable": "continue on the primary-agent path and report the host-subagent skip reason; do not claim a dispatch happened without a ledger record.",
    })
}

fn quote_arg(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

fn overlap_is_resolved(left: &SubagentRecord, right: &SubagentRecord) -> bool {
    [left.primary_review.as_ref(), right.primary_review.as_ref()]
        .into_iter()
        .flatten()
        .any(|review| {
            review
                .overlap_resolution
                .as_deref()
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
        })
}

fn dispatch_closeout_record<'a>(
    records: &'a [SubagentRecord],
    goal_id: &str,
    dispatch_request_id: &str,
) -> Option<&'a SubagentRecord> {
    records.iter().find(|record| {
        record.goal_id.as_deref() == Some(goal_id)
            && record.dispatch_request_id.as_deref() == Some(dispatch_request_id)
            && record.result_summary.is_some()
            && record.primary_review.is_some()
            && matches!(record.status.as_str(), "reviewed" | "conflict")
    })
}

fn subagent_record_id(agent_id: &str, created_at: &str) -> String {
    let sanitized = agent_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(8)
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
        "subagent_{}_{}",
        if sanitized.is_empty() {
            "record"
        } else {
            &sanitized
        },
        suffix
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn subagent_ledger_blocks_unreviewed_record_until_primary_review() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("owned.rs"), "fn main() {}\n")?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let record = manager.record(SubagentRecordRequest {
            host_agent_id: "agent-1".into(),
            goal_id: None,
            dispatch_request_id: None,
            nickname: Some("reviewer".into()),
            task: "review owned.rs".into(),
            boundary: "only owned.rs".into(),
            read_only: false,
            write_paths: vec![PathBuf::from("owned.rs")],
        })?;

        assert!(
            manager
                .success_blockers()?
                .iter()
                .any(|blocker| blocker.contains("subagent_ledger_unreviewed"))
        );

        manager.record_result(
            &record.id,
            SubagentResultRequest {
                status: "completed".into(),
                summary: "changed owned.rs".into(),
                evidence_refs: vec!["cargo test -p rayman-core".into()],
                changed_paths: vec![PathBuf::from("owned.rs")],
            },
        )?;
        manager.record_review(
            &record.id,
            SubagentReviewRequest {
                verdict: "accepted".into(),
                summary: "primary reviewed and integrated".into(),
                overlap_resolution: None,
            },
        )?;

        assert!(manager.success_blockers()?.is_empty());
        Ok(())
    }

    #[test]
    fn subagent_plan_recommends_main_model_lanes_without_auxiliary_conflation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("crates").join("rayman-core").join("src"))?;
        fs::create_dir_all(temp.path().join("docs"))?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let plan = manager.plan(SubagentPlanRequest {
            task: "审计 subagent 性能提速策略".into(),
            paths: vec![
                PathBuf::from("crates/rayman-core/src/subagent.rs"),
                PathBuf::from("docs/GOAL_WORKFLOWS.md"),
            ],
            read_only: false,
            max_lanes: 4,
        })?;

        assert_eq!(
            plan["dispatch_status"].as_str(),
            Some("dispatch_recommended")
        );
        assert!(
            plan["runtime_model_policy"]
                .as_str()
                .unwrap_or_default()
                .contains("main-model/strong-model")
        );
        assert!(
            plan["auxiliary_ai_boundary"]
                .as_str()
                .unwrap_or_default()
                .contains("not auxiliary AI")
        );
        assert_eq!(plan["auto_start_ready"].as_bool(), Some(true));
        assert_eq!(
            plan["auto_start_contract"]["host_tool"].as_str(),
            Some("multi_agent_v1.spawn_agent")
        );
        assert_eq!(
            plan["auto_start_contract"]["authorization_mode"].as_str(),
            Some("standing_workspace_authorization")
        );
        assert_eq!(
            plan["auto_start_contract"]["per_use_prompt_required"].as_bool(),
            Some(false)
        );
        assert_eq!(
            plan["auto_start_contract"]["explicit_subagent_phrase_required"].as_bool(),
            Some(false)
        );
        assert!(
            plan["auto_start_contract"]["start_when"]
                .as_str()
                .unwrap_or_default()
                .contains("no additional '开启subagent' phrase is required")
        );
        let lanes = plan["recommended_lanes"].as_array().expect("lanes");
        assert!(lanes.iter().any(|lane| lane["agent_type"] == "worker"));
        assert!(lanes.iter().any(|lane| lane["lane_id"] == "contract_sync"));
        assert!(lanes.iter().all(|lane| {
            lane.get("record_command_template")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("rayman subagent record")
        }));
        assert!(lanes.iter().all(|lane| {
            let request = &lane["spawn_agent_request"];
            request["model"].is_null()
                && request["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("RaymanCodingSkill host subagent lane")
        }));
        Ok(())
    }

    #[test]
    fn subagent_plan_splits_same_crate_files_into_parallel_worker_scopes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let src = temp.path().join("crates").join("rayman-core").join("src");
        fs::create_dir_all(&src)?;
        fs::write(src.join("subagent.rs"), "")?;
        fs::write(src.join("auxiliary.rs"), "")?;
        fs::write(src.join("research.rs"), "")?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let plan = manager.plan(SubagentPlanRequest {
            task: "审计 subagent auxiliary research 并行性能".into(),
            paths: vec![
                PathBuf::from("crates/rayman-core/src/subagent.rs"),
                PathBuf::from("crates/rayman-core/src/auxiliary.rs"),
                PathBuf::from("crates/rayman-core/src/research.rs"),
            ],
            read_only: false,
            max_lanes: 4,
        })?;

        assert_eq!(plan["expected_time_saved"].as_str(), Some("high"));
        let worker_paths = plan["recommended_lanes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|lane| lane["agent_type"] == "worker")
            .flat_map(|lane| lane["write_paths"].as_array().unwrap())
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();

        assert!(worker_paths.len() >= 2);
        assert!(
            worker_paths
                .iter()
                .all(|path| path.starts_with("crates/rayman-core/src/"))
        );
        Ok(())
    }

    #[test]
    fn subagent_plan_skips_tiny_fast_path() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("note.md"), "# note\n")?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let plan = manager.plan(SubagentPlanRequest {
            task: "fix typo".into(),
            paths: vec![PathBuf::from("note.md")],
            read_only: false,
            max_lanes: 4,
        })?;

        assert_eq!(
            plan["dispatch_status"].as_str(),
            Some("dispatch_not_recommended")
        );
        assert_eq!(plan["expected_time_saved"].as_str(), Some("low"));
        assert_eq!(plan["auto_start_ready"].as_bool(), Some(false));
        assert_eq!(
            plan["auto_start_contract"]["auto_start"].as_bool(),
            Some(false)
        );
        assert!(plan["recommended_lanes"].as_array().unwrap().is_empty());
        Ok(())
    }

    #[test]
    fn subagent_plan_read_only_intent_suppresses_worker_lanes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let src = temp.path().join("crates").join("rayman-core").join("src");
        fs::create_dir_all(&src)?;
        fs::write(src.join("subagent.rs"), "")?;
        fs::write(src.join("feature_coverage.rs"), "")?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let plan = manager.plan(SubagentPlanRequest {
            task: "read-only audit of subagent planner".into(),
            paths: vec![
                PathBuf::from("crates/rayman-core/src/subagent.rs"),
                PathBuf::from("crates/rayman-core/src/feature_coverage.rs"),
            ],
            read_only: true,
            max_lanes: 4,
        })?;

        assert_eq!(plan["read_only_intent"].as_bool(), Some(true));
        let lanes = plan["recommended_lanes"].as_array().expect("lanes");
        assert!(
            lanes
                .iter()
                .any(|lane| lane["lane_id"] == "read_only_scope_review")
        );
        assert!(lanes.iter().all(|lane| lane["read_only"] == true));
        assert!(lanes.iter().all(|lane| lane["agent_type"] != "worker"));
        assert!(lanes.iter().all(|lane| {
            lane["record_command_template"]
                .as_str()
                .unwrap_or_default()
                .contains("--read-only")
        }));
        Ok(())
    }

    #[test]
    fn subagent_status_reports_parse_error_as_blocker() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let state_dir = temp.path().join(".RaymanCodingSkill").join("subagents");
        fs::create_dir_all(&state_dir)?;
        fs::write(state_dir.join("ledger.json"), "{\"version\":1} trailing")?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let status = manager.status()?;
        assert_eq!(status["status"].as_str(), Some("blocked"));
        assert!(
            status["blockers"][0]
                .as_str()
                .unwrap_or_default()
                .contains("subagent_ledger_parse_error")
        );
        assert!(manager.success_blockers()?[0].contains("subagent_ledger_parse_error"));
        assert_eq!(
            manager.review_blockers()?[0]["type"].as_str(),
            Some("subagent_ledger")
        );
        Ok(())
    }

    #[test]
    fn subagent_dispatch_request_requires_closeout_record() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let goals_dir = temp.path().join(".RaymanCodingSkill").join("goals");
        fs::create_dir_all(&goals_dir)?;
        fs::write(
            goals_dir.join("goal_dispatch.json"),
            serde_json::to_string_pretty(&json!({
                "id": "goal_dispatch",
                "status": "in_progress",
                "metadata": {
                    "subagent_dispatch": {
                        "requests": [{
                            "request_id": "dispatch_1",
                            "auto_start_ready": true,
                            "auto_start_contract": {"host_tool": "multi_agent_v1.spawn_agent"},
                            "recommended_lanes": []
                        }]
                    }
                }
            }))?,
        )?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        assert!(
            manager
                .success_blockers()?
                .iter()
                .any(|blocker| blocker.contains("subagent_dispatch_unclosed"))
        );
        let record = manager.record(SubagentRecordRequest {
            host_agent_id: "agent-unavailable".into(),
            goal_id: Some("goal_dispatch".into()),
            dispatch_request_id: Some("dispatch_1".into()),
            nickname: Some("unavailable".into()),
            task: "host subagent unavailable".into(),
            boundary: "record unavailable host-subagent lane".into(),
            read_only: true,
            write_paths: Vec::new(),
        })?;
        manager.record_result(
            &record.id,
            SubagentResultRequest {
                status: "failed".into(),
                summary: "host subagent unavailable; primary path continued".into(),
                evidence_refs: vec!["host_tool=unavailable".into()],
                changed_paths: Vec::new(),
            },
        )?;
        manager.record_review(
            &record.id,
            SubagentReviewRequest {
                verdict: "not_used".into(),
                summary: "primary reviewed unavailable closeout".into(),
                overlap_resolution: None,
            },
        )?;

        assert!(manager.success_blockers()?.is_empty());
        let status = manager.status()?;
        assert_eq!(
            status["dispatch_requests"][0]["closeout_status"].as_str(),
            Some("closed")
        );
        Ok(())
    }

    #[test]
    fn subagent_review_requires_recorded_result() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("owned.rs"), "fn main() {}\n")?;
        let manager = SubagentLedgerManager::new(temp.path())?;
        let record = manager.record(SubagentRecordRequest {
            host_agent_id: "agent-1".into(),
            goal_id: None,
            dispatch_request_id: None,
            nickname: Some("reviewer".into()),
            task: "review owned.rs".into(),
            boundary: "only owned.rs".into(),
            read_only: false,
            write_paths: vec![PathBuf::from("owned.rs")],
        })?;

        let error = manager
            .record_review(
                &record.id,
                SubagentReviewRequest {
                    verdict: "accepted".into(),
                    summary: "primary reviewed".into(),
                    overlap_resolution: None,
                },
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("recorded result"));
        assert!(
            manager
                .success_blockers()?
                .iter()
                .any(|blocker| blocker.contains("subagent_ledger_unresolved"))
        );
        Ok(())
    }

    #[test]
    fn subagent_failed_result_cannot_be_accepted_as_success() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("owned.rs"), "fn main() {}\n")?;
        let manager = SubagentLedgerManager::new(temp.path())?;
        let record = manager.record(SubagentRecordRequest {
            host_agent_id: "agent-1".into(),
            goal_id: None,
            dispatch_request_id: None,
            nickname: Some("reviewer".into()),
            task: "review owned.rs".into(),
            boundary: "only owned.rs".into(),
            read_only: false,
            write_paths: vec![PathBuf::from("owned.rs")],
        })?;
        manager.record_result(
            &record.id,
            SubagentResultRequest {
                status: "failed".into(),
                summary: "tool failed".into(),
                evidence_refs: vec!["stderr".into()],
                changed_paths: Vec::new(),
            },
        )?;

        let error = manager
            .record_review(
                &record.id,
                SubagentReviewRequest {
                    verdict: "accepted".into(),
                    summary: "primary reviewed".into(),
                    overlap_resolution: None,
                },
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("cannot be accepted"));
        manager.record_review(
            &record.id,
            SubagentReviewRequest {
                verdict: "not_used".into(),
                summary: "primary discarded failed result and completed work directly".into(),
                overlap_resolution: None,
            },
        )?;
        assert!(manager.success_blockers()?.is_empty());
        Ok(())
    }

    #[test]
    fn subagent_ledger_blocks_overlapping_write_paths_until_resolved() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("src"))?;
        fs::write(temp.path().join("src").join("lib.rs"), "pub fn x() {}\n")?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let left = manager.record(SubagentRecordRequest {
            host_agent_id: "left".into(),
            goal_id: None,
            dispatch_request_id: None,
            nickname: None,
            task: "edit src".into(),
            boundary: "src tree".into(),
            read_only: false,
            write_paths: vec![PathBuf::from("src")],
        })?;
        let right = manager.record(SubagentRecordRequest {
            host_agent_id: "right".into(),
            goal_id: None,
            dispatch_request_id: None,
            nickname: None,
            task: "edit src/lib.rs".into(),
            boundary: "src/lib.rs".into(),
            read_only: false,
            write_paths: vec![PathBuf::from("src/lib.rs")],
        })?;

        for record in [&left, &right] {
            manager.record_result(
                &record.id,
                SubagentResultRequest {
                    status: "completed".into(),
                    summary: "done".into(),
                    evidence_refs: Vec::new(),
                    changed_paths: if record.id == left.id {
                        vec![PathBuf::from("src")]
                    } else {
                        vec![PathBuf::from("src/lib.rs")]
                    },
                },
            )?;
            manager.record_review(
                &record.id,
                SubagentReviewRequest {
                    verdict: "accepted".into(),
                    summary: "reviewed".into(),
                    overlap_resolution: None,
                },
            )?;
        }

        assert!(
            manager
                .success_blockers()?
                .iter()
                .any(|blocker| blocker.contains("subagent_ledger_overlap"))
        );

        manager.record_review(
            &right.id,
            SubagentReviewRequest {
                verdict: "accepted".into(),
                summary: "reviewed".into(),
                overlap_resolution: Some("primary reconciled overlapping edits".into()),
            },
        )?;

        assert!(manager.success_blockers()?.is_empty());
        Ok(())
    }

    #[test]
    fn subagent_ledger_rejects_read_only_records_with_write_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let error = manager
            .record(SubagentRecordRequest {
                host_agent_id: "agent".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "inspect".into(),
                boundary: "read only".into(),
                read_only: true,
                write_paths: vec![PathBuf::from("src/lib.rs")],
            })
            .unwrap_err()
            .to_string();

        assert!(error.contains("read-only subagent"));
        Ok(())
    }

    #[test]
    fn subagent_ledger_rejects_write_path_escape() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let manager = SubagentLedgerManager::new(temp.path())?;

        let error = manager
            .record(SubagentRecordRequest {
                host_agent_id: "agent".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "edit outside".into(),
                boundary: "bad".into(),
                read_only: false,
                write_paths: vec![PathBuf::from("..").join("outside.rs")],
            })
            .unwrap_err()
            .to_string();

        assert!(error.contains("subagent write path"));
        Ok(())
    }

    #[test]
    fn subagent_ledger_rejects_changed_path_outside_write_scope() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("src"))?;
        fs::create_dir_all(temp.path().join("docs"))?;
        fs::write(temp.path().join("src").join("lib.rs"), "pub fn x() {}\n")?;
        fs::write(temp.path().join("docs").join("note.md"), "# note\n")?;
        let manager = SubagentLedgerManager::new(temp.path())?;
        let record = manager.record(SubagentRecordRequest {
            host_agent_id: "agent".into(),
            goal_id: None,
            dispatch_request_id: None,
            nickname: None,
            task: "edit src only".into(),
            boundary: "src only".into(),
            read_only: false,
            write_paths: vec![PathBuf::from("src")],
        })?;

        let error = manager
            .record_result(
                &record.id,
                SubagentResultRequest {
                    status: "completed".into(),
                    summary: "changed docs".into(),
                    evidence_refs: Vec::new(),
                    changed_paths: vec![PathBuf::from("docs/note.md")],
                },
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("changed path"));
        Ok(())
    }
}
