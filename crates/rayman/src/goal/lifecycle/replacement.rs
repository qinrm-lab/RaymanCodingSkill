use super::*;

fn normalized_requirement_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// must 转移比较键。typed proof 义务是不可变合约的一部分（自 v3 起
/// transfer_goal_contract_sha256 与 replacement_contract_sha256 都把 proof_kind
/// 计入哈希——在那之前这条注释是错的，两个哈希都只覆盖 id/text/kind，所以事后
/// 剥掉 predecessor 的 `--must-proof` 不会破坏任何被 pin 住的哈希，而这里的实时
/// 比较随即就会接受它先前拒绝的普通 must）。文本同名的普通 must 不得顶替 typed
/// must，否则卡在 installation/repository_gate 等 typed 阶段的目标可以被一条
/// generic receipt 洗掉义务。`None` 与 `Some(Generic)` 在校验语义里等价
/// （见 proof_kind_matches），共用一键。
pub(in crate::goal) fn must_transfer_key(requirement: &Requirement) -> (&'static str, String) {
    (
        requirement.proof_kind.unwrap_or_default().as_str(),
        normalized_requirement_text(&requirement.text),
    )
}

pub(in crate::goal) fn must_transfer_multiset<'a>(
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
pub(in crate::goal) fn transfer_goal_contract_sha256(goal: &Goal) -> String {
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
pub(in crate::goal) fn replacement_contract_sha256(goal: &Goal) -> String {
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

pub(in crate::goal) fn replacement_authority_proof_sha256(
    proof: &ReplacementAuthorityProof,
) -> String {
    let mut hasher = Sha256::new();
    lifecycle_hash_str(
        &mut hasher,
        if proof.authority_gate_binding.is_some() {
            "rayman.lifecycle-only-replacement-proof.v4"
        } else if proof.live_authority.command_rebind.is_some() {
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
    hasher.update([u8::from(proof.authority_gate_binding.is_some())]);
    if let Some(binding) = proof.authority_gate_binding.as_ref() {
        lifecycle_hash_str(&mut hasher, &binding.policy);
        lifecycle_hash_str(&mut hasher, &binding.entrypoint);
        hasher.update((binding.dependency_sha256.len() as u64).to_le_bytes());
        for (path, hash) in &binding.dependency_sha256 {
            lifecycle_hash_str(&mut hasher, path);
            lifecycle_hash_str(&mut hasher, hash);
        }
        lifecycle_hash_str(&mut hasher, &binding.binding_sha256);
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

pub(in crate::goal) fn replacement_delta_scope_error(
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
    let (all_goals, issues) = match GoalStore::new(root).list_with_issues() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return Some(format!(
                "无法读取 lifecycle-only authority goal snapshot: {error}"
            ));
        }
    };
    if !issues.is_empty() {
        return Some(format!(
            "无法读取 lifecycle-only authority goal snapshot: {}",
            issues
                .iter()
                .map(|issue| format!("{} ({})", issue.path, issue.error))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let current = if goal.lifecycle == GoalLifecycle::Current {
        match stable_workspace_baseline(root) {
            Ok(current) => Some(current),
            Err(error) => {
                return Some(format!(
                    "could not recompute the current lifecycle-only replacement baseline: {error}"
                ));
            }
        }
    } else {
        None
    };
    replacement_authority_error_core(goal, root, fingerprint, current.as_ref(), &all_goals)
}

pub fn replacement_authority_error_with_baseline(
    goal: &Goal,
    root: &Path,
    current: &WorkspaceBaseline,
    all_goals: &[Goal],
) -> Option<String> {
    replacement_authority_error_core(
        goal,
        root,
        &current.workspace_fingerprint,
        Some(current),
        all_goals,
    )
}

/// Capture-only counterpart for readiness.  Its shape deliberately mirrors
/// the live verifier below, but all filesystem, Git and maintenance-artifact
/// observations are supplied by `GoalDecisionContext`.
pub(crate) fn replacement_authority_error_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    all_goals: &[Goal],
) -> Option<String> {
    let proof = goal.replacement_authority.as_ref()?;
    if proof.proof_sha256 != replacement_authority_proof_sha256(proof) {
        return Some("lifecycle-only replacement proof hash 无效".into());
    }
    if proof.workspace_identity != decision.captured_workspace_identity().unwrap_or_default() {
        return Some("lifecycle-only replacement 来自不同 workspace identity".into());
    }
    let Some(current) = decision.current() else {
        return Some("current lifecycle-only replacement lacks a workspace baseline".into());
    };
    let fingerprint = &current.workspace_fingerprint;
    if proof.workspace_fingerprint != *fingerprint {
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
        || live.workspace_fingerprint != *fingerprint
        || live.invocation_sha256
            != replacement_authority_invocation_sha256_with_rebind(
                &live.command,
                &goal.id,
                &proof.authority_goal_id,
                &predecessor_ids,
                live.repeat,
                live.command_rebind.as_ref(),
            )
        || validate_authority_command_with_context(decision, &live.command).is_err()
        || replacement_authority_effective_command(&live.command, live.command_rebind.as_ref())
            .is_err()
        || live.command_rebind.as_ref().is_some_and(|rebind| {
            verify_maintenance_cycle_rebind_artifact_with_context(decision, rebind).is_err()
        })
        || live.runs.iter().any(|run| {
            run.exit_code != 0
                || run.workspace_fingerprint_before != *fingerprint
                || run.workspace_fingerprint_after != *fingerprint
                || !is_sha256(&run.stdout_sha256)
                || !is_sha256(&run.stderr_sha256)
        })
    {
        return Some("live lifecycle authority receipt 无效或未绑定当前源码".into());
    }
    if workspace_delta(baseline, current) != proof.source_delta_paths {
        return Some("lifecycle-only replacement 当前 delta 与授权 proof 不一致".into());
    }
    let authority = match all_goals
        .iter()
        .find(|candidate| candidate.id == proof.authority_goal_id)
    {
        Some(authority) => authority,
        None => return Some("lifecycle-only authority goal 不存在".into()),
    };
    let Some(authority_lifecycle) = authority.lifecycle_proof.as_ref() else {
        return Some("lifecycle-only authority goal 缺少 lifecycle proof".into());
    };
    if authority.lifecycle != GoalLifecycle::Archived
        || authority.status != GoalStatus::Success
        || !authority.is_current_schema()
        || authority.current_schema_error().is_some()
        || authority_lifecycle.receipt_policy.as_deref() != Some(RECEIPT_POLICY_V3)
        || authority_lifecycle.migration.is_some()
        || authority_lifecycle.contract_sha256 != proof.authority_lifecycle_contract_sha256
        || authority
            .lifecycle_proof_error_with_context(decision, all_goals)
            .is_some()
        || !has_archived_direct_stable_authority_command_with_context(
            authority,
            decision,
            &authority_lifecycle.workspace_fingerprint,
            &live.command,
        )
    {
        return Some("lifecycle-only authority 必须是同 workspace、current-policy 且包含同命令 direct-authority 的有效 archived success".into());
    }
    if let Some(error) = authority_gate_binding_error(
        authority,
        &live.command,
        proof.authority_gate_binding.as_ref(),
    ) {
        return Some(error);
    }
    let captured_binding = match authority_gate_binding_for_goal_with_context(
        authority,
        decision,
        &live.command,
    ) {
        Ok(binding) => binding,
        Err(error) => {
            return Some(format!(
                "无法从 captured workspace 重算 repository replacement authority gate binding: {error:#}"
            ));
        }
    };
    if captured_binding != proof.authority_gate_binding {
        return Some(
            "repository replacement authority binding 与 captured authority gate closure 不一致"
                .into(),
        );
    }
    for (label, value) in goal_ledger_timestamp_bounds(authority) {
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
        let predecessor = match all_goals.iter().find(|candidate| candidate.id == *id) {
            Some(predecessor) => predecessor,
            None => return Some(format!("被转移目标不存在: {id}")),
        };
        if predecessor.status == GoalStatus::Success
            || !predecessor.is_current_schema()
            || predecessor.current_schema_error().is_some()
            || transfer_goal_contract_sha256(predecessor) != *expected_contract
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
            if let Some(error) = predecessor.lifecycle_proof_error_with_context(decision, all_goals)
            {
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
    if must_transfer_multiset(std::iter::once(goal))
        != must_transfer_multiset(predecessors.iter().copied())
    {
        return Some(
            "replacement must 与被转移目标 must（含 typed proof 义务）的精确并集不一致".into(),
        );
    }
    let planned = predecessors
        .iter()
        .flat_map(|goal| goal.plan_receipts.iter())
        .flat_map(|plan| plan.effective_changed_paths().iter().cloned())
        .collect::<BTreeSet<_>>();
    let unscoped = proof
        .source_delta_paths
        .iter()
        .filter(|path| !planned.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !unscoped.is_empty() {
        return Some(format!(
            "lifecycle-only replacement delta 未被 predecessor plan 覆盖: {}",
            unscoped.join(", ")
        ));
    }
    None
}

fn replacement_authority_error_core(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    current: Option<&WorkspaceBaseline>,
    all_goals: &[Goal],
) -> Option<String> {
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
    if goal.lifecycle == GoalLifecycle::Current {
        // Read the current workspace exactly once and use that same baseline
        // for both the fingerprint and delta checks.  The former two-read
        // shape was fail-open: a read error or drift simply skipped delta
        // validation, and a mutation between the fingerprint and baseline
        // walks mixed two points in time.  Archived history has already crossed
        // this live fixed point and remains verifiable after later source
        // evolution; a current replacement has not, so it must fail closed.
        let Some(current) = current else {
            return Some("current lifecycle-only replacement lacks a workspace baseline".into());
        };
        if current.workspace_fingerprint != fingerprint {
            return Some(
                "the lifecycle-only replacement source fingerprint changed during revalidation"
                    .into(),
            );
        }
        if workspace_delta(baseline, current) != proof.source_delta_paths {
            return Some("lifecycle-only replacement 当前 delta 与授权 proof 不一致".into());
        }
    }

    let authority = match all_goals
        .iter()
        .find(|candidate| candidate.id == proof.authority_goal_id)
    {
        Some(authority) => authority,
        None => return Some("lifecycle-only authority goal 不存在".into()),
    };
    let Some(authority_lifecycle) = authority.lifecycle_proof.as_ref() else {
        return Some("lifecycle-only authority goal 缺少 lifecycle proof".into());
    };
    if authority.lifecycle != GoalLifecycle::Archived
        || authority.status != GoalStatus::Success
        || !authority.is_current_schema()
        || authority.current_schema_error().is_some()
        || authority_lifecycle.receipt_policy.as_deref() != Some(RECEIPT_POLICY_V3)
        || authority_lifecycle.migration.is_some()
        || authority_lifecycle.contract_sha256 != proof.authority_lifecycle_contract_sha256
        || authority.lifecycle_proof_error(root).is_some()
        || !has_archived_direct_stable_authority_command(
            authority,
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
    if let Some(error) = authority_gate_binding_error(
        authority,
        &live.command,
        proof.authority_gate_binding.as_ref(),
    ) {
        return Some(error);
    }
    for (label, value) in goal_ledger_timestamp_bounds(authority) {
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
        let predecessor = match all_goals.iter().find(|candidate| candidate.id == *id) {
            Some(predecessor) => predecessor,
            None => return Some(format!("被转移目标不存在: {id}")),
        };
        if predecessor.status == GoalStatus::Success
            || !predecessor.is_current_schema()
            || predecessor.current_schema_error().is_some()
            || transfer_goal_contract_sha256(predecessor) != *expected_contract
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
    if must_transfer_multiset(std::iter::once(goal))
        != must_transfer_multiset(predecessors.iter().copied())
    {
        return Some(
            "replacement must 与被转移目标 must（含 typed proof 义务）的精确并集不一致".into(),
        );
    }
    let planned = predecessors
        .iter()
        .flat_map(|goal| goal.plan_receipts.iter())
        .flat_map(|plan| plan.effective_changed_paths().iter().cloned())
        .collect::<BTreeSet<_>>();
    let unscoped = proof
        .source_delta_paths
        .iter()
        .filter(|path| !planned.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !unscoped.is_empty() {
        let error = format!(
            "lifecycle-only replacement delta 未被 predecessor plan 覆盖: {}",
            unscoped.join(", ")
        );
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
            && historical_success_fingerprint(goal, root, ReceiptValidationPolicy::CurrentV3)
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

pub(crate) fn supersession_error_with_context(
    goal: &Goal,
    goals: &[Goal],
    decision: &GoalDecisionContext<'_>,
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
            let gaps = goal_success_receipt_gaps_with_context(
                replacement,
                decision,
                goals,
                ReceiptValidationPolicy::CurrentV3,
            );
            if !gaps.is_empty() {
                return Some(format!(
                    "superseded_by 目标 {replacement_id} 尚未 gate-ready: {}",
                    gaps.join("; ")
                ));
            }
        }
        GoalLifecycle::Archived => {
            if let Some(error) = replacement.lifecycle_proof_error_with_context(decision, goals) {
                return Some(format!(
                    "superseded_by archived 目标 {replacement_id} proof 无效: {error}"
                ));
            }
        }
        GoalLifecycle::Superseded => unreachable!("lifecycle was checked above"),
    }
    if let Some(proof) = replacement.replacement_authority.as_ref() {
        if let Some(error) = replacement_authority_error_with_context(replacement, decision, goals)
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
    let must_transfer_required = goal.status != GoalStatus::Success;
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
