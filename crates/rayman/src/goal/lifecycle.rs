use super::*;

fn lifecycle_hash_bytes(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn lifecycle_hash_str(hasher: &mut Sha256, value: &str) {
    lifecycle_hash_bytes(hasher, value.as_bytes());
}

fn lifecycle_hash_optional_str(hasher: &mut Sha256, value: Option<&str>) {
    hasher.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        lifecycle_hash_str(hasher, value);
    }
}

fn lifecycle_hash_optional_u64(hasher: &mut Sha256, value: Option<u64>) {
    hasher.update([u8::from(value.is_some())]);
    if let Some(value) = value {
        hasher.update(value.to_le_bytes());
    }
}

/// Hash an explicit, versioned projection instead of serializing `Goal`.
/// New serde-default fields therefore cannot silently invalidate archived
/// proofs.  Adding a security-relevant field requires an intentional contract
/// version bump and an explicit proof refresh.
pub(super) fn legacy_lifecycle_contract_sha256(goal: &Goal) -> String {
    let mut hasher = Sha256::new();
    let extended = goal.baseline.is_some()
        || !goal.plan_receipts.is_empty()
        || !goal.review_receipts.is_empty();
    let autonomy_extended = goal
        .plan_receipts
        .iter()
        .any(|receipt| !receipt.extensions.is_empty())
        || !goal.authority_receipts.is_empty();
    let replacement_extended = goal.replacement_authority.is_some();
    let workflow_extended = !goal.work_packages.is_empty()
        || !goal.progress_receipts.is_empty()
        || !goal.lanes.is_empty();
    let publication_extended = goal.plan_publication_policy.is_some()
        || goal.plan_publish_intent.is_some()
        || goal.plan_receipts.iter().any(|receipt| {
            receipt.publication.is_some()
                || receipt
                    .extensions
                    .iter()
                    .any(|extension| extension.publication.is_some())
        });
    lifecycle_hash_str(
        &mut hasher,
        if publication_extended {
            "rayman.lifecycle-contract.v7"
        } else if workflow_extended {
            "rayman.lifecycle-contract.v6"
        } else if replacement_extended {
            "rayman.lifecycle-contract.v5"
        } else if autonomy_extended {
            "rayman.lifecycle-contract.v3"
        } else if extended {
            "rayman.lifecycle-contract.v2"
        } else {
            "rayman.lifecycle-contract.v1"
        },
    );
    hasher.update(goal.schema_version.to_le_bytes());
    lifecycle_hash_str(&mut hasher, &goal.id);
    lifecycle_hash_str(&mut hasher, &goal.title);
    lifecycle_hash_str(&mut hasher, goal.status.as_str());
    lifecycle_hash_str(&mut hasher, goal.lifecycle.as_str());
    lifecycle_hash_optional_str(&mut hasher, goal.lifecycle_reason.as_deref());
    lifecycle_hash_optional_str(&mut hasher, goal.superseded_by.as_deref());
    if publication_extended {
        lifecycle_hash_optional_str(&mut hasher, goal.plan_publication_policy.as_deref());
        hasher.update([u8::from(goal.plan_publish_intent.is_some())]);
        if let Some(intent) = goal.plan_publish_intent.as_ref() {
            lifecycle_hash_str(&mut hasher, &intent.intent_sha256);
        }
    }
    lifecycle_hash_str(&mut hasher, &goal.created_at);
    lifecycle_hash_str(&mut hasher, &goal.updated_at);
    if extended {
        if let Some(baseline) = goal.baseline.as_ref() {
            lifecycle_hash_str(&mut hasher, &baseline.recorded_at);
            lifecycle_hash_str(&mut hasher, &baseline.workspace_fingerprint);
            hasher.update((baseline.files.len() as u64).to_le_bytes());
            for (path, hash) in &baseline.files {
                lifecycle_hash_str(&mut hasher, path);
                lifecycle_hash_str(&mut hasher, hash);
            }
        }
        hasher.update((goal.plan_receipts.len() as u64).to_le_bytes());
        for receipt in &goal.plan_receipts {
            lifecycle_hash_str(&mut hasher, &receipt.recorded_at);
            lifecycle_hash_str(&mut hasher, &receipt.baseline_fingerprint);
            lifecycle_hash_str(&mut hasher, &receipt.review_priority);
            for values in [
                &receipt.changed_paths,
                &receipt.impacted_paths,
                &receipt.recommended_checks,
            ] {
                hasher.update((values.len() as u64).to_le_bytes());
                for value in values {
                    lifecycle_hash_str(&mut hasher, value);
                }
            }
            lifecycle_hash_str(&mut hasher, &receipt.plan_sha256);
            if autonomy_extended {
                hasher.update((receipt.extensions.len() as u64).to_le_bytes());
                for extension in &receipt.extensions {
                    lifecycle_hash_str(&mut hasher, &extension.recorded_at);
                    lifecycle_hash_str(&mut hasher, &extension.previous_plan_sha256);
                    lifecycle_hash_str(&mut hasher, &extension.review_priority);
                    for values in [
                        &extension.changed_paths,
                        &extension.impacted_paths,
                        &extension.recommended_checks,
                    ] {
                        hasher.update((values.len() as u64).to_le_bytes());
                        for value in values {
                            lifecycle_hash_str(&mut hasher, value);
                        }
                    }
                    lifecycle_hash_str(&mut hasher, &extension.extension_sha256);
                }
            }
        }
        hasher.update((goal.review_receipts.len() as u64).to_le_bytes());
        for receipt in &goal.review_receipts {
            lifecycle_hash_str(&mut hasher, &receipt.recorded_at);
            lifecycle_hash_str(&mut hasher, &receipt.source_fingerprint);
            lifecycle_hash_str(&mut hasher, &receipt.reviewer);
            lifecycle_hash_str(&mut hasher, &receipt.summary);
        }
        if autonomy_extended {
            hasher.update((goal.authority_receipts.len() as u64).to_le_bytes());
            for authority in &goal.authority_receipts {
                lifecycle_hash_str(&mut hasher, &authority.requirement_id);
                lifecycle_hash_str(&mut hasher, &authority.command);
                lifecycle_hash_str(&mut hasher, &authority.recorded_at);
                lifecycle_hash_str(&mut hasher, &authority.workspace_fingerprint);
                hasher.update(authority.repeat.to_le_bytes());
                hasher.update((authority.impact_scopes.len() as u64).to_le_bytes());
                for scope in &authority.impact_scopes {
                    lifecycle_hash_str(&mut hasher, &scope.changed_path);
                    lifecycle_hash_optional_str(&mut hasher, scope.package.as_deref());
                    lifecycle_hash_optional_str(&mut hasher, scope.manifest_path.as_deref());
                }
                hasher.update([u8::from(authority.non_code)]);
                if authority.workspace_snapshot {
                    hasher.update(b"rayman.workspace-snapshot-authority-lifecycle.v1");
                }
                lifecycle_hash_str(&mut hasher, &authority.invocation_sha256);
                lifecycle_hash_str(&mut hasher, &authority.contract_sha256);
                hasher.update((authority.runs.len() as u64).to_le_bytes());
                for run in &authority.runs {
                    hasher.update(run.exit_code.to_le_bytes());
                    lifecycle_hash_str(&mut hasher, &run.workspace_fingerprint_before);
                    lifecycle_hash_str(&mut hasher, &run.workspace_fingerprint_after);
                    lifecycle_hash_str(&mut hasher, &run.stdout_sha256);
                    lifecycle_hash_str(&mut hasher, &run.stderr_sha256);
                }
            }
        }
        if workflow_extended {
            hasher.update((goal.work_packages.len() as u64).to_le_bytes());
            for package in &goal.work_packages {
                lifecycle_hash_str(&mut hasher, &package.id);
                lifecycle_hash_str(&mut hasher, &package.title);
                lifecycle_hash_optional_str(&mut hasher, package.parent_id.as_deref());
                hasher.update([u8::from(package.required)]);
                lifecycle_hash_str(
                    &mut hasher,
                    match package.status {
                        WorkPackageStatus::Open => "open",
                        WorkPackageStatus::Complete => "complete",
                    },
                );
                for values in [&package.requirement_ids, &package.progress_receipt_ids] {
                    hasher.update((values.len() as u64).to_le_bytes());
                    for value in values {
                        lifecycle_hash_str(&mut hasher, value);
                    }
                }
                lifecycle_hash_optional_str(&mut hasher, package.completed_at.as_deref());
            }
            hasher.update((goal.progress_receipts.len() as u64).to_le_bytes());
            for receipt in &goal.progress_receipts {
                for value in [
                    &receipt.id,
                    &receipt.package_id,
                    &receipt.recorded_at,
                    &receipt.message,
                    &receipt.command,
                    &receipt.cwd,
                    &receipt.workspace_fingerprint_before,
                    &receipt.workspace_fingerprint_after,
                    &receipt.stdout_sha256,
                    &receipt.stderr_sha256,
                    &receipt.invocation_sha256,
                ] {
                    lifecycle_hash_str(&mut hasher, value);
                }
                hasher.update(receipt.exit_code.to_le_bytes());
                hasher.update([u8::from(receipt.authoritative)]);
            }
            hasher.update((goal.lanes.len() as u64).to_le_bytes());
            for lane in &goal.lanes {
                lifecycle_hash_str(&mut hasher, &lane.id);
                lifecycle_hash_str(
                    &mut hasher,
                    match lane.mode {
                        LaneMode::AdvisoryReadOnly => "advisory_read_only",
                        LaneMode::Writer => "writer",
                        LaneMode::FinalReviewer => "final_reviewer",
                    },
                );
                lifecycle_hash_str(&mut hasher, &lane.opened_at);
                lifecycle_hash_str(&mut hasher, &lane.opening_baseline.workspace_fingerprint);
                for values in [&lane.allowed_paths, &lane.delta_paths] {
                    hasher.update((values.len() as u64).to_le_bytes());
                    for value in values {
                        lifecycle_hash_str(&mut hasher, value);
                    }
                }
                lifecycle_hash_str(
                    &mut hasher,
                    match lane.status {
                        LaneStatus::Open => "open",
                        LaneStatus::Closed => "closed",
                    },
                );
                lifecycle_hash_optional_str(&mut hasher, lane.closed_at.as_deref());
                lifecycle_hash_optional_str(&mut hasher, lane.closing_fingerprint.as_deref());
                lifecycle_hash_optional_str(&mut hasher, lane.violation.as_deref());
                hasher.update([u8::from(lane.authoritative)]);
            }
        }
        if let Some(proof) = goal.replacement_authority.as_ref() {
            lifecycle_hash_str(&mut hasher, &proof.recorded_at);
            lifecycle_hash_str(&mut hasher, &proof.workspace_identity);
            lifecycle_hash_str(&mut hasher, &proof.workspace_fingerprint);
            lifecycle_hash_str(&mut hasher, &proof.authority_goal_id);
            lifecycle_hash_str(&mut hasher, &proof.authority_lifecycle_contract_sha256);
            lifecycle_hash_str(&mut hasher, &proof.replacement_contract_sha256);
            hasher.update((proof.predecessor_contracts.len() as u64).to_le_bytes());
            for (id, contract) in &proof.predecessor_contracts {
                lifecycle_hash_str(&mut hasher, id);
                lifecycle_hash_str(&mut hasher, contract);
            }
            hasher.update((proof.source_delta_paths.len() as u64).to_le_bytes());
            for path in &proof.source_delta_paths {
                lifecycle_hash_str(&mut hasher, path);
            }
            lifecycle_hash_str(&mut hasher, &proof.live_authority.command);
            lifecycle_hash_str(&mut hasher, &proof.live_authority.recorded_at);
            lifecycle_hash_str(&mut hasher, &proof.live_authority.workspace_fingerprint);
            hasher.update(proof.live_authority.repeat.to_le_bytes());
            lifecycle_hash_str(&mut hasher, &proof.live_authority.invocation_sha256);
            hasher.update((proof.live_authority.runs.len() as u64).to_le_bytes());
            for run in &proof.live_authority.runs {
                hasher.update(run.exit_code.to_le_bytes());
                lifecycle_hash_str(&mut hasher, &run.workspace_fingerprint_before);
                lifecycle_hash_str(&mut hasher, &run.workspace_fingerprint_after);
                lifecycle_hash_str(&mut hasher, &run.stdout_sha256);
                lifecycle_hash_str(&mut hasher, &run.stderr_sha256);
            }
            lifecycle_hash_str(&mut hasher, &proof.proof_sha256);
        }
    }
    hasher.update((goal.requirements.len() as u64).to_le_bytes());
    for requirement in &goal.requirements {
        lifecycle_hash_str(&mut hasher, &requirement.id);
        lifecycle_hash_str(&mut hasher, &requirement.text);
        lifecycle_hash_str(&mut hasher, requirement.kind.as_str());
        lifecycle_hash_str(&mut hasher, requirement.status.as_str());
        lifecycle_hash_optional_str(&mut hasher, requirement.evidence.as_deref());
        hasher.update((requirement.validations.len() as u64).to_le_bytes());
        for validation in &requirement.validations {
            lifecycle_hash_str(&mut hasher, &validation.command);
            lifecycle_hash_str(&mut hasher, &validation.recorded_at);
            hasher.update((validation.impact_paths.len() as u64).to_le_bytes());
            for path in &validation.impact_paths {
                lifecycle_hash_str(&mut hasher, path);
            }
            hasher.update((validation.impact_scopes.len() as u64).to_le_bytes());
            for scope in &validation.impact_scopes {
                lifecycle_hash_str(&mut hasher, &scope.changed_path);
                lifecycle_hash_optional_str(&mut hasher, scope.package.as_deref());
                lifecycle_hash_optional_str(&mut hasher, scope.manifest_path.as_deref());
            }
            hasher.update([u8::from(validation.non_code)]);
            if validation.workspace_snapshot {
                hasher.update(b"rayman.workspace-snapshot-validation-lifecycle.v1");
            }
            hasher.update([u8::from(validation.receipt.is_some())]);
            if let Some(receipt) = &validation.receipt {
                hasher.update(receipt.exit_code.to_le_bytes());
                lifecycle_hash_str(&mut hasher, &receipt.cwd);
                lifecycle_hash_str(&mut hasher, &receipt.workspace_fingerprint_before);
                lifecycle_hash_str(&mut hasher, &receipt.workspace_fingerprint_after);
                lifecycle_hash_str(&mut hasher, &receipt.stdout_sha256);
                lifecycle_hash_str(&mut hasher, &receipt.stderr_sha256);
                lifecycle_hash_str(&mut hasher, &receipt.invocation_sha256);
                lifecycle_hash_optional_u64(&mut hasher, receipt.passed_tests);
                lifecycle_hash_optional_u64(&mut hasher, receipt.listed_tests);
                lifecycle_hash_optional_u64(&mut hasher, receipt.ignored_tests);
                lifecycle_hash_optional_str(&mut hasher, receipt.list_stdout_sha256.as_deref());
                lifecycle_hash_optional_str(&mut hasher, receipt.list_stderr_sha256.as_deref());
                lifecycle_hash_str(&mut hasher, &receipt.contract_sha256);
            }
        }
        hasher.update((requirement.impacts.len() as u64).to_le_bytes());
        for impact in &requirement.impacts {
            lifecycle_hash_str(&mut hasher, &impact.changed_path);
            lifecycle_hash_optional_str(&mut hasher, impact.package.as_deref());
            lifecycle_hash_optional_str(&mut hasher, impact.manifest_path.as_deref());
            for values in [
                &impact.direct_dependencies,
                &impact.direct_dependents,
                &impact.candidate_tests,
                &impact.recommended_checks,
            ] {
                hasher.update((values.len() as u64).to_le_bytes());
                for value in values {
                    lifecycle_hash_str(&mut hasher, value);
                }
            }
            lifecycle_hash_str(&mut hasher, &impact.recommendation_basis);
            lifecycle_hash_str(&mut hasher, &impact.recorded_at);
        }
    }
    format!("{:x}", hasher.finalize())
}

