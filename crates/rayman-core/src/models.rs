use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auxiliary::{
    AuxiliaryProviderAttempt, AuxiliaryProviderStateStore, AuxiliaryReconciliation,
    AuxiliaryTaskStore, provider_attempt_order,
};
use crate::config::{ConfigManager, ModelRef, ProviderProxyConfig};
use crate::stats::{
    AuxiliaryContributionEvent, AuxiliaryContributionReport, AuxiliaryContributionStore,
    AuxiliaryUsageEvent, AuxiliaryUsageStore, ContributionCounts, validation_corrected_main_ai,
};

type AuxiliaryProviderCallResult = std::result::Result<
    (ModelRef, Vec<AuxiliaryProviderAttempt>, String),
    (Vec<AuxiliaryProviderAttempt>, String),
>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteAttempt {
    pub model: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuxiliaryAttempt {
    pub task: String,
    pub model: Option<String>,
    #[serde(default)]
    pub selected_provider: Option<String>,
    pub required: bool,
    pub available: bool,
    pub status: String,
    pub skip_reason: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub provider_attempts: Vec<AuxiliaryProviderAttempt>,
    #[serde(default)]
    pub queued_task_id: Option<String>,
    #[serde(default)]
    pub async_status: Option<String>,
    #[serde(default)]
    pub reconciliation_status: Option<String>,
}

impl AuxiliaryAttempt {
    fn skipped(
        task: Option<&str>,
        model: Option<String>,
        required: bool,
        status: &str,
        reason: &str,
        record_reason: bool,
    ) -> Self {
        Self {
            task: task.unwrap_or("default").to_string(),
            model,
            selected_provider: None,
            required,
            available: false,
            status: status.to_string(),
            skip_reason: record_reason.then(|| reason.to_string()),
            error: None,
            provider_attempts: Vec::new(),
            queued_task_id: None,
            async_status: None,
            reconciliation_status: None,
        }
    }

    fn completed(
        task: Option<&str>,
        model: String,
        selected_provider: Option<String>,
        required: bool,
        status: &str,
        error: Option<String>,
        provider_attempts: Vec<AuxiliaryProviderAttempt>,
    ) -> Self {
        Self {
            task: task.unwrap_or("default").to_string(),
            model: Some(model),
            selected_provider,
            required,
            available: true,
            status: status.to_string(),
            skip_reason: None,
            error,
            provider_attempts,
            queued_task_id: None,
            async_status: None,
            reconciliation_status: None,
        }
    }

    fn queued(
        task: Option<&str>,
        model: String,
        selected_provider: Option<String>,
        required: bool,
        queued_task_id: String,
    ) -> Self {
        Self {
            task: task.unwrap_or("default").to_string(),
            model: Some(model),
            selected_provider,
            required,
            available: true,
            status: "queued".into(),
            skip_reason: None,
            error: None,
            provider_attempts: Vec::new(),
            queued_task_id: Some(queued_task_id),
            async_status: Some("queued".into()),
            reconciliation_status: Some("pending".into()),
        }
    }

    fn queue_failed(
        task: Option<&str>,
        model: Option<String>,
        selected_provider: Option<String>,
        required: bool,
        error: String,
    ) -> Self {
        Self {
            task: task.unwrap_or("default").to_string(),
            model,
            selected_provider,
            required,
            available: false,
            status: "failed".into(),
            skip_reason: None,
            error: Some(error),
            provider_attempts: Vec::new(),
            queued_task_id: None,
            async_status: Some("queue_failed".into()),
            reconciliation_status: Some("not_started".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentManager {
    pub config: ConfigManager,
    pub explicit_model: Option<ModelRef>,
    pub route_mode: Option<String>,
    pub no_fallback: bool,
    pub task_override: Option<String>,
    pub last_route_attempts: Vec<RouteAttempt>,
    pub last_auxiliary_attempt: Option<AuxiliaryAttempt>,
    pub auxiliary_contribution_round: ContributionCounts,
    pub last_auxiliary_contribution: Option<AuxiliaryContributionEvent>,
}

impl AgentManager {
    pub fn new(
        root: impl Into<std::path::PathBuf>,
        model_type: Option<String>,
        model_name: Option<String>,
        route_mode: Option<String>,
        no_fallback: bool,
    ) -> Result<Self> {
        if model_type.is_some() != model_name.is_some() {
            bail!("model_type and model_name must be provided together");
        }
        let config = ConfigManager::new(root)?;
        let explicit_model = match (model_type, model_name) {
            (Some(provider), Some(model)) => Some(ModelRef { provider, model }),
            (Some(_), None) | (None, Some(_)) => unreachable!("partial override checked above"),
            (None, None) => None,
        };
        Ok(Self {
            config,
            explicit_model,
            route_mode,
            no_fallback,
            task_override: None,
            last_route_attempts: Vec::new(),
            last_auxiliary_attempt: None,
            auxiliary_contribution_round: ContributionCounts::default(),
            last_auxiliary_contribution: None,
        })
    }

    pub fn complete(&mut self, prompt: &str, task: Option<&str>) -> Result<String> {
        let task = self
            .task_override
            .clone()
            .or_else(|| task.map(str::to_string));
        let task = task.as_deref();
        self.last_auxiliary_attempt = None;
        let async_auxiliary = self.config.auxiliary_async_enabled();
        let prompt = if async_auxiliary {
            prompt.to_string()
        } else {
            match self.auxiliary_advice(prompt, task)? {
                Some(advice) => prompt_with_advice(prompt, &advice),
                None => prompt.to_string(),
            }
        };
        let candidates = self.config.route_candidates(
            task,
            self.route_mode.as_deref(),
            self.explicit_model.clone(),
            self.no_fallback,
        );
        self.last_route_attempts.clear();
        let mut last_error = None;
        for candidate in candidates {
            let model_name = candidate.as_string();
            match self.call_model(&candidate, &prompt) {
                Ok(text) => {
                    self.last_route_attempts.push(RouteAttempt {
                        model: model_name,
                        status: "success".to_string(),
                        error: None,
                    });
                    self.record_main_ai_usage(task, &candidate.as_string())?;
                    if async_auxiliary
                        && let Err(error) =
                            self.enqueue_auxiliary_reconciliation(&prompt, &text, task)
                    {
                        let configured_models =
                            self.config.auxiliary_providers().unwrap_or_default();
                        let selected = configured_models.first();
                        let attempt = AuxiliaryAttempt::queue_failed(
                            task,
                            selected.map(ModelRef::as_string),
                            selected.map(|model| model.provider.clone()),
                            self.config.auxiliary_required_when_available(),
                            format_error_chain(&error),
                        );
                        let _ = self.record_auxiliary_usage(&attempt);
                        self.last_auxiliary_attempt = Some(attempt);
                    }
                    return Ok(text);
                }
                Err(error) => {
                    let message = format_error_chain(&error);
                    self.last_route_attempts.push(RouteAttempt {
                        model: model_name,
                        status: "failed".to_string(),
                        error: Some(message.clone()),
                    });
                    last_error = Some(message);
                }
            }
        }
        bail!(
            "所有模型路由均失败: {}",
            last_error.unwrap_or_else(|| "无可用模型".to_string())
        )
    }

    pub fn primary_advisory(&mut self, prompt: &str, task: Option<&str>) -> Result<String> {
        let task = self
            .task_override
            .clone()
            .or_else(|| task.map(str::to_string));
        let task = task.as_deref();
        self.last_auxiliary_attempt = None;
        let candidates = self.config.route_candidates(
            task,
            self.route_mode.as_deref(),
            self.explicit_model.clone(),
            self.no_fallback,
        );
        self.last_route_attempts.clear();
        let mut last_error = None;
        for candidate in candidates {
            let model_name = candidate.as_string();
            match self.call_model(&candidate, prompt) {
                Ok(text) => {
                    self.last_route_attempts.push(RouteAttempt {
                        model: model_name,
                        status: "success".to_string(),
                        error: None,
                    });
                    self.record_main_ai_usage(task, &candidate.as_string())?;
                    return Ok(text);
                }
                Err(error) => {
                    let message = format_error_chain(&error);
                    self.last_route_attempts.push(RouteAttempt {
                        model: model_name,
                        status: "failed".to_string(),
                        error: Some(message.clone()),
                    });
                    last_error = Some(message);
                }
            }
        }
        bail!(
            "所有主模型 advisory 路由均失败: {}",
            last_error.unwrap_or_else(|| "无可用模型".to_string())
        )
    }

    pub fn last_successful_model(&self) -> String {
        self.last_route_attempts
            .iter()
            .rev()
            .find(|attempt| attempt.status == "success")
            .map(|attempt| attempt.model.clone())
            .or_else(|| self.explicit_model.as_ref().map(ModelRef::as_string))
            .unwrap_or_else(|| self.config.default_model().as_string())
    }

    pub fn auxiliary_usage_json(&self) -> Value {
        let attempt = self.last_auxiliary_attempt.as_ref();
        let model = self.config.auxiliary_model();
        let model_name = attempt
            .and_then(|attempt| attempt.model.clone())
            .or_else(|| model.as_ref().map(ModelRef::as_string));
        json!({
            "enabled": self.config.auxiliary_ai_enabled(),
            "model": model_name,
            "task": attempt.map(|attempt| attempt.task.as_str()),
            "required": attempt
                .map(|attempt| attempt.required)
                .unwrap_or_else(|| self.config.auxiliary_required_when_available()),
            "available": attempt.map(|attempt| attempt.available).unwrap_or(false),
            "status": attempt
                .map(|attempt| attempt.status.as_str())
                .unwrap_or("not_used"),
            "skip_reason": attempt.and_then(|attempt| attempt.skip_reason.as_deref()),
            "error": attempt.and_then(|attempt| attempt.error.as_deref()),
            "attempt": self.last_auxiliary_attempt,
            "contribution_stats": self.auxiliary_contribution_report_json(),
            "usage_stats": self.auxiliary_usage_report_json(),
            "queued_task_id": attempt.and_then(|attempt| attempt.queued_task_id.as_deref()),
            "selected_provider": attempt.and_then(|attempt| attempt.selected_provider.as_deref()),
            "provider_attempts": attempt
                .map(|attempt| serde_json::to_value(&attempt.provider_attempts).unwrap_or(Value::Null))
                .unwrap_or_else(|| Value::Array(Vec::new())),
            "async_status": attempt.and_then(|attempt| attempt.async_status.as_deref()),
            "reconciliation_status": attempt.and_then(|attempt| attempt.reconciliation_status.as_deref()),
            "source": self
                .config
                .auxiliary_source()
                .and_then(|value| serde_json::to_value(value).ok())
                .unwrap_or(Value::Null),
        })
    }

    pub fn auxiliary_target_report(&self) -> Value {
        let mut report = self.config.auxiliary_target_report();
        if let Some(object) = report.as_object_mut()
            && let Ok(providers) = self.config.auxiliary_providers()
            && let Ok(next_index) = AuxiliaryProviderStateStore::new(&self.config.root)
                .and_then(|store| store.next_index(providers.len()))
        {
            object.insert("next_provider_index".into(), json!(next_index));
            object.insert(
                "next_provider".into(),
                providers
                    .get(next_index)
                    .map(ModelRef::as_string)
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        report
    }

    pub fn auxiliary_task_status_json(&self) -> Result<Value> {
        AuxiliaryTaskStore::new(&self.config.root)?.status_json()
    }

    pub fn reconcile_auxiliary_tasks(&self) -> Result<Value> {
        let reconciled = AuxiliaryTaskStore::new(&self.config.root)?.reconcile_ready()?;
        Ok(json!({
            "reconciled_count": reconciled.len(),
            "tasks": reconciled,
        }))
    }

    pub fn run_auxiliary_worker(&mut self, task_id: &str) -> Result<Value> {
        let store = AuxiliaryTaskStore::new(&self.config.root)?;
        let task = store.mark_running(task_id)?;
        let task_name = task.task.clone();
        let required = self.config.auxiliary_required_when_available();
        let prompt = auxiliary_reconciliation_prompt(
            &task.task,
            &task.prompt,
            task.primary_output.as_deref().unwrap_or(""),
        );
        match self.call_auxiliary_providers(&prompt, task.start_index) {
            Ok((model_ref, attempts, text)) => {
                let attempt = AuxiliaryAttempt::completed(
                    Some(&task_name),
                    model_ref.as_string(),
                    Some(model_ref.provider.clone()),
                    required,
                    "success",
                    None,
                    attempts.clone(),
                );
                self.record_auxiliary_usage(&attempt)?;
                self.last_auxiliary_attempt = Some(attempt);
                let reconciliation = AuxiliaryReconciliation::from_model_response(&text);
                let task = store.complete(task, attempts, reconciliation)?;
                let reconciled = store.reconcile_ready()?;
                Ok(json!({
                    "status": "completed",
                    "task": task,
                    "reconciled": reconciled,
                }))
            }
            Err((attempts, message)) => {
                let record_reason = self.config.auxiliary_record_skip_reason();
                let model_name = attempts
                    .first()
                    .map(AuxiliaryProviderAttempt::model_ref)
                    .or_else(|| task.selected_provider.clone())
                    .unwrap_or_else(|| "unknown/unknown".into());
                let selected_provider = attempts.first().map(|attempt| attempt.provider.clone());
                let all_skipped = !attempts.is_empty()
                    && attempts.iter().all(|attempt| {
                        attempt.status.starts_with("skipped")
                            || attempt.status == "skipped_same_model"
                    });
                let mut attempt = if all_skipped {
                    AuxiliaryAttempt::skipped(
                        Some(&task_name),
                        Some(model_name),
                        required,
                        attempts
                            .first()
                            .map(|attempt| attempt.status.as_str())
                            .unwrap_or("skipped_unavailable"),
                        &message,
                        record_reason,
                    )
                } else {
                    AuxiliaryAttempt::completed(
                        Some(&task_name),
                        model_name,
                        selected_provider.clone(),
                        required,
                        "failed",
                        Some(message.clone()),
                        Vec::new(),
                    )
                };
                attempt.selected_provider = selected_provider;
                attempt.provider_attempts = attempts.clone();
                self.record_auxiliary_usage(&attempt)?;
                self.last_auxiliary_attempt = Some(attempt);
                let task = store.fail(task, attempts, message.clone())?;
                Ok(json!({
                    "status": "failed",
                    "task": task,
                    "error": message,
                }))
            }
        }
    }

    pub fn record_auxiliary_validation_outcome(
        &mut self,
        validation_status: &str,
        original_code: &str,
        final_code: &str,
        fixes_applied_count: usize,
    ) -> Result<Value> {
        let auxiliary_status = self
            .last_auxiliary_attempt
            .as_ref()
            .map(|attempt| attempt.status.clone())
            .unwrap_or_else(|| "not_used".into());
        let corrected_main_ai = validation_corrected_main_ai(
            validation_status,
            original_code,
            final_code,
            fixes_applied_count,
        );
        let event = AuxiliaryContributionEvent::implementation_validation(
            auxiliary_status,
            validation_status,
            corrected_main_ai,
            fixes_applied_count,
        );
        self.auxiliary_contribution_round.record(event.counted);
        let project_total = AuxiliaryContributionStore::new(&self.config.root)?.record(&event)?;
        self.last_auxiliary_contribution = Some(event.clone());
        Ok(AuxiliaryContributionReport {
            round: self.auxiliary_contribution_round.clone(),
            project_total,
            last_event: Some(event),
        }
        .as_json())
    }

    pub fn auxiliary_contribution_report_json(&self) -> Value {
        match AuxiliaryContributionStore::new(&self.config.root)
            .and_then(|store| store.report_without_round())
        {
            Ok(project) => json!({
                "round": self.auxiliary_contribution_round.as_json(),
                "project_total": project
                    .get("project_total")
                    .cloned()
                    .unwrap_or(Value::Null),
                "last_event": self
                    .last_auxiliary_contribution
                    .as_ref()
                    .and_then(|event| serde_json::to_value(event).ok())
                    .unwrap_or_else(|| project.get("last_event").cloned().unwrap_or(Value::Null)),
            }),
            Err(error) => json!({
                "round": self.auxiliary_contribution_round.as_json(),
                "project_total": Value::Null,
                "last_event": self.last_auxiliary_contribution,
                "error": error.to_string(),
            }),
        }
    }

    fn enqueue_auxiliary_reconciliation(
        &mut self,
        prompt: &str,
        primary_output: &str,
        task: Option<&str>,
    ) -> Result<()> {
        let required = self.config.auxiliary_required_when_available();
        let record_reason = self.config.auxiliary_record_skip_reason();
        let configured_models = self.config.auxiliary_providers()?;
        let configured_model_name = configured_models.first().map(ModelRef::as_string);
        if !self.config.auxiliary_ai_enabled() {
            let attempt = AuxiliaryAttempt::skipped(
                task,
                configured_model_name,
                required,
                "skipped_disabled",
                "auxiliary_ai.enabled is false",
                record_reason,
            );
            self.record_auxiliary_usage(&attempt)?;
            self.last_auxiliary_attempt = Some(attempt);
            return Ok(());
        }
        if !self.config.auxiliary_task_enabled(task) {
            let attempt = AuxiliaryAttempt::skipped(
                task,
                configured_model_name,
                required,
                "skipped_task_disabled",
                "task is not listed in auxiliary_ai.tasks",
                record_reason,
            );
            self.record_auxiliary_usage(&attempt)?;
            self.last_auxiliary_attempt = Some(attempt);
            return Ok(());
        }
        if configured_models.is_empty() {
            let attempt = AuxiliaryAttempt::skipped(
                task,
                None,
                required,
                "skipped_missing_model",
                "auxiliary_ai.providers is empty",
                record_reason,
            );
            self.record_auxiliary_usage(&attempt)?;
            self.last_auxiliary_attempt = Some(attempt);
            return Ok(());
        }
        let start_index = AuxiliaryProviderStateStore::new(&self.config.root)?
            .reserve_start_index(&configured_models)?;
        let selected = configured_models.get(start_index).cloned();
        let store = AuxiliaryTaskStore::new(&self.config.root)?;
        let task_record = store.enqueue(
            task.unwrap_or("default"),
            prompt,
            Some(primary_output),
            start_index,
            selected.as_ref().map(ModelRef::as_string),
        )?;
        let attempt = AuxiliaryAttempt::queued(
            task,
            selected
                .as_ref()
                .map(ModelRef::as_string)
                .unwrap_or_else(|| configured_models[0].as_string()),
            selected.as_ref().map(|model| model.provider.clone()),
            required,
            task_record.id.clone(),
        );
        self.record_auxiliary_usage(&attempt)?;
        self.last_auxiliary_attempt = Some(attempt);
        self.spawn_auxiliary_worker(&task_record.id);
        Ok(())
    }

    fn spawn_auxiliary_worker(&self, task_id: &str) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let file_name = exe.file_name().and_then(|name| name.to_str()).unwrap_or("");
        if !file_name.eq_ignore_ascii_case("rayman.exe") && file_name != "rayman" {
            return;
        }
        let _ = self.auxiliary_worker_command(exe, task_id).spawn();
    }

    fn auxiliary_worker_command(
        &self,
        exe: std::path::PathBuf,
        task_id: &str,
    ) -> std::process::Command {
        let mut command = std::process::Command::new(exe);
        command
            .current_dir(&self.config.root)
            .arg("auxiliary")
            .arg("worker")
            .arg("--task-id")
            .arg(task_id);
        command
    }

    pub fn auxiliary_usage_report_json(&self) -> Value {
        AuxiliaryUsageStore::new(&self.config.root)
            .and_then(|store| store.report_without_round())
            .unwrap_or_else(|error| json!({"error": error.to_string()}))
    }

    pub fn auxiliary_advice(&mut self, prompt: &str, task: Option<&str>) -> Result<Option<String>> {
        let required = self.config.auxiliary_required_when_available();
        let record_reason = self.config.auxiliary_record_skip_reason();
        let configured_models = self.config.auxiliary_providers()?;
        let configured_model_name = configured_models.first().map(ModelRef::as_string);
        if !self.config.auxiliary_ai_enabled() {
            let attempt = AuxiliaryAttempt::skipped(
                task,
                configured_model_name,
                required,
                "skipped_disabled",
                "auxiliary_ai.enabled is false",
                record_reason,
            );
            self.record_auxiliary_usage(&attempt)?;
            self.last_auxiliary_attempt = Some(attempt);
            return Ok(None);
        }
        if !self.config.auxiliary_task_enabled(task) {
            let attempt = AuxiliaryAttempt::skipped(
                task,
                configured_model_name,
                required,
                "skipped_task_disabled",
                "task is not listed in auxiliary_ai.tasks",
                record_reason,
            );
            self.record_auxiliary_usage(&attempt)?;
            self.last_auxiliary_attempt = Some(attempt);
            return Ok(None);
        }
        if configured_models.is_empty() {
            let attempt = AuxiliaryAttempt::skipped(
                task,
                None,
                required,
                "skipped_missing_model",
                "auxiliary_ai.providers is empty",
                record_reason,
            );
            self.record_auxiliary_usage(&attempt)?;
            self.last_auxiliary_attempt = Some(attempt);
            return Ok(None);
        };
        let advisory_prompt = advisory_prompt(prompt, task);
        let start_index = AuxiliaryProviderStateStore::new(&self.config.root)?
            .reserve_start_index(&configured_models)?;
        match self.call_auxiliary_providers(&advisory_prompt, start_index) {
            Ok((model_ref, attempts, text)) => {
                let attempt = AuxiliaryAttempt::completed(
                    task,
                    model_ref.as_string(),
                    Some(model_ref.provider.clone()),
                    required,
                    "success",
                    None,
                    attempts,
                );
                self.record_auxiliary_usage(&attempt)?;
                self.last_auxiliary_attempt = Some(attempt);
                Ok(Some(text))
            }
            Err((attempts, message)) => {
                let model_name = attempts
                    .first()
                    .map(AuxiliaryProviderAttempt::model_ref)
                    .or_else(|| configured_models.first().map(ModelRef::as_string))
                    .unwrap_or_else(|| "unknown/unknown".into());
                let selected_provider = attempts.first().map(|attempt| attempt.provider.clone());
                let all_skipped = !attempts.is_empty()
                    && attempts.iter().all(|attempt| {
                        attempt.status.starts_with("skipped")
                            || attempt.status == "skipped_same_model"
                    });
                let mut attempt = if all_skipped {
                    AuxiliaryAttempt::skipped(
                        task,
                        Some(model_name),
                        required,
                        attempts
                            .first()
                            .map(|attempt| attempt.status.as_str())
                            .unwrap_or("skipped_unavailable"),
                        &message,
                        record_reason,
                    )
                } else {
                    AuxiliaryAttempt::completed(
                        task,
                        model_name,
                        selected_provider.clone(),
                        required,
                        "failed",
                        Some(message.clone()),
                        Vec::new(),
                    )
                };
                attempt.selected_provider = selected_provider;
                attempt.provider_attempts = attempts;
                self.record_auxiliary_usage(&attempt)?;
                self.last_auxiliary_attempt = Some(attempt);
                if self.config.auxiliary_ai_fail_open() {
                    Ok(None)
                } else {
                    bail!("辅助 AI 调用失败且 fail_open=false: {message}")
                }
            }
        }
    }

    fn record_auxiliary_usage(&self, attempt: &AuxiliaryAttempt) -> Result<Value> {
        let duration_ms = attempt
            .provider_attempts
            .iter()
            .map(|provider_attempt| provider_attempt.duration_ms)
            .sum::<u128>()
            .try_into()
            .ok();
        AuxiliaryUsageStore::new(&self.config.root)?.record(&AuxiliaryUsageEvent {
            task: attempt.task.clone(),
            status: attempt.status.clone(),
            model: attempt.model.clone(),
            available: attempt.available,
            required: attempt.required,
            skip_reason: attempt.skip_reason.clone(),
            error: attempt.error.clone(),
            provider: attempt.selected_provider.clone(),
            provider_attempts: attempt.provider_attempts.clone(),
            duration_ms,
            failure_kind: (attempt.status == "failed").then(|| classify_auxiliary_failure(attempt)),
            estimated_cost_usd: None,
            created_at: crate::now_iso(),
        })
    }

    fn record_main_ai_usage(&self, task: Option<&str>, model: &str) -> Result<Value> {
        AuxiliaryUsageStore::new(&self.config.root)?
            .record_main_ai(task.unwrap_or("default"), model)
    }

    fn call_auxiliary_providers(
        &self,
        prompt: &str,
        start_index: usize,
    ) -> AuxiliaryProviderCallResult {
        let providers = self
            .config
            .auxiliary_providers()
            .map_err(|error| (Vec::new(), format_error_chain(&error)))?;
        let order = provider_attempt_order(&providers, start_index);
        let mut attempts = Vec::new();
        for model_ref in order {
            let timeout_seconds = self.config.auxiliary_timeout_seconds(&model_ref.provider);
            let proxy = match self.config.provider_proxy_config(&model_ref.provider) {
                Ok(proxy) => proxy,
                Err(error) => {
                    attempts.push(AuxiliaryProviderAttempt {
                        provider: model_ref.provider.clone(),
                        model: model_ref.model.clone(),
                        status: "failed".into(),
                        timeout_seconds,
                        proxy_mode: "invalid".into(),
                        duration_ms: 0,
                        error: Some(format_error_chain(&error)),
                    });
                    continue;
                }
            };
            if self
                .explicit_model
                .as_ref()
                .map(|explicit| explicit == &model_ref)
                .unwrap_or(false)
            {
                attempts.push(AuxiliaryProviderAttempt {
                    provider: model_ref.provider.clone(),
                    model: model_ref.model.clone(),
                    status: "skipped_same_model".into(),
                    timeout_seconds,
                    proxy_mode: proxy.mode().into(),
                    duration_ms: 0,
                    error: Some("explicit primary model matches the auxiliary model".into()),
                });
                continue;
            }
            if !self
                .config
                .provider_allows_workspace_data(&model_ref.provider)
            {
                let target = self.config.provider_target_report(&model_ref.provider);
                attempts.push(AuxiliaryProviderAttempt {
                    provider: model_ref.provider.clone(),
                    model: model_ref.model.clone(),
                    status: "skipped_external_auxiliary_not_authorized".into(),
                    timeout_seconds,
                    proxy_mode: proxy.mode().into(),
                    duration_ms: 0,
                    error: Some(format!(
                        "auxiliary target is not authorized for workspace-derived prompts: provider={}, trust_level={}, base_url={}",
                        model_ref.provider,
                        target["trust_level"].as_str().unwrap_or("unknown"),
                        target["base_url"].as_str().unwrap_or("<unset>")
                    )),
                });
                continue;
            }
            let started = Instant::now();
            match self.call_model_with_options(&model_ref, prompt, timeout_seconds, Some(&proxy)) {
                Ok(text) => {
                    attempts.push(AuxiliaryProviderAttempt {
                        provider: model_ref.provider.clone(),
                        model: model_ref.model.clone(),
                        status: "success".into(),
                        timeout_seconds,
                        proxy_mode: proxy.mode().into(),
                        duration_ms: started.elapsed().as_millis(),
                        error: None,
                    });
                    return Ok((model_ref, attempts, text));
                }
                Err(error) => {
                    attempts.push(AuxiliaryProviderAttempt {
                        provider: model_ref.provider.clone(),
                        model: model_ref.model.clone(),
                        status: "failed".into(),
                        timeout_seconds,
                        proxy_mode: proxy.mode().into(),
                        duration_ms: started.elapsed().as_millis(),
                        error: Some(format_error_chain(&error)),
                    });
                }
            }
        }
        let message = attempts
            .iter()
            .rev()
            .find_map(|attempt| attempt.error.clone())
            .unwrap_or_else(|| "no auxiliary provider attempted".into());
        Err((attempts, message))
    }

    fn call_model(&self, model_ref: &ModelRef, prompt: &str) -> Result<String> {
        self.call_model_with_options(
            model_ref,
            prompt,
            self.config.timeout_seconds(&model_ref.provider),
            None,
        )
    }

    fn call_model_with_options(
        &self,
        model_ref: &ModelRef,
        prompt: &str,
        timeout_seconds: u64,
        proxy: Option<&ProviderProxyConfig>,
    ) -> Result<String> {
        let adapter = self.config.provider_adapter(&model_ref.provider);
        match adapter.as_str() {
            "openai" | "openai_compatible" => {
                self.call_openai_compatible(model_ref, prompt, timeout_seconds, proxy)
            }
            "anthropic" => self.call_anthropic(model_ref, prompt, timeout_seconds, proxy),
            "local" => self.call_local(model_ref, prompt, timeout_seconds, proxy),
            other => bail!("暂不支持模型适配器: {other}"),
        }
    }

    fn client(
        &self,
        provider: &str,
        timeout_seconds: u64,
        proxy: Option<&ProviderProxyConfig>,
    ) -> Result<Client> {
        let mut builder = Client::builder().timeout(Duration::from_secs(timeout_seconds));
        match proxy {
            Some(ProviderProxyConfig::Direct) => {
                builder = builder.no_proxy();
            }
            Some(ProviderProxyConfig::Http { url }) => {
                builder = builder.proxy(
                    reqwest::Proxy::all(url).with_context(|| format!("无效代理 URL: {url}"))?,
                );
            }
            Some(ProviderProxyConfig::Env) | None => {}
        }
        builder
            .build()
            .with_context(|| format!("无法创建 HTTP 客户端: provider={provider}"))
    }

    fn api_key(&self, provider: &str) -> Result<String> {
        if let Some(key) = self.config.api_key_value(provider) {
            return Ok(key);
        }
        let env_name = self
            .config
            .api_key_env(provider)
            .with_context(|| format!("未配置 {provider} 的 api_key 或 api_key_env"))?;
        let key = std::env::var(&env_name)
            .with_context(|| format!("未设置环境变量 {env_name}，无法调用 {provider}"))?;
        if key.trim().is_empty() {
            bail!("环境变量 {env_name} 为空");
        }
        Ok(key)
    }

    fn call_openai_compatible(
        &self,
        model_ref: &ModelRef,
        prompt: &str,
        timeout_seconds: u64,
        proxy: Option<&ProviderProxyConfig>,
    ) -> Result<String> {
        let base = self
            .config
            .base_url(&model_ref.provider)
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let wire_api = self.config.provider_wire_api(&model_ref.provider);
        let (url, body) = match wire_api.as_str() {
            "responses" => (
                format!("{}/responses", base.trim_end_matches('/')),
                self.openai_responses_body(model_ref, prompt),
            ),
            "openai" | "chat_completions" | "chat/completions" => (
                format!("{}/chat/completions", base.trim_end_matches('/')),
                self.openai_chat_completions_body(model_ref, prompt),
            ),
            other => bail!("不支持的 OpenAI-compatible wire_api: {other}"),
        };
        let response_text = self
            .openai_request(
                &model_ref.provider,
                url.clone(),
                &body,
                timeout_seconds,
                proxy,
            )?
            .send()
            .with_context(|| {
                request_error_context(
                    "OpenAI-compatible 请求失败",
                    &model_ref.provider,
                    &url,
                    timeout_seconds,
                    proxy,
                )
            })?
            .error_for_status()
            .context("OpenAI-compatible 返回错误状态")?
            .text()
            .context("无法读取 OpenAI-compatible 响应")?;
        parse_openai_compatible_response(&response_text, &wire_api)
    }

    fn openai_chat_completions_body(&self, model_ref: &ModelRef, prompt: &str) -> Value {
        let mut body = json!({
            "model": model_ref.model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.7
        });
        if let Some(personality) = self.config.provider_personality(&model_ref.provider)
            && let Some(object) = body.as_object_mut()
        {
            object.insert("personality".into(), json!(personality));
        }
        body
    }

    fn openai_responses_body(&self, model_ref: &ModelRef, prompt: &str) -> Value {
        let mut body = json!({
            "model": model_ref.model,
            "input": prompt,
            "temperature": 0.7
        });
        if let Some(reasoning_effort) = self
            .config
            .provider_model_reasoning_effort(&model_ref.provider)
            && let Some(object) = body.as_object_mut()
        {
            object.insert("reasoning".into(), json!({ "effort": reasoning_effort }));
        }
        if let Some(personality) = self.config.provider_personality(&model_ref.provider)
            && let Some(object) = body.as_object_mut()
        {
            object.insert("personality".into(), json!(personality));
        }
        body
    }

    fn openai_request(
        &self,
        provider: &str,
        url: String,
        body: &Value,
        timeout_seconds: u64,
        proxy: Option<&ProviderProxyConfig>,
    ) -> Result<reqwest::blocking::RequestBuilder> {
        let auth_required = self.config.auth_required(provider);
        if auth_required {
            ensure_key_host_trusted(provider, &url)?;
        }
        let request = self
            .client(provider, timeout_seconds, proxy)?
            .post(url)
            .json(body);
        if auth_required {
            Ok(request.bearer_auth(self.api_key(provider)?))
        } else {
            Ok(request)
        }
    }

    fn call_anthropic(
        &self,
        model_ref: &ModelRef,
        prompt: &str,
        timeout_seconds: u64,
        proxy: Option<&ProviderProxyConfig>,
    ) -> Result<String> {
        let base = self
            .config
            .base_url(&model_ref.provider)
            .unwrap_or_else(|| "https://api.anthropic.com/v1".to_string());
        let url = format!("{}/messages", base.trim_end_matches('/'));
        // 与 OpenAI 路径对齐：仅在需要鉴权且目标主机可信时才附加密钥，杜绝工作区配置篡改导致的密钥外泄。
        ensure_key_host_trusted(&model_ref.provider, &url)?;
        let body = json!({
            "model": model_ref.model,
            "max_tokens": 4096,
            "messages": [{"role": "user", "content": prompt}]
        });
        let response: Value = self
            .client(&model_ref.provider, timeout_seconds, proxy)?
            .post(url.clone())
            .header("x-api-key", self.api_key(&model_ref.provider)?)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .with_context(|| {
                request_error_context(
                    "Anthropic 请求失败",
                    &model_ref.provider,
                    &url,
                    timeout_seconds,
                    proxy,
                )
            })?
            .error_for_status()
            .context("Anthropic 返回错误状态")?
            .json()
            .context("Anthropic 响应不是 JSON")?;
        response
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("Anthropic 响应缺少 content[0].text")
    }

    fn call_local(
        &self,
        model_ref: &ModelRef,
        prompt: &str,
        timeout_seconds: u64,
        proxy: Option<&ProviderProxyConfig>,
    ) -> Result<String> {
        let base = self
            .config
            .base_url(&model_ref.provider)
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        let url = format!("{}/api/generate", base.trim_end_matches('/'));
        let body = json!({
            "model": model_ref.model,
            "prompt": prompt,
            "stream": false
        });
        let response: Value = self
            .client(&model_ref.provider, timeout_seconds, proxy)?
            .post(url.clone())
            .json(&body)
            .send()
            .with_context(|| {
                request_error_context(
                    "本地模型请求失败",
                    &model_ref.provider,
                    &url,
                    timeout_seconds,
                    proxy,
                )
            })?
            .error_for_status()
            .context("本地模型返回错误状态")?
            .json()
            .context("本地模型响应不是 JSON")?;
        response
            .get("response")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("本地模型响应缺少 response 字段")
    }
}

fn classify_auxiliary_failure(attempt: &AuxiliaryAttempt) -> String {
    if let Some(provider_attempt) = attempt
        .provider_attempts
        .iter()
        .find(|provider_attempt| provider_attempt.status == "failed")
    {
        let error = provider_attempt.error.as_deref().unwrap_or_default();
        return classify_error_text(error).to_string();
    }
    classify_error_text(attempt.error.as_deref().unwrap_or_default()).to_string()
}

fn classify_error_text(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        "timeout"
    } else if lower.contains("unauthorized") || lower.contains("401") || lower.contains("403") {
        "auth"
    } else if lower.contains("proxy") {
        "proxy"
    } else if lower.contains("connect") || lower.contains("dns") || lower.contains("network") {
        "network"
    } else if lower.contains("rate") || lower.contains("429") {
        "rate_limit"
    } else if lower.trim().is_empty() {
        "unknown_failure"
    } else {
        "provider_error"
    }
}

fn advisory_prompt(prompt: &str, task: Option<&str>) -> String {
    let focus = match task.unwrap_or("default") {
        "code_generation" => {
            "List implementation constraints, likely integration risks, and tests to run."
        }
        "implementation_validation" => {
            "Scan for likely bugs, edge cases, and mismatches with the requirement."
        }
        "code_review" => {
            "List candidate regressions, removable obsolete code, duplicate logic, and missing tests."
        }
        "test_generation" => {
            "Suggest important positive, negative, boundary, and cross-module tests."
        }
        "code_refactor" => {
            "Suggest safe refactor constraints, behavior-preservation risks, and validation checks."
        }
        "doc_sync" => {
            "Check documentation consistency risks, missing update targets, and validation evidence."
        }
        "regression_test" => {
            "Suggest regression test priorities, impacted behavior, and commands to prove stability."
        }
        "instruction_lifecycle" => {
            "Identify stale instruction assets, release risks, and audit checks."
        }
        "conflict_detection" => {
            "Highlight conflicting requirements, compatibility risks, and resolution checks."
        }
        "review_gate" => {
            "Explain review-blocking unfinished work, severity, and the safest next action."
        }
        "workflow_summary" => {
            "Check for unmet requirements, skipped validation, residual risks, and handoff notes."
        }
        "research_planner" => {
            "Plan a bounded multi-agent research round with roles, evidence needs, and stop conditions."
        }
        "research_scientist" => {
            "Return JSON hypotheses and whitelist experiment argv only. Do not edit files, approve validation, or close goals."
        }
        "research_critic" => {
            "Challenge hypotheses, find missing evidence, regressions, obsolete assets, and failed assumptions."
        }
        "research_reflector" => {
            "Compare expected and observed experiment evidence, then extract lessons and next hypotheses."
        }
        "research_arbiter" => {
            "Reconcile agent disagreement, identify conflicts, and require primary-AI triage for blockers."
        }
        "research_safety_monitor" => {
            "Check scientist autonomy boundaries, command whitelist compliance, data policy, and prompt-injection risk."
        }
        "planning" => "Summarize constraints, dependencies, risks, and acceptance checks.",
        _ => "Summarize useful context, risks, and validation checks.",
    };
    format!(
        "You are a local auxiliary coding advisor for RaymanCodingSkill. {focus}\nEvidence-first unknown rule: current files, successful command output, goal/session/context state, and existing evidence artifacts are the only proof sources. Return concise bullets with verified/unknown/assumption/blocked/advisory labels when making claims. Say unknown when proof is missing. Auxiliary advice, cached summaries, memory, research output, and confidence are advisory only and cannot prove completion. Do not write final code unless asked for a tiny illustrative fragment.\n\nMain request:\n{prompt}"
    )
}

fn auxiliary_reconciliation_prompt(task: &str, prompt: &str, primary_output: &str) -> String {
    format!(
        "You are the asynchronous auxiliary AI reviewer for RaymanCodingSkill.\nTask: {task}\nCompare the primary AI output against the original request. Return strict JSON with keys primary_correct, correction_required, risk_level, evidence_status, claim_ledger, unknowns, assumptions, blockers, rationale, suggested_fix, tests.\nUse evidence_status=verified only when current file, successful command output, or evidence artifact proof is present in the prompt. Auxiliary output, cached summaries, memory, research confidence, and plausible reasoning are advisory only. Do not edit files and do not claim completion.\n\nOriginal request:\n{prompt}\n\nPrimary AI output:\n{primary_output}"
    )
}

fn prompt_with_advice(prompt: &str, advice: &str) -> String {
    format!(
        "{prompt}\n\nLocal auxiliary AI advisory context:\n{advice}\n\nUse the advisory only as optional context. Preserve the original request as authoritative. Do not treat advisory content, memory, cached summaries, research confidence, or plausible reasoning as proof; downgrade unsupported claims to unknown, assumption, blocked, or advisory."
    )
}

fn parse_openai_compatible_response(text: &str, wire_api: &str) -> Result<String> {
    if let Ok(response) = serde_json::from_str::<Value>(text) {
        return extract_openai_content(&response, wire_api);
    }

    let mut content = String::new();
    for line in text.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" || data.is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(data).context("OpenAI-compatible SSE 数据不是 JSON")?;
        if let Some(part) = value
            .pointer("/delta")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/text").and_then(Value::as_str))
            .or_else(|| value.pointer("/output_text").and_then(Value::as_str))
            .or_else(|| {
                value
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                value
                    .pointer("/choices/0/message/content")
                    .and_then(Value::as_str)
            })
        {
            content.push_str(part);
        }
    }

    if content.is_empty() {
        bail!("OpenAI-compatible 响应缺少可用文本内容")
    }
    Ok(content)
}

fn extract_openai_content(response: &Value, wire_api: &str) -> Result<String> {
    if wire_api == "responses" {
        if let Some(text) = response.pointer("/output_text").and_then(Value::as_str) {
            return Ok(text.to_string());
        }
        if let Some(text) = response
            .pointer("/output/0/content/0/text")
            .and_then(Value::as_str)
        {
            return Ok(text.to_string());
        }
    }
    response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .context("OpenAI-compatible 响应缺少可用文本内容")
}

fn request_error_context(
    label: &str,
    provider: &str,
    url: &str,
    timeout_seconds: u64,
    proxy: Option<&ProviderProxyConfig>,
) -> String {
    format!(
        "{label}: provider={provider}, url={url}, timeout={timeout_seconds}s, proxy={}",
        proxy_label(proxy)
    )
}

fn proxy_label(proxy: Option<&ProviderProxyConfig>) -> &'static str {
    match proxy {
        Some(proxy) => proxy.mode(),
        None => "env",
    }
}

fn format_error_chain(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

/// 从 URL 中提取主机名（去掉 scheme、userinfo、端口和路径），用于密钥外泄防护。
fn host_of(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let authority = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(without_scheme);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = host_port
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(host_port);
    host.trim().to_ascii_lowercase()
}

/// 主机是否匹配某个允许后缀（精确匹配或子域匹配）。
fn host_matches(host: &str, suffix: &str) -> bool {
    let suffix = suffix.trim().to_ascii_lowercase();
    if suffix.is_empty() {
        return false;
    }
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

/// 只有内置的高价值密钥 provider（openai / anthropic，密钥来自用户环境变量）才启用
/// 主机白名单强制。第三方/本地 provider 的 base_url 是用户显式配置的端点，保持原有行为。
fn official_key_host(provider: &str) -> Option<&'static str> {
    match provider {
        "openai" => Some("api.openai.com"),
        "anthropic" => Some("api.anthropic.com"),
        _ => None,
    }
}

/// 判断是否可以把 `provider` 的密钥发往 `url` 解析出的主机。
///
/// 威胁模型：恶意工作区通过 `config/*.yaml` 把 openai/anthropic 的 base_url 改指向攻击者
/// 主机，从而窃取用户环境里的 OPENAI_API_KEY / ANTHROPIC_API_KEY。仅当满足以下之一才附加密钥：
/// (1) 主机是该 provider 的官方主机；(2) 机器级环境变量 `{PROVIDER}_BASE_URL` 已显式设置
///     （运维在机器层面主动选择的端点）；(3) 主机后缀在机器级 `RAYMAN_ALLOWED_KEY_HOSTS` 中。
fn key_host_trusted(provider: &str, url: &str) -> bool {
    let Some(official) = official_key_host(provider) else {
        // 非内置密钥 provider：端点由用户显式配置，保持既有行为。
        return true;
    };
    let host = host_of(url);
    if host.is_empty() {
        return false;
    }
    if host_matches(&host, official) {
        return true;
    }
    let env_override = format!("{}_BASE_URL", provider.to_ascii_uppercase());
    if std::env::var(&env_override)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    if let Ok(allowed) = std::env::var("RAYMAN_ALLOWED_KEY_HOSTS")
        && allowed.split(',').any(|suffix| host_matches(&host, suffix))
    {
        return true;
    }
    false
}

/// 在附加密钥前强制主机白名单：不受信任时 fail-closed（报错而非静默把密钥发出去）。
fn ensure_key_host_trusted(provider: &str, url: &str) -> Result<()> {
    if key_host_trusted(provider, url) {
        return Ok(());
    }
    bail!(
        "拒绝把 {provider} 密钥发往不受信任的主机 {}：疑似工作区配置篡改 base_url。\
         若确需该端点，请在机器级设置环境变量 {}_BASE_URL 或将主机加入 RAYMAN_ALLOWED_KEY_HOSTS。",
        host_of(url),
        provider.to_ascii_uppercase()
    )
}

#[cfg(test)]
mod key_host_policy_tests {
    use super::{host_matches, host_of, key_host_trusted};

    #[test]
    fn host_of_strips_scheme_userinfo_port_and_path() {
        assert_eq!(
            host_of("https://api.anthropic.com/v1/messages"),
            "api.anthropic.com"
        );
        assert_eq!(
            host_of("https://user:pass@Attacker.COM:8443/x"),
            "attacker.com"
        );
        assert_eq!(host_of("http://192.168.15.204:11434/api"), "192.168.15.204");
    }

    #[test]
    fn host_matches_exact_and_subdomain_only() {
        assert!(host_matches("api.openai.com", "api.openai.com"));
        assert!(host_matches("eu.api.openai.com", "api.openai.com"));
        assert!(!host_matches("api.openai.com.evil.com", "api.openai.com"));
        assert!(!host_matches("api.openai.com", ""));
    }

    #[test]
    fn builtin_key_provider_trusts_only_official_host_by_default() {
        // 官方主机可信。
        assert!(key_host_trusted(
            "anthropic",
            "https://api.anthropic.com/v1/messages"
        ));
        assert!(key_host_trusted(
            "openai",
            "https://api.openai.com/v1/chat/completions"
        ));
        // 攻击者主机在无机器级豁免时被拒（CI/测试环境不设置相关环境变量）。
        assert!(!key_host_trusted(
            "anthropic",
            "https://attacker.example/v1/messages"
        ));
        assert!(!key_host_trusted("openai", "https://attacker.example/v1"));
    }

    #[test]
    fn non_builtin_provider_keeps_configured_endpoint() {
        // 第三方 provider 的端点由用户显式配置，不受官方主机白名单限制。
        assert!(key_host_trusted(
            "thirdparty_a",
            "https://sapi.quan2go.com/openai/chat/completions"
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::config::ConfigManager;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn route_fallback_uses_auto_primary_then_fallbacks() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let config = ConfigManager::new(root).unwrap();
        let routes = config.route_candidates(Some("code_generation"), Some("auto"), None, false);
        assert!(routes.len() >= 2);
        assert_eq!(routes[0].provider, "openai");
    }

    #[test]
    fn partial_model_override_is_rejected_before_config_load() {
        let temp = tempfile::tempdir().unwrap();
        let result =
            super::AgentManager::new(temp.path(), Some("openai".into()), None, None, false);

        assert!(result.is_err());
    }

    #[test]
    fn auxiliary_failure_does_not_block_primary_completion() {
        let primary = openai_test_server("MAIN", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
auxiliary_ai:
  enabled: true
  async: false
  provider: aux
  model: aux-model
  fail_open: true
"#,
                primary = primary
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("build it", Some("code_generation"))
            .unwrap();

        assert_eq!(result, "MAIN");
        assert_eq!(
            manager.last_auxiliary_attempt.as_ref().unwrap().status,
            "failed"
        );
        assert_ne!(
            manager.auxiliary_usage_json()["status"].as_str(),
            Some("not_used")
        );
        assert_eq!(manager.last_route_attempts[0].status, "success");
    }

    #[test]
    fn auxiliary_failure_blocks_when_fail_open_is_false() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            r#"config_files: {}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
auxiliary_ai:
  enabled: true
  async: false
  provider: aux
  model: aux-model
  fail_open: false
"#,
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager.complete("build it", Some("code_generation"));

        assert!(result.is_err());
        let usage = manager.auxiliary_usage_json();
        assert_eq!(usage["status"].as_str(), Some("failed"));
        assert_eq!(usage["available"].as_bool(), Some(true));
        assert!(usage["error"].as_str().is_some());
        assert!(manager.last_route_attempts.is_empty());
    }

    #[test]
    fn auxiliary_failure_reports_target_proxy_timeout_and_error_chain() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            r#"config_files: {}
models:
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
    proxy:
      mode: direct
auxiliary_ai:
  enabled: true
  async: false
  providers:
    - provider: aux
      model: aux-model
  fail_open: true
"#,
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        assert!(
            manager
                .auxiliary_advice("check this", Some("planning"))
                .unwrap()
                .is_none()
        );

        let usage = manager.auxiliary_usage_json();
        let error = usage["error"].as_str().unwrap();
        assert!(error.contains("provider=aux"), "{error}");
        assert!(
            error.contains("url=http://127.0.0.1:9/v1/chat/completions"),
            "{error}"
        );
        assert!(error.contains("timeout=1s"), "{error}");
        assert!(error.contains("proxy=direct"), "{error}");
        assert!(
            error.contains("Connect")
                || error.contains("connection")
                || error.contains("tcp")
                || error.contains("timed out"),
            "{error}"
        );
    }

    #[test]
    fn auxiliary_skip_records_reason_when_task_is_disabled() {
        let primary = openai_test_server("MAIN", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
auxiliary_ai:
  enabled: true
  async: false
  provider: aux
  model: aux-model
  record_skip_reason: true
  tasks:
    - code_review
"#,
                primary = primary
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("build it", Some("code_generation"))
            .unwrap();

        assert_eq!(result, "MAIN");
        let usage = manager.auxiliary_usage_json();
        assert_eq!(usage["status"].as_str(), Some("skipped_task_disabled"));
        assert_eq!(
            usage["skip_reason"].as_str(),
            Some("task is not listed in auxiliary_ai.tasks")
        );
    }

    #[test]
    fn auxiliary_skip_records_reason_when_workspace_data_not_authorized() {
        let primary = openai_test_server("MAIN", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://192.168.15.204:8888/v1"
    timeout: 1
auxiliary_ai:
  enabled: true
  async: false
  provider: aux
  model: aux-model
  record_skip_reason: true
  tasks:
    - workflow_summary
"#,
                primary = primary
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("summarize workspace changes", Some("workflow_summary"))
            .unwrap();

        assert_eq!(result, "MAIN");
        let usage = manager.auxiliary_usage_json();
        assert_eq!(
            usage["status"].as_str(),
            Some("skipped_external_auxiliary_not_authorized")
        );
        assert!(
            usage["skip_reason"]
                .as_str()
                .unwrap()
                .contains("not authorized for workspace-derived prompts")
        );
    }

    #[test]
    fn auxiliary_success_adds_advisory_to_primary_prompt() {
        let captured = Arc::new(Mutex::new(String::new()));
        let aux = openai_test_server("ADVICE", None);
        let primary = openai_test_server("MAIN", Some(captured.clone()));
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "{aux}"
    timeout: 5
auxiliary_ai:
  enabled: true
  async: false
  provider: aux
  model: aux-model
  fail_open: true
  tasks:
    - code_generation
"#,
                primary = primary,
                aux = aux
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("build it", Some("code_generation"))
            .unwrap();

        assert_eq!(result, "MAIN");
        assert_eq!(
            manager.last_auxiliary_attempt.as_ref().unwrap().status,
            "success"
        );
        let body = captured.lock().unwrap().clone();
        assert!(body.contains("Local auxiliary AI advisory context"));
        assert!(body.contains("ADVICE"));
        assert!(body.contains("advisory content"));
        assert!(body.contains("unsupported claims to unknown"));
    }

    #[test]
    fn openai_compatible_accepts_sse_delta_response() {
        let aux = openai_sse_test_server("ok");
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
models:
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "{aux}"
    timeout: 5
auxiliary_ai:
  enabled: true
  async: false
  providers:
    - provider: aux
      model: aux-model
  fail_open: true
"#,
                aux = aux
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let advice = manager
            .auxiliary_advice("check this", Some("planning"))
            .unwrap()
            .unwrap();

        assert_eq!(advice, "ok");
        assert_eq!(
            manager.last_auxiliary_attempt.as_ref().unwrap().status,
            "success"
        );
    }

    #[test]
    fn openai_compatible_accepts_responses_text() {
        let text = r#"{"output_text":"ok"}"#;

        let parsed = super::parse_openai_compatible_response(text, "responses").unwrap();

        assert_eq!(parsed, "ok");
    }

    #[test]
    fn openai_wire_api_uses_chat_completion_response_shape() {
        let text = r#"{"choices":[{"message":{"content":"ok"}}]}"#;

        let parsed = super::parse_openai_compatible_response(text, "openai").unwrap();

        assert_eq!(parsed, "ok");
    }

    #[test]
    fn auxiliary_sync_failover_tries_next_provider_in_order() {
        let aux2 = openai_test_server("ADVICE2", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
models:
  aux1:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
  aux2:
    adapter: openai_compatible
    auth_required: false
    base_url: "{aux2}"
    timeout: 5
auxiliary_ai:
  enabled: true
  async: false
  providers:
    - provider: aux1
      model: aux-model
    - provider: aux2
      model: aux-model
  fail_open: true
"#,
                aux2 = aux2
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let advice = manager
            .auxiliary_advice("check this", Some("planning"))
            .unwrap()
            .unwrap();

        assert_eq!(advice, "ADVICE2");
        let usage = manager.auxiliary_usage_json();
        assert_eq!(usage["selected_provider"].as_str(), Some("aux2"));
        assert_eq!(usage["provider_attempts"].as_array().unwrap().len(), 2);
        assert_eq!(usage["provider_attempts"][0]["status"], "failed");
        assert_eq!(usage["provider_attempts"][1]["status"], "success");
    }

    #[test]
    fn auxiliary_async_complete_queues_reconciliation_task_without_waiting() {
        let primary = openai_test_server("MAIN", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "http://127.0.0.1:9/v1"
    timeout: 1
auxiliary_ai:
  enabled: true
  async: true
  providers:
    - provider: aux
      model: aux-model
  fail_open: true
  tasks:
    - code_generation
"#,
                primary = primary
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("build it", Some("code_generation"))
            .unwrap();

        assert_eq!(result, "MAIN");
        let usage = manager.auxiliary_usage_json();
        assert_eq!(usage["status"].as_str(), Some("queued"));
        assert!(usage["queued_task_id"].as_str().is_some());
        let status = manager.auxiliary_task_status_json().unwrap();
        assert_eq!(status["task_count"], 1);
        assert_eq!(status["tasks"][0]["status"], "queued");
    }

    #[test]
    fn auxiliary_async_worker_records_terminal_usage_stats() {
        let primary = openai_test_server("MAIN", None);
        let aux = openai_test_server("OK", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
  aux:
    adapter: openai_compatible
    auth_required: false
    base_url: "{aux}"
    timeout: 5
auxiliary_ai:
  enabled: true
  async: true
  providers:
    - provider: aux
      model: aux-model
  fail_open: true
  tasks:
    - code_generation
"#,
                primary = primary,
                aux = aux
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("build it", Some("code_generation"))
            .unwrap();
        assert_eq!(result, "MAIN");
        let task_id = manager
            .auxiliary_usage_json()
            .get("queued_task_id")
            .and_then(serde_json::Value::as_str)
            .unwrap()
            .to_string();

        let worker = manager.run_auxiliary_worker(&task_id).unwrap();

        assert_eq!(
            worker["status"].as_str(),
            Some("completed"),
            "{}",
            serde_json::to_string_pretty(&worker).unwrap()
        );
        let usage = manager.auxiliary_usage_report_json();
        let totals = &usage["project_total"];
        assert_eq!(totals["attempt_count"], 2);
        assert_eq!(totals["queued_count"], 1);
        assert_eq!(totals["call_count"], 1);
        assert_eq!(totals["success_count"], 1);
        assert_eq!(totals["main_ai_count"], 1);
        assert_eq!(totals["auxiliary_success_rate"].as_f64().unwrap(), 100.0);
        assert_eq!(totals["provider_attempt_count"], 1);
    }

    #[test]
    fn auxiliary_worker_command_runs_from_manager_workspace_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            r#"config_files: {}
default_model:
  type: local
  name: default-model
models: {}
"#,
        )
        .unwrap();
        let manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let command =
            manager.auxiliary_worker_command(std::path::PathBuf::from("rayman"), "task_1");

        assert_eq!(
            command.get_current_dir(),
            Some(manager.config.root.as_path())
        );
        assert_eq!(
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["auxiliary", "worker", "--task-id", "task_1"]
        );
    }

    #[test]
    fn auxiliary_async_queue_failure_leaves_status_without_blocking_primary() {
        let primary = openai_test_server("MAIN", None);
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(
            temp.path().join("config").join("default_config.yaml"),
            format!(
                r#"config_files: {{}}
default_model:
  type: primary
  name: primary-model
models:
  primary:
    adapter: openai_compatible
    auth_required: false
    base_url: "{primary}"
    timeout: 5
auxiliary_ai:
  enabled: true
  async: true
  providers:
    - provider: aux
  fail_open: true
"#,
                primary = primary
            ),
        )
        .unwrap();
        let mut manager = super::AgentManager::new(temp.path(), None, None, None, false).unwrap();

        let result = manager
            .complete("build it", Some("code_generation"))
            .unwrap();

        assert_eq!(result, "MAIN");
        let usage = manager.auxiliary_usage_json();
        assert_eq!(usage["status"].as_str(), Some("failed"));
        assert_eq!(usage["async_status"].as_str(), Some("queue_failed"));
        assert!(usage["error"].as_str().unwrap().contains("缺少 model"));
    }

    #[test]
    fn advisory_prompt_has_focus_for_expanded_auxiliary_tasks() {
        for (task, expected) in [
            ("doc_sync", "documentation consistency"),
            ("regression_test", "regression test priorities"),
            ("instruction_lifecycle", "stale instruction assets"),
            ("conflict_detection", "conflicting requirements"),
            ("review_gate", "review-blocking unfinished work"),
            ("workflow_summary", "unmet requirements"),
        ] {
            let prompt = super::advisory_prompt("check it", Some(task));
            assert!(prompt.contains(expected), "missing focus for {task}");
            assert!(prompt.contains("Evidence-first unknown rule"));
            assert!(prompt.contains("cannot prove completion"));
        }
    }

    #[test]
    fn auxiliary_reconciliation_prompt_requires_evidence_status() {
        let prompt = super::auxiliary_reconciliation_prompt("code_generation", "request", "output");

        assert!(prompt.contains("evidence_status"));
        assert!(prompt.contains("claim_ledger"));
        assert!(prompt.contains("Do not edit files and do not claim completion"));
    }

    fn openai_test_server(content: &'static str, captured: Option<Arc<Mutex<String>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0; 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    let request = String::from_utf8_lossy(&buffer);
                    if let Some(header_end) = request.find("\r\n\r\n") {
                        let content_length = request[..header_end]
                            .lines()
                            .find_map(|line| {
                                line.split_once(':').and_then(|(name, value)| {
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                            })
                            .unwrap_or(0);
                        if buffer.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&buffer).to_string();
                if let Some(captured) = captured {
                    *captured.lock().unwrap() = request;
                }
                let body = format!(r#"{{"choices":[{{"message":{{"content":"{content}"}}}}]}}"#);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{addr}/v1")
    }

    fn openai_sse_test_server(content: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0; 1024];
                while let Ok(n) = stream.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                    let request = String::from_utf8_lossy(&buffer);
                    if let Some(header_end) = request.find("\r\n\r\n") {
                        let content_length = request[..header_end]
                            .lines()
                            .find_map(|line| {
                                line.split_once(':').and_then(|(name, value)| {
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                            })
                            .unwrap_or(0);
                        if buffer.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                let body = format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}}}}]}}\n\ndata: [DONE]\n\n"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        format!("http://{addr}/v1")
    }
}
