use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStageKind {
    Installation,
    RepositoryAudit,
    SourceFresh,
    StableAuthority,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandoffStageContract {
    pub stage: HandoffStageKind,
    pub requirement_id: String,
    pub proof_kind: ProofKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandoffContract {
    pub source_goal_id: String,
    pub source_goal_contract_sha256: String,
    pub source_authority_sha256: String,
    pub git_commit: String,
    pub workspace_identity: String,
    pub workspace_fingerprint: String,
    pub created_at: String,
    pub stages: Vec<HandoffStageContract>,
    pub contract_sha256: String,
    /// Absent only on historical handoffs; never inferred from their title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_policy: Option<String>,
}

#[derive(Serialize)]
struct HandoffContractPayload<'a> {
    source_goal_id: &'a str,
    source_goal_contract_sha256: &'a str,
    source_authority_sha256: &'a str,
    git_commit: &'a str,
    workspace_identity: &'a str,
    workspace_fingerprint: &'a str,
    created_at: &'a str,
    stages: &'a [HandoffStageContract],
    #[serde(skip_serializing_if = "Option::is_none")]
    audit_policy: Option<&'a str>,
}

fn handoff_contract_sha256(contract: &HandoffContract) -> Result<String> {
    let payload = HandoffContractPayload {
        source_goal_id: &contract.source_goal_id,
        source_goal_contract_sha256: &contract.source_goal_contract_sha256,
        source_authority_sha256: &contract.source_authority_sha256,
        git_commit: &contract.git_commit,
        workspace_identity: &contract.workspace_identity,
        workspace_fingerprint: &contract.workspace_fingerprint,
        created_at: &contract.created_at,
        stages: &contract.stages,
        audit_policy: contract.audit_policy.as_deref(),
    };
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.release-handoff.v1");
    hasher.update(serde_json::to_vec(&payload)?);
    Ok(format!("{:x}", hasher.finalize()))
}

fn normalized_commit(commit: &str) -> Result<String> {
    let commit = commit.trim().to_ascii_lowercase();
    if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("handoff --commit must be an exact 40- or 64-hex Git commit");
    }
    Ok(commit)
}

fn clean_source_at_commit(root: &Path, commit: &str) -> Result<crate::source_state::SourceState> {
    let source = crate::source_state::inspect(root);
    if !source.available || source.kind != "git" {
        bail!(
            "release handoff requires an available Git repository: {}",
            source.error.as_deref().unwrap_or(&source.kind)
        );
    }
    if source.path_encoding_lossy {
        bail!("release handoff rejects lossy Git status paths");
    }
    if source.clean != Some(true) {
        bail!(
            "release handoff requires a clean Git worktree (tracked_dirty={} untracked={})",
            source.tracked_dirty,
            source.untracked
        );
    }
    if source.head.as_deref() != Some(commit) {
        bail!(
            "release handoff commit mismatch: requested={} HEAD={}",
            commit,
            source.head.as_deref().unwrap_or("unknown")
        );
    }
    Ok(source)
}

const COMPLETE_AUDIT_POLICY: &str = "complete_repository_audit_v1";

fn handoff_stage_specs(strict: bool) -> Vec<RequirementSpec> {
    let mut specs = vec![
        RequirementSpec {
            text: "installed CLI identity is verified".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(ProofKind::Installation),
        },
        RequirementSpec {
            text: "complete repository audit passes".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(if strict {
                ProofKind::RepositoryAudit
            } else {
                ProofKind::RepositoryGate
            }),
        },
        RequirementSpec {
            text: "installed release is proven source-fresh".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(ProofKind::SourceFresh),
        },
    ];
    if strict {
        specs.push(RequirementSpec {
            text: "final repository authority passes twice on the release snapshot".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(ProofKind::RepositoryGate),
        });
    }
    specs
}

fn handoff_stages(strict: bool) -> Vec<HandoffStageContract> {
    let mut stages = vec![
        HandoffStageContract {
            stage: HandoffStageKind::Installation,
            requirement_id: "req_1".into(),
            proof_kind: ProofKind::Installation,
        },
        HandoffStageContract {
            stage: HandoffStageKind::RepositoryAudit,
            requirement_id: "req_2".into(),
            proof_kind: if strict {
                ProofKind::RepositoryAudit
            } else {
                ProofKind::RepositoryGate
            },
        },
        HandoffStageContract {
            stage: HandoffStageKind::SourceFresh,
            requirement_id: "req_3".into(),
            proof_kind: ProofKind::SourceFresh,
        },
    ];
    if strict {
        stages.push(HandoffStageContract {
            stage: HandoffStageKind::StableAuthority,
            requirement_id: "req_4".into(),
            proof_kind: ProofKind::RepositoryGate,
        });
    }
    stages
}