fn has_goal_bound_plan_publication(goal: &Goal) -> bool {
    goal.plan_publish_intent
        .as_ref()
        .is_some_and(|intent| !intent.goal_id.is_empty())
        || goal.plan_receipts.iter().any(|receipt| {
            receipt
                .publication
                .as_ref()
                .is_some_and(|publication| !publication.goal_id.is_empty())
                || receipt.extensions.iter().any(|extension| {
                    extension
                        .publication
                        .as_ref()
                        .is_some_and(|publication| !publication.goal_id.is_empty())
                })
        })
}

fn lifecycle_contract_sha256(
    goal: &Goal,
    receipt_policy: Option<&str>,
    proof_recorded_at: Option<&str>,
) -> String {
    let legacy_contract = legacy_lifecycle_contract_sha256(goal);
    let Some(receipt_policy) = receipt_policy else {
        return legacy_contract;
    };
    let mut hasher = Sha256::new();
    if has_goal_bound_plan_publication(goal) {
        lifecycle_hash_str(&mut hasher, "rayman.lifecycle-proof-policy.v2");
        lifecycle_hash_str(
            &mut hasher,
            proof_recorded_at.expect("goal-bound lifecycle proof must bind recorded_at"),
        );
    } else {
        lifecycle_hash_str(&mut hasher, "rayman.lifecycle-proof-policy.v1");
    }
    lifecycle_hash_str(&mut hasher, receipt_policy);
    lifecycle_hash_str(&mut hasher, &legacy_contract);
    format!("{:x}", hasher.finalize())
}

pub(super) fn issue_lifecycle_proof_at(
    goal: &Goal,
    fingerprint: String,
    migration: Option<String>,
    receipt_policy: Option<String>,
    recorded_at: String,
) -> LifecycleProof {
    LifecycleProof {
        contract_sha256: lifecycle_contract_sha256(
            goal,
            receipt_policy.as_deref(),
            Some(&recorded_at),
        ),
        recorded_at,
        workspace_fingerprint: fingerprint,
        migration,
        receipt_policy,
    }
}

