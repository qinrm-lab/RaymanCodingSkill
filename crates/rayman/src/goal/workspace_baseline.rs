use super::*;

pub(crate) fn fingerprint_for_files(files: &BTreeMap<String, String>) -> String {
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

/// Bracket a security-sensitive decision with two complete workspace captures
/// and require their file/hash maps to agree.
///
/// This detects ordinary writes that remain visible across either capture or
/// between them. It is not a filesystem transaction and does not claim a
/// linearizable snapshot: a writer capable of an exact A -> B -> A cycle can
/// evade any comparison-only scheme. Callers therefore still perform one final
/// bracket after all other potentially slow checks and immediately before
/// publishing the state record.
pub(super) fn stable_workspace_baseline(root: &Path) -> Result<WorkspaceBaseline> {
    stable_workspace_baseline_with_hook(root, || {})
}

fn stable_workspace_baseline_with_hook(
    root: &Path,
    after_first_capture: impl FnOnce(),
) -> Result<WorkspaceBaseline> {
    let first = workspace_baseline(root)?;
    after_first_capture();
    let second = workspace_baseline(root)?;
    if first.files != second.files || first.workspace_fingerprint != second.workspace_fingerprint {
        bail!(
            "workspace changed while a stable replacement baseline was being captured; retry after source writes stop"
        );
    }
    Ok(second)
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
/// the effective aggregate plan. Callers use this same comparison during
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
    let current = match workspace_baseline(root) {
        Ok(current) => current,
        Err(error) => return vec![format!("无法计算 goal 实际变更集: {error}")],
    };
    if current.workspace_fingerprint != current_fingerprint {
        return vec!["goal 规划检查与调用方当前 fingerprint 不一致".into()];
    }
    goal_planning_gaps_with_baseline(goal, root, &current)
}

/// Reconcile goal planning and receipt coverage against a caller-owned
/// workspace capture. Readiness uses this to keep every goal verdict on the
/// same decision snapshot instead of walking and hashing the repository once
/// per successful goal.
pub fn goal_planning_gaps_with_baseline(
    goal: &Goal,
    root: &Path,
    current: &WorkspaceBaseline,
) -> Vec<String> {
    goal_planning_gaps_with_policy(
        goal,
        root,
        current,
        false,
        Some(ReceiptValidationPolicy::CurrentV3),
    )
}

pub(super) fn goal_planning_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    let current = match workspace_baseline(root) {
        Ok(current) => current,
        Err(error) => return vec![format!("无法计算 goal 实际变更集: {error}")],
    };
    if current.workspace_fingerprint != current_fingerprint {
        return vec!["goal 规划检查与调用方当前 fingerprint 不一致".into()];
    }
    goal_planning_gaps_with_policy(
        goal,
        root,
        &current,
        true,
        Some(ReceiptValidationPolicy::CurrentV3),
    )
}

pub(super) fn goal_plan_governance_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    let current = match workspace_baseline(root) {
        Ok(current) => current,
        Err(error) => return vec![format!("无法计算 goal 实际变更集: {error}")],
    };
    if current.workspace_fingerprint != current_fingerprint {
        return vec!["goal 规划检查与调用方当前 fingerprint 不一致".into()];
    }
    goal_planning_gaps_with_policy(goal, root, &current, true, None)
}

pub(super) fn goal_v1_governance_gaps_for_retiring_legacy_success(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> Vec<String> {
    let current = match workspace_baseline(root) {
        Ok(current) => current,
        Err(error) => return vec![format!("无法计算 goal 实际变更集: {error}")],
    };
    if current.workspace_fingerprint != current_fingerprint {
        return vec!["goal 规划检查与调用方当前 fingerprint 不一致".into()];
    }
    goal_planning_gaps_with_policy(
        goal,
        root,
        &current,
        true,
        Some(ReceiptValidationPolicy::LegacyV1),
    )
}

fn goal_planning_gaps_with_policy(
    goal: &Goal,
    root: &Path,
    current: &WorkspaceBaseline,
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
    if fingerprint_for_files(&current.files) != current.workspace_fingerprint {
        gaps.push("当前 workspace snapshot 的文件清单与 fingerprint 不匹配".into());
        return gaps;
    }
    let delta = match if retiring_legacy_success {
        goal_plan_delta_for_retiring_legacy_success(goal, current)
    } else {
        goal_plan_delta(goal, current)
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
                    &current.workspace_fingerprint,
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
            .any(|receipt| receipt.source_fingerprint == current.workspace_fingerprint)
    {
        gaps.push("high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt".into());
    }
    gaps
}
