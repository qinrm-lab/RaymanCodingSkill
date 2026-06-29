use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use sha1::{Digest, Sha1};
use walkdir::WalkDir;

use crate::assets::AssetRetirementManager;
use crate::audit;
use crate::auxiliary::AuxiliaryTaskStore;
use crate::context::ContextKernel;
use crate::evidence::scan_success_claims;
use crate::goal::GoalManager;
use crate::research::ResearchManager;
use crate::subagent::SubagentLedgerManager;
use crate::temp::TempManager;
use crate::{display_path, ensure_within, now_iso};

const ACTIVE_STATUSES: &[&str] = &["pending", "in_progress", "blocked"];
const VALID_KINDS: &[&str] = &["task", "status", "code", "review", "workflow"];
const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".RaymanCodingSkill",
    "target",
    ".tmp",
    "node_modules",
    "dist",
    "build",
    "logs",
];
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "java", "go", "c", "h", "cpp", "hpp", "cs", "php", "rb",
    "swift", "kt", "kts", "scala", "sh", "ps1", "sql",
];

#[derive(Debug, Clone)]
pub struct SessionManager {
    pub workspace: PathBuf,
    pub state_path: PathBuf,
}

impl SessionManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let state_path = workspace
            .join(".RaymanCodingSkill")
            .join("pending_work.json");
        let state_path = ensure_within(&state_path, &workspace, "待完成状态文件必须位于工作区内")?;
        Ok(Self {
            workspace,
            state_path,
        })
    }

    pub fn list_pending(&self) -> Result<Vec<Value>> {
        Ok(self
            .state()?
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| {
                item.get("status")
                    .and_then(Value::as_str)
                    .map(|status| ACTIVE_STATUSES.contains(&status))
                    .unwrap_or(false)
            })
            .collect())
    }

    pub fn add_pending(
        &self,
        title: &str,
        details: &str,
        kind: &str,
        source: &str,
        priority: &str,
        metadata: Value,
    ) -> Result<Value> {
        if title.trim().is_empty() {
            bail!("待完成项标题不能为空");
        }
        if !VALID_KINDS.contains(&kind) {
            bail!("无效待完成类型: {kind}");
        }
        let mut state = self.state()?;
        let item = json!({
            "id": self.new_item_id(title, source),
            "title": title.trim(),
            "details": details.trim(),
            "kind": kind,
            "source": source,
            "priority": priority,
            "status": "pending",
            "created_at": now_iso(),
            "updated_at": now_iso(),
            "completed_at": Value::Null,
            "evidence": [],
            "metadata": metadata,
        });
        state
            .as_object_mut()
            .context("状态文件根对象无效")?
            .entry("items")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .context("状态文件 items 字段无效")?
            .push(item.clone());
        self.write_state(state)?;
        Ok(item)
    }

    pub fn complete(&self, item_id: &str, evidence: &str) -> Result<Value> {
        let mut state = self.state()?;
        let items = state
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .context("状态文件 items 字段无效")?;
        for item in items {
            if item.get("id").and_then(Value::as_str) == Some(item_id) {
                let completed_at = now_iso();
                let object = item.as_object_mut().context("待完成项不是对象")?;
                object.insert("status".into(), Value::String("completed".into()));
                object.insert("updated_at".into(), Value::String(completed_at.clone()));
                object.insert("completed_at".into(), Value::String(completed_at));
                if !evidence.trim().is_empty() {
                    object
                        .entry("evidence")
                        .or_insert_with(|| Value::Array(Vec::new()))
                        .as_array_mut()
                        .context("evidence 字段不是数组")?
                        .push(Value::String(evidence.to_string()));
                }
                let out = Value::Object(object.clone());
                self.write_state(state)?;
                return Ok(out);
            }
        }
        bail!("待完成项不存在: {item_id}");
    }

    pub fn complete_goal_resume_items(&self, goal_id: &str, evidence: &str) -> Result<Vec<Value>> {
        let mut state = self.state()?;
        let items = state
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .context("状态文件 items 字段无效")?;
        let mut completed = Vec::new();
        for item in items {
            let matches_goal = item
                .get("metadata")
                .and_then(|metadata| metadata.get("goal_id"))
                .and_then(Value::as_str)
                == Some(goal_id);
            let active = item
                .get("status")
                .and_then(Value::as_str)
                .map(|status| ACTIVE_STATUSES.contains(&status))
                .unwrap_or(false);
            let resume_item = item
                .get("metadata")
                .and_then(|metadata| metadata.get("resume_command"))
                .and_then(Value::as_str)
                .is_some()
                || item
                    .get("source")
                    .and_then(Value::as_str)
                    .is_some_and(|source| source == "goal_close");
            if matches_goal && active && resume_item {
                let completed_at = now_iso();
                let object = item.as_object_mut().context("待完成项不是对象")?;
                object.insert("status".into(), Value::String("completed".into()));
                object.insert("updated_at".into(), Value::String(completed_at.clone()));
                object.insert("completed_at".into(), Value::String(completed_at));
                if !evidence.trim().is_empty() {
                    object
                        .entry("evidence")
                        .or_insert_with(|| Value::Array(Vec::new()))
                        .as_array_mut()
                        .context("evidence 字段不是数组")?
                        .push(Value::String(evidence.to_string()));
                }
                completed.push(Value::Object(object.clone()));
            }
        }
        if !completed.is_empty() {
            self.write_state(state)?;
        }
        Ok(completed)
    }

    pub fn close_session(
        &self,
        status: &str,
        summary: &str,
        next_steps: &[String],
    ) -> Result<Value> {
        let valid = [
            "success",
            "partial",
            "failed",
            "in_progress",
            "skipped",
            "blocked",
        ];
        if !valid.contains(&status) {
            bail!("会话状态必须是 success/partial/failed/in_progress/skipped/blocked");
        }
        if status == "success" {
            let mut blockers = manual_remote_validation_gap_blockers(summary, next_steps);
            blockers.extend(self.success_blockers()?);
            if !blockers.is_empty() {
                bail!("session close success 门禁未通过:\n{}", blockers.join("\n"));
            }
        }
        let created = if status == "success" {
            Value::Null
        } else {
            self.add_pending(
                if summary.is_empty() {
                    "unfinished session"
                } else {
                    summary
                },
                &next_steps.join("\n"),
                "task",
                "session_close",
                "must",
                json!({
                    "session_status": status,
                    "resume_command": "rayman session recover",
                    "next_steps": next_steps,
                }),
            )?
        };
        let pending = self.list_pending()?;
        Ok(json!({
            "status": status,
            "created_pending": created,
            "pending_count": pending.len(),
            "blocked": !pending.is_empty(),
            "next_priority": self.next_priority_item()?,
            "state_path": display_path(&self.state_path),
        }))
    }

    pub fn success_blockers(&self) -> Result<Vec<String>> {
        let mut blockers = Vec::new();
        for item in self.list_pending()? {
            blockers.push(format!(
                "pending_work {}: {}",
                item["id"].as_str().unwrap_or("unknown"),
                item["title"].as_str().unwrap_or("pending work")
            ));
        }
        if let Some(goal) = GoalManager::new(self.workspace.clone())?.next_active_goal()? {
            blockers.push(format!(
                "active_goal {} [{}] stage={} next_action={}",
                goal.id, goal.status, goal.current_stage, goal.next_action
            ));
        }
        for session in ResearchManager::new(self.workspace.clone())?.unresolved_blockers()? {
            blockers.push(format!(
                "research_blocker {} [{}] stage={}",
                session.id, session.status, session.current_stage
            ));
        }
        match AuxiliaryTaskStore::new(self.workspace.clone())?.success_blockers() {
            Ok(auxiliary_blockers) => {
                for blocker in auxiliary_blockers {
                    blockers.push(format!("auxiliary_ai_blocker: {blocker}"));
                }
            }
            Err(error) => blockers.push(format!("auxiliary_ai_blocker: {error}")),
        }
        let asset_report = AssetRetirementManager::new(self.workspace.clone())?.status()?;
        for blocker in asset_report.blockers {
            blockers.push(format!("asset_retirement_blocker: {blocker}"));
        }
        for blocker in TempManager::new(self.workspace.clone())?.success_blockers()? {
            blockers.push(format!("managed_temp_cleanup_blocker: {blocker}"));
        }
        for blocker in SubagentLedgerManager::new(self.workspace.clone())?.success_blockers()? {
            blockers.push(format!("subagent_ledger_blocker: {blocker}"));
        }
        for blocker in scan_success_claims(self.workspace.clone())? {
            blockers.push(format!("evidence_claim_blocker: {blocker}"));
        }
        let findings = audit::audit_repository(&self.workspace)?;
        if !findings.is_empty() {
            blockers.push(format!(
                "audit_findings:\n{}",
                audit::format_findings_with_triage(&self.workspace, &findings)
            ));
        }
        let context = ContextKernel::new(self.workspace.clone())?.status()?;
        if context["counts"]["context_index_stale"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            blockers.push(format!(
                "stale_context: {}",
                required_actions_text(&context)
            ));
        }
        if context["counts"]["context_os_stale"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            blockers.push(format!(
                "stale_context_os: {}",
                required_actions_text(&context)
            ));
        }
        if context["counts"]["review_blockers"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            blockers.push("review_blockers present in context status".into());
        }
        Ok(blockers)
    }

    pub fn next_priority_item(&self) -> Result<Value> {
        let mut pending = self.list_pending()?;
        pending.sort_by_key(|item| {
            let priority = match item
                .get("priority")
                .and_then(Value::as_str)
                .unwrap_or("must")
            {
                "must" => 0,
                "should" => 1,
                "could" => 2,
                _ => 99,
            };
            let created = item
                .get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (priority, created)
        });
        Ok(pending.into_iter().next().unwrap_or(Value::Null))
    }

    pub fn recovery_report(&self) -> Result<Value> {
        let next_pending = self.next_priority_item()?;
        let goal_manager = GoalManager::new(self.workspace.clone())?;
        let next_goal = goal_manager.next_recoverable_goal()?;
        let pending_resume = next_pending
            .get("metadata")
            .and_then(|metadata| metadata.get("resume_command"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let goal_resume = next_goal
            .as_ref()
            .map(|goal| GoalManager::resume_command(&goal.id));
        let resume_command = pending_resume
            .clone()
            .or_else(|| goal_resume.clone())
            .unwrap_or_else(|| "none".into());
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "status": if resume_command == "none" { "clean" } else { "recoverable" },
            "next_pending": next_pending,
            "next_goal": next_goal.map(serde_json::to_value).transpose()?.unwrap_or(Value::Null),
            "resume_command": resume_command,
            "pending_resume_command": pending_resume,
            "goal_resume_command": goal_resume,
            "state_path": display_path(&self.state_path),
        }))
    }

    pub fn scan_unfinished_code(&self, code: &str) -> Vec<Value> {
        code.lines()
            .enumerate()
            .filter_map(|(index, line)| line_blocker(line, index + 1, None))
            .collect()
    }

    pub fn scan_workspace_unfinished_code(&self, reviewed_path: Option<&Path>) -> Vec<Value> {
        let reviewed = reviewed_path.and_then(|p| p.canonicalize().ok());
        WalkDir::new(&self.workspace)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| !should_skip(entry.path(), &self.workspace))
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| CODE_EXTENSIONS.contains(&ext))
                    .unwrap_or(false)
            })
            .filter(|entry| reviewed.as_ref().map(|p| p != entry.path()).unwrap_or(true))
            .flat_map(|entry| scan_file(entry.path(), &self.workspace))
            .collect()
    }

    pub fn review_blockers(
        &self,
        code: Option<&str>,
        reviewed_path: Option<&Path>,
    ) -> Result<Vec<Value>> {
        let mut blockers: Vec<Value> = self
            .list_pending()?
            .into_iter()
            .map(|item| {
                json!({
                    "type": "pending_work",
                    "id": item.get("id").cloned().unwrap_or(Value::Null),
                    "title": item.get("title").cloned().unwrap_or(Value::Null),
                    "details": item.get("details").cloned().unwrap_or(Value::Null),
                    "status": item.get("status").cloned().unwrap_or(Value::Null),
                    "kind": item.get("kind").cloned().unwrap_or(Value::Null),
                    "source": item.get("source").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        blockers.extend(SubagentLedgerManager::new(self.workspace.clone())?.review_blockers()?);
        blockers.extend(AuxiliaryTaskStore::new(self.workspace.clone())?.review_blockers()?);
        if let Some(code) = code {
            blockers.extend(self.scan_unfinished_code(code));
        }
        blockers.extend(self.scan_workspace_unfinished_code(reviewed_path));
        Ok(blockers)
    }

    fn state(&self) -> Result<Value> {
        if !self.state_path.exists() {
            return Ok(empty_state(&self.workspace));
        }
        let text = fs::read_to_string(&self.state_path)
            .with_context(|| format!("无法读取状态文件: {}", self.state_path.display()))?;
        if text.trim().is_empty() {
            return Ok(empty_state(&self.workspace));
        }
        let mut state: Value = serde_json::from_str(&text)
            .with_context(|| format!("无法解析状态文件: {}", self.state_path.display()))?;
        if !state.get("items").map(Value::is_array).unwrap_or(false) {
            state
                .as_object_mut()
                .context("状态文件根节点不是对象")?
                .insert("items".into(), Value::Array(Vec::new()));
        }
        Ok(state)
    }

    fn write_state(&self, mut state: Value) -> Result<()> {
        let object = state.as_object_mut().context("状态文件根节点不是对象")?;
        object.insert(
            "workspace_path".into(),
            Value::String(display_path(&self.workspace)),
        );
        object.insert("updated_at".into(), Value::String(now_iso()));
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建状态目录: {}", parent.display()))?;
        }
        fs::write(&self.state_path, serde_json::to_string_pretty(&state)?)
            .with_context(|| format!("无法写入状态文件: {}", self.state_path.display()))
    }

    fn new_item_id(&self, title: &str, source: &str) -> String {
        let mut digest = Sha1::new();
        digest.update(display_path(&self.workspace));
        digest.update(title);
        digest.update(source);
        digest.update(now_iso());
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%6fZ");
        format!(
            "todo_{}_{}",
            stamp,
            &format!("{:x}", digest.finalize())[..8]
        )
    }
}

fn required_actions_text(context: &Value) -> String {
    context
        .get("required_actions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "run rayman context refresh and reread current files".into())
}

pub(crate) fn manual_remote_validation_gap_blockers(
    summary: &str,
    next_steps: &[String],
) -> Vec<String> {
    let mut blockers = Vec::new();
    let mut entries = Vec::new();
    entries.push(("summary", summary));
    for step in next_steps {
        entries.push(("next_step", step.as_str()));
    }
    for (source, text) in entries {
        if mentions_unresolved_validation_gap(text) {
            blockers.push(format!(
                "{source}_validation_gap: success close cannot claim completion while manual/remote validation gap remains"
            ));
        }
    }
    blockers
}

fn mentions_unresolved_validation_gap(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let has_gap = [
        "manual validation gap",
        "manual verification gap",
        "remote validation gap",
        "validation gap",
        "cannot directly invoke",
        "unable to invoke",
        "not verified",
        "unverified",
        "manual validation needed",
        "remote validation needed",
        "人工验证缺口",
        "手工验证缺口",
        "远端验证缺口",
        "无法直接 invoke",
        "无法直接调用",
        "未验证",
        "需要人工验证",
        "需要远端验证",
    ]
    .iter()
    .any(|marker| lower.contains(&marker.to_ascii_lowercase()));
    if !has_gap {
        return false;
    }
    ![
        "no manual validation gap",
        "no manual verification gap",
        "no remote validation gap",
        "no validation gap",
        "without manual validation gap",
        "without remote validation gap",
        "manual validation gap resolved",
        "remote validation gap resolved",
        "validation gap resolved",
        "manual validation completed",
        "remote validation completed",
        "verified by manual",
        "verified remotely",
        "无人工验证缺口",
        "无远端验证缺口",
        "没有人工验证缺口",
        "没有远端验证缺口",
        "人工验证已完成",
        "远端验证已完成",
        "验证缺口已解决",
        "已验证",
    ]
    .iter()
    .any(|marker| lower.contains(&marker.to_ascii_lowercase()))
}

fn empty_state(workspace: &Path) -> Value {
    json!({
        "version": 1,
        "workspace_path": display_path(workspace),
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "items": [],
    })
}

fn should_skip(path: &Path, workspace: &Path) -> bool {
    path.strip_prefix(workspace)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref())
            })
        })
        .unwrap_or(true)
}