#[cfg(test)]
pub(super) fn issue_lifecycle_proof(
    goal: &Goal,
    fingerprint: String,
    migration: Option<String>,
    receipt_policy: Option<String>,
) -> LifecycleProof {
    issue_lifecycle_proof_at(goal, fingerprint, migration, receipt_policy, now_iso())
}

fn completed_current_schema_history(goal: &Goal) -> bool {
    if goal.schema_version != GOAL_SCHEMA_VERSION
        || goal.loaded_from_legacy
        || goal.status != GoalStatus::Success
    {
        return false;
    }
    if goal
        .requirements
        .iter()
        .filter(|requirement| requirement.kind == RequirementKind::Must)
        .any(|requirement| {
            requirement.status != RequirementStatus::Done
                || requirement
                    .evidence
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                || requirement.validations.is_empty()
        })
    {
        return false;
    }
    true
}

fn goal_created_before_timestamp(timestamp: &str, rollout_at: &str) -> bool {
    let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        return false;
    };
    let rollout = chrono::DateTime::parse_from_rfc3339(rollout_at)
        .expect("receipt policy rollout timestamp must be valid");
    timestamp < rollout
}

fn goal_created_before(goal: &Goal, rollout_at: &str) -> bool {
    goal_created_before_timestamp(&goal.created_at, rollout_at)
}

/// `--migrate-unreceipted` 的资格。
///
/// 文档承诺它"只适用于从来没有 receipt 的 pre-rollout 记录"，所以这里必须真的
/// 要求一条 receipt 都没有。缺了这条判定时，一个**有** receipt、且 receipt 完整性
/// 复核失败（或存在未验证 drift）的目标同样能被洗成合法归档证明——即这个 flag
/// 会把"证明失效"降级成"从来没有证明"，而后者是被无条件接受的。
pub(super) fn pre_receipt_migration_eligible(goal: &Goal) -> bool {
    completed_current_schema_history(goal)
        && goal_created_before(goal, STRICT_RECEIPT_ROLLOUT_AT)
        && goal.requirements.iter().all(|requirement| {
            requirement
                .validations
                .iter()
                .all(|validation| validation.receipt.is_none())
        })
}

pub(super) fn receipt_policy_v1_migration_eligible(goal: &Goal) -> bool {
    completed_current_schema_history(goal)
        && goal_created_before(goal, RECEIPT_POLICY_V2_ROLLOUT_AT)
}

pub(super) fn quarantined_history_eligible(goal: &Goal) -> bool {
    goal.lifecycle == GoalLifecycle::Archived
        && completed_current_schema_history(goal)
        && goal_created_before(goal, STRICT_RECEIPT_ROLLOUT_AT)
        && goal.requirements.iter().any(|requirement| {
            requirement
                .validations
                .iter()
                .any(|validation| validation.receipt.is_some())
        })
}

pub(super) fn integrity_quarantine_eligible(goal: &Goal) -> bool {
    goal.lifecycle == GoalLifecycle::Archived && completed_current_schema_history(goal)
}

