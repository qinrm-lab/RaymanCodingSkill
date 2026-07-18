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
fn legacy_lifecycle_contract_sha256(goal: &Goal) -> String {
    let mut hasher = Sha256::new();
    let extended = goal.baseline.is_some()
        || !goal.plan_receipts.is_empty()
        || !goal.review_receipts.is_empty();
    lifecycle_hash_str(
        &mut hasher,
        if extended { "rayman.lifecycle-contract.v2" } else { "rayman.lifecycle-contract.v1" },
    );
    hasher.update(goal.schema_version.to_le_bytes());
    lifecycle_hash_str(&mut hasher, &goal.id);
    lifecycle_hash_str(&mut hasher, &goal.title);
    lifecycle_hash_str(&mut hasher, goal.status.as_str());
    lifecycle_hash_str(&mut hasher, goal.lifecycle.as_str());
    lifecycle_hash_optional_str(&mut hasher, goal.lifecycle_reason.as_deref());
    lifecycle_hash_optional_str(&mut hasher, goal.superseded_by.as_deref());
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
        }
        hasher.update((goal.review_receipts.len() as u64).to_le_bytes());
        for receipt in &goal.review_receipts {
            lifecycle_hash_str(&mut hasher, &receipt.recorded_at);
            lifecycle_hash_str(&mut hasher, &receipt.source_fingerprint);
            lifecycle_hash_str(&mut hasher, &receipt.reviewer);
            lifecycle_hash_str(&mut hasher, &receipt.summary);
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

fn lifecycle_contract_sha256(goal: &Goal, receipt_policy: Option<&str>) -> String {
    let legacy_contract = legacy_lifecycle_contract_sha256(goal);
    let Some(receipt_policy) = receipt_policy else {
        return legacy_contract;
    };
    let mut hasher = Sha256::new();
    lifecycle_hash_str(&mut hasher, "rayman.lifecycle-proof-policy.v1");
    lifecycle_hash_str(&mut hasher, receipt_policy);
    lifecycle_hash_str(&mut hasher, &legacy_contract);
    format!("{:x}", hasher.finalize())
}

fn issue_lifecycle_proof(
    goal: &Goal,
    fingerprint: String,
    migration: Option<String>,
    receipt_policy: Option<String>,
) -> LifecycleProof {
    LifecycleProof {
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint,
        contract_sha256: lifecycle_contract_sha256(goal, receipt_policy.as_deref()),
        migration,
        receipt_policy,
    }
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

fn pre_receipt_migration_eligible(goal: &Goal) -> bool {
    completed_current_schema_history(goal)
        && goal_created_before(goal, STRICT_RECEIPT_ROLLOUT_AT)
}

fn receipt_policy_v1_migration_eligible(goal: &Goal) -> bool {
    completed_current_schema_history(goal)
        && goal_created_before(goal, RECEIPT_POLICY_V2_ROLLOUT_AT)
}

fn historical_success_fingerprint(
    goal: &Goal,
    root: &Path,
    policy: ReceiptValidationPolicy,
) -> Option<String> {
    let candidates = goal
        .requirements
        .iter()
        .flat_map(|requirement| requirement.validations.iter())
        .filter_map(|validation| validation.receipt.as_ref())
        .filter(|receipt| {
            receipt.workspace_fingerprint_before == receipt.workspace_fingerprint_after
                && is_sha256(&receipt.workspace_fingerprint_after)
        })
        .map(|receipt| receipt.workspace_fingerprint_after.clone())
        .collect::<BTreeSet<_>>();
    candidates.into_iter().find(|fingerprint| {
        goal_success_receipt_gaps_for_policy(goal, root, fingerprint, false, policy).is_empty()
    })
}

fn fingerprint_for_files(files: &BTreeMap<String, String>) -> String {
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

pub fn workspace_delta(
    baseline: &WorkspaceBaseline,
    current: &WorkspaceBaseline,
) -> Vec<String> {
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

pub fn goal_planning_gaps(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
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
    let actual = workspace_delta(baseline, &current);
    let valid_plans = goal
        .plan_receipts
        .iter()
        .filter(|receipt| {
            receipt.baseline_fingerprint == baseline.workspace_fingerprint
                && receipt.plan_sha256 == plan_receipt_sha256(receipt)
        })
        .collect::<Vec<_>>();
    if actual.len() >= 2 && valid_plans.is_empty() {
        gaps.push(format!(
            "实际变更 {} 个文件但缺少首次修改前的 goal plan receipt",
            actual.len()
        ));
    }
    let planned = valid_plans
        .iter()
        .flat_map(|receipt| receipt.changed_paths.iter().cloned())
        .collect::<BTreeSet<_>>();
    let unplanned = actual
        .iter()
        .filter(|path| !planned.contains(*path))
        .cloned()
        .collect::<Vec<_>>();
    if !valid_plans.is_empty() && !unplanned.is_empty() {
        gaps.push(format!("实际变更超出 plan: {}", unplanned.join(", ")));
    }

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
                ReceiptValidationPolicy::CurrentV2,
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

    if valid_plans
        .iter()
        .any(|receipt| receipt.review_priority == "high")
        && !goal
            .review_receipts
            .iter()
            .any(|receipt| receipt.source_fingerprint == current_fingerprint)
    {
        gaps.push("high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt".into());
    }
    gaps
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
                if self.status != GoalStatus::Success {
                    Some("只有 success goal 可以 archived".into())
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
                    || !matches!(receipt.review_priority.as_str(), "normal" | "broad" | "high")
                    || receipt.plan_sha256 != plan_receipt_sha256(receipt)
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
        } else if !self.plan_receipts.is_empty() || !self.review_receipts.is_empty() {
            return Some("缺少 baseline 的 goal 不能携带 plan/review receipt".into());
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
        let policy = match proof.receipt_policy.as_deref() {
            None if goal_created_before(self, RECEIPT_POLICY_V2_ROLLOUT_AT) => {
                ReceiptValidationPolicy::LegacyV1
            }
            None => ReceiptValidationPolicy::CurrentV2,
            Some(RECEIPT_POLICY_V1) => ReceiptValidationPolicy::LegacyV1,
            Some(RECEIPT_POLICY_V2) => ReceiptValidationPolicy::CurrentV2,
            Some(other) => return Some(format!("未知 lifecycle receipt policy: {other}")),
        };
        let expected = lifecycle_contract_sha256(self, proof.receipt_policy.as_deref());
        if proof.contract_sha256 != expected {
            return Some("lifecycle_proof 与当前 goal 合约不匹配".into());
        }
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
    if goal.status != GoalStatus::Success {
        let replacement_must = replacement
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == RequirementKind::Must)
            .map(|requirement| normalized_requirement_text(&requirement.text))
            .collect::<BTreeSet<_>>();
        let missing = goal
            .requirements
            .iter()
            .filter(|requirement| requirement.kind == RequirementKind::Must)
            .filter(|requirement| {
                !replacement_must.contains(&normalized_requirement_text(&requirement.text))
            })
            .map(|requirement| requirement.text.as_str())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Some(format!(
                "非 success goal 的 must 未完整转移到 replacement: {}",
                missing.join(" | ")
            ));
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyGoal {
    id: String,
    status: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    contract: LegacyContract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyContract {
    goal: String,
    #[serde(default)]
    requirements: Vec<LegacyRequirement>,
    #[serde(default)]
    verification: Vec<String>,
    #[serde(default)]
    created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyRequirement {
    id: String,
    text: String,
    #[serde(default = "legacy_must_kind")]
    priority: String,
    #[serde(default = "legacy_open_status")]
    status: String,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    validation_commands: Vec<String>,
}
