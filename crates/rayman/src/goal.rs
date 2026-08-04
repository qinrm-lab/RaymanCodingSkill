//! 最小目标契约 + 自主推进边界。
//!
//! 除了 success evidence gate，这里还保留三类直接影响交付真实性的状态：可单调
//! 扩展但不能事后补票的计划、可重复且不改变工作区的 authority receipt，以及能
//! 区分 agent/human/external owner 的结构化 blocker。它们都是本地确定性合同，
//! 不在 CLI 内恢复旧版 LLM/runtime 编排。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::file_io::{read_json, write_json};
use crate::state_lock::acquire_state_lock;
#[cfg(test)]
use crate::state_lock::is_state_lock_contention;
use crate::state_paths;

use crate::timefmt::now_iso;

const GOALS_DIR: &str = ".RaymanCodingSkill/goals";
#[cfg(test)]
const PENDING_PATH: &str = ".RaymanCodingSkill/pending.json";
const GOALS_RELATIVE: &str = "goals";
pub const GOAL_SCHEMA_VERSION: u32 = 2;
const STRICT_RECEIPT_ROLLOUT_AT: &str = "2026-07-14T00:00:00Z";
const PRE_RECEIPT_MIGRATION: &str = "pre_receipt_schema_v2";
const RECEIPT_POLICY_V1: &str = "receipt_integrity_v1";
const RECEIPT_POLICY_V2: &str = "receipt_integrity_v2";
const VERIFIED_REPLACEMENT_TRANSFER_POLICY: &str = "verified_replacement_transfer_v1";
const RECEIPT_POLICY_QUARANTINED: &str = "untrusted_legacy_history_v1";
const RECEIPT_POLICY_INTEGRITY_QUARANTINED: &str = "receipt_integrity_quarantined";
const RECEIPT_POLICY_V2_ROLLOUT_AT: &str = "2026-07-18T04:34:13Z";
const RECEIPT_POLICY_V1_MIGRATION: &str = "pre_receipt_policy_v2";
const QUARANTINED_HISTORY_MIGRATION: &str = "invalid_legacy_receipts_quarantined";
const INTEGRITY_QUARANTINE_MIGRATION: &str = "invalid_archived_success_quarantined_v1";

mod long_task;
pub use long_task::*;

mod model;
pub use model::*;

mod handoff;
mod lifecycle;
mod pending;
mod rebind;
mod validation;

pub use handoff::*;
pub use lifecycle::*;
pub use pending::*;
pub use rebind::*;
pub use validation::*;

fn legacy_must_kind() -> String {
    "must".into()
}

fn legacy_open_status() -> String {
    "open".into()
}

#[derive(Debug, Clone)]
pub struct GoalLoadIssue {
    pub path: String,
    pub error: String,
}

pub struct GoalStore {
    root: PathBuf,
}

fn short_id(prefix: &str, seed: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{prefix}_{}", &digest[..10])
}

fn normalize_path_list(paths: &mut Vec<String>) {
    for path in paths.iter_mut() {
        *path = path
            .trim()
            .trim_start_matches("./")
            .trim_start_matches(".\\")
            .replace('\\', "/");
    }
    paths.retain(|path| !path.is_empty());
    paths.sort();
    paths.dedup();
}

fn ordinary_workspace_relative_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn hash_string_sequence(hasher: &mut Sha256, values: &[String]) {
    hasher.update((values.len() as u64).to_le_bytes());
    for value in values {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
}

pub fn plan_receipt_sha256(receipt: &PlanReceipt) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.goal-plan-receipt.v1");
    hasher.update(receipt.baseline_fingerprint.as_bytes());
    hasher.update(receipt.review_priority.as_bytes());
    hash_string_sequence(&mut hasher, &receipt.changed_paths);
    hash_string_sequence(&mut hasher, &receipt.impacted_paths);
    hash_string_sequence(&mut hasher, &receipt.recommended_checks);
    format!("{:x}", hasher.finalize())
}

pub fn plan_extension_sha256(
    baseline_fingerprint: &str,
    extension: &PlanExtensionReceipt,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.goal-plan-extension.v1");
    hasher.update(baseline_fingerprint.as_bytes());
    hasher.update(extension.previous_plan_sha256.as_bytes());
    hasher.update(extension.review_priority.as_bytes());
    hash_string_sequence(&mut hasher, &extension.changed_paths);
    hash_string_sequence(&mut hasher, &extension.impacted_paths);
    hash_string_sequence(&mut hasher, &extension.recommended_checks);
    format!("{:x}", hasher.finalize())
}

fn review_priority_rank(priority: &str) -> Option<u8> {
    match priority {
        "normal" => Some(0),
        "broad" => Some(1),
        "high" => Some(2),
        _ => None,
    }
}

fn max_review_priority(left: &str, right: &str) -> Result<String> {
    let left_rank = review_priority_rank(left)
        .ok_or_else(|| anyhow::anyhow!("未知 review_priority: {left}"))?;
    let right_rank = review_priority_rank(right)
        .ok_or_else(|| anyhow::anyhow!("未知 review_priority: {right}"))?;
    Ok(if left_rank >= right_rank { left } else { right }.to_string())
}

impl PlanReceipt {
    pub fn effective_changed_paths(&self) -> &[String] {
        self.extensions
            .last()
            .map(|extension| extension.changed_paths.as_slice())
            .unwrap_or(&self.changed_paths)
    }

    pub fn effective_impacted_paths(&self) -> &[String] {
        self.extensions
            .last()
            .map(|extension| extension.impacted_paths.as_slice())
            .unwrap_or(&self.impacted_paths)
    }

    pub fn effective_recommended_checks(&self) -> &[String] {
        self.extensions
            .last()
            .map(|extension| extension.recommended_checks.as_slice())
            .unwrap_or(&self.recommended_checks)
    }

    pub fn effective_review_priority(&self) -> &str {
        self.extensions
            .last()
            .map(|extension| extension.review_priority.as_str())
            .unwrap_or(&self.review_priority)
    }

    pub fn effective_plan_sha256(&self) -> &str {
        self.extensions
            .last()
            .map(|extension| extension.extension_sha256.as_str())
            .unwrap_or(&self.plan_sha256)
    }
}

pub fn plan_extensions_are_valid(receipt: &PlanReceipt) -> bool {
    let mut previous_sha256 = receipt.plan_sha256.clone();
    let mut previous_changed = receipt
        .changed_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut previous_impacted = receipt
        .impacted_paths
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut previous_checks = receipt
        .recommended_checks
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let Some(mut previous_priority) = review_priority_rank(&receipt.review_priority) else {
        return false;
    };
    for extension in &receipt.extensions {
        let mut changed = extension.changed_paths.clone();
        let mut impacted = extension.impacted_paths.clone();
        let mut checks = extension.recommended_checks.clone();
        normalize_path_list(&mut changed);
        normalize_path_list(&mut impacted);
        checks.sort();
        checks.dedup();
        let changed_set = changed.iter().cloned().collect::<BTreeSet<_>>();
        let impacted_set = impacted.iter().cloned().collect::<BTreeSet<_>>();
        let checks_set = checks.iter().cloned().collect::<BTreeSet<_>>();
        let Some(priority) = review_priority_rank(&extension.review_priority) else {
            return false;
        };
        if extension.previous_plan_sha256 != previous_sha256
            || changed != extension.changed_paths
            || impacted != extension.impacted_paths
            || checks != extension.recommended_checks
            || !changed_set.is_superset(&previous_changed)
            || changed_set == previous_changed
            || !impacted_set.is_superset(&previous_impacted)
            || !checks_set.is_superset(&previous_checks)
            || priority < previous_priority
            || extension.extension_sha256
                != plan_extension_sha256(&receipt.baseline_fingerprint, extension)
        {
            return false;
        }
        previous_sha256 = extension.extension_sha256.clone();
        previous_changed = changed_set;
        previous_impacted = impacted_set;
        previous_checks = checks_set;
        previous_priority = priority;
    }
    true
}