pub(super) fn fingerprint_for_files(files: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    for (relative, hash) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(hash.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn workspace_baseline(root: &Path) -> Result<WorkspaceBaseline> {
    let mut files = BTreeMap::new();
    for path in crate::walk::workspace_files_checked(root)? {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        files.insert(relative, crate::hash::sha256_file(&path)?);
    }
    Ok(WorkspaceBaseline {
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint_for_files(&files),
        files,
    })
}

pub fn workspace_fingerprint(root: &Path) -> Result<String> {
    Ok(workspace_baseline(root)?.workspace_fingerprint)
}

pub fn workspace_delta(baseline: &WorkspaceBaseline, current: &WorkspaceBaseline) -> Vec<String> {
    let mut paths = baseline
        .files
        .keys()
        .chain(current.files.keys())
        .filter(|path| baseline.files.get(*path) != current.files.get(*path))
        .cloned()
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Reconcile a current workspace snapshot against the goal-owned baseline and
/// the effective aggregate plan.  Callers use this same comparison during
/// prepare, validation, and readiness checks so plan scope cannot drift
/// between lifecycle stages.
pub fn goal_plan_delta(goal: &Goal, current: &WorkspaceBaseline) -> Result<GoalPlanDelta> {
    if !goal.is_current_schema() {
        bail!(
            "目标 {} 不是当前 schema，不能作为 plan reconciliation authority",
            goal.id
        );
    }
    if let Some(error) = goal.current_schema_error() {
        bail!("goal {} 合约无效: {error}", goal.id);
    }
    goal_plan_delta_after_schema_validation(goal, current)
}

fn goal_plan_delta_for_retiring_legacy_success(
    goal: &Goal,
    current: &WorkspaceBaseline,
) -> Result<GoalPlanDelta> {
    let mut archived_view = goal.clone();
    archived_view.lifecycle = GoalLifecycle::Archived;
    if !goal.is_current_schema()
        || goal.lifecycle != GoalLifecycle::Current
        || goal.status != GoalStatus::Success
        || goal.lifecycle_error().is_some()
        || goal.plan_publication_policy.is_some()
        || plan_chain_error(&archived_view).is_some()
    {
        bail!(
            "目标 {} 不满足 retiring legacy-success plan reconciliation 条件",
            goal.id
        );
    }
    goal_plan_delta_after_schema_validation(goal, current)
}

fn goal_plan_delta_after_schema_validation(
    goal: &Goal,
    current: &WorkspaceBaseline,
) -> Result<GoalPlanDelta> {
    let Some(baseline) = goal.baseline.as_ref() else {
        bail!(
            "目标 {} 缺少开工 baseline；不能核对实际变更，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive",
            goal.id
        );
    };
    if fingerprint_for_files(&current.files) != current.workspace_fingerprint {
        bail!("当前 workspace snapshot 的文件清单与 fingerprint 不匹配");
    }

    let actual_changed_paths = workspace_delta(baseline, current);
    let planned_changed_paths = goal
        .plan_receipts
        .iter()
        .flat_map(|receipt| receipt.effective_changed_paths().iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let unplanned_changed_paths = actual_changed_paths
        .iter()
        .filter(|path| planned_changed_paths.binary_search(path).is_err())
        .cloned()
        .collect::<Vec<_>>();
    let plan_recorded = !goal.plan_receipts.is_empty();
    let plan_required = actual_changed_paths.len() >= 2;
    let covered = if plan_recorded {
        unplanned_changed_paths.is_empty()
    } else {
        !plan_required
    };

    Ok(GoalPlanDelta {
        baseline_fingerprint: baseline.workspace_fingerprint.clone(),
        current_fingerprint: current.workspace_fingerprint.clone(),
        actual_changed_paths,
        planned_changed_paths,
        unplanned_changed_paths,
        plan_recorded,
        plan_required,
        covered,
    })
}

pub fn goal_planning_gaps(goal: &Goal, root: &Path, current_fingerprint: &str) -> Vec<String> {
    goal_planning_gaps_with_policy(
        goal,
        root,
        current_fingerprint,
        false,
        Some(ReceiptValidationPolicy::CurrentV2),
    )
}

pub(super) fn goal_planning_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    goal_planning_gaps_with_policy(
        goal,
        root,
        current_fingerprint,
        true,
        Some(ReceiptValidationPolicy::CurrentV2),
    )
}

pub(super) fn goal_plan_governance_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    goal_planning_gaps_with_policy(goal, root, current_fingerprint, true, None)
}

pub(super) fn goal_v1_governance_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    goal_planning_gaps_with_policy(
        goal,
        root,
        current_fingerprint,
        true,
        Some(ReceiptValidationPolicy::LegacyV1),
    )
}

fn goal_planning_gaps_with_policy(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
    retiring_legacy_success: bool,
    validation_policy: Option<ReceiptValidationPolicy>,
) -> Vec<String> {
    let Some(baseline) = goal.baseline.as_ref() else {
        return if goal.is_current_schema() && goal.lifecycle == GoalLifecycle::Current {
            vec![
                "current goal 缺少开工 baseline；不能作为当前成功证据，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive"
                    .into(),
            ]
        } else {
            Vec::new()
        };
    };
    let mut gaps = Vec::new();
    if fingerprint_for_files(&baseline.files) != baseline.workspace_fingerprint {
        gaps.push("goal baseline 文件清单与 fingerprint 不匹配".into());
        return gaps;
    }
    let current = match workspace_baseline(root) {
        Ok(current) => current,
        Err(error) => {
            gaps.push(format!("无法计算 goal 实际变更集: {error}"));
            return gaps;
        }
    };
    if current.workspace_fingerprint != current_fingerprint {
        gaps.push("goal 规划检查与调用方当前 fingerprint 不一致".into());
        return gaps;
    }
    let delta = match if retiring_legacy_success {
        goal_plan_delta_for_retiring_legacy_success(goal, &current)
    } else {
        goal_plan_delta(goal, &current)
    } {
        Ok(delta) => delta,
        Err(error) => {
            gaps.push(format!("无法核对 goal plan: {error}"));
            return gaps;
        }
    };
    let actual = &delta.actual_changed_paths;
    if delta.plan_required && !delta.plan_recorded {
        gaps.push(format!(
            "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
            actual.len()
        ));
    }
    if delta.plan_recorded && !delta.unplanned_changed_paths.is_empty() {
        gaps.push(format!(
            "实际变更超出 plan: {}",
            delta.unplanned_changed_paths.join(", ")
        ));
    }

    if let Some(validation_policy) = validation_policy {
        let mut validated = BTreeSet::new();
        for requirement in &goal.requirements {
            let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
                continue;
            };
            for validation in &requirement.validations {
                if validation_has_receipt_for_fingerprint(
                    validation,
                    root,
                    current_fingerprint,
                    &contract_sha256,
                    true,
                    validation_policy,
                ) {
                    validated.extend(
                        validation
                            .impact_scopes
                            .iter()
                            .map(|scope| scope.changed_path.replace('\\', "/")),
                    );
                }
            }
        }
        let undeclared = actual
            .iter()
            .filter(|path| !validated.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        if !undeclared.is_empty() {
            gaps.push(format!(
                "实际变更未被当前 validation receipt 声明: {}",
                undeclared.join(", ")
            ));
        }
    }

    if goal
        .plan_receipts
        .iter()
        .any(|receipt| receipt.effective_review_priority() == "high")
        && !goal
            .review_receipts
            .iter()
            .any(|receipt| receipt.source_fingerprint == current_fingerprint)
    {
        gaps.push("high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt".into());
    }
    gaps
}

/// 单个目标在 standard 门禁下的判定。
pub struct GoalGateVerdict {
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
}

/// standard 档对单个目标的完整门禁判定。
///
/// `check`/`finish` 与 autosave 的"工作是否已完成"必须共用这一份：此前 autosave
/// 手工复刻了同一套语义，两边因此可以独立漂移——整目标差量门禁只加到了 check
/// 一侧，autosave 就会在 check 判定未就绪的状态下认为工作已完成并自停快照。
pub fn goal_gate_verdict(
    goal: &Goal,
    all_goals: &[Goal],
    root: &Path,
    current_fingerprint: Option<&str>,
) -> GoalGateVerdict {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();

    if let Some(error) = goal.current_schema_error() {
        blockers.push(format!("goal {} 合约无效: {error}", goal.id));
        return GoalGateVerdict { blockers, warnings };
    }
    if let Some(error) = goal.lifecycle_proof_error(root) {
        blockers.push(format!("goal {} lifecycle proof 无效: {error}", goal.id));
        return GoalGateVerdict { blockers, warnings };
    }
    if let Some(fingerprint) = current_fingerprint
        && let Some(error) = supersession_error(goal, all_goals, root, fingerprint)
    {
        blockers.push(format!("goal {} supersession 合约无效: {error}", goal.id));
        return GoalGateVerdict { blockers, warnings };
    }
    if goal.lifecycle != GoalLifecycle::Current {
        warnings.push(format!(
            "historical goal {} lifecycle={} 已保留但不参与当前 readiness{}",
            goal.id,
            goal.lifecycle,
            goal.superseded_by
                .as_deref()
                .map(|id| format!("（superseded_by={id}）"))
                .unwrap_or_default()
        ));
        return GoalGateVerdict { blockers, warnings };
    }
    if goal.loaded_from_legacy {
        blockers.push(format!(
            "legacy goal {} 仍为 current（status={}）；legacy 记录不能生成当前 receipt，请显式 archive 历史 success，或新建 current-schema replacement 后 supersede",
            goal.id, goal.status
        ));
        return GoalGateVerdict { blockers, warnings };
    }
    if goal.handoff.is_some() {
        let Some(fingerprint) = current_fingerprint else {
            blockers.push(format!(
                "goal {} handoff cannot be verified without a source fingerprint",
                goal.id
            ));
            return GoalGateVerdict { blockers, warnings };
        };
        if let Some(error) = handoff_contract_error(goal, all_goals, root, fingerprint) {
            blockers.push(format!(
                "goal {} handoff contract invalid: {error}",
                goal.id
            ));
            return GoalGateVerdict { blockers, warnings };
        }
    }

    let requires_receipt = goal.is_current_schema();
    let lifecycle_only = if goal.replacement_authority.is_some() {
        let Some(fingerprint) = current_fingerprint else {
            blockers.push(format!(
                "goal {} lifecycle-only replacement 无法在缺少 source fingerprint 时验证",
                goal.id
            ));
            return GoalGateVerdict { blockers, warnings };
        };
        if let Some(error) = replacement_authority_error(goal, root, fingerprint) {
            blockers.push(format!(
                "goal {} lifecycle-only replacement proof 无效: {error}",
                goal.id
            ));
            return GoalGateVerdict { blockers, warnings };
        }
        true
    } else {
        false
    };
    match goal.status {
        GoalStatus::Success => {}
        GoalStatus::Active => blockers.push(format!(
            "goal {} 仍为 active；用 goal validate 记录实际验证后必须 goal close",
            goal.id
        )),
        GoalStatus::Partial | GoalStatus::Blocked => blockers.push(format!(
            "goal {} 状态为 {}，不能作为 standard READY",
            goal.id, goal.status
        )),
    }
    for req in &goal.requirements {
        let is_must = req.kind == RequirementKind::Must;
        if goal.status == GoalStatus::Active && is_must && req.status != RequirementStatus::Done {
            blockers.push(format!(
                "active goal {} 的 must 需求 {} 仍未完成",
                goal.id, req.id
            ));
        }
        if goal.status == GoalStatus::Success && is_must && req.status != RequirementStatus::Done {
            blockers.push(format!(
                "success goal {} 的 must 需求 {} 未处于 done 状态",
                goal.id, req.id
            ));
        }
        if req.status == RequirementStatus::Done
            && req
                .evidence
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            blockers.push(format!(
                "goal {} 需求 {} 缺少 evidence 文本",
                goal.id, req.id
            ));
        }
        if !lifecycle_only && req.status == RequirementStatus::Done && req.validations.is_empty() {
            blockers.push(format!("goal {} 需求 {} 缺少验证 receipt", goal.id, req.id));
        }
        if !req.impacts.is_empty()
            && let Some(fingerprint) = current_fingerprint
        {
            for gap in validation_relevance_gaps(req, goal, root, fingerprint) {
                blockers.push(format!("goal {} 需求 {} {gap}", goal.id, req.id));
            }
        }
        if !lifecycle_only && requires_receipt && goal.status == GoalStatus::Success && is_must {
            let has_current_receipt = req.validations.iter().any(|validation| {
                current_fingerprint.is_some_and(|fingerprint| {
                    validation_has_current_receipt(validation, goal, req, root, fingerprint)
                })
            });
            if !has_current_receipt {
                blockers.push(format!(
                    "success goal {} 的 must 需求 {} 没有绑定当前工作区的成功 validation receipt",
                    goal.id, req.id
                ));
            }
        }
        if req.status == RequirementStatus::Done
            && req.impacts.is_empty()
            && !req.validations.is_empty()
        {
            warnings.push(format!(
                "goal {} 需求 {} 没有 impact 快照；非代码变更可忽略",
                goal.id, req.id
            ));
        }
    }

    // 整目标级的规划/差量门禁：baseline 缺失、超出 plan 的实际变更、未被任何当前
    // receipt 声明的变更、high-priority plan 的 review 绑定。这些规则曾只在
    // `goal close` 内生效，而 close 不会重置 status，已关闭的 success 目标可以
    // 原地反复重新验证，于是它们全部逃过了交付门禁。
    if !lifecycle_only
        && requires_receipt
        && goal.status == GoalStatus::Success
        && let Some(fingerprint) = current_fingerprint
    {
        for gap in goal_planning_gaps(goal, root, fingerprint) {
            blockers.push(format!("goal {} {gap}", goal.id));
        }
    }

    GoalGateVerdict { blockers, warnings }
}

impl Goal {
    pub fn is_current_schema(&self) -> bool {
        self.schema_version == GOAL_SCHEMA_VERSION && !self.loaded_from_legacy
    }

    pub fn lifecycle_error(&self) -> Option<String> {
        match self.lifecycle {
            GoalLifecycle::Current => {
                if self.lifecycle_reason.is_some()
                    || self.superseded_by.is_some()
                    || self.lifecycle_proof.is_some()
                {
                    Some(
                        "current goal 不能保留 lifecycle_reason、superseded_by 或 lifecycle_proof"
                            .into(),
                    )
                } else {
                    None
                }
            }
            GoalLifecycle::Archived => {
                // Archiving retires a record from readiness; it asserts nothing
                // about completion. A goal that was honestly closed `partial` or
                // `blocked` may therefore be retired too — otherwise abandoned
                // work has no disposal path at all, because `supersede` needs a
                // replacement that is already gate-ready success. Every consumer
                // of an archived record (lifecycle-only authority, replacement
                // authorization, historical-receipt quarantine) additionally
                // requires `status == Success`, so this can never mint evidence.
                if !matches!(
                    self.status,
                    GoalStatus::Success | GoalStatus::Partial | GoalStatus::Blocked
                ) {
                    Some("只有 success/partial/blocked goal 可以 archived".into())
                } else if self
                    .lifecycle_reason
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                {
                    Some("archived goal 必须记录非空 lifecycle_reason".into())
                } else if self.superseded_by.is_some() {
                    Some("archived goal 不能设置 superseded_by".into())
                } else if self.lifecycle_proof.is_none() {
                    Some("archived goal 缺少 lifecycle_proof".into())
                } else {
                    None
                }
            }
            GoalLifecycle::Superseded => {
                if self
                    .superseded_by
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                {
                    Some("superseded goal 必须记录 superseded_by".into())
                } else if self
                    .lifecycle_reason
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                {
                    Some("superseded goal 必须记录 lifecycle_reason".into())
                } else if self.lifecycle_proof.is_none() {
                    Some("superseded goal 缺少 lifecycle_proof".into())
                } else {
                    None
                }
            }
        }
    }

