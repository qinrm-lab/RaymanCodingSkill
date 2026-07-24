use super::*;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkPackageStatus {
    #[default]
    Open,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkPackage {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub requirement_ids: Vec<String>,
    #[serde(default = "default_required_package")]
    pub required: bool,
    #[serde(default)]
    pub status: WorkPackageStatus,
    #[serde(default)]
    pub progress_receipt_ids: Vec<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

fn default_required_package() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProgressReceipt {
    pub id: String,
    pub package_id: String,
    pub recorded_at: String,
    pub message: String,
    pub command: String,
    pub exit_code: i32,
    pub cwd: String,
    pub workspace_fingerprint_before: String,
    pub workspace_fingerprint_after: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub invocation_sha256: String,
    /// Always false: progress is resumability evidence, never final validation.
    pub authoritative: bool,
}

pub struct ProgressReceiptSubmission {
    pub message: String,
    pub command: String,
    pub exit_code: i32,
    pub cwd: String,
    pub workspace_fingerprint_before: String,
    pub workspace_fingerprint_after: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaneMode {
    AdvisoryReadOnly,
    Writer,
    FinalReviewer,
}

impl LaneMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "advisory_read_only" | "advisory-read-only" => Ok(Self::AdvisoryReadOnly),
            "writer" => Ok(Self::Writer),
            "final_reviewer" | "final-reviewer" => Ok(Self::FinalReviewer),
            _ => bail!(
                "未知 lane mode: {value}（可用: advisory-read-only | writer | final-reviewer）"
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaneStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaneRecord {
    pub id: String,
    pub mode: LaneMode,
    pub opened_at: String,
    pub opening_baseline: WorkspaceBaseline,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub status: LaneStatus,
    #[serde(default)]
    pub closed_at: Option<String>,
    #[serde(default)]
    pub closing_fingerprint: Option<String>,
    #[serde(default)]
    pub delta_paths: Vec<String>,
    #[serde(default)]
    pub violation: Option<String>,
    /// Lane evidence is coordination evidence only, never validation authority.
    pub authoritative: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalSummary {
    pub goal_id: String,
    pub status: GoalStatus,
    pub open_must: usize,
    pub done_must: usize,
    pub open_should: usize,
    pub done_should: usize,
    pub planned_paths: usize,
    pub plan_extensions: usize,
    pub work_packages: usize,
    pub open_packages: usize,
    pub completed_packages: usize,
    pub progress_receipts: usize,
    pub validation_receipts: usize,
    pub authority_receipts: usize,
    pub open_lanes: usize,
    pub closed_lanes: usize,
    pub warnings: Vec<String>,
}

pub fn progress_invocation_sha256(
    goal_id: &str,
    package_id: &str,
    submission: &ProgressReceiptSubmission,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.goal-progress-receipt.v1");
    for value in [
        goal_id,
        package_id,
        submission.message.trim(),
        submission.command.trim(),
        &submission.exit_code.to_string(),
        &submission.cwd,
        &submission.workspace_fingerprint_before,
        &submission.workspace_fingerprint_after,
        &submission.stdout_sha256,
        &submission.stderr_sha256,
    ] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn work_package_graph_error(goal: &Goal) -> Option<String> {
    let requirement_ids = goal
        .requirements
        .iter()
        .map(|requirement| requirement.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut packages = BTreeMap::new();
    for package in &goal.work_packages {
        if package.id.is_empty()
            || !package
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            || package.title.trim().is_empty()
        {
            return Some("work package id/title 无效".into());
        }
        if packages.insert(package.id.as_str(), package).is_some() {
            return Some(format!("重复 work package id: {}", package.id));
        }
        let mut normalized = package.requirement_ids.clone();
        normalized.sort();
        normalized.dedup();
        if normalized != package.requirement_ids
            || normalized
                .iter()
                .any(|requirement| !requirement_ids.contains(requirement.as_str()))
        {
            return Some(format!("work package {} requirement 绑定无效", package.id));
        }
    }
    for package in &goal.work_packages {
        let mut seen = BTreeSet::new();
        let mut cursor = Some(package.id.as_str());
        while let Some(id) = cursor {
            if !seen.insert(id) {
                return Some(format!("work package 父子图包含环: {id}"));
            }
            let Some(current) = packages.get(id) else {
                return Some(format!("work package 父节点不存在: {id}"));
            };
            cursor = current.parent_id.as_deref();
        }
    }

    let mut receipts = BTreeMap::new();
    for receipt in &goal.progress_receipts {
        if receipt.id.trim().is_empty()
            || receipts.insert(receipt.id.as_str(), receipt).is_some()
            || !packages.contains_key(receipt.package_id.as_str())
            || receipt.authoritative
            || receipt.exit_code != 0
            || receipt.message.trim().is_empty()
            || receipt.command.trim().is_empty()
            || receipt.workspace_fingerprint_before != receipt.workspace_fingerprint_after
            || !is_sha256(&receipt.workspace_fingerprint_before)
            || !is_sha256(&receipt.stdout_sha256)
            || !is_sha256(&receipt.stderr_sha256)
        {
            return Some("goal progress receipt 结构或源码绑定无效".into());
        }
        let submission = ProgressReceiptSubmission {
            message: receipt.message.clone(),
            command: receipt.command.clone(),
            exit_code: receipt.exit_code,
            cwd: receipt.cwd.clone(),
            workspace_fingerprint_before: receipt.workspace_fingerprint_before.clone(),
            workspace_fingerprint_after: receipt.workspace_fingerprint_after.clone(),
            stdout_sha256: receipt.stdout_sha256.clone(),
            stderr_sha256: receipt.stderr_sha256.clone(),
        };
        if receipt.invocation_sha256
            != progress_invocation_sha256(&goal.id, &receipt.package_id, &submission)
        {
            return Some(format!("goal progress receipt {} 摘要无效", receipt.id));
        }
    }
    for package in &goal.work_packages {
        if package.status == WorkPackageStatus::Complete {
            if package.completed_at.is_none() {
                return Some(format!("已完成 work package {} 缺少完成时间", package.id));
            }
            if package.required && package.progress_receipt_ids.is_empty() {
                return Some(format!("required work package {} 缺少进度收据", package.id));
            }
            if package.progress_receipt_ids.iter().any(|id| {
                receipts
                    .get(id.as_str())
                    .is_none_or(|receipt| receipt.package_id != package.id)
            }) {
                return Some(format!("work package {} 进度收据引用无效", package.id));
            }
        } else if package.completed_at.is_some() || !package.progress_receipt_ids.is_empty() {
            return Some(format!("open work package {} 含完成态字段", package.id));
        }
    }
    None
}

pub fn lane_ledger_error(goal: &Goal) -> Option<String> {
    let mut ids = BTreeSet::new();
    for lane in &goal.lanes {
        if lane.id.is_empty()
            || !lane
                .id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            || !ids.insert(lane.id.as_str())
            || lane.authoritative
            || !is_sha256(&lane.opening_baseline.workspace_fingerprint)
            || lifecycle::fingerprint_for_files(&lane.opening_baseline.files)
                != lane.opening_baseline.workspace_fingerprint
        {
            return Some("lane ledger id、baseline 或 authority 标记无效".into());
        }
        let mut allowed = lane.allowed_paths.clone();
        normalize_path_list(&mut allowed);
        if allowed != lane.allowed_paths
            || allowed
                .iter()
                .any(|path| !ordinary_workspace_relative_path(path))
            || (lane.mode == LaneMode::Writer && allowed.is_empty())
            || (lane.mode != LaneMode::Writer && !allowed.is_empty())
        {
            return Some(format!("lane {} allowed_paths 与 mode 不匹配", lane.id));
        }
        let mut delta = lane.delta_paths.clone();
        normalize_path_list(&mut delta);
        if delta != lane.delta_paths {
            return Some(format!("lane {} delta_paths 未规范化", lane.id));
        }
        match lane.status {
            LaneStatus::Open => {
                if lane.closed_at.is_some()
                    || lane.closing_fingerprint.is_some()
                    || !lane.delta_paths.is_empty()
                {
                    return Some(format!("open lane {} 含关闭态字段", lane.id));
                }
            }
            LaneStatus::Closed => {
                if lane.closed_at.is_none()
                    || !lane.closing_fingerprint.as_deref().is_some_and(is_sha256)
                    || lane.violation.is_some()
                {
                    return Some(format!("closed lane {} 关闭证明无效", lane.id));
                }
                if lane.mode != LaneMode::Writer && !lane.delta_paths.is_empty() {
                    return Some(format!("read-only/reviewer lane {} 发生源码漂移", lane.id));
                }
                if lane.mode == LaneMode::Writer
                    && lane
                        .delta_paths
                        .iter()
                        .any(|path| !lane.allowed_paths.contains(path))
                {
                    return Some(format!("writer lane {} 越出允许路径", lane.id));
                }
            }
        }
    }
    None
}

impl Goal {
    pub fn summary(&self) -> GoalSummary {
        let count = |kind, status| {
            self.requirements
                .iter()
                .filter(|requirement| requirement.kind == kind && requirement.status == status)
                .count()
        };
        let planned_paths = self
            .plan_receipts
            .first()
            .map(|receipt| receipt.effective_changed_paths().len())
            .unwrap_or(0);
        let plan_extensions = self
            .plan_receipts
            .first()
            .map(|receipt| receipt.extensions.len())
            .unwrap_or(0);
        let mut warnings = Vec::new();
        if planned_paths >= 12 && self.work_packages.is_empty() {
            warnings.push(format!(
                "计划包含 {planned_paths} 个路径但没有 work package；建议分阶段绑定责任和恢复点"
            ));
        }
        if planned_paths >= 12 && self.progress_receipts.is_empty() {
            warnings.push(format!(
                "计划包含 {planned_paths} 个路径但尚无 progress receipt；长任务存在证据悬崖"
            ));
        }
        if plan_extensions >= 3 {
            warnings.push(format!(
                "计划已扩展 {plan_extensions} 次；请复核范围是否仍属于同一目标"
            ));
        }
        GoalSummary {
            goal_id: self.id.clone(),
            status: self.status,
            open_must: count(RequirementKind::Must, RequirementStatus::Open),
            done_must: count(RequirementKind::Must, RequirementStatus::Done),
            open_should: count(RequirementKind::Should, RequirementStatus::Open),
            done_should: count(RequirementKind::Should, RequirementStatus::Done),
            planned_paths,
            plan_extensions,
            work_packages: self.work_packages.len(),
            open_packages: self
                .work_packages
                .iter()
                .filter(|package| package.status == WorkPackageStatus::Open)
                .count(),
            completed_packages: self
                .work_packages
                .iter()
                .filter(|package| package.status == WorkPackageStatus::Complete)
                .count(),
            progress_receipts: self.progress_receipts.len(),
            validation_receipts: self
                .requirements
                .iter()
                .flat_map(|requirement| &requirement.validations)
                .filter(|validation| validation.receipt.is_some())
                .count(),
            authority_receipts: self.authority_receipts.len(),
            open_lanes: self
                .lanes
                .iter()
                .filter(|lane| lane.status == LaneStatus::Open)
                .count(),
            closed_lanes: self
                .lanes
                .iter()
                .filter(|lane| lane.status == LaneStatus::Closed)
                .count(),
            warnings,
        }
    }
}

impl GoalStore {
    pub fn add_work_package(
        &self,
        id: &str,
        package_id: &str,
        title: &str,
        parent_id: Option<&str>,
        mut requirement_ids: Vec<String>,
        required: bool,
    ) -> Result<Goal> {
        if package_id.is_empty()
            || !package_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            || title.trim().is_empty()
        {
            bail!("work package id 只允许字母、数字、下划线和连字符，且标题不能为空");
        }
        requirement_ids
            .iter_mut()
            .for_each(|value| *value = value.trim().to_string());
        requirement_ids.retain(|value| !value.is_empty());
        requirement_ids.sort();
        requirement_ids.dedup();
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema()
            || goal.lifecycle != GoalLifecycle::Current
            || goal.status != GoalStatus::Active
        {
            bail!("只有 current-schema active/current 目标可以增加 work package");
        }
        if goal
            .work_packages
            .iter()
            .any(|package| package.id == package_id)
        {
            bail!("work package 已存在: {package_id}");
        }
        if let Some(parent) = parent_id
            && !goal
                .work_packages
                .iter()
                .any(|package| package.id == parent)
        {
            bail!("父 work package 不存在: {parent}");
        }
        for requirement in &requirement_ids {
            if !goal
                .requirements
                .iter()
                .any(|candidate| candidate.id == *requirement)
            {
                bail!("work package 指向未知 requirement: {requirement}");
            }
        }
        goal.work_packages.push(WorkPackage {
            id: package_id.to_string(),
            title: title.trim().to_string(),
            parent_id: parent_id.map(str::to_string),
            requirement_ids,
            required,
            status: WorkPackageStatus::Open,
            progress_receipt_ids: Vec::new(),
            completed_at: None,
        });
        if let Some(error) = work_package_graph_error(&goal) {
            bail!("work package 图无效: {error}");
        }
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }

    pub fn record_progress_receipt(
        &self,
        id: &str,
        package_id: &str,
        submission: ProgressReceiptSubmission,
    ) -> Result<Goal> {
        if submission.message.trim().is_empty() || submission.command.trim().is_empty() {
            bail!("progress --message 与 --command 都不能为空");
        }
        if submission.exit_code != 0
            || submission.workspace_fingerprint_before != submission.workspace_fingerprint_after
            || !is_sha256(&submission.workspace_fingerprint_before)
            || !is_sha256(&submission.stdout_sha256)
            || !is_sha256(&submission.stderr_sha256)
        {
            bail!("progress receipt 必须证明同一源码快照上的零退出执行与有效输出摘要");
        }
        let expected_fingerprint = submission.workspace_fingerprint_after.clone();
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if !goal.is_current_schema()
            || goal.lifecycle != GoalLifecycle::Current
            || goal.status != GoalStatus::Active
        {
            bail!("只有 current-schema active/current 目标可以记录 progress receipt");
        }
        if workspace_fingerprint(&self.root)? != expected_fingerprint {
            bail!("progress receipt 源码快照已过期");
        }
        let Some(package) = goal
            .work_packages
            .iter()
            .find(|package| package.id == package_id)
        else {
            bail!("work package 不存在: {package_id}");
        };
        if package.status != WorkPackageStatus::Open {
            bail!("work package {package_id} 已完成，不能追加 progress receipt");
        }
        let invocation_sha256 = progress_invocation_sha256(id, package_id, &submission);
        let receipt = ProgressReceipt {
            id: format!("progress_{}", &invocation_sha256[..12]),
            package_id: package_id.to_string(),
            recorded_at: now_iso(),
            message: submission.message.trim().to_string(),
            command: submission.command.trim().to_string(),
            exit_code: submission.exit_code,
            cwd: submission.cwd,
            workspace_fingerprint_before: submission.workspace_fingerprint_before,
            workspace_fingerprint_after: submission.workspace_fingerprint_after,
            stdout_sha256: submission.stdout_sha256,
            stderr_sha256: submission.stderr_sha256,
            invocation_sha256,
            authoritative: false,
        };
        if !goal
            .progress_receipts
            .iter()
            .any(|existing| existing.id == receipt.id)
        {
            goal.progress_receipts.push(receipt);
            goal.updated_at = now_iso();
            if workspace_fingerprint(&self.root)? != expected_fingerprint {
                bail!("progress receipt 写入前源码快照发生漂移");
            }
            write_json(&path, &goal)?;
        }
        Ok(goal)
    }

    pub fn complete_work_package(
        &self,
        id: &str,
        package_id: &str,
        receipt_id: &str,
    ) -> Result<Goal> {
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle != GoalLifecycle::Current || goal.status != GoalStatus::Active {
            bail!("只有 active/current 目标可以完成 work package");
        }
        let current = workspace_fingerprint(&self.root)?;
        let Some(receipt) = goal.progress_receipts.iter().find(|receipt| {
            receipt.id == receipt_id
                && receipt.package_id == package_id
                && receipt.workspace_fingerprint_after == current
        }) else {
            bail!("work package 完成要求同包且绑定当前源码快照的 progress receipt");
        };
        let receipt_id = receipt.id.clone();
        let Some(package) = goal
            .work_packages
            .iter_mut()
            .find(|package| package.id == package_id)
        else {
            bail!("work package 不存在: {package_id}");
        };
        package.status = WorkPackageStatus::Complete;
        package.progress_receipt_ids = vec![receipt_id];
        package.completed_at = Some(now_iso());
        if let Some(error) = work_package_graph_error(&goal) {
            bail!("work package 图无效: {error}");
        }
        if workspace_fingerprint(&self.root)? != current {
            bail!("work package 完成写入前源码快照发生漂移");
        }
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }

    pub fn open_lane(
        &self,
        id: &str,
        lane_id: &str,
        mode: LaneMode,
        mut allowed_paths: Vec<String>,
    ) -> Result<Goal> {
        if lane_id.is_empty()
            || !lane_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            bail!("lane id 只允许字母、数字、下划线和连字符");
        }
        normalize_path_list(&mut allowed_paths);
        if allowed_paths
            .iter()
            .any(|path| !ordinary_workspace_relative_path(path))
        {
            bail!("lane --allow 只接受普通工作区相对文件路径");
        }
        if mode == LaneMode::Writer && allowed_paths.is_empty() {
            bail!("writer lane 必须声明至少一个 --allow 路径");
        }
        if mode != LaneMode::Writer && !allowed_paths.is_empty() {
            bail!("advisory-read-only/final-reviewer lane 不接受 --allow");
        }
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        if goal.lifecycle != GoalLifecycle::Current || goal.status != GoalStatus::Active {
            bail!("只有 active/current 目标可以打开 lane");
        }
        if goal.lanes.iter().any(|lane| lane.id == lane_id) {
            bail!("lane id 已存在: {lane_id}");
        }
        goal.lanes.push(LaneRecord {
            id: lane_id.to_string(),
            mode,
            opened_at: now_iso(),
            opening_baseline: workspace_baseline(&self.root)?,
            allowed_paths,
            status: LaneStatus::Open,
            closed_at: None,
            closing_fingerprint: None,
            delta_paths: Vec::new(),
            violation: None,
            authoritative: false,
        });
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }

    pub fn close_lane(&self, id: &str, lane_id: &str) -> Result<Goal> {
        let path = self.goal_path(id)?;
        let _lock = acquire_state_lock(&path)?;
        let Some(mut goal) = Self::load_goal_file(&path)? else {
            bail!("目标不存在: {id}");
        };
        let current = workspace_baseline(&self.root)?;
        let Some(lane) = goal.lanes.iter_mut().find(|lane| lane.id == lane_id) else {
            bail!("lane 不存在: {lane_id}");
        };
        if lane.status != LaneStatus::Open {
            return Ok(goal);
        }
        let delta = workspace_delta(&lane.opening_baseline, &current);
        let violation = match lane.mode {
            LaneMode::AdvisoryReadOnly | LaneMode::FinalReviewer if !delta.is_empty() => {
                Some(format!("只读 lane 检测到源码漂移: {}", delta.join(", ")))
            }
            LaneMode::Writer => {
                let escaped = delta
                    .iter()
                    .filter(|path| !lane.allowed_paths.contains(*path))
                    .cloned()
                    .collect::<Vec<_>>();
                (!escaped.is_empty())
                    .then(|| format!("writer lane 越出允许路径: {}", escaped.join(", ")))
            }
            _ => None,
        };
        if let Some(violation) = violation {
            lane.violation = Some(violation.clone());
            goal.updated_at = now_iso();
            write_json(&path, &goal)?;
            bail!("lane {lane_id} 关闭被拒绝：{violation}");
        }
        if workspace_fingerprint(&self.root)? != current.workspace_fingerprint {
            bail!("lane {lane_id} 关闭检查期间源码快照发生漂移");
        }
        lane.status = LaneStatus::Closed;
        lane.closed_at = Some(now_iso());
        lane.closing_fingerprint = Some(current.workspace_fingerprint);
        lane.delta_paths = delta;
        lane.violation = None;
        goal.updated_at = now_iso();
        write_json(&path, &goal)?;
        Ok(goal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_packages_reject_cycles_and_progress_never_substitutes_validation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
        let store = GoalStore::new(root);
        let goal = store
            .start("staged work", &[("finish safely".into(), true)])
            .unwrap();
        store
            .add_work_package(
                &goal.id,
                "root",
                "root package",
                None,
                vec!["req_1".into()],
                true,
            )
            .unwrap();
        let mut goal = store
            .add_work_package(
                &goal.id,
                "child",
                "child package",
                Some("root"),
                vec!["req_1".into()],
                true,
            )
            .unwrap();
        goal.work_packages[0].parent_id = Some("child".into());
        assert!(
            goal.current_schema_error()
                .is_some_and(|error| error.contains("包含环"))
        );

        let path = store.goal_path(&goal.id).unwrap();
        let mut persisted = store.get(&goal.id).unwrap().unwrap();
        persisted.work_packages[1].required = false;
        write_json(&path, &persisted).unwrap();
        let fingerprint = workspace_fingerprint(root).unwrap();
        let progress = store
            .record_progress_receipt(
                &goal.id,
                "root",
                ProgressReceiptSubmission {
                    message: "focused stage passed".into(),
                    command: "cargo test -p rayman goal".into(),
                    exit_code: 0,
                    cwd: root.display().to_string(),
                    workspace_fingerprint_before: fingerprint.clone(),
                    workspace_fingerprint_after: fingerprint,
                    stdout_sha256: "a".repeat(64),
                    stderr_sha256: "b".repeat(64),
                },
            )
            .unwrap();
        let receipt = progress.progress_receipts[0].id.clone();
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n").unwrap();
        assert!(
            store
                .complete_work_package(&goal.id, "root", &receipt)
                .is_err(),
            "stale progress must not complete a package"
        );
        std::fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
        store
            .complete_work_package(&goal.id, "root", &receipt)
            .unwrap();
        assert!(
            store.close(&goal.id, "success").is_err(),
            "a progress receipt must never substitute final validation"
        );
    }

    #[test]
    fn lane_ledger_rejects_read_only_drift_and_writer_scope_escape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "one\n").unwrap();
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        let store = GoalStore::new(root);
        let goal = store
            .start("lanes", &[("coordinate".into(), true)])
            .unwrap();
        assert!(
            store
                .open_lane(
                    &goal.id,
                    "escape",
                    LaneMode::Writer,
                    vec!["../outside".into()]
                )
                .is_err()
        );

        store
            .open_lane(&goal.id, "audit", LaneMode::AdvisoryReadOnly, Vec::new())
            .unwrap();
        std::fs::write(root.join("README.md"), "drift\n").unwrap();
        assert!(store.close_lane(&goal.id, "audit").is_err());
        let violated = store.get(&goal.id).unwrap().unwrap();
        assert_eq!(violated.lanes[0].status, LaneStatus::Open);
        assert!(violated.lanes[0].violation.is_some());
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        store.close_lane(&goal.id, "audit").unwrap();

        store
            .open_lane(
                &goal.id,
                "writer",
                LaneMode::Writer,
                vec!["src/lib.rs".into()],
            )
            .unwrap();
        std::fs::write(root.join("README.md"), "escape\n").unwrap();
        assert!(store.close_lane(&goal.id, "writer").is_err());
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "two\n").unwrap();
        let closed = store.close_lane(&goal.id, "writer").unwrap();
        let writer = closed
            .lanes
            .iter()
            .find(|lane| lane.id == "writer")
            .unwrap();
        assert_eq!(writer.status, LaneStatus::Closed);
        assert_eq!(writer.delta_paths, vec!["src/lib.rs"]);
        assert!(!writer.authoritative);
    }

    #[test]
    fn large_goal_summary_warns_before_staged_progress_exists() {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = store.start("large", &[("deliver".into(), true)]).unwrap();
        goal.plan_receipts.push(PlanReceipt {
            recorded_at: now_iso(),
            baseline_fingerprint: goal
                .baseline
                .as_ref()
                .unwrap()
                .workspace_fingerprint
                .clone(),
            changed_paths: (0..12).map(|index| format!("src/{index}.rs")).collect(),
            review_priority: "high".into(),
            impacted_paths: Vec::new(),
            recommended_checks: Vec::new(),
            plan_sha256: String::new(),
            extensions: Vec::new(),
        });
        let summary = goal.summary();
        assert_eq!(summary.planned_paths, 12);
        assert_eq!(summary.warnings.len(), 2);
    }
}
