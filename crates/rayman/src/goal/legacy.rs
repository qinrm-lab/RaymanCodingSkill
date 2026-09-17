use super::*;

pub(super) fn goal_from_legacy(legacy: LegacyGoal) -> Goal {
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

fn historical_receipt_fingerprints(
    goal: &Goal,
    excluded_fingerprint: Option<&str>,
) -> BTreeSet<String> {
    goal.requirements
        .iter()
        .flat_map(|requirement| requirement.validations.iter())
        .filter_map(|validation| validation.receipt.as_ref())
        .filter(|receipt| {
            receipt.workspace_fingerprint_before == receipt.workspace_fingerprint_after
                && is_sha256(&receipt.workspace_fingerprint_after)
        })
        .map(|receipt| receipt.workspace_fingerprint_after.clone())
        .filter(|fingerprint| excluded_fingerprint != Some(fingerprint.as_str()))
        .collect()
}

pub(super) fn historical_success_fingerprint(
    goal: &Goal,
    root: &Path,
    policy: ReceiptValidationPolicy,
) -> Option<String> {
    historical_success_fingerprint_excluding(goal, root, policy, None)
}

pub(super) fn historical_success_fingerprint_excluding(
    goal: &Goal,
    root: &Path,
    policy: ReceiptValidationPolicy,
    excluded_fingerprint: Option<&str>,
) -> Option<String> {
    if policy == ReceiptValidationPolicy::CurrentV3
        && let Some(proof) = goal.replacement_authority.as_ref()
        && excluded_fingerprint != Some(proof.workspace_fingerprint.as_str())
        && goal_success_receipt_gaps_for_policy(
            goal,
            root,
            &proof.workspace_fingerprint,
            false,
            policy,
        )
        .is_empty()
    {
        return Some(proof.workspace_fingerprint.clone());
    }
    historical_receipt_fingerprints(goal, excluded_fingerprint)
        .into_iter()
        .find(|fingerprint| {
            goal_success_receipt_gaps_for_policy(goal, root, fingerprint, false, policy).is_empty()
        })
}

pub(super) fn historical_success_fingerprint_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    policy: ReceiptValidationPolicy,
    excluded_fingerprint: Option<&str>,
) -> Option<String> {
    if policy == ReceiptValidationPolicy::CurrentV3
        && let Some(proof) = goal.replacement_authority.as_ref()
        && excluded_fingerprint != Some(proof.workspace_fingerprint.as_str())
        && goal_success_receipt_gaps_for_historical_legacy_success(
            goal,
            root,
            &proof.workspace_fingerprint,
            policy,
        )
        .is_empty()
    {
        return Some(proof.workspace_fingerprint.clone());
    }
    historical_receipt_fingerprints(goal, excluded_fingerprint)
        .into_iter()
        .find(|fingerprint| {
            goal_success_receipt_gaps_for_historical_legacy_success(goal, root, fingerprint, policy)
                .is_empty()
        })
}

fn historical_legacy_plan_paths(goal: &Goal) -> BTreeSet<String> {
    goal.plan_receipts
        .iter()
        .flat_map(|receipt| receipt.effective_changed_paths().iter())
        .map(|path| path.replace('\\', "/"))
        .collect()
}

/// Historical legacy success accepts a complete proof subset contained by the
/// immutable aggregate plan. Plan-external receipts are ignored: they neither
/// poison a safe subset nor contribute must, relevance, or delta coverage.
/// Pre-plan history retains its original one-file compatibility; its aggregate
/// declared set is still checked below so two or more paths require a plan. An
/// empty non-code scope remains contained in every plan.
pub(super) fn validation_is_plan_contained_for_historical_legacy_success(
    goal: &Goal,
    validation: &ValidationEvidence,
) -> bool {
    let planned = historical_legacy_plan_paths(goal);
    planned.is_empty()
        || validation
            .impact_scopes
            .iter()
            .all(|scope| planned.contains(&scope.changed_path.replace('\\', "/")))
}

pub(super) fn goal_historical_planning_gaps_for_legacy_success(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    validation_policy: ReceiptValidationPolicy,
) -> Vec<String> {
    let Some(baseline) = goal.baseline.as_ref() else {
        return vec![
            "current goal 缺少开工 baseline；不能作为当前成功证据，请用新的 baseline-bound goal supersede，或将已完成记录显式 archive"
                .into(),
        ];
    };
    let mut gaps = Vec::new();
    if fingerprint_for_files(&baseline.files) != baseline.workspace_fingerprint {
        gaps.push("goal baseline 文件清单与 fingerprint 不匹配".into());
        return gaps;
    }

    // A historical fingerprint has no persisted file map, so the live
    // workspace cannot honestly reconstruct its delta. Reconcile only the
    // ledger facts in the plan-contained proof subset. Plan-external extras
    // are unrelated history and must not poison or expand that subset.
    let mut declared = BTreeSet::new();
    for requirement in &goal.requirements {
        let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
            continue;
        };
        for validation in &requirement.validations {
            if validation_is_plan_contained_for_historical_legacy_success(goal, validation)
                && validation_has_receipt_for_fingerprint(
                    validation,
                    root,
                    fingerprint,
                    &contract_sha256,
                    false,
                    validation_policy,
                )
            {
                declared.extend(
                    validation
                        .impact_scopes
                        .iter()
                        .map(|scope| scope.changed_path.replace('\\', "/")),
                );
            }
        }
    }
    let planned = historical_legacy_plan_paths(goal);
    if planned.is_empty() && declared.len() >= 2 {
        gaps.push(format!(
            "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
            declared.len()
        ));
    }
    if goal
        .plan_receipts
        .iter()
        .any(|receipt| receipt.effective_review_priority() == "high")
        && !goal
            .review_receipts
            .iter()
            .any(|receipt| receipt.source_fingerprint == fingerprint)
    {
        gaps.push("high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt".into());
    }
    gaps
}