    /// Validate the persisted contract before a readiness gate trusts it.  The
    /// CLI cannot be the only enforcement point: a hand-written or corrupted
    /// JSON file can otherwise claim `success` with no mandatory requirement.
    /// Legacy records are deliberately handled by the migration branch in the
    /// caller and are not reinterpreted as v2.
    pub fn current_schema_error(&self) -> Option<String> {
        if let Some(error) = self.lifecycle_error() {
            return Some(error);
        }
        if self.loaded_from_legacy {
            return None;
        }
        if self.schema_version != GOAL_SCHEMA_VERSION {
            return Some(format!(
                "不支持的 goal schema_version={}（当前只接受 v{}；请迁移或重新创建目标）",
                self.schema_version, GOAL_SCHEMA_VERSION
            ));
        }
        if self.id.trim().is_empty() || self.title.trim().is_empty() {
            return Some("goal id 或标题为空".into());
        }
        let mut ids = BTreeSet::new();
        let mut must_count = 0usize;
        for requirement in &self.requirements {
            if requirement.id.trim().is_empty() || requirement.text.trim().is_empty() {
                return Some("goal 包含空的 requirement id 或文本".into());
            }
            if !ids.insert(requirement.id.as_str()) {
                return Some(format!("goal 包含重复 requirement id: {}", requirement.id));
            }
            if requirement.kind == RequirementKind::Must {
                must_count += 1;
            }
        }
        if must_count == 0 {
            return Some("goal 至少需要一个 must 需求".into());
        }
        if let Some(error) = work_package_graph_error(self) {
            return Some(error);
        }
        if let Some(error) = lane_ledger_error(self) {
            return Some(error);
        }
        if self.status == GoalStatus::Success
            && let Some(package) = self.work_packages.iter().find(|package| {
                package.required
                    && (package.status != WorkPackageStatus::Complete
                        || package.progress_receipt_ids.is_empty())
            })
        {
            return Some(format!(
                "required work package {} 未完成或缺少 progress receipt",
                package.id
            ));
        }
        if self.status == GoalStatus::Success
            && let Some(lane) = self
                .lanes
                .iter()
                .find(|lane| lane.status == LaneStatus::Open)
        {
            return Some(format!("lane {} 尚未关闭", lane.id));
        }
        if let Some(baseline) = self.baseline.as_ref() {
            if !is_sha256(&baseline.workspace_fingerprint)
                || fingerprint_for_files(&baseline.files) != baseline.workspace_fingerprint
            {
                return Some("goal baseline fingerprint 与文件清单不匹配".into());
            }
            if self.plan_receipts.len() > 1 {
                return Some("goal 只能携带一个不可拆分的聚合 plan receipt".into());
            }
            for receipt in &self.plan_receipts {
                let mut changed = receipt.changed_paths.clone();
                let mut impacted = receipt.impacted_paths.clone();
                normalize_path_list(&mut changed);
                normalize_path_list(&mut impacted);
                if changed.is_empty()
                    || changed != receipt.changed_paths
                    || impacted != receipt.impacted_paths
                    || receipt.baseline_fingerprint != baseline.workspace_fingerprint
                    || !matches!(
                        receipt.review_priority.as_str(),
                        "normal" | "broad" | "high"
                    )
                {
                    return Some("goal plan receipt 无效、未规范化或未绑定 baseline".into());
                }
            }
            for receipt in &self.review_receipts {
                if !is_sha256(&receipt.source_fingerprint)
                    || receipt.reviewer.trim().is_empty()
                    || receipt.summary.trim().is_empty()
                {
                    return Some("goal review receipt 无效".into());
                }
            }
            for authority in &self.authority_receipts {
                let Ok(contract_sha256) =
                    validation_contract_sha256(self, &authority.requirement_id)
                else {
                    return Some("authority receipt 指向未知 requirement".into());
                };
                if authority.repeat < 2
                    || authority.runs.len() != authority.repeat as usize
                    || !authority_scope_is_well_formed(authority)
                    || authority.contract_sha256 != contract_sha256
                    || !is_sha256(&authority.workspace_fingerprint)
                    || authority.invocation_sha256
                        != authority_invocation_sha256_mode(
                            &authority.command,
                            &authority.requirement_id,
                            authority.repeat,
                            &authority.impact_scopes,
                            authority.non_code,
                            authority.workspace_snapshot,
                        )
                    || authority.runs.iter().any(|run| {
                        run.exit_code != 0
                            || run.workspace_fingerprint_before != authority.workspace_fingerprint
                            || run.workspace_fingerprint_after != authority.workspace_fingerprint
                            || !is_sha256(&run.stdout_sha256)
                            || !is_sha256(&run.stderr_sha256)
                    })
                {
                    return Some("authority receipt 未证明重复稳定执行或摘要无效".into());
                }
            }
        } else if !self.plan_receipts.is_empty()
            || self.plan_publish_intent.is_some()
            || !self.review_receipts.is_empty()
            || !self.authority_receipts.is_empty()
            || !self.work_packages.is_empty()
            || !self.progress_receipts.is_empty()
            || !self.lanes.is_empty()
        {
            return Some(
                "缺少 baseline 的 goal 不能携带 plan/review/authority/work-package receipt".into(),
            );
        }
        if let Some(proof) = self.replacement_authority.as_ref() {
            if self.status != GoalStatus::Success {
                return Some("lifecycle-only replacement 必须是 success".into());
            }
            if self.baseline.is_none()
                || proof.recorded_at.trim().is_empty()
                || proof.authority_goal_id.trim().is_empty()
                || proof.predecessor_contracts.is_empty()
                || proof.predecessor_contracts.contains_key(&self.id)
                || proof
                    .predecessor_contracts
                    .contains_key(&proof.authority_goal_id)
                || !is_sha256(&proof.workspace_identity)
                || !is_sha256(&proof.workspace_fingerprint)
                || !is_sha256(&proof.authority_lifecycle_contract_sha256)
                || !is_sha256(&proof.replacement_contract_sha256)
                || !is_sha256(&proof.proof_sha256)
                || proof.live_authority.command.trim().is_empty()
                || proof.live_authority.recorded_at.trim().is_empty()
                || !is_sha256(&proof.live_authority.workspace_fingerprint)
                || !is_sha256(&proof.live_authority.invocation_sha256)
                || proof
                    .predecessor_contracts
                    .iter()
                    .any(|(id, contract)| id.trim().is_empty() || !is_sha256(contract))
            {
                return Some("lifecycle-only replacement proof 结构或摘要无效".into());
            }
        }
        if let Some(error) = plan_chain_error(self) {
            return Some(format!("goal plan publication contract invalid: {error}"));
        }
        let retired_pending_publication = self.lifecycle == GoalLifecycle::Archived
            && matches!(self.status, GoalStatus::Partial | GoalStatus::Blocked);
        if let Some(intent) = self.plan_publish_intent.as_ref()
            && !retired_pending_publication
        {
            return Some(format!(
                "goal 存在未完成的 plan publish intent（kind={:?} intent_sha256={}）；源码可能在计划发布窗口内漂移，必须恢复原快照后重试或退休该 goal",
                intent.kind, intent.intent_sha256
            ));
        }
        None
    }

