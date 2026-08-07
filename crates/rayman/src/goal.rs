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
pub const PLAN_PUBLICATION_POLICY_V1: &str = "write_ahead_v1";
const PLAN_PUBLICATION_ROLLOUT_AT: &str = "2026-08-05T10:30:00Z";
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
mod legacy;
mod lifecycle;
mod pending;
mod plan_publication;
mod rebind;
mod validation;

pub use handoff::*;
use legacy::*;
pub use lifecycle::*;
pub use pending::*;
use plan_publication::*;
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

fn hash_required_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}

fn hash_optional_string(hasher: &mut Sha256, value: Option<&str>) {
    hasher.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        hash_required_string(hasher, value);
    }
}

pub fn plan_receipt_sha256(receipt: &PlanReceipt) -> String {
    let mut hasher = Sha256::new();
    let publication_goal_bound = receipt
        .publication
        .as_ref()
        .is_some_and(|publication| !publication.goal_id.is_empty());
    hasher.update(
        match (receipt.publication.is_some(), publication_goal_bound) {
            (true, true) => b"rayman.goal-plan-receipt.v3".as_slice(),
            (true, false) => b"rayman.goal-plan-receipt.v2".as_slice(),
            (false, _) => b"rayman.goal-plan-receipt.v1".as_slice(),
        },
    );
    if publication_goal_bound {
        hash_required_string(&mut hasher, &receipt.recorded_at);
    }
    hasher.update(receipt.baseline_fingerprint.as_bytes());
    hasher.update(receipt.review_priority.as_bytes());
    hash_string_sequence(&mut hasher, &receipt.changed_paths);
    hash_string_sequence(&mut hasher, &receipt.impacted_paths);
    hash_string_sequence(&mut hasher, &receipt.recommended_checks);
    if let Some(publication) = receipt.publication.as_ref() {
        hasher.update(publication.publication_sha256.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn plan_extension_sha256(
    baseline_fingerprint: &str,
    extension: &PlanExtensionReceipt,
) -> String {
    let mut hasher = Sha256::new();
    let publication_goal_bound = extension
        .publication
        .as_ref()
        .is_some_and(|publication| !publication.goal_id.is_empty());
    hasher.update(
        match (extension.publication.is_some(), publication_goal_bound) {
            (true, true) => b"rayman.goal-plan-extension.v3".as_slice(),
            (true, false) => b"rayman.goal-plan-extension.v2".as_slice(),
            (false, _) => b"rayman.goal-plan-extension.v1".as_slice(),
        },
    );
    if publication_goal_bound {
        hash_required_string(&mut hasher, &extension.recorded_at);
    }
    hasher.update(baseline_fingerprint.as_bytes());
    hasher.update(extension.previous_plan_sha256.as_bytes());
    hasher.update(extension.review_priority.as_bytes());
    hash_string_sequence(&mut hasher, &extension.changed_paths);
    hash_string_sequence(&mut hasher, &extension.impacted_paths);
    hash_string_sequence(&mut hasher, &extension.recommended_checks);
    if let Some(publication) = extension.publication.as_ref() {
        hasher.update(publication.publication_sha256.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn timestamp_before(value: &str, boundary: &chrono::DateTime<chrono::FixedOffset>) -> bool {
    chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|value| value < *boundary)
}

fn plan_timestamp(
    label: &str,
    value: &str,
) -> std::result::Result<chrono::DateTime<chrono::FixedOffset>, String> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| format!("{label} 必须是 RFC3339 timestamp"))
}

fn latest_timestamp<'a>(bounds: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<String> {
    let now = now_iso();
    let mut latest_at = chrono::DateTime::parse_from_rfc3339(&now)
        .expect("now_iso must always produce an RFC3339 timestamp");
    let mut latest = now;
    for (label, value) in bounds {
        let parsed = plan_timestamp(label, value).map_err(anyhow::Error::msg)?;
        if parsed > latest_at {
            latest_at = parsed;
            latest = value.to_string();
        }
    }
    Ok(latest)
}

fn latest_valid_timestamp<'a>(bounds: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let now = now_iso();
    let mut latest_at = chrono::DateTime::parse_from_rfc3339(&now)
        .expect("now_iso must always produce an RFC3339 timestamp");
    let mut latest = now;
    for (_, value) in bounds {
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) else {
            continue;
        };
        if parsed > latest_at {
            latest_at = parsed;
            latest = value.to_string();
        }
    }
    latest
}

pub(super) fn goal_ledger_timestamp_bounds(goal: &Goal) -> Vec<(&'static str, &str)> {
    let mut bounds = vec![
        ("goal.created_at", goal.created_at.as_str()),
        ("goal.updated_at", goal.updated_at.as_str()),
    ];
    if let Some(baseline) = goal.baseline.as_ref() {
        bounds.push(("goal.baseline.recorded_at", baseline.recorded_at.as_str()));
    }
    if let Some(intent) = goal.plan_publish_intent.as_ref() {
        bounds.push((
            "goal.plan_publish_intent.prepared_at",
            intent.prepared_at.as_str(),
        ));
    }
    for receipt in &goal.plan_receipts {
        bounds.push((
            "goal.plan_receipt.recorded_at",
            receipt.recorded_at.as_str(),
        ));
        if let Some(publication) = receipt.publication.as_ref() {
            bounds.push((
                "goal.plan_receipt.publication.published_at",
                publication.published_at.as_str(),
            ));
            if let Some(committed_at) = publication.committed_at.as_deref() {
                bounds.push(("goal.plan_receipt.publication.committed_at", committed_at));
            }
        }
        for extension in &receipt.extensions {
            bounds.push((
                "goal.plan_extension.recorded_at",
                extension.recorded_at.as_str(),
            ));
            if let Some(publication) = extension.publication.as_ref() {
                bounds.push((
                    "goal.plan_extension.publication.published_at",
                    publication.published_at.as_str(),
                ));
                if let Some(committed_at) = publication.committed_at.as_deref() {
                    bounds.push(("goal.plan_extension.publication.committed_at", committed_at));
                }
            }
        }
    }
    for receipt in &goal.review_receipts {
        bounds.push((
            "goal.review_receipt.recorded_at",
            receipt.recorded_at.as_str(),
        ));
    }
    for receipt in &goal.authority_receipts {
        bounds.push((
            "goal.authority_receipt.recorded_at",
            receipt.recorded_at.as_str(),
        ));
    }
    for requirement in &goal.requirements {
        for validation in &requirement.validations {
            bounds.push((
                "goal.validation_evidence.recorded_at",
                validation.recorded_at.as_str(),
            ));
        }
        for impact in &requirement.impacts {
            bounds.push((
                "goal.impact_evidence.recorded_at",
                impact.recorded_at.as_str(),
            ));
        }
    }
    for package in &goal.work_packages {
        if let Some(completed_at) = package.completed_at.as_deref() {
            bounds.push(("goal.work_package.completed_at", completed_at));
        }
    }
    for receipt in &goal.progress_receipts {
        bounds.push((
            "goal.progress_receipt.recorded_at",
            receipt.recorded_at.as_str(),
        ));
    }
    for lane in &goal.lanes {
        bounds.push(("goal.lane.opened_at", lane.opened_at.as_str()));
        bounds.push((
            "goal.lane.opening_baseline.recorded_at",
            lane.opening_baseline.recorded_at.as_str(),
        ));
        if let Some(closed_at) = lane.closed_at.as_deref() {
            bounds.push(("goal.lane.closed_at", closed_at));
        }
    }
    if let Some(handoff) = goal.handoff.as_ref() {
        bounds.push(("goal.handoff.created_at", handoff.created_at.as_str()));
    }
    if let Some(proof) = goal.lifecycle_proof.as_ref() {
        bounds.push((
            "goal.lifecycle_proof.recorded_at",
            proof.recorded_at.as_str(),
        ));
    }
    if let Some(proof) = goal.replacement_authority.as_ref() {
        bounds.push((
            "goal.replacement_authority.recorded_at",
            proof.recorded_at.as_str(),
        ));
        bounds.push((
            "goal.replacement_authority.live_authority.recorded_at",
            proof.live_authority.recorded_at.as_str(),
        ));
    }
    bounds
}

