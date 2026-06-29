use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auxiliary::AuxiliaryProviderAttempt;
use crate::temp::atomic_temp_path;
use crate::{display_path, ensure_within, now_iso};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuxiliaryUsageCounts {
    pub attempt_count: u64,
    #[serde(default)]
    pub call_count: u64,
    pub success_count: u64,
    pub failed_count: u64,
    pub skipped_count: u64,
    #[serde(default)]
    pub queued_count: u64,
    pub main_ai_count: u64,
    #[serde(default)]
    pub total_duration_ms: u64,
    #[serde(default)]
    pub provider_attempt_count: u64,
    #[serde(default)]
    pub estimated_cost_usd: f64,
    #[serde(default)]
    pub failure_kinds: BTreeMap<String, u64>,
    #[serde(default)]
    pub skip_reasons: BTreeMap<String, u64>,
}

impl AuxiliaryUsageCounts {
    pub fn record(&mut self, status: &str) {
        self.attempt_count += 1;
        if status == "success" {
            self.call_count += 1;
            self.success_count += 1;
        } else if status == "failed" {
            self.call_count += 1;
            self.failed_count += 1;
        } else if status == "queued" {
            self.queued_count += 1;
        } else if status.starts_with("skipped") {
            self.skipped_count += 1;
        }
    }

    pub fn record_main_ai(&mut self) {
        self.main_ai_count += 1;
    }

    pub fn record_event(&mut self, event: &AuxiliaryUsageEvent) {
        self.record(&event.status);
        if let Some(duration_ms) = event.duration_ms {
            self.total_duration_ms = self.total_duration_ms.saturating_add(duration_ms);
        }
        self.provider_attempt_count = self
            .provider_attempt_count
            .saturating_add(event.provider_attempts.len() as u64);
        if let Some(cost) = event.estimated_cost_usd {
            self.estimated_cost_usd += cost;
        }
        if event.status == "failed" {
            let kind = event
                .failure_kind
                .as_deref()
                .or(event.error.as_deref())
                .unwrap_or("unknown_failure");
            *self.failure_kinds.entry(kind.to_string()).or_insert(0) += 1;
        }
        if event.status.starts_with("skipped") {
            let reason = event.skip_reason.as_deref().unwrap_or(&event.status);
            *self.skip_reasons.entry(reason.to_string()).or_insert(0) += 1;
        }
    }

    pub fn auxiliary_success_rate(&self) -> f64 {
        if self.call_count == 0 {
            0.0
        } else {
            (self.success_count as f64 / self.call_count as f64) * 100.0
        }
    }