fn expected_handoff_stages(contract: &HandoffContract) -> Option<Vec<HandoffStageContract>> {
    match contract.audit_policy.as_deref() {
        None => Some(handoff_stages(false)),
        Some(COMPLETE_AUDIT_POLICY) => Some(handoff_stages(true)),
        Some(_) => None,
    }
}

fn handoff_authority_stage_error(
    goal: &Goal,
    valid: impl Fn(&AuthorityReceipt) -> bool,
) -> Option<String> {
    let handoff = goal.handoff.as_ref()?;
    if handoff.audit_policy.as_deref() != Some(COMPLETE_AUDIT_POLICY) {
        return None;
    }
    let stage = handoff
        .stages
        .iter()
        .find(|stage| stage.stage == HandoffStageKind::StableAuthority)?;
    let requirement = goal
        .requirements
        .iter()
        .find(|requirement| requirement.id == stage.requirement_id)?;
    if requirement.status == RequirementStatus::Done
        && !goal
            .authority_receipts
            .iter()
            .any(|receipt| receipt.requirement_id == stage.requirement_id && valid(receipt))
    {
        return Some("handoff final authority receipt is missing or stale".into());
    }
    None
}

impl GoalStore {
    pub fn start_handoff(&self, source_goal_id: &str, commit: &str) -> Result<Goal> {
        let commit = normalized_commit(commit)?;
        clean_source_at_commit(&self.root, &commit)?;
        let source = self
            .get(source_goal_id)?
            .ok_or_else(|| anyhow::anyhow!("source goal does not exist: {source_goal_id}"))?;
        if source.handoff.is_some() {
            bail!("a release handoff cannot be used as its own implementation source");
        }
        // A release must start from a *current* implementation. goal_gate_verdict returns
        // only warnings (no blockers) for non-current lifecycles, so without this guard an
        // archived or superseded source passes the blocker-emptiness gate below and drives a
        // release from a retired record. The fingerprint/authority binding already forces the
        // current bytes to match the source's proven state, so this is defense-in-depth, but it
        // keeps the release aligned with the lifecycle contract (retired goals do not
        // participate in current readiness).
        if source.lifecycle != GoalLifecycle::Current {
            bail!(
                "source implementation goal {} lifecycle={}; a release handoff must start from a current goal (archive/supersede retires it from current readiness)",
                source.id,
                source.lifecycle
            );
        }
        let fingerprint = workspace_fingerprint(&self.root)?;
        let source_verdict = goal_gate_verdict(
            &source,
            std::slice::from_ref(&source),
            &self.root,
            Some(&fingerprint),
        );
        if !source_verdict.blockers.is_empty() {
            bail!(
                "source implementation goal is not gate-ready: {}",
                source_verdict.blockers.join("; ")
            );
        }
        let authority = source
            .authority_receipts
            .iter()
            .rev()
            .find(|authority| {
                direct_stable_authority_receipt_is_valid(
                    &source,
                    &self.root,
                    &fingerprint,
                    authority,
                )
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "source implementation goal lacks a current repeated stable authority receipt"
                )
            })?;
        let now = now_iso();
        let requirements = handoff_stage_specs(true)
            .into_iter()
            .enumerate()
            .map(|(index, requirement)| Requirement {
                id: format!("req_{}", index + 1),
                text: requirement.text,
                kind: requirement.kind,
                proof_kind: requirement.proof_kind,
                status: RequirementStatus::Open,
                evidence: None,
                validations: Vec::new(),
                impacts: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut handoff = HandoffContract {
            source_goal_id: source.id.clone(),
            source_goal_contract_sha256: goal_contract_sha256(&source)?,
            source_authority_sha256: authority_receipt_sha256(authority)?,
            git_commit: commit.clone(),
            workspace_identity: workspace_identity(&self.root),
            workspace_fingerprint: fingerprint,
            created_at: now.clone(),
            stages: handoff_stages(true),
            contract_sha256: String::new(),
            audit_policy: Some(COMPLETE_AUDIT_POLICY.to_string()),
        };
        handoff.contract_sha256 = handoff_contract_sha256(&handoff)?;
        let id = short_id("goal", &format!("handoff:{}:{commit}:{now}", source.id));
        let goal = Goal {
            schema_version: GOAL_SCHEMA_VERSION,
            id: id.clone(),
            title: format!("Release handoff for {}", source.id),
            status: GoalStatus::Active,
            lifecycle: GoalLifecycle::Current,
            lifecycle_reason: None,
            superseded_by: None,
            lifecycle_proof: None,
            replacement_authority: None,
            created_at: now.clone(),
            updated_at: now,
            baseline: Some(workspace_baseline(&self.root)?),
            plan_receipts: Vec::new(),
            plan_publish_intent: None,
            plan_publication_policy: Some(super::PLAN_PUBLICATION_POLICY_V1.to_string()),
            review_receipts: Vec::new(),
            authority_receipts: Vec::new(),
            work_packages: Vec::new(),
            progress_receipts: Vec::new(),
            lanes: Vec::new(),
            handoff: Some(handoff),
            requirements,
            loaded_from_legacy: false,
        };
        let goals_dir =
            state_paths::managed_state_dir(&self.root, Path::new(GOALS_RELATIVE), true)?
                .ok_or_else(|| anyhow::anyhow!("cannot create goal state directory"))?;
        let _lock = acquire_state_lock(&goals_dir.join(".store"))?;
        crate::file_io::write_json(&self.goal_path(&id)?, &goal)?;
        Ok(goal)
    }
}

pub fn handoff_contract_error(
    goal: &Goal,
    all_goals: &[Goal],
    root: &Path,
    current_fingerprint: &str,
) -> Option<String> {
    let handoff = goal.handoff.as_ref()?;
    // `None` from this function means "the contract is valid", so a `?` on a
    // hashing failure reported a passing contract instead of a blocked one.
    let Ok(expected_contract) = handoff_contract_sha256(handoff) else {
        return Some("handoff contract hash could not be computed".into());
    };
    if handoff.contract_sha256 != expected_contract {
        return Some("handoff contract hash is invalid".into());
    }
    if handoff.workspace_identity != workspace_identity(root)
        || handoff.workspace_fingerprint != current_fingerprint
    {
        return Some("handoff workspace identity or fingerprint drifted".into());
    }
    if clean_source_at_commit(root, &handoff.git_commit).is_err() {
        return Some("handoff Git HEAD or clean-state binding is no longer valid".into());
    }
    let Some(source) = all_goals
        .iter()
        .find(|candidate| candidate.id == handoff.source_goal_id)
    else {
        return Some("handoff source goal is missing".into());
    };
    if source.handoff.is_some()
        || source.status != GoalStatus::Success
        || goal_contract_sha256(source).ok().as_deref()
            != Some(handoff.source_goal_contract_sha256.as_str())
    {
        return Some("handoff source goal contract or success state changed".into());
    }
    let authority_matches = source.authority_receipts.iter().any(|authority| {
        authority_receipt_sha256(authority).ok().as_deref()
            == Some(handoff.source_authority_sha256.as_str())
            && direct_stable_authority_receipt_is_valid(
                source,
                root,
                current_fingerprint,
                authority,
            )
    });
    if !authority_matches {
        return Some("handoff source authority receipt is missing or stale".into());
    }
    let stages_match = expected_handoff_stages(handoff).as_ref() == Some(&handoff.stages)
        && handoff.stages.iter().all(|stage| {
            goal.requirements.iter().any(|requirement| {
                requirement.id == stage.requirement_id
                    && requirement.kind == RequirementKind::Must
                    && requirement.proof_kind == Some(stage.proof_kind)
            })
        });
    if !stages_match {
        return Some("handoff stage requirements do not match the structured contract".into());
    }
    handoff_authority_stage_error(goal, |authority| {
        direct_stable_authority_receipt_is_valid(goal, root, current_fingerprint, authority)
    })
}

/// Evaluate a release handoff from the caller-owned readiness capture.  The
/// write path intentionally observes live Git state; the readiness path must
/// instead bind every check to the same source snapshot as its goal receipts.
pub(crate) fn handoff_contract_error_with_context(
    goal: &Goal,
    all_goals: &[Goal],
    decision: &GoalDecisionContext<'_>,
) -> Option<String> {
    let handoff = goal.handoff.as_ref()?;
    let Ok(expected_contract) = handoff_contract_sha256(handoff) else {
        return Some("handoff contract hash could not be computed".into());
    };
    if handoff.contract_sha256 != expected_contract {
        return Some("handoff contract hash is invalid".into());
    }
    let Some(current) = decision.current() else {
        return Some("handoff captured workspace snapshot is missing".into());
    };
    if handoff.workspace_identity != decision.captured_workspace_identity().unwrap_or_default()
        || handoff.workspace_fingerprint != current.workspace_fingerprint
    {
        return Some("handoff workspace identity or fingerprint drifted".into());
    }
    let Some(source_state) = decision.captured_source() else {
        return Some("handoff captured source state is missing".into());
    };
    if !source_state.available
        || source_state.kind != "git"
        || source_state.path_encoding_lossy
        || source_state.clean != Some(true)
        || source_state.head.as_deref() != Some(handoff.git_commit.as_str())
    {
        return Some("handoff Git HEAD or clean-state binding is no longer valid".into());
    }
    let Some(source) = all_goals
        .iter()
        .find(|candidate| candidate.id == handoff.source_goal_id)
    else {
        return Some("handoff source goal is missing".into());
    };
    if source.handoff.is_some()
        || source.status != GoalStatus::Success
        || goal_contract_sha256(source).ok().as_deref()
            != Some(handoff.source_goal_contract_sha256.as_str())
    {
        return Some("handoff source goal contract or success state changed".into());
    }
    let authority_matches = source.authority_receipts.iter().any(|authority| {
        authority_receipt_sha256(authority).ok().as_deref()
            == Some(handoff.source_authority_sha256.as_str())
            && direct_stable_authority_receipt_is_valid_with_context(source, decision, authority)
    });
    if !authority_matches {
        return Some("handoff source authority receipt is missing or stale".into());
    }
    let stages_match = expected_handoff_stages(handoff).as_ref() == Some(&handoff.stages)
        && handoff.stages.iter().all(|stage| {
            goal.requirements.iter().any(|requirement| {
                requirement.id == stage.requirement_id
                    && requirement.kind == RequirementKind::Must
                    && requirement.proof_kind == Some(stage.proof_kind)
            })
        });
    if !stages_match {
        return Some("handoff stage requirements do not match the structured contract".into());
    }
    handoff_authority_stage_error(goal, |authority| {
        direct_stable_authority_receipt_is_valid_with_context(goal, decision, authority)
    })
}

#[cfg(test)]
mod audit_policy_tests {
    use super::*;

