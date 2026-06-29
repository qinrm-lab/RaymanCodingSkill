use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::ModelRef;
use crate::session::SessionManager;
use crate::{display_path, ensure_within, now_iso};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuxiliaryProviderAttempt {
    pub provider: String,
    pub model: String,
    pub status: String,
    pub timeout_seconds: u64,
    pub proxy_mode: String,
    pub duration_ms: u128,
    pub error: Option<String>,
}

impl AuxiliaryProviderAttempt {
    pub fn model_ref(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxiliaryReconciliation {
    pub primary_correct: bool,
    pub correction_required: bool,
    pub risk_level: String,
    pub rationale: String,
    pub suggested_fix: Option<String>,
    pub tests: Vec<String>,
    pub raw_response: Option<String>,
}

impl AuxiliaryReconciliation {
    pub fn from_model_response(text: &str) -> Self {
        let parsed = extract_json(text);
        let primary_correct = parsed
            .as_ref()
            .and_then(|value| value.get("primary_correct"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let correction_required = parsed
            .as_ref()
            .and_then(|value| value.get("correction_required"))
            .and_then(Value::as_bool)
            .unwrap_or(!primary_correct);
        let risk_level = parsed
            .as_ref()
            .and_then(|value| value.get("risk_level"))
            .and_then(Value::as_str)
            .unwrap_or(if correction_required { "high" } else { "low" })
            .to_string();
        let rationale = parsed
            .as_ref()
            .and_then(|value| value.get("rationale"))
            .and_then(Value::as_str)
            .unwrap_or(text)
            .to_string();
        let suggested_fix = parsed
            .as_ref()
            .and_then(|value| value.get("suggested_fix"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let tests = parsed
            .as_ref()
            .and_then(|value| value.get("tests"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            primary_correct,
            correction_required,
            risk_level,
            rationale,
            suggested_fix,
            tests,
            raw_response: Some(text.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxiliaryTask {
    pub id: String,
    pub task: String,
    pub status: String,
    pub prompt: String,
    pub primary_output: Option<String>,
    pub start_index: usize,
    pub selected_provider: Option<String>,
    pub provider_attempts: Vec<AuxiliaryProviderAttempt>,
    pub reconciliation: Option<AuxiliaryReconciliation>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct AuxiliaryTaskStore {
    workspace: PathBuf,
    root_dir: PathBuf,
    tasks_dir: PathBuf,
}

impl AuxiliaryTaskStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into();
        let workspace = if workspace.exists() {
            workspace.canonicalize().context("无法解析工作区路径")?
        } else {
            workspace
        };
        let root_dir = ensure_within(
            &workspace.join(".RaymanCodingSkill").join("auxiliary"),
            &workspace,
            "辅助 AI 状态目录必须位于工作区内",
        )?;
        let tasks_dir = ensure_within(
            &root_dir.join("tasks"),
            &workspace,
            "辅助 AI 任务目录必须位于工作区内",
        )?;
        Ok(Self {
            workspace,
            root_dir,
            tasks_dir,
        })
    }

    pub fn enqueue(
        &self,
        task: &str,
        prompt: &str,
        primary_output: Option<&str>,
        start_index: usize,
        selected_provider: Option<String>,
    ) -> Result<AuxiliaryTask> {
        let timestamp = now_iso();
        let id = format!(
            "aux_{}_{}",
            timestamp
                .chars()
                .filter(|ch| ch.is_ascii_alphanumeric())
                .collect::<String>(),
            std::process::id()
        );
        let task = AuxiliaryTask {
            id,
            task: task.to_string(),
            status: "queued".into(),
            prompt: prompt.to_string(),
            primary_output: primary_output.map(str::to_string),
            start_index,
            selected_provider,
            provider_attempts: Vec::new(),
            reconciliation: None,
            error: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        self.write_task(&task)?;
        Ok(task)
    }

    pub fn load(&self, id: &str) -> Result<AuxiliaryTask> {
        let path = self.task_path(id)?;
        let text = fs::read_to_string(&path)
            .with_context(|| format!("无法读取辅助 AI 任务: {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("无法解析辅助 AI 任务: {}", path.display()))
    }

    pub fn list(&self) -> Result<Vec<AuxiliaryTask>> {
        if !self.tasks_dir.exists() {
            return Ok(Vec::new());
        }
        let mut tasks = Vec::new();
        for entry in fs::read_dir(&self.tasks_dir)
            .with_context(|| format!("无法读取辅助 AI 任务目录: {}", self.tasks_dir.display()))?
        {
            let entry = entry?;
            if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let path = entry.path();
            let text = fs::read_to_string(&path)
                .with_context(|| format!("无法读取辅助 AI 任务: {}", path.display()))?;
            let task = serde_json::from_str::<AuxiliaryTask>(&text)
                .map_err(|error| anyhow::anyhow!("{}", self.parse_error_blocker(&path, &error)))?;
            tasks.push(task);
        }
        tasks.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        Ok(tasks)
    }

    pub fn mark_running(&self, id: &str) -> Result<AuxiliaryTask> {
        let mut task = self.load(id)?;
        task.status = "running".into();
        task.updated_at = now_iso();
        self.write_task(&task)?;
        Ok(task)
    }

    pub fn complete(
        &self,
        mut task: AuxiliaryTask,
        attempts: Vec<AuxiliaryProviderAttempt>,
        reconciliation: AuxiliaryReconciliation,
    ) -> Result<AuxiliaryTask> {
        task.status = "succeeded".into();
        task.provider_attempts = attempts;
        task.reconciliation = Some(reconciliation);
        task.error = None;
        task.updated_at = now_iso();
        self.write_task(&task)?;
        Ok(task)
    }

    pub fn fail(
        &self,
        mut task: AuxiliaryTask,
        attempts: Vec<AuxiliaryProviderAttempt>,
        error: String,
    ) -> Result<AuxiliaryTask> {
        task.status = "failed".into();
        task.provider_attempts = attempts;
        task.error = Some(error);
        task.updated_at = now_iso();
        self.write_task(&task)?;
        Ok(task)
    }

    pub fn reconcile_ready(&self) -> Result<Vec<AuxiliaryTask>> {
        let mut reconciled = Vec::new();
        for task in self.list()? {
            if task.status == "succeeded" {
                reconciled.push(self.reconcile_task(task)?);
            }
        }
        Ok(reconciled)
    }

    pub fn unresolved_conflicts(&self) -> Result<Vec<AuxiliaryTask>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|task| task.status == "conflict" || task.status == "repairing")
            .collect())
    }

    pub fn success_blockers(&self) -> Result<Vec<String>> {
        Ok(self
            .unresolved_conflicts()?
            .into_iter()
            .map(|task| {
                format!(
                    "auxiliary_ai_unresolved_conflict {}: task={} status={} recovery=run `rayman auxiliary reconcile` or repair the task evidence",
                    task.id, task.task, task.status
                )
            })
            .collect())
    }

    pub fn review_blockers(&self) -> Result<Vec<Value>> {
        match self.success_blockers() {
            Ok(blockers) => Ok(blockers
                .into_iter()
                .map(|reason| {
                    json!({
                        "type": "auxiliary_ai",
                        "reason": reason,
                    })
                })
                .collect()),
            Err(error) => Ok(vec![json!({
                "type": "auxiliary_ai",
                "reason": error.to_string(),
            })]),
        }
    }

    pub fn status_json(&self) -> Result<Value> {
        let tasks = self.list()?;
        let unresolved = tasks
            .iter()
            .filter(|task| task.status == "conflict" || task.status == "repairing")
            .count();
        Ok(json!({
            "state_dir": display_path(&self.root_dir),
            "task_count": tasks.len(),
            "unresolved_conflicts": unresolved,
            "tasks": tasks,
        }))
    }

    fn reconcile_task(&self, mut task: AuxiliaryTask) -> Result<AuxiliaryTask> {
        let Some(reconciliation) = task.reconciliation.clone() else {
            bail!("辅助 AI 任务缺少 reconciliation: {}", task.id);
        };
        if reconciliation.correction_required || !reconciliation.primary_correct {
            task.status = "conflict".into();
            SessionManager::new(self.workspace.clone())?.add_pending(
                &format!("resolve auxiliary AI conflict {}", task.id),
                &reconciliation.rationale,
                "workflow",
                "auxiliary_reconcile",
                "must",
                json!({
                    "task_id": task.id,
                    "risk_level": reconciliation.risk_level,
                    "suggested_fix": reconciliation.suggested_fix,
                    "tests": reconciliation.tests,
                }),
            )?;
        } else {
            task.status = "reconciled_ok".into();
        }
        task.updated_at = now_iso();
        self.write_task(&task)?;
        Ok(task)
    }

    fn task_path(&self, id: &str) -> Result<PathBuf> {
        if !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            bail!("无效辅助 AI 任务 ID: {id}");
        }
        ensure_within(
            &self.tasks_dir.join(format!("{id}.json")),
            &self.workspace,
            "辅助 AI 任务文件必须位于工作区内",
        )
    }

    fn write_task(&self, task: &AuxiliaryTask) -> Result<()> {
        let path = self.task_path(&task.id)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建辅助 AI 任务目录: {}", parent.display()))?;
        }
        fs::write(&path, serde_json::to_string_pretty(task)?)
            .with_context(|| format!("无法写入辅助 AI 任务: {}", path.display()))
    }

    fn parse_error_blocker(&self, path: &Path, error: &serde_json::Error) -> String {
        format!(
            "auxiliary_task_parse_error: state_path={} error={} recovery=repair or remove the malformed auxiliary task JSON before closing success",
            display_path(path),
            error
        )
    }
}

#[derive(Debug, Clone)]
pub struct AuxiliaryProviderStateStore {
    workspace: PathBuf,
    state_path: PathBuf,
}

impl AuxiliaryProviderStateStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into();
        let workspace = if workspace.exists() {
            workspace.canonicalize().context("无法解析工作区路径")?
        } else {
            workspace
        };
        let state_path = ensure_within(
            &workspace
                .join(".RaymanCodingSkill")
                .join("auxiliary")
                .join("provider_state.json"),
            &workspace,
            "辅助 AI provider 状态必须位于工作区内",
        )?;
        Ok(Self {
            workspace,
            state_path,
        })
    }

    pub fn reserve_start_index(&self, providers: &[ModelRef]) -> Result<usize> {
        if providers.is_empty() {
            bail!("辅助 AI provider 列表为空");
        }
        let mut state = self.state()?;
        let current = state.get("next_index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let selected = current % providers.len();
        state["next_index"] = json!((selected + 1) % providers.len());
        state["updated_at"] = json!(now_iso());
        self.write_state(&state)?;
        Ok(selected)
    }

    pub fn next_index(&self, provider_count: usize) -> Result<usize> {
        if provider_count == 0 {
            return Ok(0);
        }
        Ok(self
            .state()?
            .get("next_index")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize
            % provider_count)
    }

    fn state(&self) -> Result<Value> {
        if !self.state_path.exists() {
            return Ok(json!({
                "version": 1,
                "workspace_path": display_path(&self.workspace),
                "next_index": 0,
                "created_at": now_iso(),
                "updated_at": now_iso(),
            }));
        }
        let text = fs::read_to_string(&self.state_path).with_context(|| {
            format!(
                "无法读取辅助 AI provider 状态: {}",
                self.state_path.display()
            )
        })?;
        serde_json::from_str(&text).with_context(|| {
            format!(
                "无法解析辅助 AI provider 状态: {}",
                self.state_path.display()
            )
        })
    }

    fn write_state(&self, state: &Value) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("无法创建辅助 AI provider 状态目录: {}", parent.display())
            })?;
        }
        fs::write(&self.state_path, serde_json::to_string_pretty(state)?).with_context(|| {
            format!(
                "无法写入辅助 AI provider 状态: {}",
                self.state_path.display()
            )
        })
    }
}

pub fn provider_attempt_order(providers: &[ModelRef], start_index: usize) -> Vec<ModelRef> {
    if providers.is_empty() {
        return Vec::new();
    }
    (0..providers.len())
        .map(|offset| providers[(start_index + offset) % providers.len()].clone())
        .collect()
}

fn extract_json(text: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_state_rotates_across_calls() {
        let temp = tempfile::tempdir().unwrap();
        let store = AuxiliaryProviderStateStore::new(temp.path()).unwrap();
        let providers = vec![
            ModelRef {
                provider: "a".into(),
                model: "m".into(),
            },
            ModelRef {
                provider: "b".into(),
                model: "m".into(),
            },
        ];

        assert_eq!(store.reserve_start_index(&providers).unwrap(), 0);
        assert_eq!(store.reserve_start_index(&providers).unwrap(), 1);
        assert_eq!(store.reserve_start_index(&providers).unwrap(), 0);
    }

    #[test]
    fn reconciliation_conflict_creates_pending_work() {
        let temp = tempfile::tempdir().unwrap();
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
        let task = store
            .complete(
                task,
                Vec::new(),
                AuxiliaryReconciliation {
                    primary_correct: false,
                    correction_required: true,
                    risk_level: "high".into(),
                    rationale: "primary missed a requirement".into(),
                    suggested_fix: Some("repair it".into()),
                    tests: vec!["cargo test".into()],
                    raw_response: None,
                },
            )
            .unwrap();

        let task = store.reconcile_task(task).unwrap();

        assert_eq!(task.status, "conflict");
        let pending = SessionManager::new(temp.path())
            .unwrap()
            .list_pending()
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0]["metadata"]["task_id"], task.id);
    }

    #[test]
    fn malformed_auxiliary_task_json_is_a_success_blocker() {
        let temp = tempfile::tempdir().unwrap();
        let task_dir = temp
            .path()
            .join(".RaymanCodingSkill")
            .join("auxiliary")
            .join("tasks");
        fs::create_dir_all(&task_dir).unwrap();
        fs::write(task_dir.join("bad.json"), "{not json").unwrap();
        let store = AuxiliaryTaskStore::new(temp.path()).unwrap();

        let error = store.success_blockers().unwrap_err().to_string();
        let review = store.review_blockers().unwrap();

        assert!(error.contains("auxiliary_task_parse_error"));
        assert!(
            review[0]["reason"]
                .as_str()
                .unwrap()
                .contains("auxiliary_task_parse_error")
        );
    }
}