fn scan_file(path: &Path, workspace: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| line_blocker(line, index + 1, Some((path, workspace))))
        .collect()
}

fn line_blocker(line: &str, line_number: usize, path: Option<(&Path, &Path)>) -> Option<Value> {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    let comment_like = trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*')
        || trimmed.starts_with('#')
        || trimmed.starts_with("--");
    let marker_reason =
        if lower.contains("todo") || trimmed.contains("待完成") || trimmed.contains("未完成")
        {
            Some("unfinished marker")
        } else if lower.contains("fixme") || lower.contains("xxx") || lower.contains("tbd") {
            Some("fix marker")
        } else {
            None
        };
    let reason = if comment_like {
        marker_reason?
    } else if trimmed.starts_with("raise NotImplemented")
        || trimmed.starts_with("todo!()")
        || trimmed.starts_with("unimplemented!()")
    {
        "not implemented"
    } else {
        return None;
    };
    if let Some((path, workspace)) = path {
        let relative = path.strip_prefix(workspace).unwrap_or(path);
        return Some(json!({
            "type": "unfinished_workspace_code",
            "path": relative.to_string_lossy().replace('\\', "/"),
            "line": line_number,
            "reason": reason,
            "snippet": line.trim().chars().take(200).collect::<String>(),
        }));
    }
    Some(json!({
        "type": "unfinished_code",
        "line": line_number,
        "reason": reason,
        "snippet": line.trim().chars().take(200).collect::<String>(),
    }))
}