    pub fn as_json(&self) -> Value {
        let avg_duration_ms = if self.call_count == 0 {
            0.0
        } else {
            self.total_duration_ms as f64 / self.call_count as f64
        };
        json!({
            "attempt_count": self.attempt_count,
            "call_count": self.call_count,
            "success_count": self.success_count,
            "failed_count": self.failed_count,
            "skipped_count": self.skipped_count,
            "queued_count": self.queued_count,
            "main_ai_count": self.main_ai_count,
            "auxiliary_success_rate": self.auxiliary_success_rate(),
            "auxiliary_call_success_rate": self.auxiliary_success_rate(),
            "total_duration_ms": self.total_duration_ms,
            "avg_duration_ms": avg_duration_ms,
            "provider_attempt_count": self.provider_attempt_count,
            "estimated_cost_usd": self.estimated_cost_usd,
            "failure_kinds": self.failure_kinds,
            "skip_reasons": self.skip_reasons,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxiliaryUsageEvent {
    pub task: String,
    pub status: String,
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    pub available: bool,
    pub required: bool,
    pub skip_reason: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub provider_attempts: Vec<AuxiliaryProviderAttempt>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub failure_kind: Option<String>,
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct AuxiliaryUsageStore {
    pub workspace: PathBuf,
    pub state_path: PathBuf,
}

impl AuxiliaryUsageStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into();
        let workspace = if workspace.exists() {
            workspace.canonicalize().context("无法解析工作区路径")?
        } else {
            workspace
        };
        let state_path = workspace
            .join(".RaymanCodingSkill")
            .join("auxiliary_usage.json");
        let state_path =
            ensure_within(&state_path, &workspace, "辅助 AI 使用统计必须位于工作区内")?;
        Ok(Self {
            workspace,
            state_path,
        })
    }

    pub fn record(&self, event: &AuxiliaryUsageEvent) -> Result<Value> {
        let _guard = stats_state_lock()?;
        let mut state = self.state()?;
        let mut counts = usage_counts_from_state(&state);
        counts.record_event(event);
        let mut task_counts = task_usage_counts_from_state(&state, &event.task);
        task_counts.record_event(event);
        let provider_counts = event.provider.as_ref().map(|provider| {
            let mut counts = provider_usage_counts_from_state(&state, provider);
            counts.record_event(event);
            counts
        });
        let object = state
            .as_object_mut()
            .context("使用统计状态根节点不是对象")?;
        object.insert("totals".into(), counts.as_json());
        let by_task = object
            .entry("by_task")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("使用统计 by_task 字段不是对象")?;
        by_task.insert(event.task.clone(), task_counts.as_json());
        if let Some(provider) = &event.provider
            && let Some(provider_counts) = provider_counts
        {
            let by_provider = object
                .entry("by_provider")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("使用统计 by_provider 字段不是对象")?;
            by_provider.insert(provider.clone(), provider_counts.as_json());
        }
        object.insert("last_event".into(), serde_json::to_value(event)?);
        self.write_state(state)?;
        self.report_without_round_unlocked()
    }

    pub fn record_main_ai(&self, task: &str, model: &str) -> Result<Value> {
        let _guard = stats_state_lock()?;
        let mut state = self.state()?;
        let mut counts = usage_counts_from_state(&state);
        counts.record_main_ai();
        let mut task_counts = task_usage_counts_from_state(&state, task);
        task_counts.record_main_ai();
        let provider_counts = model.split_once('/').map(|(provider, _)| {
            let mut counts = provider_usage_counts_from_state(&state, provider);
            counts.record_main_ai();
            (provider.to_string(), counts)
        });
        let object = state
            .as_object_mut()
            .context("使用统计状态根节点不是对象")?;
        object.insert("totals".into(), counts.as_json());
        let by_task = object
            .entry("by_task")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("使用统计 by_task 字段不是对象")?;
        by_task.insert(task.to_string(), task_counts.as_json());
        if let Some((provider, provider_counts)) = provider_counts {
            let by_provider = object
                .entry("by_provider")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("使用统计 by_provider 字段不是对象")?;
            by_provider.insert(provider.to_string(), provider_counts.as_json());
        }
        object.insert(
            "last_main_ai_event".into(),
            json!({
                "task": task,
                "model": model,
                "status": "success",
                "created_at": now_iso(),
            }),
        );
        self.write_state(state)?;
        self.report_without_round_unlocked()
    }

    pub fn report_without_round(&self) -> Result<Value> {
        let _guard = stats_state_lock()?;
        self.report_without_round_unlocked()
    }

    fn report_without_round_unlocked(&self) -> Result<Value> {
        let state = self.state()?;
        Ok(json!({
            "project_total": usage_counts_from_state(&state).as_json(),
            "by_task": state.get("by_task").cloned().unwrap_or_else(|| json!({})),
            "by_provider": state.get("by_provider").cloned().unwrap_or_else(|| json!({})),
            "last_event": state.get("last_event").cloned().unwrap_or(Value::Null),
            "state_path": display_path(&self.state_path),
        }))
    }

    fn state(&self) -> Result<Value> {
        if !self.state_path.exists() {
            return Ok(empty_usage_state(&self.workspace));
        }
        let text = fs::read_to_string(&self.state_path)
            .with_context(|| format!("无法读取使用统计状态文件: {}", self.state_path.display()))?;
        if text.trim().is_empty() {
            return Ok(empty_usage_state(&self.workspace));
        }
        let mut state: Value = serde_json::from_str(&text)
            .with_context(|| format!("无法解析使用统计状态文件: {}", self.state_path.display()))?;
        let object = state
            .as_object_mut()
            .context("使用统计状态根节点不是对象")?;
        if !object.get("totals").map(Value::is_object).unwrap_or(false) {
            object.insert("totals".into(), AuxiliaryUsageCounts::default().as_json());
        }
        if !object.get("by_task").map(Value::is_object).unwrap_or(false) {
            object.insert("by_task".into(), json!({}));
        }
        if !object
            .get("by_provider")
            .map(Value::is_object)
            .unwrap_or(false)
        {
            object.insert("by_provider".into(), json!({}));
        }
        Ok(state)
    }

    fn write_state(&self, mut state: Value) -> Result<()> {
        let object = state
            .as_object_mut()
            .context("使用统计状态根节点不是对象")?;
        object.insert(
            "workspace_path".into(),
            Value::String(display_path(&self.workspace)),
        );
        object.insert("updated_at".into(), Value::String(now_iso()));
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建使用统计状态目录: {}", parent.display()))?;
        }
        write_state_file(&self.state_path, &serde_json::to_string_pretty(&state)?)
            .with_context(|| format!("无法写入使用统计状态文件: {}", self.state_path.display()))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContributionCounts {
    pub production_count: u64,
    pub contribution_count: u64,
}

impl ContributionCounts {
    pub fn record(&mut self, counted: bool) {
        self.production_count += 1;
        if counted {
            self.contribution_count += 1;
        }
    }

    pub fn percentage(&self) -> f64 {
        if self.production_count == 0 {
            0.0
        } else {
            (self.contribution_count as f64 / self.production_count as f64) * 100.0
        }
    }

    pub fn as_json(&self) -> Value {
        json!({
            "production_count": self.production_count,
            "contribution_count": self.contribution_count,
            "contribution_percentage": self.percentage(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxiliaryContributionEvent {
    pub task: String,
    pub counted: bool,
    pub corrected_main_ai: bool,
    pub auxiliary_status: String,
    pub validation_status: String,
    pub fixes_applied_count: usize,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub created_at: String,
}

impl AuxiliaryContributionEvent {
    pub fn implementation_validation(
        auxiliary_status: impl Into<String>,
        validation_status: impl Into<String>,
        corrected_main_ai: bool,
        fixes_applied_count: usize,
    ) -> Self {
        Self::implementation_validation_with_evidence(
            auxiliary_status,
            validation_status,
            corrected_main_ai,
            fixes_applied_count,
            Vec::<String>::new(),
        )
    }

    pub fn implementation_validation_with_evidence(
        auxiliary_status: impl Into<String>,
        validation_status: impl Into<String>,
        corrected_main_ai: bool,
        fixes_applied_count: usize,
        evidence: Vec<String>,
    ) -> Self {
        let auxiliary_status = auxiliary_status.into();
        let validation_status = validation_status.into();
        let counted = auxiliary_status == "success" && corrected_main_ai;
        Self {
            task: "implementation_validation".into(),
            counted,
            corrected_main_ai,
            auxiliary_status,
            validation_status,
            fixes_applied_count,
            evidence,
            reason: contribution_reason(counted, corrected_main_ai, fixes_applied_count),
            created_at: now_iso(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuxiliaryContributionReport {
    pub round: ContributionCounts,
    pub project_total: ContributionCounts,
    pub last_event: Option<AuxiliaryContributionEvent>,
}

impl AuxiliaryContributionReport {
    pub fn as_json(&self) -> Value {
        json!({
            "round": self.round.as_json(),
            "project_total": self.project_total.as_json(),
            "last_event": self.last_event,
        })
    }
}

#[derive(Debug, Clone)]
pub struct AuxiliaryContributionStore {
    pub workspace: PathBuf,
    pub state_path: PathBuf,
}

impl AuxiliaryContributionStore {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into();
        let workspace = if workspace.exists() {
            workspace.canonicalize().context("无法解析工作区路径")?
        } else {
            workspace
        };
        let state_path = workspace
            .join(".RaymanCodingSkill")
            .join("auxiliary_contributions.json");
        let state_path =
            ensure_within(&state_path, &workspace, "辅助 AI 贡献统计必须位于工作区内")?;
        Ok(Self {
            workspace,
            state_path,
        })
    }

    pub fn record(&self, event: &AuxiliaryContributionEvent) -> Result<ContributionCounts> {
        let _guard = stats_state_lock()?;
        let mut state = self.state()?;
        let mut counts = counts_from_state(&state);
        counts.record(event.counted);
        let object = state.as_object_mut().context("统计状态根节点不是对象")?;
        object.insert("totals".into(), counts.as_json());
        object.insert("last_event".into(), serde_json::to_value(event)?);
        append_contribution_event(object, event)?;
        self.write_state(state)?;
        Ok(counts)
    }

    pub fn report_without_round(&self) -> Result<Value> {
        let _guard = stats_state_lock()?;
        self.report_without_round_unlocked()
    }

    fn report_without_round_unlocked(&self) -> Result<Value> {
        let state = self.state()?;
        let counts = counts_from_state(&state);
        Ok(json!({
            "round": Value::Null,
            "project_total": counts.as_json(),
            "last_event": state.get("last_event").cloned().unwrap_or(Value::Null),
            "events": state.get("events").cloned().unwrap_or_else(|| json!([])),
            "state_path": display_path(&self.state_path),
        }))
    }

    fn state(&self) -> Result<Value> {
        if !self.state_path.exists() {
            return Ok(empty_state(&self.workspace));
        }
        let text = fs::read_to_string(&self.state_path)
            .with_context(|| format!("无法读取统计状态文件: {}", self.state_path.display()))?;
        if text.trim().is_empty() {
            return Ok(empty_state(&self.workspace));
        }
        let mut state: Value = serde_json::from_str(&text)
            .with_context(|| format!("无法解析统计状态文件: {}", self.state_path.display()))?;
        if !state.get("totals").map(Value::is_object).unwrap_or(false) {
            state
                .as_object_mut()
                .context("统计状态根节点不是对象")?
                .insert("totals".into(), ContributionCounts::default().as_json());
        }
        if !state.get("events").map(Value::is_array).unwrap_or(false) {
            state
                .as_object_mut()
                .context("统计状态根节点不是对象")?
                .insert("events".into(), json!([]));
        }
        Ok(state)
    }

    fn write_state(&self, mut state: Value) -> Result<()> {
        let object = state.as_object_mut().context("统计状态根节点不是对象")?;
        object.insert(
            "workspace_path".into(),
            Value::String(display_path(&self.workspace)),
        );
        object.insert("updated_at".into(), Value::String(now_iso()));
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建统计状态目录: {}", parent.display()))?;
        }
        write_state_file(&self.state_path, &serde_json::to_string_pretty(&state)?)
            .with_context(|| format!("无法写入统计状态文件: {}", self.state_path.display()))
    }
}

pub fn validation_corrected_main_ai(
    validation_status: &str,
    original_code: &str,
    final_code: &str,
    fixes_applied_count: usize,
) -> bool {
    validation_status.eq_ignore_ascii_case("fixed")
        || fixes_applied_count > 0
        || original_code.trim() != final_code.trim()
}

fn contribution_reason(
    counted: bool,
    corrected_main_ai: bool,
    fixes_applied_count: usize,
) -> Option<String> {
    if counted {
        Some("auxiliary validation corrected the primary result".into())
    } else if !corrected_main_ai {
        Some("auxiliary validation did not identify a primary-result correction".into())
    } else if fixes_applied_count == 0 {
        Some("correction was not backed by applied fixes".into())
    } else {
        Some("auxiliary attempt was not successful".into())
    }
}

fn append_contribution_event(
    object: &mut serde_json::Map<String, Value>,
    event: &AuxiliaryContributionEvent,
) -> Result<()> {
    let events = object
        .entry("events")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("统计状态 events 字段不是数组")?;
    events.push(serde_json::to_value(event)?);
    const MAX_EVENTS: usize = 50;
    if events.len() > MAX_EVENTS {
        events.drain(0..events.len() - MAX_EVENTS);
    }
    Ok(())
}

fn counts_from_state(state: &Value) -> ContributionCounts {
    let totals = state.get("totals");
    ContributionCounts {
        production_count: totals
            .and_then(|value| value.get("production_count"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        contribution_count: totals
            .and_then(|value| value.get("contribution_count"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn empty_state(workspace: &Path) -> Value {
    json!({
        "version": 1,
        "workspace_path": display_path(workspace),
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "totals": ContributionCounts::default().as_json(),
        "last_event": Value::Null,
        "events": [],
    })
}

fn usage_counts_from_state(state: &Value) -> AuxiliaryUsageCounts {
    usage_counts_from_value(state.get("totals"))
}

fn task_usage_counts_from_state(state: &Value, task: &str) -> AuxiliaryUsageCounts {
    usage_counts_from_value(state.get("by_task").and_then(|tasks| tasks.get(task)))
}

fn provider_usage_counts_from_state(state: &Value, provider: &str) -> AuxiliaryUsageCounts {
    usage_counts_from_value(
        state
            .get("by_provider")
            .and_then(|providers| providers.get(provider)),
    )
}

fn usage_counts_from_value(value: Option<&Value>) -> AuxiliaryUsageCounts {
    let success_count = value
        .and_then(|value| value.get("success_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let failed_count = value
        .and_then(|value| value.get("failed_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let attempt_count = value
        .and_then(|value| value.get("attempt_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let skipped_count = value
        .and_then(|value| value.get("skipped_count"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let inferred_call_count = success_count.saturating_add(failed_count);
    let queued_count = value
        .and_then(|value| value.get("queued_count"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            attempt_count
                .saturating_sub(inferred_call_count)
                .saturating_sub(skipped_count)
        });
    AuxiliaryUsageCounts {
        attempt_count,
        call_count: value
            .and_then(|value| value.get("call_count"))
            .and_then(Value::as_u64)
            .unwrap_or(inferred_call_count),
        success_count,
        failed_count,
        skipped_count,
        queued_count,
        main_ai_count: value
            .and_then(|value| value.get("main_ai_count"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        total_duration_ms: value
            .and_then(|value| value.get("total_duration_ms"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        provider_attempt_count: value
            .and_then(|value| value.get("provider_attempt_count"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        estimated_cost_usd: value
            .and_then(|value| value.get("estimated_cost_usd"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        failure_kinds: string_count_map(value.and_then(|value| value.get("failure_kinds"))),
        skip_reasons: string_count_map(value.and_then(|value| value.get("skip_reasons"))),
    }
}

fn string_count_map(value: Option<&Value>) -> BTreeMap<String, u64> {
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| value.as_u64().map(|count| (key.clone(), count)))
                .collect()
        })
        .unwrap_or_default()
}

fn empty_usage_state(workspace: &Path) -> Value {
    json!({
        "version": 1,
        "workspace_path": display_path(workspace),
        "created_at": now_iso(),
        "updated_at": now_iso(),
        "totals": AuxiliaryUsageCounts::default().as_json(),
        "by_task": {},
        "by_provider": {},
        "last_event": Value::Null,
        "last_main_ai_event": Value::Null,
    })
}

fn stats_state_lock() -> Result<std::sync::MutexGuard<'static, ()>> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow!("辅助 AI 统计锁已损坏"))
}

fn write_state_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建统计状态目录: {}", parent.display()))?;
    }
    let temp = atomic_temp_path(path, "stats");
    fs::write(&temp, text).with_context(|| format!("无法写入临时统计状态: {}", temp.display()))?;
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("无法替换旧统计状态: {}", path.display()))?;
    }
    fs::rename(&temp, path).with_context(|| {
        let _ = fs::remove_file(&temp);
        format!(
            "无法替换统计状态文件: {} -> {}",
            temp.display(),
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_is_zero_without_productions() {
        assert_eq!(ContributionCounts::default().percentage(), 0.0);
    }

    #[test]
    fn records_only_counted_auxiliary_corrections() {
        let temp = tempfile::tempdir().unwrap();
        let store = AuxiliaryContributionStore::new(temp.path()).unwrap();
        let counted = AuxiliaryContributionEvent::implementation_validation_with_evidence(
            "success",
            "fixed",
            true,
            1,
            vec!["validator changed final code".into()],
        );
        let not_counted =
            AuxiliaryContributionEvent::implementation_validation("success", "passed", false, 0);

        let totals = store.record(&counted).unwrap();
        assert_eq!(totals.production_count, 1);
        assert_eq!(totals.contribution_count, 1);

        let totals = store.record(&not_counted).unwrap();
        assert_eq!(totals.production_count, 2);
        assert_eq!(totals.contribution_count, 1);
        assert_eq!(totals.percentage(), 50.0);
        let report = store.report_without_round().unwrap();
        assert_eq!(report["events"].as_array().unwrap().len(), 2);
        assert_eq!(
            report["events"][0]["evidence"][0],
            "validator changed final code"
        );
    }

    #[test]
    fn detects_real_validation_corrections() {
        assert!(validation_corrected_main_ai("fixed", "a", "a", 0));
        assert!(validation_corrected_main_ai("passed", "a", "a", 1));
        assert!(validation_corrected_main_ai("passed", "a", "b", 0));
        assert!(!validation_corrected_main_ai("passed", "a", "a", 0));
    }

    #[test]
    fn records_auxiliary_usage_by_status_and_task_without_contribution() {
        let temp = tempfile::tempdir().unwrap();
        let usage = AuxiliaryUsageStore::new(temp.path()).unwrap();
        for (task, status) in [
            ("planning", "success"),
            ("planning", "failed"),
            ("doc_sync", "skipped_task_disabled"),
        ] {
            let provider_attempts = if status == "failed" {
                vec![AuxiliaryProviderAttempt {
                    provider: "aux".into(),
                    model: "aux-model".into(),
                    status: "failed".into(),
                    timeout_seconds: 30,
                    proxy_mode: "direct".into(),
                    duration_ms: 15,
                    error: Some("timeout".into()),
                }]
            } else {
                Vec::new()
            };
            usage
                .record(&AuxiliaryUsageEvent {
                    task: task.into(),
                    status: status.into(),
                    model: Some("aux/auto".into()),
                    provider: Some("aux".into()),
                    available: status != "skipped_task_disabled",
                    required: true,
                    skip_reason: (status == "skipped_task_disabled")
                        .then(|| "task disabled".into()),
                    error: None,
                    provider_attempts,
                    duration_ms: (status == "failed").then_some(30),
                    failure_kind: (status == "failed").then(|| "provider_error".into()),
                    estimated_cost_usd: (status == "failed").then_some(0.01),
                    created_at: now_iso(),
                })
                .unwrap();
        }

        let report = usage.report_without_round().unwrap();
        assert_eq!(report["project_total"]["attempt_count"], 3);
        assert_eq!(report["project_total"]["call_count"], 2);
        assert_eq!(report["project_total"]["success_count"], 1);
        assert_eq!(report["project_total"]["failed_count"], 1);
        assert_eq!(report["project_total"]["skipped_count"], 1);
        assert_eq!(report["project_total"]["queued_count"], 0);
        assert_eq!(report["project_total"]["main_ai_count"], 0);
        assert_eq!(report["project_total"]["total_duration_ms"], 30);
        assert_eq!(report["project_total"]["provider_attempt_count"], 1);
        assert_eq!(
            report["project_total"]["failure_kinds"]["provider_error"],
            1
        );
        assert_eq!(report["project_total"]["skip_reasons"]["task disabled"], 1);
        assert_eq!(
            report["project_total"]["estimated_cost_usd"]
                .as_f64()
                .unwrap(),
            0.01
        );
        assert_eq!(report["by_task"]["planning"]["attempt_count"], 2);
        assert_eq!(report["by_provider"]["aux"]["attempt_count"], 3);
        assert_eq!(report["by_provider"]["aux"]["success_count"], 1);

        let contribution = AuxiliaryContributionStore::new(temp.path())
            .unwrap()
            .report_without_round()
            .unwrap();
        assert_eq!(contribution["project_total"]["production_count"], 0);
        assert!(
            !temp
                .path()
                .join(".RaymanCodingSkill")
                .join("auxiliary_contributions.json")
                .exists()
        );
    }

    #[test]
    fn implementation_validation_correction_creates_contribution_state() {
        let temp = tempfile::tempdir().unwrap();
        let store = AuxiliaryContributionStore::new(temp.path()).unwrap();

        store
            .record(
                &AuxiliaryContributionEvent::implementation_validation_with_evidence(
                    "success",
                    "fixed",
                    true,
                    1,
                    vec!["validator applied a correction".into()],
                ),
            )
            .unwrap();

        assert!(store.state_path.exists());
        let report = store.report_without_round().unwrap();
        assert_eq!(report["project_total"]["production_count"], 1);
        assert_eq!(report["project_total"]["contribution_count"], 1);
        assert_eq!(report["events"][0]["task"], "implementation_validation");
    }

    #[test]
    fn records_main_ai_usage_separately_from_auxiliary_attempts() {
        let temp = tempfile::tempdir().unwrap();
        let usage = AuxiliaryUsageStore::new(temp.path()).unwrap();

        usage
            .record_main_ai("code_generation", "openai/gpt-4o")
            .unwrap();
        usage
            .record(&AuxiliaryUsageEvent {
                task: "code_generation".into(),
                status: "success".into(),
                model: Some("aux/auto".into()),
                provider: Some("aux".into()),
                available: true,
                required: true,
                skip_reason: None,
                error: None,
                provider_attempts: Vec::new(),
                duration_ms: Some(12),
                failure_kind: None,
                estimated_cost_usd: None,
                created_at: now_iso(),
            })
            .unwrap();

        let report = usage.report_without_round().unwrap();
        assert_eq!(report["project_total"]["success_count"], 1);
        assert_eq!(report["project_total"]["attempt_count"], 1);
        assert_eq!(report["project_total"]["call_count"], 1);
        assert_eq!(report["project_total"]["main_ai_count"], 1);
        assert_eq!(
            report["project_total"]["auxiliary_success_rate"]
                .as_f64()
                .unwrap(),
            100.0
        );
        assert_eq!(report["by_task"]["code_generation"]["main_ai_count"], 1);
        assert_eq!(report["by_provider"]["openai"]["main_ai_count"], 1);
        assert_eq!(report["by_provider"]["aux"]["success_count"], 1);
    }

    #[test]
    fn separates_recorded_steps_from_real_auxiliary_calls() {
        let temp = tempfile::tempdir().unwrap();
        let usage = AuxiliaryUsageStore::new(temp.path()).unwrap();

        for status in ["queued", "success", "failed", "skipped_task_disabled"] {
            usage
                .record(&AuxiliaryUsageEvent {
                    task: "code_generation".into(),
                    status: status.into(),
                    model: Some("aux/auto".into()),
                    provider: Some("aux".into()),
                    available: status != "skipped_task_disabled",
                    required: true,
                    skip_reason: (status == "skipped_task_disabled")
                        .then(|| "task disabled".into()),
                    error: None,
                    provider_attempts: Vec::new(),
                    duration_ms: matches!(status, "success" | "failed").then_some(20),
                    failure_kind: (status == "failed").then(|| "provider_error".into()),
                    estimated_cost_usd: None,
                    created_at: now_iso(),
                })
                .unwrap();
        }

        let report = usage.report_without_round().unwrap();
        assert_eq!(report["project_total"]["attempt_count"], 4);
        assert_eq!(report["project_total"]["call_count"], 2);
        assert_eq!(report["project_total"]["queued_count"], 1);
        assert_eq!(report["project_total"]["skipped_count"], 1);
        assert_eq!(
            report["project_total"]["auxiliary_success_rate"]
                .as_f64()
                .unwrap(),
            50.0
        );
        assert_eq!(
            report["project_total"]["avg_duration_ms"].as_f64().unwrap(),
            20.0
        );
    }

    #[test]
    fn legacy_usage_state_infers_call_and_queue_counts() {
        let temp = tempfile::tempdir().unwrap();
        let state_dir = temp.path().join(".RaymanCodingSkill");
        fs::create_dir_all(&state_dir).unwrap();
        fs::write(
            state_dir.join("auxiliary_usage.json"),
            r#"{
  "version": 1,
  "totals": {
    "attempt_count": 4,
    "success_count": 1,
    "failed_count": 1,
    "skipped_count": 1,
    "main_ai_count": 0
  }
}"#,
        )
        .unwrap();

        let report = AuxiliaryUsageStore::new(temp.path())
            .unwrap()
            .report_without_round()
            .unwrap();

        assert_eq!(report["project_total"]["attempt_count"], 4);
        assert_eq!(report["project_total"]["call_count"], 2);
        assert_eq!(report["project_total"]["queued_count"], 1);
        assert_eq!(
            report["project_total"]["auxiliary_success_rate"]
                .as_f64()
                .unwrap(),
            50.0
        );
    }
}