fn goal_event_timestamp(goal: &Goal) -> Result<String> {
    latest_timestamp(goal_ledger_timestamp_bounds(goal))
}

fn quarantine_event_timestamp(goal: &Goal) -> String {
    // Quarantine is the one recovery path whose input is expected to contain
    // an invalid lifecycle/receipt ledger.  Unparseable timestamps have no
    // sortable lower bound; retain the exact proof failure in the quarantine
    // reason and clamp the replacement envelope to every valid timestamp.
    latest_valid_timestamp(goal_ledger_timestamp_bounds(goal))
}

fn goal_event_timestamp_after_all(
    goal: &Goal,
    additional_bounds: &[(&str, &str)],
) -> Result<String> {
    let lower_bound = goal_event_timestamp(goal)?;
    let mut bounds = vec![("goal event lower bound", lower_bound.as_str())];
    bounds.extend_from_slice(additional_bounds);
    latest_timestamp(bounds)
}

fn goal_event_timestamp_after(goal: &Goal, label: &str, reference: &str) -> Result<String> {
    goal_event_timestamp_after_all(goal, &[(label, reference)])
}

fn publication_end_timestamp(
    publication: &PlanPublicationProof,
) -> std::result::Result<chrono::DateTime<chrono::FixedOffset>, String> {
    plan_timestamp(
        "plan publication end",
        publication
            .committed_at
            .as_deref()
            .unwrap_or(&publication.published_at),
    )
}