impl GoalStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// id 直接拼进文件名；拒绝分隔符等字符，防止 `--id ../../x` 越出 goals 目录读写。
    fn goal_path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            bail!("非法目标 id: {id}（只允许字母、数字、下划线和连字符）");
        }
        state_paths::managed_state_file(
            &self.root,
            &Path::new(GOALS_RELATIVE).join(format!("{id}.json")),
            false,
        )
    }

    /// 新建目标。`requirements` 为 (text, is_must) 列表。
    pub fn start(&self, title: &str, requirements: &[(String, bool)]) -> Result<Goal> {
        let requirements = requirements
            .iter()
            .map(|(text, is_must)| RequirementSpec {
                text: text.clone(),
                kind: if *is_must {
                    RequirementKind::Must
                } else {
                    RequirementKind::Should
                },
                proof_kind: None,
            })
            .collect::<Vec<_>>();
        self.start_with_specs(title, &requirements)
    }

    pub fn start_with_specs(&self, title: &str, requirements: &[RequirementSpec]) -> Result<Goal> {
        if title.trim().is_empty() {
            bail!("目标标题不能为空");
        }
        if !requirements.iter().any(|requirement| {
            requirement.kind == RequirementKind::Must && !requirement.text.trim().is_empty()
        }) {
            bail!("新目标至少需要一个非空 --must 需求");
        }
        // `any` alone let a second, blank requirement through, and
        // `current_schema_error` — which every gate re-runs on read — rejects
        // an empty requirement text. The store would report the goal created
        // while no reader would ever accept it.
        if requirements
            .iter()
            .any(|requirement| requirement.text.trim().is_empty())
        {
            bail!("goal 包含空的 requirement id 或文本");
        }
        let goals_dir =
            state_paths::managed_state_dir(&self.root, Path::new(GOALS_RELATIVE), true)?
                .ok_or_else(|| anyhow::anyhow!("无法创建目标状态目录"))?;
        let _lock = acquire_state_lock(&goals_dir.join(".store"))?;
        let now = now_iso();
        let id = short_id("goal", &format!("{title}{now}"));
        let requirements = requirements
            .iter()
            .enumerate()
            .map(|(index, requirement)| Requirement {
                id: format!("req_{}", index + 1),
                text: requirement.text.clone(),
                kind: requirement.kind,
                proof_kind: requirement.proof_kind,
                status: RequirementStatus::Open,
                evidence: None,
                validations: Vec::new(),
                impacts: Vec::new(),
            })
            .collect();
        let baseline = Some(workspace_baseline(&self.root)?);
        let goal = Goal {
            schema_version: GOAL_SCHEMA_VERSION,
            id: id.clone(),
            title: title.into(),
            status: GoalStatus::Active,
            lifecycle: GoalLifecycle::Current,
            lifecycle_reason: None,
            superseded_by: None,
            lifecycle_proof: None,
            replacement_authority: None,
            created_at: now.clone(),
            updated_at: now,
            baseline,
            plan_receipts: Vec::new(),
            review_receipts: Vec::new(),
            authority_receipts: Vec::new(),
            work_packages: Vec::new(),
            progress_receipts: Vec::new(),
            lanes: Vec::new(),
            handoff: None,
            requirements,
            loaded_from_legacy: false,
        };
        write_json(&self.goal_path(&id)?, &goal)?;
        Ok(goal)
    }

    pub fn record_plan(&self, id: &str, mut submission: PlanReceiptSubmission) -> Result<Goal> {
        if !matches!(
            submission.review_priority.as_str(),
            "normal" | "broad" | "high"
        ) {
            bail!("未知 review_priority: {}", submission.review_priority);
        }
        normalize_path_list(&mut submission.changed_paths);
        normalize_path_list(&mut submission.impacted_paths);
        submission.recommended_checks.sort();
        submission.recommended_checks.dedup();
        if submission.changed_paths.is_empty() {
            bail!("goal plan 至少需要一个变更路径");
        }

        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema() {
            bail!("目标 {id} 不是当前 schema，不能记录 plan receipt");
        }
        if goal.lifecycle != GoalLifecycle::Current || goal.status != GoalStatus::Active {
            bail!("只有 active/current 目标可以记录 plan receipt");
        }
        let Some(baseline) = goal.baseline.as_ref() else {
            bail!("目标缺少开工 baseline；请新建目标后在首次修改前执行 goal plan");
        };
        let current = workspace_baseline(&self.root)?;
        if current.workspace_fingerprint != baseline.workspace_fingerprint {
            bail!(
                "工作区已偏离 goal 开工 baseline；拒绝事后补 plan。baseline={} current={}",
                baseline.workspace_fingerprint,
                current.workspace_fingerprint
            );
        }
        let mut receipt = PlanReceipt {
            recorded_at: now_iso(),
            baseline_fingerprint: baseline.workspace_fingerprint.clone(),
            changed_paths: submission.changed_paths,
            review_priority: submission.review_priority,
            impacted_paths: submission.impacted_paths,
            recommended_checks: submission.recommended_checks,
            plan_sha256: String::new(),
            extensions: Vec::new(),
        };
        receipt.plan_sha256 = plan_receipt_sha256(&receipt);
        if let Some(existing) = goal.plan_receipts.first() {
            if goal.plan_receipts.len() != 1 {
                bail!("目标包含多个 plan receipt；拒绝继续使用可拆分绕过的计划状态");
            }
            if existing.plan_sha256 == receipt.plan_sha256 {
                return Ok(goal);
            }
            bail!(
                "goal plan 是首次修改前的一次性聚合合同，不能追加或拆分；请在变更前一次列出完整路径"
            );
        }
        goal.plan_receipts.push(receipt);
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Widen an existing aggregate plan without allowing post-hoc coverage.
    /// Already changed paths must be covered by the previous effective plan,
    /// and every newly added path must still match the goal baseline.
    pub fn extend_plan(&self, id: &str, mut submission: PlanReceiptSubmission) -> Result<Goal> {
        if review_priority_rank(&submission.review_priority).is_none() {
            bail!("未知 review_priority: {}", submission.review_priority);
        }
        normalize_path_list(&mut submission.changed_paths);
        normalize_path_list(&mut submission.impacted_paths);
        submission.recommended_checks.sort();
        submission.recommended_checks.dedup();
        if submission.changed_paths.is_empty() {
            bail!("goal plan --extend 至少需要一个变更路径");
        }

        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema()
            || goal.lifecycle != GoalLifecycle::Current
            || goal.status != GoalStatus::Active
        {
            bail!("只有 current-schema active/current 目标可以扩展 plan");
        }
        let Some(baseline) = goal.baseline.as_ref() else {
            bail!("目标缺少开工 baseline，不能扩展 plan");
        };
        if goal.plan_receipts.len() != 1 {
            bail!("goal plan --extend 要求恰好一个基础聚合 plan receipt");
        }
        let receipt = goal.plan_receipts.first_mut().expect("checked one plan");
        let existing = receipt
            .effective_changed_paths()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let additions = submission
            .changed_paths
            .iter()
            .filter(|candidate| !existing.contains(*candidate))
            .cloned()
            .collect::<Vec<_>>();
        if additions.is_empty() {
            return Ok(goal);
        }

        let current = workspace_baseline(&self.root)?;
        let actual = workspace_delta(baseline, &current);
        let prior_unplanned = actual
            .iter()
            .filter(|changed| !existing.contains(*changed))
            .cloned()
            .collect::<Vec<_>>();
        if !prior_unplanned.is_empty() {
            bail!(
                "goal plan --extend 拒绝事后补票；已有未计划变更: {}",
                prior_unplanned.join(", ")
            );
        }
        for added in &additions {
            match (baseline.files.get(added), current.files.get(added)) {
                (Some(expected), Some(actual)) if expected == actual => {}
                (None, None) => {}
                _ => bail!("goal plan --extend 拒绝已发生变化的新路径: {added}"),
            }
        }

        let mut changed_paths = receipt.effective_changed_paths().to_vec();
        changed_paths.extend(additions);
        normalize_path_list(&mut changed_paths);
        let mut impacted_paths = receipt.effective_impacted_paths().to_vec();
        impacted_paths.extend(submission.impacted_paths);
        normalize_path_list(&mut impacted_paths);
        let mut recommended_checks = receipt.effective_recommended_checks().to_vec();
        recommended_checks.extend(submission.recommended_checks);
        recommended_checks.sort();
        recommended_checks.dedup();
        let review_priority = max_review_priority(
            receipt.effective_review_priority(),
            &submission.review_priority,
        )?;
        let previous_plan_sha256 = receipt.effective_plan_sha256().to_string();
        let mut extension = PlanExtensionReceipt {
            recorded_at: now_iso(),
            previous_plan_sha256,
            changed_paths,
            review_priority,
            impacted_paths,
            recommended_checks,
            extension_sha256: String::new(),
        };
        extension.extension_sha256 =
            plan_extension_sha256(&baseline.workspace_fingerprint, &extension);
        receipt.extensions.push(extension);
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }

    pub fn record_review(&self, id: &str, reviewer: &str, summary: &str) -> Result<Goal> {
        if reviewer.trim().is_empty() || summary.trim().is_empty() {
            bail!("reviewer 与 summary 都不能为空");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema()
            || goal.lifecycle != GoalLifecycle::Current
            || !matches!(goal.status, GoalStatus::Active | GoalStatus::Success)
        {
            bail!("只有 active/success 的 current-schema 目标可以记录 review receipt");
        }
        if goal.plan_receipts.is_empty() {
            bail!("review receipt 必须绑定已记录的 goal plan");
        }
        let receipt = ReviewReceipt {
            recorded_at: now_iso(),
            source_fingerprint: workspace_fingerprint(&self.root)?,
            reviewer: reviewer.trim().to_string(),
            summary: summary.trim().to_string(),
        };
        if !goal.review_receipts.iter().any(|existing| {
            existing.source_fingerprint == receipt.source_fingerprint
                && existing.reviewer == receipt.reviewer
                && existing.summary == receipt.summary
        }) {
            goal.review_receipts.push(receipt);
            goal.updated_at = now_iso();
            write_json(&path, &goal)?;
        }
        Ok(goal)
    }

    pub fn get(&self, id: &str) -> Result<Option<Goal>> {
        Self::load_goal_file(&self.goal_path(id)?)
    }

    pub fn with_locked_goal<T>(
        &self,
        id: &str,
        operation: impl FnOnce(&Goal) -> Result<T>,
    ) -> Result<T> {
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let goal =
            Self::load_goal_file(&path)?.ok_or_else(|| anyhow::anyhow!("目标不存在: {id}"))?;
        operation(&goal)
    }
    pub fn validation_contract_hash(&self, id: &str, requirement_id: &str) -> Result<String> {
        let Some(goal) = self.get(id)? else {
            bail!("目标不存在: {id}");
        };
        validation_contract_sha256(&goal, requirement_id)
    }

    pub fn list(&self) -> Result<Vec<Goal>> {
        let (goals, _) = self.list_with_issues()?;
        Ok(goals)
    }

    pub fn list_with_issues(&self) -> Result<(Vec<Goal>, Vec<GoalLoadIssue>)> {
        let mut goals = Vec::new();
        let mut issues = Vec::new();
        let dir = match state_paths::managed_state_dir(&self.root, Path::new(GOALS_RELATIVE), false)
        {
            Ok(Some(dir)) => dir,
            Ok(None) => return Ok((goals, issues)),
            Err(error) => {
                issues.push(GoalLoadIssue {
                    path: self.root.join(GOALS_DIR).display().to_string(),
                    error: error.to_string(),
                });
                return Ok((goals, issues));
            }
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((goals, issues));
            }
            Err(error) => {
                issues.push(GoalLoadIssue {
                    path: dir.display().to_string(),
                    error: error.to_string(),
                });
                return Ok((goals, issues));
            }
        };
        for entry_result in entries {
            let entry = match entry_result {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(GoalLoadIssue {
                        path: dir.display().to_string(),
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            let listed_path = dir.join(&name);
            if Path::new(&name).extension().and_then(|ext| ext.to_str()) == Some("json") {
                // `read_dir` returns a lexical entry path.  Do not read it
                // directly: a goal file can itself be a symlink/junction even
                // when the goals directory was verified above.
                let path = match state_paths::managed_state_file(
                    &self.root,
                    &Path::new(GOALS_RELATIVE).join(&name),
                    false,
                ) {
                    Ok(path) => path,
                    Err(error) => {
                        issues.push(GoalLoadIssue {
                            path: listed_path.display().to_string(),
                            error: error.to_string(),
                        });
                        continue;
                    }
                };
                match Self::load_goal_file(&path) {
                    Ok(Some(goal)) => goals.push(goal),
                    Ok(None) => {}
                    Err(error) => issues.push(GoalLoadIssue {
                        path: listed_path.display().to_string(),
                        error: error.to_string(),
                    }),
                }
            }
        }
        goals.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok((goals, issues))
    }

    fn load_goal_file(path: &Path) -> Result<Option<Goal>> {
        let Some(value) = read_json::<serde_json::Value>(path)? else {
            return Ok(None);
        };
        match serde_json::from_value::<Goal>(value.clone()) {
            Ok(mut goal) => {
                // Earlier lean goals were already serialized in a shape close
                // to `Goal`, but had no schema marker.  Mutating one of those
                // records writes schema_version=0; keep treating that exact
                // migration shape as legacy on every later load.  A nonzero
                // unknown version remains a current-format incompatibility
                // and is rejected by the standard gate instead of silently
                // downgraded to history.
                goal.loaded_from_legacy = goal.schema_version == 0;
                Ok(Some(goal))
            }
            Err(current_error) => match serde_json::from_value::<LegacyGoal>(value) {
                Ok(legacy) => Ok(Some(goal_from_legacy(legacy))),
                Err(legacy_error) => bail!(
                    "无法解析 goal 文件: current schema: {current_error}; legacy schema: {legacy_error}"
                ),
            },
        }
    }

    /// 记录某个需求的证据、验证命令和变更影响快照，并标记完成。
    ///
    /// 这是 SKILL.md 描述的"evidence-only completion"那一层的输入路径：它写出的
    /// validation 没有 receipt，因此**不能**支撑任何 standard/release 主张——门禁
    /// 要的是 `goal validate` 产生的 receipt。它只用于记录尚未被机器验证的进展。
    pub fn record_evidence_with_context(
        &self,
        id: &str,
        req_id: &str,
        evidence: &str,
        validation_commands: Vec<String>,
        impacts: Vec<ImpactEvidence>,
    ) -> Result<Goal> {
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle != GoalLifecycle::Current {
            bail!(
                "目标 {id} lifecycle={}，不能追加证据；先用 `goal current {id}` 恢复为 current",
                goal.lifecycle
            );
        }
        // 已关闭为 success 的目标不接受无 receipt 的补记：否则一条人工声明就能把
        // 需求翻成 done 并追加 receipt-less validation，污染已完成的证据链。
        if goal.status == GoalStatus::Success {
            bail!(
                "目标 {id} 已关闭为 success，不能再追加人工证据；请用 `goal validate` 写入带 receipt 的验证，或先 supersede/archive"
            );
        }
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        if let Some(proof_kind) = req.proof_kind {
            bail!(
                "typed requirement {req_id} requires a matching goal validate receipt (proof_kind={})",
                proof_kind.as_str()
            );
        }
        let now = now_iso();
        req.evidence = Some(evidence.into());
        let impact_paths = impacts
            .iter()
            .map(|impact| impact.changed_path.clone())
            .collect::<Vec<_>>();
        let impact_scopes = validation_scopes_for_impacts(&impacts);
        // 追加而非覆写：补记一条说明不应销毁先前的验证与影响面审计记录。
        for command in validation_commands {
            if !req.validations.iter().any(|v| v.command == command) {
                req.validations.push(ValidationEvidence {
                    command,
                    recorded_at: now.clone(),
                    impact_paths: impact_paths.clone(),
                    impact_scopes: impact_scopes.clone(),
                    non_code: impacts.is_empty(),
                    receipt: None,
                });
            }
        }
        req.impacts.extend(impacts);
        req.status = RequirementStatus::Done;
        goal.updated_at = now;
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// 记录由 rayman 实际执行后生成的验证 receipt。只有 current schema 的 success
    /// 目标可把这种 receipt 当作 standard/release 证据。
    pub fn record_validation_receipt(
        &self,
        id: &str,
        req_id: &str,
        submission: ValidationReceiptSubmission,
    ) -> Result<Goal> {
        self.record_validation_receipt_inner(id, req_id, submission, None)
    }

    pub fn record_authority_validation_receipt(
        &self,
        id: &str,
        req_id: &str,
        submission: AuthorityReceiptSubmission,
    ) -> Result<Goal> {
        self.record_validation_receipt_inner(
            id,
            req_id,
            submission.validation,
            Some(submission.authority),
        )
    }

    fn record_validation_receipt_inner(
        &self,
        id: &str,
        req_id: &str,
        submission: ValidationReceiptSubmission,
        authority: Option<AuthorityReceipt>,
    ) -> Result<Goal> {
        let ValidationReceiptSubmission {
            evidence,
            command,
            receipt,
            impacts,
            non_code,
        } = submission;
        if evidence.trim().is_empty() {
            bail!("验证证据说明不能为空");
        }
        validate_command_for_impacts(&self.root, &command, &impacts, non_code)?;
        let impact_paths = impacts
            .iter()
            .map(|impact| impact.changed_path.clone())
            .collect::<Vec<_>>();
        let impact_scopes = validation_scopes_for_impacts(&impacts);
        if receipt.invocation_sha256
            != validation_invocation_sha256_scoped(&command, &impact_scopes, non_code)
        {
            bail!("validation receipt 与命令/影响路径不匹配");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema() {
            bail!("目标 {id} 不是当前 schema，不能写入可验证 receipt；请新建目标");
        }
        if goal.lifecycle != GoalLifecycle::Current {
            bail!(
                "目标 {id} lifecycle={}，不能写入 receipt；先用 `goal current {id}` 恢复为 current",
                goal.lifecycle
            );
        }
        let required_proof = goal
            .requirements
            .iter()
            .find(|requirement| requirement.id == req_id)
            .ok_or_else(|| anyhow::anyhow!("需求不存在: {req_id}"))?
            .proof_kind;
        let actual_proof = validation_proof_kind(&self.root, &command)?;
        if !proof_kind_matches(required_proof, actual_proof) {
            bail!(
                "validation proof kind mismatch: requirement={} command={}",
                required_proof.unwrap_or_default().as_str(),
                actual_proof.as_str()
            );
        }
        // baseline 缺失时整个 plan/差量门禁曾被静默跳过（无 else 分支），于是这类
        // 目标可以吸收任意未声明变更并仍写出 receipt。文档承诺此类目标"永远不
        // gate-ready"，写入侧同样必须 fail-closed。
        let Some(baseline) = goal.baseline.as_ref() else {
            bail!(
                "目标 {id} 缺少开工 baseline，无法核对实际变更；请用新的 baseline-bound goal supersede，或将已完成记录显式 archive"
            );
        };
        let current = workspace_baseline(&self.root)?;
        let actual = workspace_delta(baseline, &current);
        let valid_plans = goal
            .plan_receipts
            .iter()
            .filter(|plan| {
                plan.baseline_fingerprint == baseline.workspace_fingerprint
                    && plan.plan_sha256 == plan_receipt_sha256(plan)
                    && plan_extensions_are_valid(plan)
            })
            .collect::<Vec<_>>();
        if actual.len() >= 2 && valid_plans.is_empty() {
            bail!(
                "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
                actual.len()
            );
        }
        let planned = valid_plans
            .iter()
            .flat_map(|plan| plan.effective_changed_paths().iter().cloned())
            .collect::<BTreeSet<_>>();
        let unplanned = actual
            .iter()
            .filter(|changed| !planned.contains(*changed))
            .cloned()
            .collect::<Vec<_>>();
        if !valid_plans.is_empty() && !unplanned.is_empty() {
            bail!("validation 拒绝未计划的实际变更: {}", unplanned.join(", "));
        }
        if !valid_plans.is_empty() {
            let undeclared_plan = impact_paths
                .iter()
                .map(|path| path.replace('\\', "/"))
                .filter(|changed| !planned.contains(changed))
                .collect::<Vec<_>>();
            if !undeclared_plan.is_empty() {
                bail!(
                    "validation --changed 超出 goal plan: {}",
                    undeclared_plan.join(", ")
                );
            }
        }
        let expected_contract = validation_contract_sha256(&goal, req_id)?;
        if receipt.contract_sha256 != expected_contract {
            bail!("validation receipt 与 immutable goal/requirement contract 不匹配");
        }
        if let Some(authority) = authority.as_ref() {
            if authority.requirement_id != req_id
                || authority.command != command
                || authority.contract_sha256 != expected_contract
                || authority.impact_scopes != impact_scopes
                || authority.non_code != non_code
            {
                bail!("authority receipt 与 requirement/command/scope 合同不匹配");
            }
            if authority.repeat < 2 || authority.runs.len() != authority.repeat as usize {
                bail!("authority receipt 必须包含至少两次完整稳定执行");
            }
            if authority.invocation_sha256
                != authority_invocation_sha256(
                    &command,
                    req_id,
                    authority.repeat,
                    &impact_scopes,
                    non_code,
                )
            {
                bail!("authority receipt invocation hash 无效");
            }
            if authority.workspace_fingerprint != current.workspace_fingerprint
                || authority.runs.iter().any(|run| {
                    run.exit_code != 0
                        || run.workspace_fingerprint_before != authority.workspace_fingerprint
                        || run.workspace_fingerprint_after != authority.workspace_fingerprint
                        || !is_sha256(&run.stdout_sha256)
                        || !is_sha256(&run.stderr_sha256)
                })
            {
                bail!("authority receipt 未证明同一 workspace fingerprint 上的重复稳定 PASS");
            }
        }
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        let now = now_iso();
        req.evidence = Some(evidence);
        req.validations.push(ValidationEvidence {
            command,
            recorded_at: now.clone(),
            impact_paths,
            impact_scopes,
            non_code,
            receipt: Some(receipt),
        });
        req.impacts.extend(impacts);
        req.status = RequirementStatus::Done;
        if let Some(authority) = authority {
            goal.authority_receipts.push(authority);
        }
        goal.updated_at = now;
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// 关闭目标。status=success 时，每个 must 需求必须带 `goal validate` 写入的
    /// 当前 receipt（仅有人工证据不够，只能关成 partial/blocked），否则拒绝。
    /// success 是终态：不能再降级，重开走 supersede。
    pub fn close(&self, id: &str, status: &str) -> Result<Goal> {
        let status = match status {
            "success" => GoalStatus::Success,
            "partial" => GoalStatus::Partial,
            "blocked" => GoalStatus::Blocked,
            _ => {
                bail!("未知的关闭状态: {status}（可用: success | partial | blocked）");
            }
        };
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle != GoalLifecycle::Current {
            bail!(
                "目标 {id} lifecycle={}，不能关闭；先用 `goal current {id}` 恢复为 current",
                goal.lifecycle
            );
        }
        if status == GoalStatus::Blocked
            && !PendingStore::new(&self.root).proven_non_agent_boundary(id)?
        {
            bail!(
                "拒绝关闭为 blocked：必须先记录至少一个带完整解决方案包的 human/external pending，且不能仍有 agent-owned pending"
            );
        }
        if status == GoalStatus::Success {
            if !goal.is_current_schema() {
                bail!(
                    "拒绝关闭为 success：legacy goal 不能生成当前 receipt；只可归档已是 success 的历史记录"
                );
            }
            let mut candidate = goal.clone();
            candidate.status = GoalStatus::Success;
            // current_schema_error 里的 lane-closed / required-work-package 不变量以
            // status==Success 为前提。必须在已置 Success 的候选上校验；若在仍为 Active 的
            // goal 上跑，这两条永不触发，一个开着 lane 或 required package 尚未关闭的目标
            // 就能被 close 成 success（close/frontier 谎报完成，只有读路径的 check 才拦）。
            if let Some(error) = candidate.current_schema_error() {
                bail!("拒绝关闭为 success：目标合约无效: {error}");
            }
            let fingerprint = workspace_fingerprint(&self.root)?;
            let gaps = goal_success_receipt_gaps(&candidate, &self.root, &fingerprint);
            if !gaps.is_empty() {
                bail!(
                    "拒绝关闭为 success：必须先用 goal validate 写入当前且相关的 receipt: {}",
                    gaps.join("; ")
                );
            }
            // handoff 契约（fingerprint、clean-git-at-commit、source goal、stage
            // 绑定）此前只在读门禁（goal_gate_verdict）校验：写路径不查，漂移的
            // release-handoff 目标能以 exit 0 关成 success，然后被 check 永久拦下。
            if candidate.handoff.is_some() {
                let all_goals = self.list()?;
                if let Some(error) =
                    handoff_contract_error(&candidate, &all_goals, &self.root, &fingerprint)
                {
                    bail!("拒绝关闭为 success：handoff contract invalid: {error}");
                }
            }
            goal = candidate;
        } else {
            // success 是终态。允许降级会抹掉一次已完成的记录，而且是绕过"已关闭
            // success 不能再追加人工证据"那条守卫的现成路径：降级 → 追加伪造
            // evidence → 重新关闭为 success，证据链被污染而门禁读不出来。
            // 要继续做就 supersede，要保留历史就 archive。
            if goal.status == GoalStatus::Success {
                bail!(
                    "目标 {id} 已关闭为 success，不能降级为 {status}；请用新的 baseline-bound goal supersede，或将该记录 archive"
                );
            }
            goal.status = status;
        }
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Keep historical state without deleting its JSON record.  Archiving is
    /// explicit and reasoned because current goals are readiness blockers.
    pub fn archive(&self, id: &str, reason: &str, migrate_unreceipted: bool) -> Result<Goal> {
        self.archive_with_receipt_policy(id, reason, migrate_unreceipted, None)
    }

    pub fn archive_with_receipt_policy(
        &self,
        id: &str,
        reason: &str,
        migrate_unreceipted: bool,
        migrate_receipt_policy: Option<&str>,
    ) -> Result<Goal> {
        if reason.trim().is_empty() {
            bail!("归档原因不能为空");
        }
        if migrate_unreceipted && migrate_receipt_policy.is_some() {
            bail!("pre-receipt migration 与 receipt-policy migration 不能同时使用");
        }
        if migrate_receipt_policy.is_some_and(|policy| policy != RECEIPT_POLICY_V1) {
            bail!("未知历史 receipt policy；当前只支持 {RECEIPT_POLICY_V1}");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle == GoalLifecycle::Archived && migrate_unreceipted {
            // Same one-way rule `mark_current` enforces: this branch is the only
            // other lifecycle_proof rewriter, and it would re-bless a quarantined
            // record as trusted history — overwriting the `[invalid proof: ...]`
            // reason and swapping the retained historical fingerprint for the
            // current one. The sibling receipt-policy branch already refuses any
            // record that carries an explicit policy.
            if is_quarantined(&goal) {
                bail!(
                    "目标 {id} 已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能用 migration 刷新为可信历史"
                );
            }
            if !pre_receipt_migration_eligible(&goal) {
                bail!("只有符合 rollout 前条件的 schema-v2 success goal 可以刷新 migration proof");
            }
            goal.lifecycle_reason = Some(reason.trim().to_string());
            goal.superseded_by = None;
            goal.lifecycle_proof = None;
            goal.updated_at = now_iso();
            let fingerprint = workspace_fingerprint(&self.root)?;
            goal.lifecycle_proof = Some(issue_lifecycle_proof(
                &goal,
                fingerprint,
                Some(PRE_RECEIPT_MIGRATION.to_string()),
                Some(RECEIPT_POLICY_V2.to_string()),
            ));
            write_json(&path, &goal)?;
            return Ok(goal);
        }
        if goal.lifecycle == GoalLifecycle::Archived && migrate_receipt_policy.is_some() {
            if goal
                .lifecycle_proof
                .as_ref()
                .and_then(|proof| proof.receipt_policy.as_deref())
                .is_some()
            {
                bail!("archived goal 已有显式 receipt policy；拒绝降级或重复迁移");
            }
            if let Some(error) = goal.current_schema_error() {
                bail!("目标合约无效，不能迁移 historical policy: {error}");
            }
            if !receipt_policy_v1_migration_eligible(&goal) {
                bail!(
                    "只有 receipt-policy-v2 rollout 前的 schema-v2 success goal 可以迁移 v1 proof"
                );
            }
            let Some(fingerprint) = historical_success_fingerprint(
                &goal,
                &self.root,
                ReceiptValidationPolicy::LegacyV1,
            ) else {
                bail!("历史 goal 不满足 receipt_integrity_v1；拒绝刷新 lifecycle proof");
            };
            goal.lifecycle_reason = Some(reason.trim().to_string());
            goal.superseded_by = None;
            goal.lifecycle_proof = None;
            goal.updated_at = now_iso();
            goal.lifecycle_proof = Some(issue_lifecycle_proof(
                &goal,
                fingerprint,
                Some(RECEIPT_POLICY_V1_MIGRATION.to_string()),
                Some(RECEIPT_POLICY_V1.to_string()),
            ));
            write_json(&path, &goal)?;
            return Ok(goal);
        }
        if goal.lifecycle != GoalLifecycle::Current {
            bail!(
                "只有 current goal 可以归档；已迁移的 archived goal 可用 --migrate-unreceipted 幂等刷新 proof"
            );
        }
        // `active` must still be closed first: stating `partial`/`blocked` is
        // the honest record of what actually happened, and archiving is only
        // the retirement of an already-stated outcome. Abandoned work used to
        // have no disposal path — `archive` demanded success and `supersede`
        // demanded a replacement that was already gate-ready success — so real
        // sessions simply stopped recording anything.
        if goal.status == GoalStatus::Active {
            bail!(
                "active goal 不能直接归档；先 `rayman goal close {id} --status partial`（或 blocked）如实记录结果，再归档"
            );
        }
        if let Some(error) = goal.current_schema_error() {
            bail!("目标合约无效，不能归档: {error}");
        }
        let current_fingerprint = workspace_fingerprint(&self.root)?;
        let mut proof_fingerprint = current_fingerprint.clone();
        let mut migration = None;
        let mut receipt_policy = Some(RECEIPT_POLICY_V2.to_string());
        // Receipt integrity is a *success* contract: it exists so an archived
        // success can later serve as lifecycle-only authority. A goal retired as
        // partial/blocked makes no such claim and is refused by every consumer
        // of archived evidence, so demanding success receipts from it only
        // stranded abandoned work with nowhere to go.
        if !goal.loaded_from_legacy && goal.status == GoalStatus::Success {
            let gaps = goal_success_receipt_gaps(&goal, &self.root, &current_fingerprint);
            if !gaps.is_empty() {
                if migrate_unreceipted && pre_receipt_migration_eligible(&goal) {
                    migration = Some(PRE_RECEIPT_MIGRATION.to_string());
                } else if let Some(historical) = historical_success_fingerprint(
                    &goal,
                    &self.root,
                    ReceiptValidationPolicy::CurrentV2,
                ) {
                    proof_fingerprint = historical;
                } else if migrate_receipt_policy == Some(RECEIPT_POLICY_V1)
                    && receipt_policy_v1_migration_eligible(&goal)
                    && let Some(historical) = historical_success_fingerprint(
                        &goal,
                        &self.root,
                        ReceiptValidationPolicy::LegacyV1,
                    )
                {
                    proof_fingerprint = historical;
                    migration = Some(RECEIPT_POLICY_V1_MIGRATION.to_string());
                    receipt_policy = Some(RECEIPT_POLICY_V1.to_string());
                } else {
                    bail!(
                        "目标 success receipt 未通过当前或历史完整性复核: {}。仅对应 rollout 前历史可显式使用 --migrate-unreceipted 或 --migrate-receipt-policy {RECEIPT_POLICY_V1}",
                        gaps.join("; ")
                    );
                }
            } else if migrate_receipt_policy.is_some() {
                bail!("目标已满足当前 receipt policy，不需要降级迁移");
            }
        }
        goal.lifecycle = GoalLifecycle::Archived;
        goal.lifecycle_reason = Some(reason.trim().to_string());
        goal.superseded_by = None;
        goal.lifecycle_proof = None;
        goal.updated_at = now_iso();
        goal.lifecycle_proof = Some(issue_lifecycle_proof(
            &goal,
            proof_fingerprint,
            migration,
            receipt_policy,
        ));
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Retain an invalid archived success as explicitly untrusted history.
    ///
    /// This is a one-way evidence downgrade, never a repair of the archived
    /// receipts. The requirements and validation ledger remain untouched, the
    /// original historical workspace fingerprint is retained, and every
    /// replacement/supersession consumer rejects the quarantine policy.
    pub fn quarantine_invalid_history(&self, id: &str, reason: &str) -> Result<Goal> {
        if reason.trim().is_empty() {
            bail!("隔离原因不能为空");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle != GoalLifecycle::Archived || goal.status != GoalStatus::Success {
            bail!("只允许隔离已归档的 success 历史；current/未完成目标不能隐藏");
        }
        if let Some(error) = goal.current_schema_error() {
            bail!("目标合约无效，不能隔离 historical receipt: {error}");
        }
        if !integrity_quarantine_eligible(&goal) {
            bail!("只有 must 已完整结束的 current-schema archived success 可以隔离");
        }
        let Some(old_proof) = goal.lifecycle_proof.clone() else {
            bail!("历史目标缺少旧 lifecycle proof，不能证明该归档证据曾经失效");
        };
        if matches!(
            old_proof.receipt_policy.as_deref(),
            Some(RECEIPT_POLICY_QUARANTINED | RECEIPT_POLICY_INTEGRITY_QUARANTINED)
        ) {
            bail!("目标已经是 untrusted history quarantine，不能重复隔离");
        }
        if !is_sha256(&old_proof.workspace_fingerprint) {
            bail!("历史 lifecycle proof 的 workspace fingerprint 非法，不能生成可核验隔离记录");
        }
        let Some(proof_error) = goal.lifecycle_proof_error(&self.root) else {
            bail!("归档 success 的 lifecycle proof 仍然有效；拒绝把有效证据降级为 quarantine");
        };

        let previous_reason = goal
            .lifecycle_reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("archived success");
        goal.lifecycle_reason = Some(format!(
            "{previous_reason}; quarantine: {} [invalid proof: {proof_error}]",
            reason.trim()
        ));
        goal.superseded_by = None;
        goal.lifecycle_proof = None;
        goal.updated_at = now_iso();
        goal.lifecycle_proof = Some(issue_lifecycle_proof(
            &goal,
            old_proof.workspace_fingerprint,
            Some(INTEGRITY_QUARANTINE_MIGRATION.to_string()),
            Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED.to_string()),
        ));
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Complete a lifecycle-only replacement by binding its exact mandatory
    /// contracts and source delta to a live repeated run of the same direct
    /// authority command trusted by an archived success. Ordinary validation
    /// remains unchanged.
    pub fn authorize_replacement(
        &self,
        id: &str,
        predecessor_ids: &[String],
        authority_goal_id: &str,
        live_authority: ReplacementAuthorityReceipt,
    ) -> Result<Goal> {
        if predecessor_ids.is_empty() {
            bail!("lifecycle-only replacement 至少需要一个 --supersedes 目标");
        }
        let mut normalized_ids = predecessor_ids.to_vec();
        normalized_ids.sort();
        normalized_ids.dedup();
        if normalized_ids.len() != predecessor_ids.len() {
            bail!("--supersedes 不能包含重复目标");
        }
        if normalized_ids.iter().any(|candidate| candidate == id)
            || authority_goal_id == id
            || normalized_ids
                .iter()
                .any(|candidate| candidate == authority_goal_id)
        {
            bail!("replacement、authority goal 与被转移目标必须彼此不同");
        }

        let goals_dir =
            state_paths::managed_state_dir(&self.root, Path::new(GOALS_RELATIVE), false)?
                .ok_or_else(|| anyhow::anyhow!("目标状态目录不存在"))?;
        let _store_lock = acquire_state_lock(&goals_dir.join(".store"))?;
        let path = self.goal_path(id)?;
        // 单目标写者（evidence/validate/close/…）只持 per-goal 锁。这里在
        // .store 锁之外还必须持有同一把 per-goal 锁，否则从 load 到 write_json
        // 之间的并发单目标提交会被本函数的陈旧内存态覆盖（丢更新）。锁序固定
        // 为 .store → per-goal；单目标写者不反向等待 .store，无死锁环。
        let _goal_lock = acquire_state_lock(&path)?;
        let Some(mut replacement) = Self::load_goal_file(&path)? else {
            bail!("替代目标不存在: {id}");
        };
        if replacement.lifecycle != GoalLifecycle::Current
            || replacement.status != GoalStatus::Active
            || !replacement.is_current_schema()
            || replacement.replacement_authority.is_some()
        {
            bail!("替代目标必须是未授权的 current/active current-schema goal");
        }
        if let Some(error) = replacement.current_schema_error() {
            bail!("替代目标合约无效: {error}");
        }
        if !replacement.plan_receipts.is_empty()
            || !replacement.review_receipts.is_empty()
            || !replacement.authority_receipts.is_empty()
            || replacement.requirements.iter().any(|requirement| {
                requirement.kind != RequirementKind::Must
                    || requirement.status != RequirementStatus::Open
                    || requirement.evidence.is_some()
                    || !requirement.validations.is_empty()
                    || !requirement.impacts.is_empty()
            })
        {
            bail!("lifecycle-only replacement 必须保持 pristine 且只能包含 open must");
        }
        let current = workspace_baseline(&self.root)?;
        let Some(baseline) = replacement.baseline.as_ref() else {
            bail!("lifecycle-only replacement 缺少 baseline");
        };
        let source_delta_paths = workspace_delta(baseline, &current);

        let mut predecessors = Vec::new();
        let mut predecessor_contracts = BTreeMap::new();
        for predecessor_id in &normalized_ids {
            let Some(predecessor) = self.get(predecessor_id)? else {
                bail!("被转移目标不存在: {predecessor_id}");
            };
            if predecessor.lifecycle != GoalLifecycle::Current
                || predecessor.status == GoalStatus::Success
                || !predecessor.is_current_schema()
            {
                bail!("被转移目标 {predecessor_id} 必须是 current 非 success current-schema goal");
            }
            if let Some(error) = predecessor.current_schema_error() {
                bail!("被转移目标 {predecessor_id} 合约无效: {error}");
            }
            predecessor_contracts.insert(
                predecessor_id.clone(),
                transfer_goal_contract_sha256(&predecessor),
            );
            predecessors.push(predecessor);
        }
        if must_transfer_multiset(std::iter::once(&replacement))
            != must_transfer_multiset(predecessors.iter())
        {
            bail!(
                "replacement must 必须与 --supersedes 目标 must（含 typed proof 义务）的精确并集一致"
            );
        }
        if let Some(error) = replacement_delta_scope_error(&predecessors, &source_delta_paths) {
            bail!("{error}");
        }

        let Some(authority) = self.get(authority_goal_id)? else {
            bail!("authority goal 不存在: {authority_goal_id}");
        };
        let Some(authority_lifecycle) = authority.lifecycle_proof.as_ref() else {
            bail!("authority goal 缺少 lifecycle proof");
        };
        let fingerprint = current.workspace_fingerprint;
        if authority.lifecycle != GoalLifecycle::Archived
            || authority.status != GoalStatus::Success
            || !authority.is_current_schema()
            || authority.current_schema_error().is_some()
            || authority_lifecycle.receipt_policy.as_deref() != Some(RECEIPT_POLICY_V2)
            || authority_lifecycle.migration.is_some()
            || authority.lifecycle_proof_error(&self.root).is_some()
            || historical_success_fingerprint(
                &authority,
                &self.root,
                ReceiptValidationPolicy::CurrentV2,
            )
            .as_deref()
                != Some(authority_lifecycle.workspace_fingerprint.as_str())
            || !has_direct_stable_authority_command(
                &authority,
                &self.root,
                &authority_lifecycle.workspace_fingerprint,
                &live_authority.command,
            )
        {
            bail!(
                "authority goal 必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success"
            );
        }
        if live_authority.repeat < 2
            || live_authority.runs.len() != live_authority.repeat as usize
            || live_authority.workspace_fingerprint != fingerprint
            || live_authority.invocation_sha256
                != replacement_authority_invocation_sha256_with_rebind(
                    &live_authority.command,
                    id,
                    authority_goal_id,
                    &normalized_ids,
                    live_authority.repeat,
                    live_authority.command_rebind.as_ref(),
                )
            || validate_authority_command(&self.root, &live_authority.command).is_err()
            || replacement_authority_effective_command(
                &live_authority.command,
                live_authority.command_rebind.as_ref(),
            )
            .is_err()
            || live_authority
                .command_rebind
                .as_ref()
                .is_some_and(|rebind| {
                    verify_maintenance_cycle_rebind_artifact(&self.root, rebind).is_err()
                })
            || live_authority.runs.iter().any(|run| {
                run.exit_code != 0
                    || run.workspace_fingerprint_before != fingerprint
                    || run.workspace_fingerprint_after != fingerprint
                    || !is_sha256(&run.stdout_sha256)
                    || !is_sha256(&run.stderr_sha256)
            })
        {
            bail!("live lifecycle authority 未证明当前源码上的重复稳定仓库 gate");
        }

        let now = now_iso();
        let evidence = format!(
            "lifecycle-only exact must transfer authorized by archived goal {authority_goal_id}"
        );
        replacement.status = GoalStatus::Success;
        replacement.updated_at = now.clone();
        for requirement in &mut replacement.requirements {
            requirement.status = RequirementStatus::Done;
            requirement.evidence = Some(evidence.clone());
        }
        let mut proof = ReplacementAuthorityProof {
            recorded_at: now,
            workspace_identity: workspace_identity(&self.root),
            workspace_fingerprint: fingerprint.clone(),
            authority_goal_id: authority_goal_id.to_string(),
            authority_lifecycle_contract_sha256: authority_lifecycle.contract_sha256.clone(),
            replacement_contract_sha256: replacement_contract_sha256(&replacement),
            predecessor_contracts,
            source_delta_paths,
            live_authority,
            proof_sha256: String::new(),
        };
        proof.proof_sha256 = replacement_authority_proof_sha256(&proof);
        replacement.replacement_authority = Some(proof);
        if let Some(error) = replacement.current_schema_error() {
            bail!("拒绝写入 lifecycle-only replacement: {error}");
        }
        if let Some(error) = replacement_authority_error(&replacement, &self.root, &fingerprint) {
            bail!("拒绝写入 lifecycle-only replacement proof: {error}");
        }
        write_json(&path, &replacement)?;
        Ok(replacement)
    }

    /// Mark one goal as historical because another current goal replaced it.
    pub fn supersede(&self, id: &str, replacement_id: &str) -> Result<Goal> {
        if id == replacement_id {
            bail!("目标不能 supersede 自己");
        }
        let Some(replacement) = self.get(replacement_id)? else {
            bail!("替代目标不存在: {replacement_id}");
        };
        if replacement.lifecycle != GoalLifecycle::Current {
            bail!(
                "替代目标 {replacement_id} lifecycle={}，必须先恢复为 current",
                replacement.lifecycle
            );
        }
        if !replacement.is_current_schema() {
            bail!(
                "替代目标 {replacement_id} 必须是 current schema；legacy success 只能显式 archive"
            );
        }
        if let Some(error) = replacement.current_schema_error() {
            bail!("替代目标 {replacement_id} 合约无效: {error}");
        }

        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle != GoalLifecycle::Current {
            bail!("只有 current goal 可以被 supersede");
        }
        if let Some(error) = goal.current_schema_error() {
            bail!("目标合约无效，不能 supersede: {error}");
        }
        let current_fingerprint = workspace_fingerprint(&self.root)?;
        let mut proof_fingerprint = current_fingerprint.clone();
        let mut lifecycle_receipt_policy = RECEIPT_POLICY_V2;
        if goal.status == GoalStatus::Success
            && !goal.loaded_from_legacy
            && let Some(historical) = historical_success_fingerprint(
                &goal,
                &self.root,
                ReceiptValidationPolicy::CurrentV2,
            )
        {
            proof_fingerprint = historical;
        } else if goal.status == GoalStatus::Success && !goal.loaded_from_legacy {
            lifecycle_receipt_policy = VERIFIED_REPLACEMENT_TRANSFER_POLICY;
        }
        if let Some(error) = supersession_error(
            &{
                let mut candidate = goal.clone();
                candidate.lifecycle = GoalLifecycle::Superseded;
                candidate.lifecycle_reason = Some(format!("superseded by {replacement_id}"));
                candidate.superseded_by = Some(replacement_id.to_string());
                candidate
            },
            std::slice::from_ref(&replacement),
            &self.root,
            &current_fingerprint,
        ) {
            bail!("不能 supersede 目标 {id}: {error}");
        }
        goal.lifecycle = GoalLifecycle::Superseded;
        goal.lifecycle_reason = Some(format!("superseded by {replacement_id}"));
        goal.superseded_by = Some(replacement_id.to_string());
        goal.lifecycle_proof = None;
        goal.updated_at = now_iso();
        goal.lifecycle_proof = Some(issue_lifecycle_proof(
            &goal,
            proof_fingerprint,
            None,
            Some(lifecycle_receipt_policy.to_string()),
        ));
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Restore an archived/superseded record to the active readiness set.
    pub fn mark_current(&self, id: &str) -> Result<Goal> {
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        // quarantine 是单向 evidence 降级：mark_current 会清空 lifecycle_proof
        // 与 lifecycle_reason，等于抹掉隔离标记和 `[invalid proof: ...]` 审计
        // 痕迹，再经 close/archive 重铸为可信历史。这里必须拒绝。
        if is_quarantined(&goal) {
            bail!(
                "目标 {id} 已隔离为 untrusted history；隔离是单向降级，审计记录必须保留，不能恢复为 current"
            );
        }
        goal.lifecycle = GoalLifecycle::Current;
        goal.lifecycle_reason = None;
        goal.superseded_by = None;
        goal.lifecycle_proof = None;
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }
}

/// Is this record an explicitly untrusted, quarantined history?
///
/// One predicate for every path that could undo the downgrade. It used to be
/// inlined in `mark_current` alone, which is how `archive --migrate-unreceipted`
/// — the only other lifecycle_proof rewriter — kept its escape.
fn is_quarantined(goal: &Goal) -> bool {
    goal.lifecycle_proof.as_ref().is_some_and(|proof| {
        matches!(
            proof.receipt_policy.as_deref(),
            Some(RECEIPT_POLICY_QUARANTINED | RECEIPT_POLICY_INTEGRITY_QUARANTINED)
        )
    })
}

fn goal_from_legacy(legacy: LegacyGoal) -> Goal {
    let created_at = legacy
        .created_at
        .or_else(|| legacy.contract.created_at.clone())
        .unwrap_or_default();
    let updated_at = legacy.updated_at.unwrap_or_else(|| created_at.clone());
    let requirements = legacy
        .contract
        .requirements
        .into_iter()
        .map(|req| {
            let validation_commands = if req.validation_commands.is_empty() {
                legacy.contract.verification.clone()
            } else {
                req.validation_commands
            };
            Requirement {
                id: req.id,
                text: req.text,
                kind: match req.priority.as_str() {
                    "must" => RequirementKind::Must,
                    "should" => RequirementKind::Should,
                    other => {
                        // Legacy data must never silently downgrade an unknown mandatory kind.
                        // Keep it as must so standard check can require migration/receipt.
                        let _ = other;
                        RequirementKind::Must
                    }
                },
                proof_kind: None,
                status: match req.status.as_str() {
                    "satisfied" | "done" => RequirementStatus::Done,
                    _ => RequirementStatus::Open,
                },
                evidence: req.evidence,
                validations: validation_commands
                    .into_iter()
                    .map(|command| ValidationEvidence {
                        command,
                        recorded_at: updated_at.clone(),
                        impact_paths: Vec::new(),
                        impact_scopes: Vec::new(),
                        non_code: false,
                        receipt: None,
                    })
                    .collect(),
                impacts: Vec::new(),
            }
        })
        .collect();
    Goal {
        schema_version: 0,
        id: legacy.id,
        title: legacy.contract.goal,
        status: match legacy.status.as_str() {
            "success" => GoalStatus::Success,
            "partial" => GoalStatus::Partial,
            "blocked" => GoalStatus::Blocked,
            _ => GoalStatus::Active,
        },
        lifecycle: GoalLifecycle::Current,
        lifecycle_reason: None,
        superseded_by: None,
        lifecycle_proof: None,
        replacement_authority: None,
        created_at,
        baseline: None,
        plan_receipts: Vec::new(),
        review_receipts: Vec::new(),
        authority_receipts: Vec::new(),
        work_packages: Vec::new(),
        progress_receipts: Vec::new(),
        lanes: Vec::new(),
        handoff: None,
        updated_at,
        requirements,
        loaded_from_legacy: true,
    }
}

#[cfg(test)]
#[path = "goal/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "goal/installer_tests.rs"]
mod installer_tests;
