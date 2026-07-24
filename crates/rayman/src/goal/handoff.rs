use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandoffStageKind {
    Installation,
    RepositoryAudit,
    SourceFresh,
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

fn handoff_stage_specs() -> Vec<RequirementSpec> {
    vec![
        RequirementSpec {
            text: "installed CLI identity is verified".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(ProofKind::Installation),
        },
        RequirementSpec {
            text: "repository audit passes as a stable delivery gate".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(ProofKind::RepositoryGate),
        },
        RequirementSpec {
            text: "installed release is proven source-fresh".into(),
            kind: RequirementKind::Must,
            proof_kind: Some(ProofKind::SourceFresh),
        },
    ]
}

fn handoff_stages() -> Vec<HandoffStageContract> {
    vec![
        HandoffStageContract {
            stage: HandoffStageKind::Installation,
            requirement_id: "req_1".into(),
            proof_kind: ProofKind::Installation,
        },
        HandoffStageContract {
            stage: HandoffStageKind::RepositoryAudit,
            requirement_id: "req_2".into(),
            proof_kind: ProofKind::RepositoryGate,
        },
        HandoffStageContract {
            stage: HandoffStageKind::SourceFresh,
            requirement_id: "req_3".into(),
            proof_kind: ProofKind::SourceFresh,
        },
    ]
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
        let requirements = handoff_stage_specs()
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
            stages: handoff_stages(),
            contract_sha256: String::new(),
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
    if handoff.contract_sha256 != handoff_contract_sha256(handoff).ok()? {
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
    let stages_match = handoff.stages == handoff_stages()
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
    None
}
