//! Pure transitions for the protected store. The worker must persist the new
//! ledger atomically while holding the registered project transaction lock.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    pub revision: u64,
    pub tasks: BTreeMap<String, TaskState>,
    pub requests: BTreeMap<String, RecordedRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskState {
    pub title: String,
    pub revision: u64,
    pub stage: Option<String>,
    pub message: Option<String>,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedRequest {
    pub request_sha256: String,
    pub registration_sha256: String,
    pub project_id: String,
    pub worktree_id: String,
    pub result: RecordedResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordedResult {
    Storage { details: serde_json::Value },
    StateApplied { task_revision: u64 },
    RecoveryRequired,
    EffectSucceeded { outcome_sha256: String },
    EffectFailed { error_code: String },
}

#[derive(Debug, Clone)]
pub struct Transition {
    pub next: Ledger,
    pub result: RecordedResult,
    /// Only true after a new effect intent must be durably published. An
    /// existing recovery-required request must never execute a second time.
    pub execute_effect: bool,
    pub replayed: bool,
}

impl Ledger {
    pub fn accept(&self, q: &Request, r: &Registration, now: i64) -> Result<Transition> {
        validate_request_structure(q, r)?;
        let digest = q.digest()?;
        if let Some(old) = self.requests.get(&q.request_id) {
            if old.request_sha256 != digest
                || old.registration_sha256 != r.digest()?
                || old.project_id != q.project_id
                || old.worktree_id != q.worktree_id
            {
                bail!("request ID was already used with different content or registration");
            }
            return Ok(Transition {
                next: self.clone(),
                result: old.result.clone(),
                execute_effect: false,
                replayed: true,
            });
        }
        validate_request(q, r, now)?;
        if self.requests.len() >= 100_000 || self.tasks.len() >= 10_000 {
            bail!("global state capacity reached; explicit archival is required");
        }
        let mut next = self.clone();
        next.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("ledger revision overflow"))?;
        let result = match &q.operation {
            Operation::State {
                task_id,
                expected_revision,
                mutation,
            } => {
                // Namespace belongs to the registration, not just the task ID.
                let key = format!("{}:{}:{}", r.project_id, r.worktree_id, task_id);
                let old = self.tasks.get(&key);
                match mutation {
                    StateMutation::Begin { title } => {
                        if old.is_some() {
                            bail!("task already exists");
                        }
                        next.tasks.insert(
                            key,
                            TaskState {
                                title: title.clone(),
                                revision: 1,
                                stage: None,
                                message: None,
                                source_sha256: q.source_sha256.clone(),
                            },
                        );
                        RecordedResult::StateApplied { task_revision: 1 }
                    }
                    StateMutation::Progress { stage, message } => {
                        let old = old.ok_or_else(|| anyhow::anyhow!("task does not exist"))?;
                        if old.revision != *expected_revision {
                            bail!("state compare-and-swap conflict");
                        }
                        let revision = old
                            .revision
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("task revision overflow"))?;
                        next.tasks.insert(
                            key,
                            TaskState {
                                title: old.title.clone(),
                                revision,
                                stage: Some(stage.clone()),
                                message: Some(message.clone()),
                                source_sha256: q.source_sha256.clone(),
                            },
                        );
                        RecordedResult::StateApplied {
                            task_revision: revision,
                        }
                    }
                }
            }
            Operation::Storage { .. } => {
                bail!("application storage uses its own file transaction handler")
            }
            Operation::RecoverCommit { .. } => {
                bail!("commit recovery must use its original admitted intent");
            }
            Operation::Commit { .. } | Operation::Install { .. } => {
                RecordedResult::RecoveryRequired
            }
        };
        let execute_effect = matches!(result, RecordedResult::RecoveryRequired);
        next.requests.insert(
            q.request_id.clone(),
            RecordedRequest {
                request_sha256: digest,
                registration_sha256: r.digest()?,
                project_id: q.project_id.clone(),
                worktree_id: q.worktree_id.clone(),
                result: result.clone(),
            },
        );
        Ok(Transition {
            next,
            result,
            execute_effect,
            replayed: false,
        })
    }

    /// The protected executor supplies an observed terminal result after its
    /// own commit/install journal is terminal. This is not a client operation.
    pub fn record_effect_result(
        &self,
        id: &str,
        request_sha256: &str,
        outcome: RecordedResult,
    ) -> Result<Self> {
        let valid = match &outcome {
            RecordedResult::EffectSucceeded { outcome_sha256 } => is_sha256(outcome_sha256),
            RecordedResult::EffectFailed { error_code } => is_label(error_code),
            _ => false,
        };
        if !valid {
            bail!("invalid executor result");
        }
        let current = self
            .requests
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("effect has no durable intent"))?;
        if current.request_sha256 != request_sha256 {
            bail!("effect request digest changed");
        }
        if current.result == outcome {
            return Ok(self.clone());
        }
        if current.result != RecordedResult::RecoveryRequired {
            bail!("terminal effect result cannot be changed");
        }
        let mut next = self.clone();
        next.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("ledger revision overflow"))?;
        next.requests.get_mut(id).expect("checked above").result = outcome;
        Ok(next)
    }
}

/// Reuse requires the entire execution context, not just unchanged source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageBinding {
    pub registration_sha256: String,
    pub source_sha256: String,
    pub stage: String,
    pub scope_sha256: String,
    pub policy_sha256: String,
    pub environment_sha256: String,
    pub toolchain_sha256: String,
    pub artifacts: BTreeMap<String, String>,
}

impl StageBinding {
    pub fn digest(&self) -> Result<String> {
        if !is_label(&self.stage)
            || [
                &self.registration_sha256,
                &self.source_sha256,
                &self.scope_sha256,
                &self.policy_sha256,
                &self.environment_sha256,
                &self.toolchain_sha256,
            ]
            .iter()
            .any(|v| !is_sha256(v))
            || self
                .artifacts
                .iter()
                .any(|(k, v)| !is_label(k) || !is_sha256(v))
        {
            bail!("invalid stage binding");
        }
        Ok(crate::hash::sha256_bytes(&serde_json::to_vec(self)?))
    }
}