    pub fn lifecycle_proof_error(&self, root: &Path) -> Option<String> {
        if self.lifecycle == GoalLifecycle::Current {
            return None;
        }
        let Some(proof) = self.lifecycle_proof.as_ref() else {
            return Some("缺少 lifecycle_proof".into());
        };
        if !is_sha256(&proof.workspace_fingerprint) || !is_sha256(&proof.contract_sha256) {
            return Some("lifecycle_proof 包含非法摘要".into());
        }
        let proof_recorded_at = match chrono::DateTime::parse_from_rfc3339(&proof.recorded_at) {
            Ok(value) => value,
            Err(_) => return Some("lifecycle_proof.recorded_at 必须是 RFC3339 timestamp".into()),
        };
        let goal_updated_at = match chrono::DateTime::parse_from_rfc3339(&self.updated_at) {
            Ok(value) => value,
            Err(_) => return Some("goal.updated_at 必须是 RFC3339 timestamp".into()),
        };
        if proof_recorded_at < goal_updated_at {
            return Some("lifecycle_proof.recorded_at 不得早于 goal.updated_at".into());
        }
        let expected = lifecycle_contract_sha256(
            self,
            proof.receipt_policy.as_deref(),
            Some(&proof.recorded_at),
        );
        if proof.contract_sha256 != expected {
            return Some("lifecycle_proof 与当前 goal 合约不匹配".into());
        }
        if proof.receipt_policy.as_deref() == Some(RECEIPT_POLICY_QUARANTINED) {
            return if proof.migration.as_deref() == Some(QUARANTINED_HISTORY_MIGRATION)
                && quarantined_history_eligible(self)
            {
                None
            } else {
                Some("lifecycle_proof 使用了无效的 legacy quarantine".into())
            };
        }
        if proof.receipt_policy.as_deref() == Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED) {
            return if proof.migration.as_deref() == Some(INTEGRITY_QUARANTINE_MIGRATION)
                && integrity_quarantine_eligible(self)
            {
                None
            } else {
                Some("lifecycle_proof 使用了无效的 receipt integrity quarantine".into())
            };
        }
        if proof.receipt_policy.as_deref() == Some(VERIFIED_REPLACEMENT_TRANSFER_POLICY) {
            return if self.lifecycle == GoalLifecycle::Superseded
                && self.status == GoalStatus::Success
                && !self.loaded_from_legacy
                && proof.migration.is_none()
            {
                None
            } else {
                Some("verified replacement transfer 只允许无额外 migration 的 superseded current-schema success".into())
            };
        }
        let policy = match proof.receipt_policy.as_deref() {
            None if goal_created_before(self, RECEIPT_POLICY_V2_ROLLOUT_AT) => {
                ReceiptValidationPolicy::LegacyV1
            }
            None => ReceiptValidationPolicy::CurrentV2,
            Some(RECEIPT_POLICY_V1) => ReceiptValidationPolicy::LegacyV1,
            Some(RECEIPT_POLICY_V2) => ReceiptValidationPolicy::CurrentV2,
            Some(other) => return Some(format!("未知 lifecycle receipt policy: {other}")),
        };
        if self.status == GoalStatus::Success && !self.loaded_from_legacy {
            if let Some(migration) = proof.migration.as_deref() {
                match migration {
                    PRE_RECEIPT_MIGRATION if pre_receipt_migration_eligible(self) => return None,
                    RECEIPT_POLICY_V1_MIGRATION
                        if policy == ReceiptValidationPolicy::LegacyV1
                            && receipt_policy_v1_migration_eligible(self) => {}
                    _ => return Some("lifecycle_proof 使用了无效的历史迁移".into()),
                }
            } else if proof.receipt_policy.as_deref() == Some(RECEIPT_POLICY_V1) {
                return Some("显式 v1 receipt policy proof 缺少受控迁移标记".into());
            }
            let gaps = goal_success_receipt_gaps_for_policy(
                self,
                root,
                &proof.workspace_fingerprint,
                false,
                policy,
            );
            if !gaps.is_empty() {
                return Some(format!(
                    "历史化时的 success receipt proof 无效: {}",
                    gaps.join("; ")
                ));
            }
        }
        None
    }
}

fn normalized_requirement_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(super) fn workspace_identity(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    crate::hash::sha256_bytes(canonical.to_string_lossy().as_bytes())
}

/// must 转移比较键。typed proof 义务是不可变合约的一部分（自 v3 起
/// transfer_goal_contract_sha256 与 replacement_contract_sha256 都把 proof_kind
/// 计入哈希——在那之前这条注释是错的，两个哈希都只覆盖 id/text/kind，所以事后
/// 剥掉 predecessor 的 `--must-proof` 不会破坏任何被 pin 住的哈希，而这里的实时
/// 比较随即就会接受它先前拒绝的普通 must）。文本同名的普通 must 不得顶替 typed
/// must，否则卡在 installation/repository_gate 等 typed 阶段的目标可以被一条
/// generic receipt 洗掉义务。`None` 与 `Some(Generic)` 在校验语义里等价
/// （见 proof_kind_matches），共用一键。
pub(super) fn must_transfer_key(requirement: &Requirement) -> (&'static str, String) {
    (
        requirement.proof_kind.unwrap_or_default().as_str(),
        normalized_requirement_text(&requirement.text),
    )
}

pub(super) fn must_transfer_multiset<'a>(
    goals: impl IntoIterator<Item = &'a Goal>,
) -> BTreeMap<(&'static str, String), usize> {
    let mut result = BTreeMap::new();
    for goal in goals {
        for requirement in goal
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == RequirementKind::Must)
        {
            *result.entry(must_transfer_key(requirement)).or_default() += 1;
        }
    }
    result
}

/// v3 adds `proof_kind`. v2 hashed only id/text/kind, so the hash whose whole
/// purpose is pinning "the exact mandatory contract of the named unfinished
/// goals" did not pin their typed proof obligations: stripping `--must-proof`
/// from a predecessor after the fact left every pinned hash intact, and the
/// live must-transfer comparison — which does key on proof_kind — then matched
/// a plain replacement it had previously refused.
pub(super) fn transfer_goal_contract_sha256(goal: &Goal) -> String {
    let mut hasher = Sha256::new();
    lifecycle_hash_str(
        &mut hasher,
        if goal.plan_publication_policy.is_some() {
            "rayman.transfer-goal-contract.v4"
        } else {
            "rayman.transfer-goal-contract.v3"
        },
    );
    hasher.update(goal.schema_version.to_le_bytes());
    if goal.plan_publication_policy.is_some() {
        lifecycle_hash_optional_str(&mut hasher, goal.plan_publication_policy.as_deref());
    }
    lifecycle_hash_str(&mut hasher, &goal.id);
    lifecycle_hash_str(&mut hasher, &goal.title);
    lifecycle_hash_str(&mut hasher, &goal.created_at);
    hasher.update([u8::from(goal.baseline.is_some())]);
    if let Some(baseline) = goal.baseline.as_ref() {
        lifecycle_hash_str(&mut hasher, &baseline.workspace_fingerprint);
    }
    hasher.update((goal.plan_receipts.len() as u64).to_le_bytes());
    for plan in &goal.plan_receipts {
        lifecycle_hash_str(&mut hasher, &plan.plan_sha256);
        hasher.update((plan.extensions.len() as u64).to_le_bytes());
        for extension in &plan.extensions {
            lifecycle_hash_str(&mut hasher, &extension.extension_sha256);
        }
    }
    let must = goal
        .requirements
        .iter()
        .filter(|requirement| requirement.kind == RequirementKind::Must)
        .collect::<Vec<_>>();
    hasher.update((must.len() as u64).to_le_bytes());
    for requirement in must {
        lifecycle_hash_str(&mut hasher, &requirement.id);
        lifecycle_hash_str(&mut hasher, &requirement.text);
        lifecycle_hash_str(&mut hasher, requirement.kind.as_str());
        lifecycle_hash_str(
            &mut hasher,
            requirement.proof_kind.unwrap_or_default().as_str(),
        );
    }
    format!("{:x}", hasher.finalize())
}

