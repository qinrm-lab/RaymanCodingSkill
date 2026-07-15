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
fn lifecycle_contract_sha256(goal: &Goal) -> String {
    let mut hasher = Sha256::new();
    lifecycle_hash_str(&mut hasher, "rayman.lifecycle-contract.v1");
    hasher.update(goal.schema_version.to_le_bytes());
    lifecycle_hash_str(&mut hasher, &goal.id);
    lifecycle_hash_str(&mut hasher, &goal.title);
    lifecycle_hash_str(&mut hasher, goal.status.as_str());
    lifecycle_hash_str(&mut hasher, goal.lifecycle.as_str());
    lifecycle_hash_optional_str(&mut hasher, goal.lifecycle_reason.as_deref());
    lifecycle_hash_optional_str(&mut hasher, goal.superseded_by.as_deref());
    lifecycle_hash_str(&mut hasher, &goal.created_at);
    lifecycle_hash_str(&mut hasher, &goal.updated_at);
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

fn issue_lifecycle_proof(
    goal: &Goal,
    fingerprint: String,
    migration: Option<String>,
) -> LifecycleProof {
    LifecycleProof {
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint,
        contract_sha256: lifecycle_contract_sha256(goal),
        migration,
    }
}

fn pre_receipt_migration_eligible(goal: &Goal) -> bool {
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
    let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(&goal.created_at) else {
        return false;
    };
    let rollout = chrono::DateTime::parse_from_rfc3339(STRICT_RECEIPT_ROLLOUT_AT)
        .expect("receipt rollout timestamp must be valid");
    created_at < rollout
}

pub fn workspace_fingerprint(root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for path in crate::walk::workspace_files_checked(root)? {
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(crate::hash::sha256_file(&path)?.as_bytes());
        hasher.update([0]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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
        let expected = lifecycle_contract_sha256(self);
        if proof.contract_sha256 != expected {
            return Some("lifecycle_proof 与当前 goal 合约不匹配".into());
        }
        if self.status == GoalStatus::Success && !self.loaded_from_legacy {
            if let Some(migration) = proof.migration.as_deref() {
                if migration != PRE_RECEIPT_MIGRATION || !pre_receipt_migration_eligible(self) {
                    return Some("lifecycle_proof 使用了无效的 pre-receipt migration".into());
                }
                return None;
            }
            let gaps = goal_success_receipt_gaps_for_fingerprint(
                self,
                root,
                &proof.workspace_fingerprint,
                false,
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
    if replacement.lifecycle != GoalLifecycle::Current {
        return Some(format!(
            "superseded_by 目标 {replacement_id} lifecycle={}，必须为 current",
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
    let replacement_gaps = goal_success_receipt_gaps(replacement, root, current_fingerprint);
    if !replacement_gaps.is_empty() {
        return Some(format!(
            "superseded_by 目标 {replacement_id} 尚未 gate-ready: {}",
            replacement_gaps.join("; ")
        ));
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