fn legacy_plan_chronology_error(goal: &Goal, baseline: &WorkspaceBaseline) -> Option<String> {
    let created_at = match plan_timestamp("goal.created_at", &goal.created_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let baseline_at = match plan_timestamp("goal.baseline.recorded_at", &baseline.recorded_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let updated_at = match plan_timestamp("goal.updated_at", &goal.updated_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    if baseline_at < created_at {
        return Some(
            "legacy plan 时间顺序必须满足 goal <= baseline <= receipt <= extensions <= updated"
                .into(),
        );
    }

    let mut previous = baseline_at;
    for receipt in &goal.plan_receipts {
        let receipt_at =
            match plan_timestamp("legacy plan receipt recorded_at", &receipt.recorded_at) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
        if receipt_at < previous {
            return Some(
                "legacy plan 时间顺序必须满足 goal <= baseline <= receipt <= extensions <= updated"
                    .into(),
            );
        }
        previous = receipt_at;
        for extension in &receipt.extensions {
            let extension_at =
                match plan_timestamp("legacy plan extension recorded_at", &extension.recorded_at) {
                    Ok(value) => value,
                    Err(error) => return Some(error),
                };
            if extension_at < previous {
                return Some(
                    "legacy plan 时间顺序必须满足 goal <= baseline <= receipt <= extensions <= updated"
                        .into(),
                );
            }
            previous = extension_at;
        }
    }
    if updated_at < previous {
        return Some(
            "legacy plan 时间顺序必须满足 goal <= baseline <= receipt <= extensions <= updated"
                .into(),
        );
    }
    None
}

fn write_ahead_plan_chronology_error(goal: &Goal, baseline: &WorkspaceBaseline) -> Option<String> {
    let receipt = goal.plan_receipts.first()?;
    let created_at = match plan_timestamp("goal.created_at", &goal.created_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let baseline_at = match plan_timestamp("goal.baseline.recorded_at", &baseline.recorded_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let updated_at = match plan_timestamp("goal.updated_at", &goal.updated_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let receipt_at = match plan_timestamp("plan receipt recorded_at", &receipt.recorded_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    if baseline_at < created_at || receipt_at < baseline_at {
        return Some(
            "plan publication 时间顺序必须满足 goal <= baseline <= receipt <= published <= committed"
                .into(),
        );
    }

    // A structurally incomplete publication remains invalid after an update,
    // so chronology validation follows every timestamp that is actually
    // present without turning retirement into a repair path for other defects.
    let mut previous_end = receipt_at;
    if let Some(base_publication) = receipt.publication.as_ref() {
        let base_published_at = match plan_timestamp(
            "plan publication published_at",
            &base_publication.published_at,
        ) {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        let end = match publication_end_timestamp(base_publication) {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        if base_published_at < receipt_at || end < base_published_at {
            return Some(
                "plan publication 时间顺序必须满足 goal <= baseline <= receipt <= published <= committed"
                    .into(),
            );
        }
        previous_end = end;
    }

    for (index, extension) in receipt.extensions.iter().enumerate() {
        let recorded_at = match plan_timestamp(
            &format!("plan extension {} recorded_at", index + 1),
            &extension.recorded_at,
        ) {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        if recorded_at < previous_end {
            return Some(format!(
                "plan extension {} 时间顺序必须位于前一 publication 之后",
                index + 1
            ));
        }
        previous_end = recorded_at;
        if let Some(publication) = extension.publication.as_ref() {
            let published_at = match plan_timestamp(
                &format!("plan extension {} published_at", index + 1),
                &publication.published_at,
            ) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let end = match publication_end_timestamp(publication) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            if published_at < recorded_at || end < published_at {
                return Some(format!(
                    "plan extension {} 时间顺序必须位于前一 publication 之后",
                    index + 1
                ));
            }
            previous_end = end;
        }
    }
    if updated_at < previous_end {
        return Some("goal.updated_at 不得早于最终 plan publication".into());
    }
    None
}

fn plan_chronology_error_before_update(goal: &Goal) -> Option<String> {
    let baseline = goal.baseline.as_ref()?;
    match goal.plan_publication_policy.as_deref() {
        None => legacy_plan_chronology_error(goal, baseline),
        Some(PLAN_PUBLICATION_POLICY_V1) => write_ahead_plan_chronology_error(goal, baseline),
        Some(_) => None,
    }
}

fn ensure_plan_chronology_before_update(goal: &Goal) -> Result<()> {
    if let Some(error) = plan_chronology_error_before_update(goal) {
        bail!(error);
    }
    Ok(())
}

fn legacy_plan_publication_eligible(goal: &Goal) -> bool {
    if goal.lifecycle == GoalLifecycle::Current {
        return false;
    }
    let Some(baseline) = goal.baseline.as_ref() else {
        return false;
    };
    let Ok(created) = chrono::DateTime::parse_from_rfc3339(&goal.created_at) else {
        return false;
    };
    let rollout = chrono::DateTime::parse_from_rfc3339(PLAN_PUBLICATION_ROLLOUT_AT)
        .expect("plan publication rollout timestamp must be valid");
    created < rollout
        && timestamp_before(&baseline.recorded_at, &rollout)
        && goal.plan_receipts.iter().all(|receipt| {
            timestamp_before(&receipt.recorded_at, &rollout)
                && receipt
                    .extensions
                    .iter()
                    .all(|extension| timestamp_before(&extension.recorded_at, &rollout))
        })
}

/// Validate the entire plan chain and its write-ahead publication epoch.
///
/// Legacy v15 goals remain readable only when they predate the rollout and the
/// whole chain is legacy.  v16 never appends to them.  A governed v16 chain is
/// either fully committed, or contains exactly one pending tail node that is
/// atomically paired with the single persisted publish intent.
pub(super) fn plan_chain_error(goal: &Goal) -> Option<String> {
    let allow_unbound_retired_history = goal.lifecycle == GoalLifecycle::Archived
        && matches!(goal.status, GoalStatus::Partial | GoalStatus::Blocked);
    let Some(baseline) = goal.baseline.as_ref() else {
        if goal.plan_receipts.is_empty() && goal.plan_publish_intent.is_none() {
            return None;
        }
        return Some("缺少 baseline 的 goal 不得携带 plan publication state".into());
    };
    if goal.plan_receipts.len() > 1 {
        return Some("goal 只能携带一个不可拆分的聚合 plan receipt".into());
    }

    match goal.plan_publication_policy.as_deref() {
        None => {
            if !legacy_plan_publication_eligible(goal) {
                return Some(format!(
                    "legacy plan chain 只允许作为 rollout {PLAN_PUBLICATION_ROLLOUT_AT} 前产生且已退休的历史记录"
                ));
            }
            if goal.plan_publish_intent.is_some()
                || goal.plan_receipts.iter().any(|receipt| {
                    receipt.publication.is_some()
                        || receipt
                            .extensions
                            .iter()
                            .any(|extension| extension.publication.is_some())
                })
            {
                return Some("legacy plan chain 不得混入 v16 publication 节点或 intent".into());
            }
            if goal.plan_receipts.iter().any(|receipt| {
                receipt.plan_sha256 != plan_receipt_sha256(receipt)
                    || !plan_extensions_are_valid(receipt)
            }) {
                return Some("legacy plan chain hash 或单调扩展关系无效".into());
            }
            if let Some(error) = legacy_plan_chronology_error(goal, baseline) {
                return Some(error);
            }
            return None;
        }
        Some(PLAN_PUBLICATION_POLICY_V1) => {}
        Some(other) => {
            return Some(format!("未知 plan_publication_policy: {other}"));
        }
    }

    let Some(receipt) = goal.plan_receipts.first() else {
        return goal
            .plan_publish_intent
            .as_ref()
            .map(|_| "plan publish intent 缺少对应 pending plan 节点".into());
    };
    if receipt.baseline_fingerprint != baseline.workspace_fingerprint
        || receipt.plan_sha256 != plan_receipt_sha256(receipt)
        || !plan_extensions_are_valid(receipt)
    {
        return Some("plan chain 外层 hash、baseline 或单调扩展关系无效".into());
    }

    let pending_kind = goal.plan_publish_intent.as_ref().map(|intent| intent.kind);
    let base_state = if pending_kind == Some(PlanPublishIntentKind::Initial) {
        if !receipt.extensions.is_empty() {
            return Some("initial pending publication 后不得已有 extension".into());
        }
        PlanPublicationState::Pending
    } else {
        PlanPublicationState::Committed
    };
    let Some(base_publication) = receipt.publication.as_ref() else {
        return Some("write_ahead_v1 plan receipt 缺少 publication proof".into());
    };
    if let Some(error) = publication_error(
        PublicationExpectation {
            enclosing_goal_id: &goal.id,
            allow_unbound_retired_history,
            kind: PlanPublishIntentKind::Initial,
            baseline_fingerprint: &receipt.baseline_fingerprint,
            previous_plan_sha256: None,
            changed_paths: &receipt.changed_paths,
            review_priority: &receipt.review_priority,
            impacted_paths: &receipt.impacted_paths,
            recommended_checks: &receipt.recommended_checks,
            state: base_state,
        },
        base_publication,
    ) {
        return Some(error);
    }
    if base_publication.precheck_fingerprint != receipt.baseline_fingerprint {
        return Some("initial plan publication precheck 必须等于 goal baseline".into());
    }

    let last_extension = receipt.extensions.len().checked_sub(1);
    for (index, extension) in receipt.extensions.iter().enumerate() {
        let state = if pending_kind == Some(PlanPublishIntentKind::Extension)
            && Some(index) == last_extension
        {
            PlanPublicationState::Pending
        } else {
            PlanPublicationState::Committed
        };
        let Some(publication) = extension.publication.as_ref() else {
            return Some(format!(
                "write_ahead_v1 extension {} 缺少 publication proof",
                index + 1
            ));
        };
        if let Some(error) = publication_error(
            PublicationExpectation {
                enclosing_goal_id: &goal.id,
                allow_unbound_retired_history,
                kind: PlanPublishIntentKind::Extension,
                baseline_fingerprint: &receipt.baseline_fingerprint,
                previous_plan_sha256: Some(&extension.previous_plan_sha256),
                changed_paths: &extension.changed_paths,
                review_priority: &extension.review_priority,
                impacted_paths: &extension.impacted_paths,
                recommended_checks: &extension.recommended_checks,
                state,
            },
            publication,
        ) {
            return Some(format!("extension {}: {error}", index + 1));
        }
    }

    if let Some(intent) = goal.plan_publish_intent.as_ref() {
        if (intent.goal_id != goal.id
            && !(allow_unbound_retired_history && intent.goal_id.is_empty()))
            || intent.intent_sha256 != plan_publish_intent_sha256(intent)
            || intent.baseline_fingerprint != baseline.workspace_fingerprint
            || plan_timestamp("plan publish intent prepared_at", &intent.prepared_at).is_err()
        {
            return Some("plan publish intent hash、goal、timestamp 或 baseline 绑定无效".into());
        }
        let publication = match intent.kind {
            PlanPublishIntentKind::Initial => base_publication,
            PlanPublishIntentKind::Extension => {
                let Some(extension) = receipt.extensions.last() else {
                    return Some("extension intent 缺少 pending 链尾".into());
                };
                extension
                    .publication
                    .as_ref()
                    .expect("extension publication checked above")
            }
        };
        if publication.intent_sha256 != intent.intent_sha256
            || publication.goal_id != intent.goal_id
            || publication.precheck_fingerprint != intent.precheck_fingerprint
            || publication.published_at != intent.prepared_at
        {
            return Some("pending publication 与 persisted intent 不匹配".into());
        }
    } else if base_publication.state == PlanPublicationState::Pending
        || receipt.extensions.iter().any(|extension| {
            extension
                .publication
                .as_ref()
                .is_some_and(|proof| proof.state == PlanPublicationState::Pending)
        })
    {
        return Some("pending publication 缺少 persisted intent".into());
    }

    write_ahead_plan_chronology_error(goal, baseline)
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
        let baseline = workspace_baseline(&self.root)?;
        let updated_at = latest_timestamp([
            ("goal.created_at", now.as_str()),
            ("goal.baseline.recorded_at", baseline.recorded_at.as_str()),
        ])?;
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
            updated_at,
            baseline: Some(baseline),
            plan_receipts: Vec::new(),
            plan_publish_intent: None,
            plan_publication_policy: Some(PLAN_PUBLICATION_POLICY_V1.to_string()),
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

    pub fn record_plan(&self, id: &str, submission: PlanReceiptSubmission) -> Result<Goal> {
        self.record_plan_with_before_confirm(id, submission, || {})
    }

    fn record_plan_with_before_confirm<F>(
        &self,
        id: &str,
        mut submission: PlanReceiptSubmission,
        before_confirm: F,
    ) -> Result<Goal>
    where
        F: FnOnce(),
    {
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
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        if goal.plan_publication_policy.as_deref() != Some(PLAN_PUBLICATION_POLICY_V1) {
            bail!(
                "goal 不属于 {PLAN_PUBLICATION_POLICY_V1} plan publication epoch；旧 goal 只能读取或退休，不能追加计划"
            );
        }
        if let Some(error) = plan_chain_error(&goal) {
            bail!("goal plan publication contract invalid: {error}");
        }
        let current = workspace_baseline(&self.root)?;
        let event_at = goal_event_timestamp_after(
            &goal,
            "plan precheck baseline recorded_at",
            &current.recorded_at,
        )?;
        let mut receipt = PlanReceipt {
            recorded_at: event_at.clone(),
            baseline_fingerprint: baseline.workspace_fingerprint.clone(),
            changed_paths: submission.changed_paths.clone(),
            review_priority: submission.review_priority.clone(),
            impacted_paths: submission.impacted_paths.clone(),
            recommended_checks: submission.recommended_checks.clone(),
            publication: None,
            plan_sha256: String::new(),
            extensions: Vec::new(),
        };
        if let Some(intent) = goal.plan_publish_intent.as_ref() {
            let matching = intent.kind == PlanPublishIntentKind::Initial
                && intent.baseline_fingerprint == baseline.workspace_fingerprint
                && intent.precheck_fingerprint == current.workspace_fingerprint
                && intent.previous_plan_sha256.is_none()
                && intent.changed_paths == submission.changed_paths
                && intent.review_priority == submission.review_priority
                && intent.impacted_paths == submission.impacted_paths
                && intent.recommended_checks == submission.recommended_checks
                && intent.intent_sha256 == plan_publish_intent_sha256(intent);
            let pending_receipt = goal.plan_receipts.first();
            let receipt_matches = pending_receipt.is_some_and(|pending| {
                goal.plan_receipts.len() == 1
                    && pending.extensions.is_empty()
                    && pending.baseline_fingerprint == intent.baseline_fingerprint
                    && pending.changed_paths == intent.changed_paths
                    && pending.review_priority == intent.review_priority
                    && pending.impacted_paths == intent.impacted_paths
                    && pending.recommended_checks == intent.recommended_checks
                    && pending.publication.as_ref().is_some_and(|publication| {
                        publication.state == PlanPublicationState::Pending
                            && publication.intent_sha256 == intent.intent_sha256
                            && publication.publication_sha256
                                == plan_publication_sha256(publication)
                    })
                    && pending.plan_sha256 == plan_receipt_sha256(pending)
            });
            if !matching || !receipt_matches {
                bail!(
                    "goal 存在未完成且与本次调用不匹配的 plan publish intent；拒绝覆盖，必须恢复 intent 的 precheck 快照后用同一计划重试或退休该 goal"
                );
            }
            let pending = goal
                .plan_receipts
                .first_mut()
                .expect("checked pending plan");
            let publication = pending
                .publication
                .as_mut()
                .expect("checked pending publication");
            commit_plan_publication(publication, &current.workspace_fingerprint, &event_at)?;
            pending.plan_sha256 = plan_receipt_sha256(pending);
            goal.plan_publish_intent = None;
            goal.updated_at = event_at;
            write_json(&path, &goal)?;
            return Ok(goal);
        }
        receipt.plan_sha256 = plan_receipt_sha256(&receipt);
        if let Some(existing) = goal.plan_receipts.first() {
            if goal.plan_receipts.len() != 1 {
                bail!("目标包含多个 plan receipt；拒绝继续使用可拆分绕过的计划状态");
            }
            if existing.baseline_fingerprint == receipt.baseline_fingerprint
                && existing.changed_paths == receipt.changed_paths
                && existing.review_priority == receipt.review_priority
                && existing.impacted_paths == receipt.impacted_paths
                && existing.recommended_checks == receipt.recommended_checks
                && existing.publication.as_ref().is_some_and(|publication| {
                    publication.state == PlanPublicationState::Committed
                        && publication.publication_sha256 == plan_publication_sha256(publication)
                })
                && existing.plan_sha256 == plan_receipt_sha256(existing)
            {
                return Ok(goal);
            }
            bail!(
                "goal plan 是首次修改前的一次性聚合合同，不能追加或拆分；请在变更前一次列出完整路径"
            );
        }
        if current.workspace_fingerprint != baseline.workspace_fingerprint {
            bail!(
                "工作区已偏离 goal 开工 baseline；拒绝事后补 plan。baseline={} current={}",
                baseline.workspace_fingerprint,
                current.workspace_fingerprint
            );
        }

        let mut intent = PlanPublishIntent {
            goal_id: goal.id.clone(),
            prepared_at: event_at.clone(),
            kind: PlanPublishIntentKind::Initial,
            baseline_fingerprint: baseline.workspace_fingerprint.clone(),
            precheck_fingerprint: current.workspace_fingerprint.clone(),
            previous_plan_sha256: None,
            changed_paths: submission.changed_paths,
            review_priority: submission.review_priority,
            impacted_paths: submission.impacted_paths,
            recommended_checks: submission.recommended_checks,
            intent_sha256: String::new(),
        };
        intent.intent_sha256 = plan_publish_intent_sha256(&intent);
        receipt.publication = Some(pending_plan_publication(&intent));
        receipt.plan_sha256 = plan_receipt_sha256(&receipt);
        goal.plan_publish_intent = Some(intent);
        goal.plan_receipts.push(receipt);
        goal.updated_at = event_at;
        write_json(&path, &goal)?;

        // The write above is the plan publication linearization point.  The
        // final compare detects a source writer that raced the precheck.  On
        // drift the intent deliberately remains persisted and every normal
        // gate fails closed; silently deleting it would permit a post-hoc
        // retry against the already changed tree.
        before_confirm();
        let confirmed = workspace_baseline(&self.root)?;
        if confirmed.workspace_fingerprint != current.workspace_fingerprint {
            bail!(
                "源码在 plan 发布 CAS 窗口内发生变化；已保留 fail-closed plan publish intent（precheck={} confirmed={}），恢复原快照后用同一计划重试或退休该 goal",
                current.workspace_fingerprint,
                confirmed.workspace_fingerprint
            );
        }
        let commit_at = goal_event_timestamp_after(
            &goal,
            "plan confirmation baseline recorded_at",
            &confirmed.recorded_at,
        )?;
        let receipt = goal
            .plan_receipts
            .first_mut()
            .expect("pending plan was published");
        let publication = receipt
            .publication
            .as_mut()
            .expect("pending plan publication was published");
        commit_plan_publication(publication, &confirmed.workspace_fingerprint, &commit_at)?;
        receipt.plan_sha256 = plan_receipt_sha256(receipt);
        goal.plan_publish_intent = None;
        goal.updated_at = commit_at;
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Widen an existing aggregate plan without allowing post-hoc coverage.
    /// Already changed paths must be covered by the previous effective plan,
    /// and every newly added path must still match the goal baseline.
    pub fn extend_plan(&self, id: &str, submission: PlanReceiptSubmission) -> Result<Goal> {
        self.extend_plan_with_before_confirm(id, submission, || {})
    }

    fn extend_plan_with_before_confirm<F>(
        &self,
        id: &str,
        mut submission: PlanReceiptSubmission,
        before_confirm: F,
    ) -> Result<Goal>
    where
        F: FnOnce(),
    {
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
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        if goal.plan_publication_policy.as_deref() != Some(PLAN_PUBLICATION_POLICY_V1) {
            bail!(
                "goal 不属于 {PLAN_PUBLICATION_POLICY_V1} plan publication epoch；旧 goal 只能读取或退休，不能扩展计划"
            );
        }
        if goal.plan_receipts.len() != 1 {
            bail!("goal plan --extend 要求恰好一个基础聚合 plan receipt");
        }
        if let Some(error) = plan_chain_error(&goal) {
            bail!("goal plan publication contract invalid: {error}");
        }
        let current = workspace_baseline(&self.root)?;
        let event_at = goal_event_timestamp_after(
            &goal,
            "plan extension precheck baseline recorded_at",
            &current.recorded_at,
        )?;

        if let Some(intent) = goal.plan_publish_intent.as_ref() {
            if intent.kind != PlanPublishIntentKind::Extension
                || current.workspace_fingerprint != intent.precheck_fingerprint
            {
                bail!(
                    "goal 存在未完成且与当前源码不匹配的 plan extension intent；必须恢复 intent 的 precheck 快照后用同一扩展重试或退休该 goal"
                );
            }
            let receipt = goal.plan_receipts.first().expect("checked one plan");
            let pending_index = receipt
                .extensions
                .len()
                .checked_sub(1)
                .expect("validated extension intent has a pending tail");
            let (prior_changed, prior_impacted, prior_checks, prior_priority, prior_sha256) =
                if pending_index == 0 {
                    (
                        receipt.changed_paths.as_slice(),
                        receipt.impacted_paths.as_slice(),
                        receipt.recommended_checks.as_slice(),
                        receipt.review_priority.as_str(),
                        receipt.plan_sha256.as_str(),
                    )
                } else {
                    let prior = &receipt.extensions[pending_index - 1];
                    (
                        prior.changed_paths.as_slice(),
                        prior.impacted_paths.as_slice(),
                        prior.recommended_checks.as_slice(),
                        prior.review_priority.as_str(),
                        prior.extension_sha256.as_str(),
                    )
                };
            let prior_set = prior_changed.iter().cloned().collect::<BTreeSet<_>>();
            let additions = submission
                .changed_paths
                .iter()
                .filter(|candidate| !prior_set.contains(*candidate))
                .cloned()
                .collect::<Vec<_>>();
            let mut expected_changed = prior_changed.to_vec();
            expected_changed.extend(additions);
            normalize_path_list(&mut expected_changed);
            let mut expected_impacted = prior_impacted.to_vec();
            expected_impacted.extend(submission.impacted_paths.clone());
            normalize_path_list(&mut expected_impacted);
            let mut expected_checks = prior_checks.to_vec();
            expected_checks.extend(submission.recommended_checks.clone());
            expected_checks.sort();
            expected_checks.dedup();
            let expected_priority =
                max_review_priority(prior_priority, &submission.review_priority)?;
            let matching = intent.baseline_fingerprint == baseline.workspace_fingerprint
                && intent.previous_plan_sha256.as_deref() == Some(prior_sha256)
                && intent.changed_paths == expected_changed
                && intent.review_priority == expected_priority
                && intent.impacted_paths == expected_impacted
                && intent.recommended_checks == expected_checks
                && intent.intent_sha256 == plan_publish_intent_sha256(intent);
            if !matching {
                bail!(
                    "goal 存在未完成且与本次调用不匹配的 plan extension intent；拒绝覆盖，必须用原扩展参数重试或退休该 goal"
                );
            }
            let extension = goal
                .plan_receipts
                .first_mut()
                .expect("checked one plan")
                .extensions
                .last_mut()
                .expect("validated pending extension");
            let publication = extension
                .publication
                .as_mut()
                .expect("validated pending extension publication");
            commit_plan_publication(publication, &current.workspace_fingerprint, &event_at)?;
            extension.extension_sha256 =
                plan_extension_sha256(&baseline.workspace_fingerprint, extension);
            goal.plan_publish_intent = None;
            goal.updated_at = event_at;
            write_json(&path, &goal)?;
            return Ok(goal);
        }

        let receipt = goal.plan_receipts.first().expect("checked one plan");
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
            recorded_at: event_at.clone(),
            previous_plan_sha256,
            changed_paths,
            review_priority,
            impacted_paths,
            recommended_checks,
            publication: None,
            extension_sha256: String::new(),
        };
        let mut intent = PlanPublishIntent {
            goal_id: goal.id.clone(),
            prepared_at: event_at.clone(),
            kind: PlanPublishIntentKind::Extension,
            baseline_fingerprint: baseline.workspace_fingerprint.clone(),
            precheck_fingerprint: current.workspace_fingerprint.clone(),
            previous_plan_sha256: Some(extension.previous_plan_sha256.clone()),
            changed_paths: extension.changed_paths.clone(),
            review_priority: extension.review_priority.clone(),
            impacted_paths: extension.impacted_paths.clone(),
            recommended_checks: extension.recommended_checks.clone(),
            intent_sha256: String::new(),
        };
        intent.intent_sha256 = plan_publish_intent_sha256(&intent);
        extension.publication = Some(pending_plan_publication(&intent));
        extension.extension_sha256 =
            plan_extension_sha256(&baseline.workspace_fingerprint, &extension);
        goal.plan_publish_intent = Some(intent);
        goal.plan_receipts
            .first_mut()
            .expect("checked one plan")
            .extensions
            .push(extension);
        goal.updated_at = event_at;
        write_json(&path, &goal)?;

        before_confirm();
        let confirmed = workspace_baseline(&self.root)?;
        if confirmed.workspace_fingerprint != current.workspace_fingerprint {
            bail!(
                "源码在 plan extension 发布 CAS 窗口内发生变化；已保留 fail-closed plan publish intent（precheck={} confirmed={}），恢复原快照后用同一扩展重试或退休该 goal",
                current.workspace_fingerprint,
                confirmed.workspace_fingerprint
            );
        }
        let commit_at = goal_event_timestamp_after(
            &goal,
            "plan extension confirmation baseline recorded_at",
            &confirmed.recorded_at,
        )?;
        let extension = goal
            .plan_receipts
            .first_mut()
            .expect("checked one plan")
            .extensions
            .last_mut()
            .expect("pending extension was published");
        let publication = extension
            .publication
            .as_mut()
            .expect("pending extension publication was published");
        commit_plan_publication(publication, &confirmed.workspace_fingerprint, &commit_at)?;
        extension.extension_sha256 =
            plan_extension_sha256(&baseline.workspace_fingerprint, extension);
        goal.plan_publish_intent = None;
        goal.updated_at = commit_at;
        write_json(&path, &goal)?;
        Ok(goal)
    }

    pub fn record_review(&self, id: &str, reviewer: &str, summary: &str) -> Result<Goal> {
        if reviewer.trim().is_empty() || summary.trim().is_empty() {
            bail!("reviewer 与 summary 都不能为空");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        let event_at = goal_event_timestamp(&goal)?;
        let receipt = ReviewReceipt {
            recorded_at: event_at.clone(),
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
            goal.updated_at = event_at;
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

    /// Load an existing goal for mutation without allowing an `updated_at`
    /// refresh to repair a previously invalid plan ledger chronology.
    fn load_goal_file_for_update(path: &Path) -> Result<Option<Goal>> {
        let goal = Self::load_goal_file(path)?;
        if let Some(goal) = goal.as_ref() {
            ensure_plan_chronology_before_update(goal)?;
        }
        Ok(goal)
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
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        let impact_times = impacts
            .iter()
            .map(|impact| {
                (
                    "new impact evidence recorded_at",
                    impact.recorded_at.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let event_at = goal_event_timestamp_after_all(&goal, &impact_times)?;
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        if let Some(proof_kind) = req.proof_kind {
            bail!(
                "typed requirement {req_id} requires a matching goal validate receipt (proof_kind={})",
                proof_kind.as_str()
            );
        }
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
                    recorded_at: event_at.clone(),
                    impact_paths: impact_paths.clone(),
                    impact_scopes: impact_scopes.clone(),
                    non_code: impacts.is_empty(),
                    workspace_snapshot: false,
                    receipt: None,
                });
            }
        }
        req.impacts.extend(impacts);
        req.status = RequirementStatus::Done;
        goal.updated_at = event_at;
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
        let workspace_snapshot = authority
            .as_ref()
            .is_some_and(|receipt| receipt.workspace_snapshot);
        if evidence.trim().is_empty() {
            bail!("验证证据说明不能为空");
        }
        validate_command_for_scope(&self.root, &command, &impacts, non_code, workspace_snapshot)?;
        if workspace_snapshot && authority.is_none() {
            bail!("workspace snapshot receipt 必须是 authority receipt");
        }
        if workspace_snapshot {
            validate_authority_command(&self.root, &command)?;
        }
        let impact_paths = impacts
            .iter()
            .map(|impact| impact.changed_path.clone())
            .collect::<Vec<_>>();
        let impact_scopes = validation_scopes_for_impacts(&impacts);
        if receipt.invocation_sha256
            != validation_invocation_sha256_scoped_mode(
                &command,
                &impact_scopes,
                non_code,
                workspace_snapshot,
            )
        {
            bail!("validation receipt 与命令/影响路径不匹配");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        let current = workspace_baseline(&self.root)?;
        let plan_delta = goal_plan_delta(&goal, &current)?;
        if plan_delta.plan_required && !plan_delta.plan_recorded {
            bail!(
                "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
                plan_delta.actual_changed_paths.len()
            );
        }
        if plan_delta.plan_recorded && !plan_delta.unplanned_changed_paths.is_empty() {
            bail!(
                "validation 拒绝未计划的实际变更: {}",
                plan_delta.unplanned_changed_paths.join(", ")
            );
        }
        if workspace_snapshot && !plan_delta.actual_changed_paths.is_empty() {
            bail!(
                "workspace snapshot receipt 要求 goal baseline delta 为空；发现真实变更: {}",
                plan_delta.actual_changed_paths.join(", ")
            );
        }
        if plan_delta.plan_recorded {
            let undeclared_plan = impact_paths
                .iter()
                .map(|path| path.replace('\\', "/"))
                .filter(|changed| {
                    plan_delta
                        .planned_changed_paths
                        .binary_search(changed)
                        .is_err()
                })
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
                || authority.workspace_snapshot != workspace_snapshot
            {
                bail!("authority receipt 与 requirement/command/scope 合同不匹配");
            }
            if authority.repeat < 2 || authority.runs.len() != authority.repeat as usize {
                bail!("authority receipt 必须包含至少两次完整稳定执行");
            }
            if authority.invocation_sha256
                != authority_invocation_sha256_mode(
                    &command,
                    req_id,
                    authority.repeat,
                    &impact_scopes,
                    non_code,
                    workspace_snapshot,
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
        let mut new_event_times = impacts
            .iter()
            .map(|impact| {
                (
                    "new impact evidence recorded_at",
                    impact.recorded_at.as_str(),
                )
            })
            .collect::<Vec<_>>();
        if let Some(authority) = authority.as_ref() {
            new_event_times.push((
                "new authority receipt recorded_at",
                authority.recorded_at.as_str(),
            ));
        }
        let event_at = goal_event_timestamp_after_all(&goal, &new_event_times)?;
        let Some(req) = goal.requirements.iter_mut().find(|req| req.id == req_id) else {
            bail!("需求不存在: {req_id}");
        };
        req.evidence = Some(evidence);
        req.validations.push(ValidationEvidence {
            command,
            recorded_at: event_at.clone(),
            impact_paths,
            impact_scopes,
            non_code,
            workspace_snapshot,
            receipt: Some(receipt),
        });
        req.impacts.extend(impacts);
        req.status = RequirementStatus::Done;
        if let Some(authority) = authority {
            goal.authority_receipts.push(authority);
        }
        goal.updated_at = event_at;
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
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        goal.updated_at = goal_event_timestamp(&goal)?;
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
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
            bail!("目标不存在: {id}");
        };
        let event_at = goal_event_timestamp(&goal)?;
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
            goal.updated_at = event_at.clone();
            let fingerprint = workspace_fingerprint(&self.root)?;
            goal.lifecycle_proof = Some(issue_lifecycle_proof_at(
                &goal,
                fingerprint,
                Some(PRE_RECEIPT_MIGRATION.to_string()),
                Some(RECEIPT_POLICY_V2.to_string()),
                event_at,
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
            goal.updated_at = event_at.clone();
            goal.lifecycle_proof = Some(issue_lifecycle_proof_at(
                &goal,
                fingerprint,
                Some(RECEIPT_POLICY_V1_MIGRATION.to_string()),
                Some(RECEIPT_POLICY_V1.to_string()),
                event_at,
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
        let retiring_non_success = matches!(goal.status, GoalStatus::Partial | GoalStatus::Blocked);
        let mut retiring_legacy_success = false;
        // A pre-rollout legacy plan is intentionally invalid while current and
        // becomes readable only after retirement.  Permit exactly that
        // transition by checking the unchanged plan chain through an archived
        // view.  Preserve the ordinary guard for every other defect: archive
        // must not wash lifecycle proof corruption or repair chronology merely
        // by replacing lifecycle fields and updated_at below.
        if !retiring_non_success && let Some(error) = goal.current_schema_error() {
            let mut archived_view = goal.clone();
            archived_view.lifecycle = GoalLifecycle::Archived;
            retiring_legacy_success = goal.lifecycle_error().is_none()
                && goal.plan_publication_policy.is_none()
                && plan_chain_error(&archived_view).is_none();
            if !retiring_legacy_success {
                bail!("目标合约无效，不能归档: {error}");
            }
        }
        let current_fingerprint = workspace_fingerprint(&self.root)?;
        let mut proof_fingerprint = current_fingerprint.clone();
        let mut migration = None;
        let mut receipt_policy = Some(RECEIPT_POLICY_V2.to_string());
        let legacy_unreceipted_migration_gaps = if retiring_legacy_success {
            goal_retiring_legacy_success_unreceipted_migration_gaps(
                &goal,
                &self.root,
                &current_fingerprint,
            )
        } else {
            Vec::new()
        };
        let legacy_v1_migration_gaps = if retiring_legacy_success {
            goal_retiring_legacy_success_v1_migration_gaps(&goal, &self.root, &current_fingerprint)
        } else {
            Vec::new()
        };
        // Receipt integrity is a *success* contract: it exists so an archived
        // success can later serve as lifecycle-only authority. A goal retired as
        // partial/blocked makes no such claim and is refused by every consumer
        // of archived evidence, so demanding success receipts from it only
        // stranded abandoned work with nowhere to go.
        if !goal.loaded_from_legacy && goal.status == GoalStatus::Success {
            let gaps = if retiring_legacy_success {
                // The archived-view plan chain was validated above.  Receipt
                // integrity at the live fingerprint must still enforce current
                // command security plus baseline/plan/review reconciliation.
                // Only the known legacy plan-publication schema defect is
                // bypassed by the dedicated planning path.
                goal_success_receipt_gaps_for_retiring_legacy_success(
                    &goal,
                    &self.root,
                    &current_fingerprint,
                )
            } else {
                goal_success_receipt_gaps(&goal, &self.root, &current_fingerprint)
            };
            if !gaps.is_empty() {
                if migrate_unreceipted && pre_receipt_migration_eligible(&goal) {
                    if !legacy_unreceipted_migration_gaps.is_empty() {
                        bail!(
                            "legacy success migration 不能修复当前 command/plan/review 缺口: {}",
                            legacy_unreceipted_migration_gaps.join("; ")
                        );
                    }
                    migration = Some(PRE_RECEIPT_MIGRATION.to_string());
                } else if let Some(historical) = if retiring_legacy_success {
                    historical_success_fingerprint_for_retiring_legacy_success(
                        &goal,
                        &self.root,
                        ReceiptValidationPolicy::CurrentV2,
                        Some(&current_fingerprint),
                    )
                } else {
                    historical_success_fingerprint(
                        &goal,
                        &self.root,
                        ReceiptValidationPolicy::CurrentV2,
                    )
                } {
                    proof_fingerprint = historical;
                } else if migrate_receipt_policy == Some(RECEIPT_POLICY_V1)
                    && receipt_policy_v1_migration_eligible(&goal)
                    && let Some(historical) = if retiring_legacy_success {
                        historical_success_fingerprint_for_retiring_legacy_success(
                            &goal,
                            &self.root,
                            ReceiptValidationPolicy::LegacyV1,
                            (!legacy_v1_migration_gaps.is_empty())
                                .then_some(current_fingerprint.as_str()),
                        )
                    } else {
                        historical_success_fingerprint_excluding(
                            &goal,
                            &self.root,
                            ReceiptValidationPolicy::LegacyV1,
                            None,
                        )
                    }
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
        goal.updated_at = event_at.clone();
        goal.lifecycle_proof = Some(issue_lifecycle_proof_at(
            &goal,
            proof_fingerprint,
            migration,
            receipt_policy,
            event_at,
        ));
        if let Some(error) = goal.current_schema_error() {
            bail!("归档后的目标合约无效: {error}");
        }
        if let Some(error) = goal.lifecycle_proof_error(&self.root) {
            bail!("归档后的 lifecycle proof 无效: {error}");
        }
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Retain an invalid completed success as explicitly untrusted history.
    ///
    /// This is a one-way evidence downgrade, never a repair of the receipts.
    /// An already archived success retains its original historical fingerprint.
    /// A current pre-publication-policy success may be atomically retired only
    /// when retirement fixes its sole schema boundary and every trusted archive
    /// or migration path still fails. The requirements and validation ledger
    /// remain untouched, and every replacement/supersession consumer rejects the
    /// quarantine policy.
    pub fn quarantine_invalid_history(&self, id: &str, reason: &str) -> Result<Goal> {
        if reason.trim().is_empty() {
            bail!("隔离原因不能为空");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.status != GoalStatus::Success
            || !matches!(
                goal.lifecycle,
                GoalLifecycle::Current | GoalLifecycle::Archived
            )
        {
            bail!(
                "只允许隔离 proof 已失效的已归档 success，或无法生成可信归档 proof 的完整 current legacy success；有效或尚未结束的 current goal 不能隐藏"
            );
        }

        let (proof_fingerprint, proof_error) = if goal.lifecycle == GoalLifecycle::Current {
            let Some(current_error) = goal.current_schema_error() else {
                bail!(
                    "只允许隔离 proof 已失效的已归档 success，或无法生成可信归档 proof 的完整 current legacy success；有效或尚未结束的 current goal 不能隐藏"
                );
            };
            if let Some(error) = goal.lifecycle_error() {
                bail!("目标合约无效，不能隔离 historical receipt: {error}");
            }

            // A legacy plan is invalid while current by design.  Quarantine may
            // retire exactly that otherwise immutable record, but must not wash
            // any unrelated plan/schema defect merely by flipping lifecycle.
            let mut archived_view = goal.clone();
            archived_view.lifecycle = GoalLifecycle::Archived;
            archived_view.lifecycle_reason = Some("current success quarantine candidate".into());
            archived_view.superseded_by = None;
            archived_view.lifecycle_proof = None;
            if goal.plan_publication_policy.is_some()
                || plan_chain_error(&archived_view).is_some()
                || !integrity_quarantine_eligible(&archived_view)
            {
                bail!(
                    "目标合约无效，不能隔离 historical receipt: {error}",
                    error = current_error
                );
            }

            let current_fingerprint = workspace_fingerprint(&self.root)?;
            let current_gaps = goal_success_receipt_gaps_for_retiring_legacy_success(
                &goal,
                &self.root,
                &current_fingerprint,
            );
            let historical_v2 = historical_success_fingerprint_for_retiring_legacy_success(
                &goal,
                &self.root,
                ReceiptValidationPolicy::CurrentV2,
                Some(&current_fingerprint),
            );
            let unreceipted_migration_works = pre_receipt_migration_eligible(&goal)
                && goal_retiring_legacy_success_unreceipted_migration_gaps(
                    &goal,
                    &self.root,
                    &current_fingerprint,
                )
                .is_empty();
            let v1_current_gaps = goal_retiring_legacy_success_v1_migration_gaps(
                &goal,
                &self.root,
                &current_fingerprint,
            );
            let historical_v1 = receipt_policy_v1_migration_eligible(&goal).then(|| {
                historical_success_fingerprint_for_retiring_legacy_success(
                    &goal,
                    &self.root,
                    ReceiptValidationPolicy::LegacyV1,
                    (!v1_current_gaps.is_empty()).then_some(current_fingerprint.as_str()),
                )
            });
            if current_gaps.is_empty()
                || historical_v2.is_some()
                || unreceipted_migration_works
                || historical_v1.flatten().is_some()
            {
                bail!(
                    "current success 仍可生成可信 archive proof；拒绝降级为 quarantine，请使用普通 archive 或显式历史 receipt migration"
                );
            }

            goal = archived_view;
            (
                current_fingerprint,
                format!(
                    "{current_error}; success receipt proof invalid: {}",
                    current_gaps.join("; ")
                ),
            )
        } else {
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
            (old_proof.workspace_fingerprint, proof_error)
        };

        let previous_reason = goal
            .lifecycle_reason
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("archived success");
        let event_at = quarantine_event_timestamp(&goal);
        goal.lifecycle_reason = Some(format!(
            "{previous_reason}; quarantine: {} [invalid proof: {proof_error}]",
            reason.trim()
        ));
        goal.superseded_by = None;
        goal.lifecycle_proof = None;
        goal.updated_at = event_at.clone();
        goal.lifecycle_proof = Some(issue_lifecycle_proof_at(
            &goal,
            proof_fingerprint,
            Some(INTEGRITY_QUARANTINE_MIGRATION.to_string()),
            Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED.to_string()),
            event_at,
        ));
        if let Some(error) = goal.current_schema_error() {
            bail!("隔离后的目标合约无效: {error}");
        }
        if let Some(error) = goal.lifecycle_proof_error(&self.root) {
            bail!("隔离后的 lifecycle proof 无效: {error}");
        }
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
        let Some(mut replacement) = Self::load_goal_file_for_update(&path)? else {
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
        let fingerprint = current.workspace_fingerprint.clone();
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

        let authority_event_at = goal_event_timestamp(&authority)?;
        let predecessor_event_times = predecessors
            .iter()
            .map(goal_event_timestamp)
            .collect::<Result<Vec<_>>>()?;
        let mut causal_times = vec![
            (
                "replacement live authority recorded_at",
                live_authority.recorded_at.as_str(),
            ),
            (
                "replacement workspace baseline recorded_at",
                current.recorded_at.as_str(),
            ),
            (
                "replacement authority goal ledger",
                authority_event_at.as_str(),
            ),
        ];
        causal_times.extend(
            predecessor_event_times
                .iter()
                .map(|timestamp| ("replacement predecessor goal ledger", timestamp.as_str())),
        );
        let event_at = goal_event_timestamp_after_all(&replacement, &causal_times)?;
        let evidence = format!(
            "lifecycle-only exact must transfer authorized by archived goal {authority_goal_id}"
        );
        replacement.status = GoalStatus::Success;
        replacement.updated_at = event_at.clone();
        for requirement in &mut replacement.requirements {
            requirement.status = RequirementStatus::Done;
            requirement.evidence = Some(evidence.clone());
        }
        let mut proof = ReplacementAuthorityProof {
            recorded_at: event_at,
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
        let replacement_event_at = goal_event_timestamp(&replacement)?;

        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file_for_update(&path)? else {
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
        let event_at = goal_event_timestamp_after(
            &goal,
            "superseding replacement goal ledger",
            &replacement_event_at,
        )?;
        goal.lifecycle_proof = None;
        goal.updated_at = event_at.clone();
        goal.lifecycle_proof = Some(issue_lifecycle_proof_at(
            &goal,
            proof_fingerprint,
            None,
            Some(lifecycle_receipt_policy.to_string()),
            event_at,
        ));
        write_json(&path, &goal)?;
        Ok(goal)
    }

    /// Restore an archived/superseded record to the active readiness set.
    pub fn mark_current(&self, id: &str) -> Result<Goal> {
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(goal) = Self::load_goal_file_for_update(&path)? else {
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
        let event_at = goal_event_timestamp(&goal)?;
        let mut candidate = goal;
        candidate.lifecycle = GoalLifecycle::Current;
        candidate.lifecycle_reason = None;
        candidate.superseded_by = None;
        candidate.lifecycle_proof = None;
        // A governed pending publication is intentionally restorable so it
        // becomes a visible current blocker again. A retired legacy plan has
        // no valid current form, however, and must remain immutable history.
        if candidate.plan_publication_policy.is_none()
            && let Some(error) = plan_chain_error(&candidate)
        {
            bail!("{error}");
        }
        candidate.updated_at = event_at;
        write_json(&path, &candidate)?;
        Ok(candidate)
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
                        workspace_snapshot: false,
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
        plan_publish_intent: None,
        plan_publication_policy: None,
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