pub fn object_from_pairs(pairs: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), value.clone());
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextKernel;
    use crate::goal::GoalManager;
    use std::fs;

    #[test]
    fn pending_state_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();
        let item = manager
            .add_pending("finish gate", "details", "task", "test", "must", json!({}))
            .unwrap();
        assert_eq!(manager.list_pending().unwrap().len(), 1);
        manager
            .complete(item["id"].as_str().unwrap(), "verified")
            .unwrap();
        assert!(manager.list_pending().unwrap().is_empty());
    }

    #[test]
    fn add_pending_reports_corrupt_root_instead_of_panicking() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();
        fs::create_dir_all(temp.path().join(".RaymanCodingSkill")).unwrap();
        fs::write(
            temp.path()
                .join(".RaymanCodingSkill")
                .join("pending_work.json"),
            serde_json::to_string_pretty(&json!(["not an object"])).unwrap(),
        )
        .unwrap();

        let error = manager
            .add_pending("finish gate", "details", "task", "test", "must", json!({}))
            .unwrap_err()
            .to_string();

        assert!(error.contains("状态文件根节点不是对象"));
    }

    #[test]
    fn recovery_report_returns_resume_command_from_pending_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();
        manager
            .add_pending(
                "resume goal goal_abc",
                "blocked",
                "workflow",
                "goal_close",
                "must",
                json!({
                    "goal_id": "goal_abc",
                    "resume_command": "rayman goal resume --id goal_abc --until blocked",
                }),
            )
            .unwrap();

        let report = manager.recovery_report().unwrap();

        assert_eq!(report["status"], "recoverable");
        assert_eq!(
            report["resume_command"],
            "rayman goal resume --id goal_abc --until blocked"
        );
    }

    #[test]
    fn success_close_blocks_on_pending_work() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();
        manager
            .add_pending("finish audit", "details", "task", "test", "must", json!({}))
            .unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("pending_work"));
    }

    #[test]
    fn success_close_blocks_on_unreviewed_subagent_ledger() {
        let temp = tempfile::tempdir().unwrap();
        crate::subagent::SubagentLedgerManager::new(temp.path())
            .unwrap()
            .record(crate::subagent::SubagentRecordRequest {
                host_agent_id: "agent-1".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "inspect session close".into(),
                boundary: "read-only".into(),
                read_only: true,
                write_paths: Vec::new(),
            })
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("subagent_ledger_blocker"));
    }

    #[test]
    fn success_close_blocks_on_malformed_auxiliary_task_json() {
        let temp = tempfile::tempdir().unwrap();
        let task_dir = temp
            .path()
            .join(".RaymanCodingSkill")
            .join("auxiliary")
            .join("tasks");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(task_dir.join("bad.json"), "{not json").unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("auxiliary_task_parse_error"));
    }

    #[test]
    fn review_blockers_include_unreviewed_subagent_ledger() {
        let temp = tempfile::tempdir().unwrap();
        crate::subagent::SubagentLedgerManager::new(temp.path())
            .unwrap()
            .record(crate::subagent::SubagentRecordRequest {
                host_agent_id: "agent-1".into(),
                goal_id: None,
                dispatch_request_id: None,
                nickname: None,
                task: "inspect review".into(),
                boundary: "read-only".into(),
                read_only: true,
                write_paths: Vec::new(),
            })
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let blockers = manager.review_blockers(None, None).unwrap();

        assert!(blockers.iter().any(|blocker| {
            blocker["type"].as_str() == Some("subagent_ledger")
                && blocker["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("subagent_ledger_unreviewed")
        }));
    }

    #[test]
    fn success_close_blocks_on_active_goal() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        goals
            .start(
                "finish requested feature",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("active_goal"));
    }

    #[test]
    fn success_close_blocks_on_audit_findings() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(
            temp.path().join("docs").join("old-gate.md"),
            "old validation uses pytest\n",
        )
        .unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("audit_findings"));
        assert!(error.contains("triage="));
    }

    #[test]
    fn success_close_blocks_on_completed_managed_temp() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let temp_manager = TempManager::new(temp.path()).unwrap();
        let run = temp_manager.run_dir("finished validation").unwrap();
        run.complete().unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("managed_temp_cleanup_blocker"));
        assert!(error.contains("rayman temp cleanup --completed"));
    }

    #[test]
    fn success_close_blocks_on_all_managed_temp_cleanup_states() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
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
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("managed_temp_active"));
        assert!(error.contains("managed_temp_completed"));
        assert!(error.contains("managed_temp_stale"));
        assert!(error.contains("managed_temp_failed"));
        assert!(error.contains("managed_temp_foreign"));
    }

    #[test]
    fn success_close_blocks_on_stale_context() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        fs::write(temp.path().join("README.md"), "# changed").unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &[])
            .unwrap_err()
            .to_string();

        assert!(error.contains("stale_context"));
    }

    #[test]
    fn partial_close_records_pending_even_when_audit_would_fail() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("docs")).unwrap();
        fs::write(
            temp.path().join("docs").join("old-gate.md"),
            "old validation uses pytest\n",
        )
        .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let result = manager
            .close_session("partial", "audit finding remains", &["fix old docs".into()])
            .unwrap();

        assert_eq!(result["status"], "partial");
        assert_eq!(result["pending_count"], 1);
        assert_eq!(manager.list_pending().unwrap().len(), 1);
    }

    #[test]
    fn success_close_passes_when_session_gates_are_clear() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let result = manager.close_session("success", "done", &[]).unwrap();

        assert_eq!(result["status"], "success");
        assert_eq!(result["blocked"], false);
    }

    #[test]
    fn success_close_blocks_on_manual_remote_validation_gap_summary() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session(
                "success",
                "manual validation gap remains for remote deployment",
                &[],
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("summary_validation_gap"));
    }

    #[test]
    fn success_close_blocks_on_manual_remote_validation_gap_next_step() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let error = manager
            .close_session("success", "done", &["需要远端验证后才能确认".into()])
            .unwrap_err()
            .to_string();

        assert!(error.contains("next_step_validation_gap"));
    }

    #[test]
    fn success_close_allows_negated_manual_remote_gap_statement() {
        let temp = tempfile::tempdir().unwrap();
        ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();

        let result = manager
            .close_session(
                "success",
                "no manual validation gap; remote validation completed",
                &[],
            )
            .unwrap();

        assert_eq!(result["status"], "success");
    }

    #[test]
    fn unfinished_scan_ignores_marker_words_inside_code() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();
        assert!(
            manager
                .scan_unfinished_code("let word = \"todo\";\n// FIXME: finish this")
                .len()
                == 1
        );
    }

    #[test]
    fn unfinished_scan_ignores_marker_words_inside_scanner_code() {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path()).unwrap();
        let code =
            r#"line.contains("NotImplementedError") || line.contains("raise NotImplemented")"#;

        assert!(manager.scan_unfinished_code(code).is_empty());
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
