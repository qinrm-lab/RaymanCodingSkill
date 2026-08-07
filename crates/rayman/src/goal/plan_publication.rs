//! Pure plan-publication construction and validation.
//!
//! The `GoalStore` keeps locking, workspace CAS checks, and atomic persistence;
//! this module owns the deterministic publication envelope.

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

use super::{
    PlanPublicationProof, PlanPublicationState, PlanPublishIntent, PlanPublishIntentKind,
    hash_optional_string, hash_required_string, hash_string_sequence, plan_timestamp, validation,
};

pub(super) fn plan_publication_sha256(publication: &PlanPublicationProof) -> String {
    let mut hasher = Sha256::new();
    if publication.goal_id.is_empty() {
        hasher.update(b"rayman.goal-plan-publication.v2");
    } else {
        hasher.update(b"rayman.goal-plan-publication.v3");
        hash_required_string(&mut hasher, &publication.goal_id);
    }
    hash_required_string(
        &mut hasher,
        match publication.state {
            PlanPublicationState::Pending => "pending",
            PlanPublicationState::Committed => "committed",
        },
    );
    hash_required_string(&mut hasher, &publication.intent_sha256);
    hash_required_string(&mut hasher, &publication.precheck_fingerprint);
    hash_optional_string(&mut hasher, publication.confirmed_fingerprint.as_deref());
    hash_required_string(&mut hasher, &publication.published_at);
    hash_optional_string(&mut hasher, publication.committed_at.as_deref());
    format!("{:x}", hasher.finalize())
}

pub(super) fn pending_plan_publication(intent: &PlanPublishIntent) -> PlanPublicationProof {
    let mut publication = PlanPublicationProof {
        goal_id: intent.goal_id.clone(),
        state: PlanPublicationState::Pending,
        intent_sha256: intent.intent_sha256.clone(),
        precheck_fingerprint: intent.precheck_fingerprint.clone(),
        confirmed_fingerprint: None,
        published_at: intent.prepared_at.clone(),
        committed_at: None,
        publication_sha256: String::new(),
    };
    publication.publication_sha256 = plan_publication_sha256(&publication);
    publication
}

pub(super) fn commit_plan_publication(
    publication: &mut PlanPublicationProof,
    confirmed_fingerprint: &str,
    committed_at: &str,
) -> Result<()> {
    let published_at = plan_timestamp("plan publication published_at", &publication.published_at)
        .map_err(anyhow::Error::msg)?;
    let committed = plan_timestamp("plan publication committed_at", committed_at)
        .map_err(anyhow::Error::msg)?;
    if committed < published_at {
        bail!(
            "plan publication 时间顺序必须满足 goal <= baseline <= receipt <= published <= committed"
        );
    }
    publication.state = PlanPublicationState::Committed;
    publication.confirmed_fingerprint = Some(confirmed_fingerprint.to_string());
    publication.committed_at = Some(committed_at.to_string());
    publication.publication_sha256 = plan_publication_sha256(publication);
    Ok(())
}

pub(super) fn plan_publish_intent_sha256(intent: &PlanPublishIntent) -> String {
    let mut hasher = Sha256::new();
    if intent.goal_id.is_empty() {
        hasher.update(b"rayman.goal-plan-publish-intent.v2");
    } else {
        hasher.update(b"rayman.goal-plan-publish-intent.v3");
        hash_required_string(&mut hasher, &intent.goal_id);
    }
    hash_required_string(
        &mut hasher,
        match intent.kind {
            PlanPublishIntentKind::Initial => "initial",
            PlanPublishIntentKind::Extension => "extension",
        },
    );
    hash_required_string(&mut hasher, &intent.prepared_at);
    hash_required_string(&mut hasher, &intent.baseline_fingerprint);
    hash_required_string(&mut hasher, &intent.precheck_fingerprint);
    hash_optional_string(&mut hasher, intent.previous_plan_sha256.as_deref());
    hash_required_string(&mut hasher, &intent.review_priority);
    hash_string_sequence(&mut hasher, &intent.changed_paths);
    hash_string_sequence(&mut hasher, &intent.impacted_paths);
    hash_string_sequence(&mut hasher, &intent.recommended_checks);
    format!("{:x}", hasher.finalize())
}

pub(super) struct PublicationExpectation<'a> {
    pub(super) enclosing_goal_id: &'a str,
    pub(super) allow_unbound_retired_history: bool,
    pub(super) kind: PlanPublishIntentKind,
    pub(super) baseline_fingerprint: &'a str,
    pub(super) previous_plan_sha256: Option<&'a str>,
    pub(super) changed_paths: &'a [String],
    pub(super) review_priority: &'a str,
    pub(super) impacted_paths: &'a [String],
    pub(super) recommended_checks: &'a [String],
    pub(super) state: PlanPublicationState,
}

pub(super) fn publication_error(
    expected: PublicationExpectation<'_>,
    publication: &PlanPublicationProof,
) -> Option<String> {
    if publication.goal_id != expected.enclosing_goal_id
        && !(expected.allow_unbound_retired_history && publication.goal_id.is_empty())
    {
        return Some("plan publication 未绑定 enclosing goal_id".into());
    }
    if publication.state != expected.state {
        return Some(format!(
            "plan publication state={:?}, expected={:?}",
            publication.state, expected.state
        ));
    }
    if !validation::is_sha256(&publication.intent_sha256)
        || !validation::is_sha256(&publication.precheck_fingerprint)
        || chrono::DateTime::parse_from_rfc3339(&publication.published_at).is_err()
        || publication.publication_sha256 != plan_publication_sha256(publication)
    {
        return Some("plan publication hash 或必需字段无效".into());
    }
    match publication.state {
        PlanPublicationState::Pending => {
            if publication.confirmed_fingerprint.is_some() || publication.committed_at.is_some() {
                return Some("pending plan publication 不得携带 confirmed/committed 字段".into());
            }
        }
        PlanPublicationState::Committed => {
            if publication.confirmed_fingerprint.as_deref()
                != Some(publication.precheck_fingerprint.as_str())
                || publication
                    .committed_at
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
            {
                return Some(
                    "committed plan publication 必须证明 confirmed==precheck 并记录 committed_at"
                        .into(),
                );
            }
            let published_at = chrono::DateTime::parse_from_rfc3339(&publication.published_at)
                .expect("published_at checked above");
            let committed_at = publication
                .committed_at
                .as_deref()
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok());
            if committed_at.is_none_or(|committed_at| committed_at < published_at) {
                return Some(
                    "committed plan publication 的 committed_at 必须是 RFC3339 且不早于 published_at"
                        .into(),
                );
            }
        }
    }
    let reconstructed = PlanPublishIntent {
        goal_id: publication.goal_id.clone(),
        prepared_at: publication.published_at.clone(),
        kind: expected.kind,
        baseline_fingerprint: expected.baseline_fingerprint.to_string(),
        precheck_fingerprint: publication.precheck_fingerprint.clone(),
        previous_plan_sha256: expected.previous_plan_sha256.map(str::to_string),
        changed_paths: expected.changed_paths.to_vec(),
        review_priority: expected.review_priority.to_string(),
        impacted_paths: expected.impacted_paths.to_vec(),
        recommended_checks: expected.recommended_checks.to_vec(),
        intent_sha256: String::new(),
    };
    if publication.intent_sha256 != plan_publish_intent_sha256(&reconstructed) {
        return Some("plan publication 未绑定对应 plan payload".into());
    }
    None
}