/// v2 adds `proof_kind`, for the same reason as
/// [`transfer_goal_contract_sha256`]: a replacement's own typed obligations
/// must be pinned by the proof that certifies it.
pub(super) fn replacement_contract_sha256(goal: &Goal) -> String {
    let mut hasher = Sha256::new();
    lifecycle_hash_str(
        &mut hasher,
        if goal.plan_publication_policy.is_some() {
            "rayman.lifecycle-only-replacement-contract.v3"
        } else {
            "rayman.lifecycle-only-replacement-contract.v2"
        },
    );
    hasher.update(goal.schema_version.to_le_bytes());
    if goal.plan_publication_policy.is_some() {
        lifecycle_hash_optional_str(&mut hasher, goal.plan_publication_policy.as_deref());
    }
    lifecycle_hash_str(&mut hasher, &goal.id);
    lifecycle_hash_str(&mut hasher, &goal.title);
    lifecycle_hash_str(&mut hasher, goal.status.as_str());
    lifecycle_hash_str(&mut hasher, &goal.created_at);
    if let Some(baseline) = goal.baseline.as_ref() {
        lifecycle_hash_str(&mut hasher, &baseline.workspace_fingerprint);
    }
    hasher.update((goal.requirements.len() as u64).to_le_bytes());
    for requirement in &goal.requirements {
        lifecycle_hash_str(&mut hasher, &requirement.id);
        lifecycle_hash_str(&mut hasher, &requirement.text);
        lifecycle_hash_str(&mut hasher, requirement.kind.as_str());
        lifecycle_hash_str(
            &mut hasher,
            requirement.proof_kind.unwrap_or_default().as_str(),
        );
        lifecycle_hash_str(&mut hasher, requirement.status.as_str());
        lifecycle_hash_optional_str(&mut hasher, requirement.evidence.as_deref());
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn replacement_authority_proof_sha256(proof: &ReplacementAuthorityProof) -> String {
    let mut hasher = Sha256::new();
    lifecycle_hash_str(
        &mut hasher,
        if proof.live_authority.command_rebind.is_some() {
            "rayman.lifecycle-only-replacement-proof.v3"
        } else {
            "rayman.lifecycle-only-replacement-proof.v2"
        },
    );
    lifecycle_hash_str(&mut hasher, &proof.recorded_at);
    lifecycle_hash_str(&mut hasher, &proof.workspace_identity);
    lifecycle_hash_str(&mut hasher, &proof.workspace_fingerprint);
    lifecycle_hash_str(&mut hasher, &proof.authority_goal_id);
    lifecycle_hash_str(&mut hasher, &proof.authority_lifecycle_contract_sha256);
    lifecycle_hash_str(&mut hasher, &proof.replacement_contract_sha256);
    hasher.update((proof.predecessor_contracts.len() as u64).to_le_bytes());
    for (id, contract) in &proof.predecessor_contracts {
        lifecycle_hash_str(&mut hasher, id);
        lifecycle_hash_str(&mut hasher, contract);
    }
    hasher.update((proof.source_delta_paths.len() as u64).to_le_bytes());
    for path in &proof.source_delta_paths {
        lifecycle_hash_str(&mut hasher, path);
    }
    lifecycle_hash_str(&mut hasher, &proof.live_authority.command);
    if let Some(rebind) = proof.live_authority.command_rebind.as_ref() {
        lifecycle_hash_rebind(&mut hasher, rebind);
    }
    lifecycle_hash_str(&mut hasher, &proof.live_authority.recorded_at);
    lifecycle_hash_str(&mut hasher, &proof.live_authority.workspace_fingerprint);
    hasher.update(proof.live_authority.repeat.to_le_bytes());
    lifecycle_hash_str(&mut hasher, &proof.live_authority.invocation_sha256);
    hasher.update((proof.live_authority.runs.len() as u64).to_le_bytes());
    for run in &proof.live_authority.runs {
        hasher.update(run.exit_code.to_le_bytes());
        lifecycle_hash_str(&mut hasher, &run.workspace_fingerprint_before);
        lifecycle_hash_str(&mut hasher, &run.workspace_fingerprint_after);
        lifecycle_hash_str(&mut hasher, &run.stdout_sha256);
        lifecycle_hash_str(&mut hasher, &run.stderr_sha256);
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn replacement_delta_scope_error(
    predecessors: &[Goal],
    source_delta_paths: &[String],
) -> Option<String> {
    let planned = predecessors
        .iter()
        .flat_map(|goal| goal.plan_receipts.iter())
        .flat_map(|plan| plan.effective_changed_paths().iter().cloned())
        .collect::<BTreeSet<_>>();
    let unscoped = source_delta_paths
        .iter()
        .filter(|path| !planned.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    if unscoped.is_empty() {
        None
    } else {
        Some(format!(
            "lifecycle-only replacement delta 未被 predecessor plan 覆盖: {}",
            unscoped.join(", ")
        ))
    }
}

pub fn replacement_authority_invocation_sha256(
    command: &str,
    replacement_id: &str,
    authority_goal_id: &str,
    predecessor_ids: &[String],
    repeat: u32,
) -> String {
    replacement_authority_invocation_sha256_with_rebind(
        command,
        replacement_id,
        authority_goal_id,
        predecessor_ids,
        repeat,
        None,
    )
}

fn lifecycle_hash_rebind(hasher: &mut Sha256, rebind: &ReplacementAuthorityCommandRebind) {
    lifecycle_hash_str(hasher, &rebind.schema);
    lifecycle_hash_str(hasher, &rebind.flag);
    lifecycle_hash_str(hasher, &rebind.archived_value);
    lifecycle_hash_str(hasher, &rebind.current_value);
    lifecycle_hash_str(hasher, &rebind.current_sha256);
}

pub fn replacement_authority_invocation_sha256_with_rebind(
    command: &str,
    replacement_id: &str,
    authority_goal_id: &str,
    predecessor_ids: &[String],
    repeat: u32,
    command_rebind: Option<&ReplacementAuthorityCommandRebind>,
) -> String {
    let mut hasher = Sha256::new();
    lifecycle_hash_str(
        &mut hasher,
        if command_rebind.is_some() {
            "rayman.lifecycle-live-authority-invocation.v2"
        } else {
            "rayman.lifecycle-live-authority-invocation.v1"
        },
    );
    lifecycle_hash_str(&mut hasher, command);
    if let Some(rebind) = command_rebind {
        lifecycle_hash_rebind(&mut hasher, rebind);
    }
    lifecycle_hash_str(&mut hasher, replacement_id);
    lifecycle_hash_str(&mut hasher, authority_goal_id);
    let mut predecessors = predecessor_ids.to_vec();
    predecessors.sort();
    predecessors.dedup();
    hasher.update((predecessors.len() as u64).to_le_bytes());
    for id in predecessors {
        lifecycle_hash_str(&mut hasher, &id);
    }
    hasher.update(repeat.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

/// Revalidate a lifecycle-only replacement without trusting its serialized
/// conclusion.  All referenced records are loaded from this workspace's
/// managed goal store, so copied cross-workspace JSON cannot mint authority.
pub fn replacement_authority_error(goal: &Goal, root: &Path, fingerprint: &str) -> Option<String> {
    let proof = goal.replacement_authority.as_ref()?;
    if proof.proof_sha256 != replacement_authority_proof_sha256(proof) {
        return Some("lifecycle-only replacement proof hash 无效".into());
    }
    if proof.workspace_identity != workspace_identity(root) {
        return Some("lifecycle-only replacement 来自不同 workspace identity".into());
    }
    if proof.workspace_fingerprint != fingerprint {
        return Some("lifecycle-only replacement source fingerprint 已过期".into());
    }
    let Some(baseline) = goal.baseline.as_ref() else {
        return Some("lifecycle-only replacement 缺少 baseline".into());
    };
    let proof_recorded_at =
        match plan_timestamp("replacement_authority.recorded_at", &proof.recorded_at) {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
    let live = &proof.live_authority;
    let live_recorded_at = match plan_timestamp(
        "replacement_authority.live_authority.recorded_at",
        &live.recorded_at,
    ) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let goal_created_at = match plan_timestamp("goal.created_at", &goal.created_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let goal_updated_at = match plan_timestamp("goal.updated_at", &goal.updated_at) {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    let baseline_recorded_at =
        match plan_timestamp("goal.baseline.recorded_at", &baseline.recorded_at) {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
    for (label, lower_bound) in [
        ("goal.created_at", goal_created_at),
        ("goal.baseline.recorded_at", baseline_recorded_at),
        (
            "replacement_authority.live_authority.recorded_at",
            live_recorded_at,
        ),
    ] {
        if lower_bound > proof_recorded_at {
            return Some(format!(
                "{label} 不得晚于 replacement_authority.recorded_at"
            ));
        }
    }
    if proof_recorded_at > goal_updated_at {
        return Some("replacement_authority.recorded_at 不得晚于 goal.updated_at".into());
    }
    if replacement_contract_sha256(goal) != proof.replacement_contract_sha256
        || !goal.plan_receipts.is_empty()
        || !goal.review_receipts.is_empty()
        || !goal.authority_receipts.is_empty()
        || goal.requirements.iter().any(|requirement| {
            requirement.status != RequirementStatus::Done
                || requirement
                    .evidence
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .is_empty()
                || !requirement.validations.is_empty()
                || !requirement.impacts.is_empty()
        })
    {
        return Some("lifecycle-only replacement 合约、baseline 或专用迁移形态无效".into());
    }
    let mut normalized_delta = proof.source_delta_paths.clone();
    normalize_path_list(&mut normalized_delta);
    let predecessor_ids = proof
        .predecessor_contracts
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    if normalized_delta != proof.source_delta_paths
        || live.repeat < 2
        || live.runs.len() != live.repeat as usize
        || live.workspace_fingerprint != fingerprint
        || live.invocation_sha256
            != replacement_authority_invocation_sha256_with_rebind(
                &live.command,
                &goal.id,
                &proof.authority_goal_id,
                &predecessor_ids,
                live.repeat,
                live.command_rebind.as_ref(),
            )
        || validate_authority_command(root, &live.command).is_err()
        || replacement_authority_effective_command(&live.command, live.command_rebind.as_ref())
            .is_err()
        // 写路径（main.rs 的 live 运行前后与 authorize_replacement）都把
        // rebind 工件哈希当作 fatal 校验；读侧复验器承诺"不信任序列化结论"，
        // 若独漏这一项，工件（可位于 gitignored、不进 workspace fingerprint
        // 的路径）在授权后被改写也不会翻红。
        || live
            .command_rebind
            .as_ref()
            .is_some_and(|rebind| verify_maintenance_cycle_rebind_artifact(root, rebind).is_err())
        || live.runs.iter().any(|run| {
            run.exit_code != 0
                || run.workspace_fingerprint_before != fingerprint
                || run.workspace_fingerprint_after != fingerprint
                || !is_sha256(&run.stdout_sha256)
                || !is_sha256(&run.stderr_sha256)
        })
    {
        return Some("live lifecycle authority receipt 无效或未绑定当前源码".into());
    }
    if workspace_fingerprint(root).is_ok_and(|current| current == fingerprint) {
        let Ok(current) = workspace_baseline(root) else {
            return Some("无法复算 lifecycle-only replacement 当前 delta".into());
        };
        if workspace_delta(baseline, &current) != proof.source_delta_paths {
            return Some("lifecycle-only replacement 当前 delta 与授权 proof 不一致".into());
        }
    }

    let store = GoalStore::new(root);
    let authority = match store.get(&proof.authority_goal_id) {
        Ok(Some(authority)) => authority,
        Ok(None) => return Some("lifecycle-only authority goal 不存在".into()),
        Err(error) => return Some(format!("无法读取 lifecycle-only authority goal: {error}")),
    };
    let Some(authority_lifecycle) = authority.lifecycle_proof.as_ref() else {
        return Some("lifecycle-only authority goal 缺少 lifecycle proof".into());
    };
    if authority.lifecycle != GoalLifecycle::Archived
        || authority.status != GoalStatus::Success
        || !authority.is_current_schema()
        || authority.current_schema_error().is_some()
        || authority_lifecycle.receipt_policy.as_deref() != Some(RECEIPT_POLICY_V2)
        || authority_lifecycle.migration.is_some()
        || authority_lifecycle.contract_sha256 != proof.authority_lifecycle_contract_sha256
        || authority.lifecycle_proof_error(root).is_some()
        || historical_success_fingerprint(&authority, root, ReceiptValidationPolicy::CurrentV2)
            .as_deref()
            != Some(authority_lifecycle.workspace_fingerprint.as_str())
        || !has_direct_stable_authority_command(
            &authority,
            root,
            &authority_lifecycle.workspace_fingerprint,
            &live.command,
        )
    {
        return Some(
            "lifecycle-only authority 必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success"
                .into(),
        );
    }
    for (label, value) in goal_ledger_timestamp_bounds(&authority) {
        let timestamp = match plan_timestamp(label, value) {
            Ok(value) => value,
            Err(error) => return Some(format!("lifecycle-only authority {error}")),
        };
        if timestamp > proof_recorded_at {
            return Some(format!(
                "lifecycle-only authority {label} 不得晚于 replacement_authority.recorded_at"
            ));
        }
    }

    let mut predecessors = Vec::new();
    for (id, expected_contract) in &proof.predecessor_contracts {
        let predecessor = match store.get(id) {
            Ok(Some(predecessor)) => predecessor,
            Ok(None) => return Some(format!("被转移目标不存在: {id}")),
            Err(error) => return Some(format!("无法读取被转移目标 {id}: {error}")),
        };
        if predecessor.status == GoalStatus::Success
            || !predecessor.is_current_schema()
            || predecessor.current_schema_error().is_some()
            || transfer_goal_contract_sha256(&predecessor) != *expected_contract
            || !matches!(
                predecessor.lifecycle,
                GoalLifecycle::Current | GoalLifecycle::Superseded
            )
            || (predecessor.lifecycle == GoalLifecycle::Superseded
                && predecessor.superseded_by.as_deref() != Some(goal.id.as_str()))
        {
            return Some(format!("被转移目标 {id} 的合约或 lifecycle 已失效"));
        }
        if predecessor.lifecycle == GoalLifecycle::Superseded {
            if let Some(error) = predecessor.lifecycle_proof_error(root) {
                return Some(format!(
                    "被转移目标 {id} 的 supersession proof 无效: {error}"
                ));
            }
            let Some(supersession) = predecessor.lifecycle_proof.as_ref() else {
                return Some(format!("被转移目标 {id} 缺少 supersession proof"));
            };
            let superseded_at = match plan_timestamp(
                "predecessor.lifecycle_proof.recorded_at",
                &supersession.recorded_at,
            ) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            if superseded_at < proof_recorded_at {
                return Some(format!(
                    "被转移目标 {id} 的 supersession 不得早于 replacement_authority.recorded_at"
                ));
            }
        }
        predecessors.push(predecessor);
    }
    if must_transfer_multiset(std::iter::once(goal)) != must_transfer_multiset(predecessors.iter())
    {
        return Some(
            "replacement must 与被转移目标 must（含 typed proof 义务）的精确并集不一致".into(),
        );
    }
    if let Some(error) = replacement_delta_scope_error(&predecessors, &proof.source_delta_paths) {
        return Some(error);
    }
    None
}

pub fn supersession_error(
    goal: &Goal,
    goals: &[Goal],
    root: &Path,
    current_fingerprint: &str,
) -> Option<String> {
    if goal.lifecycle != GoalLifecycle::Superseded {
        return None;
    }
    let replacement_id = goal.superseded_by.as_deref()?;
    let Some(replacement) = goals
        .iter()
        .find(|candidate| candidate.id == replacement_id)
    else {
        return Some(format!("superseded_by 目标不存在: {replacement_id}"));
    };
    if !matches!(
        replacement.lifecycle,
        GoalLifecycle::Current | GoalLifecycle::Archived
    ) {
        return Some(format!(
            "superseded_by 目标 {replacement_id} lifecycle={}，必须为 current 或带有效 proof 的 archived success",
            replacement.lifecycle
        ));
    }
    if matches!(
        replacement
            .lifecycle_proof
            .as_ref()
            .and_then(|proof| proof.receipt_policy.as_deref()),
        Some(RECEIPT_POLICY_QUARANTINED | RECEIPT_POLICY_INTEGRITY_QUARANTINED)
    ) {
        return Some(format!(
            "superseded_by archived 目标 {replacement_id} 是 untrusted history quarantine，不能作为完成证明"
        ));
    }
    if !replacement.is_current_schema() {
        return Some(format!(
            "superseded_by 目标 {replacement_id} 必须是 current schema，legacy success 只能显式 archive"
        ));
    }
    if let Some(error) = replacement.current_schema_error() {
        return Some(format!(
            "superseded_by 目标 {replacement_id} 合约无效: {error}"
        ));
    }
    if replacement.status != GoalStatus::Success {
        return Some(format!(
            "superseded_by 目标 {replacement_id} 状态为 {}，必须先 gate-ready success",
            replacement.status
        ));
    }
    match replacement.lifecycle {
        GoalLifecycle::Current => {
            let replacement_gaps =
                goal_success_receipt_gaps(replacement, root, current_fingerprint);
            if !replacement_gaps.is_empty() {
                return Some(format!(
                    "superseded_by 目标 {replacement_id} 尚未 gate-ready: {}",
                    replacement_gaps.join("; ")
                ));
            }
        }
        GoalLifecycle::Archived => {
            if let Some(error) = replacement.lifecycle_proof_error(root) {
                return Some(format!(
                    "superseded_by archived 目标 {replacement_id} proof 无效: {error}"
                ));
            }
        }
        GoalLifecycle::Superseded => unreachable!("lifecycle was checked above"),
    }
    if let Some(proof) = replacement.replacement_authority.as_ref() {
        if let Some(error) =
            replacement_authority_error(replacement, root, &proof.workspace_fingerprint)
        {
            return Some(format!(
                "lifecycle-only replacement authority proof 无效: {error}"
            ));
        }
        if !proof.predecessor_contracts.contains_key(&goal.id) {
            return Some(format!(
                "lifecycle-only replacement 未显式绑定被替代目标 {}",
                goal.id
            ));
        }
        if let Some(supersession) = goal.lifecycle_proof.as_ref() {
            let replacement_recorded_at =
                match plan_timestamp("replacement_authority.recorded_at", &proof.recorded_at) {
                    Ok(value) => value,
                    Err(error) => return Some(error),
                };
            let superseded_at = match plan_timestamp(
                "predecessor.lifecycle_proof.recorded_at",
                &supersession.recorded_at,
            ) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            if superseded_at < replacement_recorded_at {
                return Some(format!(
                    "目标 {} 的 supersession 不得早于 replacement_authority.recorded_at",
                    goal.id
                ));
            }
        }
    }
    let must_transfer_required = goal.status != GoalStatus::Success
        || (!goal.loaded_from_legacy
            && historical_success_fingerprint(goal, root, ReceiptValidationPolicy::CurrentV2)
                .is_none());
    if must_transfer_required {
        let replacement_must = replacement
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == RequirementKind::Must)
            .map(must_transfer_key)
            .collect::<BTreeSet<_>>();
        let missing = goal
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == RequirementKind::Must)
            .filter(|requirement| !replacement_must.contains(&must_transfer_key(requirement)))
            .map(|requirement| match requirement.proof_kind {
                Some(kind) if kind != ProofKind::Generic => {
                    format!("{} [proof:{}]", requirement.text, kind.as_str())
                }
                _ => requirement.text.clone(),
            })
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Some(format!(
                "未证明完成的 goal，其 must 未完整转移到 replacement: {}",
                missing.join(" | ")
            ));
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LegacyGoal {
    pub(super) id: String,
    pub(super) status: String,
    #[serde(default)]
    pub(super) created_at: Option<String>,
    #[serde(default)]
    pub(super) updated_at: Option<String>,
    pub(super) contract: LegacyContract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LegacyContract {
    pub(super) goal: String,
    #[serde(default)]
    pub(super) requirements: Vec<LegacyRequirement>,
    #[serde(default)]
    pub(super) verification: Vec<String>,
    #[serde(default)]
    pub(super) created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LegacyRequirement {
    pub(super) id: String,
    pub(super) text: String,
    #[serde(default = "legacy_must_kind")]
    pub(super) priority: String,
    #[serde(default = "legacy_open_status")]
    pub(super) status: String,
    #[serde(default)]
    pub(super) evidence: Option<String>,
    #[serde(default)]
    pub(super) validation_commands: Vec<String>,
}