    #[test]
    fn handoff_audit_policy_preserves_legacy_hash_and_rejects_unknown_policy() {
        let mut contract = HandoffContract {
            source_goal_id: "goal_source".into(),
            source_goal_contract_sha256: "a".repeat(64),
            source_authority_sha256: "b".repeat(64),
            git_commit: "c".repeat(40),
            workspace_identity: "workspace".into(),
            workspace_fingerprint: "d".repeat(64),
            created_at: "2026-09-16T00:00:00Z".into(),
            stages: handoff_stages(false),
            contract_sha256: String::new(),
            audit_policy: None,
        };
        let old_payload = serde_json::json!({
            "source_goal_id":contract.source_goal_id,
            "source_goal_contract_sha256":contract.source_goal_contract_sha256,
            "source_authority_sha256":contract.source_authority_sha256,
            "git_commit":contract.git_commit,"workspace_identity":contract.workspace_identity,
            "workspace_fingerprint":contract.workspace_fingerprint,"created_at":contract.created_at,
            "stages":contract.stages,
        });
        // Deserialize old bytes, then require the optional policy to stay absent.
        let serialized = serde_json::to_value(&contract).unwrap();
        assert!(serialized.get("audit_policy").is_none());
        let mut old = serialized;
        old["contract_sha256"] =
            serde_json::Value::String(handoff_contract_sha256(&contract).unwrap());
        let decoded: HandoffContract = serde_json::from_value(old).unwrap();
        assert_eq!(
            decoded.contract_sha256,
            handoff_contract_sha256(&decoded).unwrap()
        );
        assert_eq!(
            serde_json::to_value(HandoffContractPayload {
                source_goal_id: &contract.source_goal_id,
                source_goal_contract_sha256: &contract.source_goal_contract_sha256,
                source_authority_sha256: &contract.source_authority_sha256,
                git_commit: &contract.git_commit,
                workspace_identity: &contract.workspace_identity,
                workspace_fingerprint: &contract.workspace_fingerprint,
                created_at: &contract.created_at,
                stages: &contract.stages,
                audit_policy: None,
            })
            .unwrap(),
            old_payload
        );
        assert_eq!(expected_handoff_stages(&contract).unwrap().len(), 3);
        let historical_hash = handoff_contract_sha256(&contract).unwrap();
        assert_eq!(
            historical_hash,
            "89041f002f3bda9b7f34282a9f1402b85529de5f1e5a6a027895a731d31d1456"
        );
        contract.audit_policy = Some(COMPLETE_AUDIT_POLICY.into());
        contract.stages = handoff_stages(true);
        assert_eq!(contract.stages[1].proof_kind, ProofKind::RepositoryAudit);
        assert_eq!(contract.stages[3].stage, HandoffStageKind::StableAuthority);
        assert_ne!(historical_hash, handoff_contract_sha256(&contract).unwrap());
        let goal: Goal = serde_json::from_value(serde_json::json!({
            "id":"goal_release", "title":"release", "status":"active",
            "created_at":"2026-09-16T00:00:00Z", "updated_at":"2026-09-16T00:00:00Z",
            "handoff":contract, "requirements":[{
                "id":"req_4","text":"stable authority","kind":"must",
                "proof_kind":"repository_gate","status":"done"
            }]
        }))
        .unwrap();
        assert!(handoff_authority_stage_error(&goal, |_| true).is_some());
        contract.audit_policy = Some("unknown".into());
        assert!(expected_handoff_stages(&contract).is_none());
    }
}
