use super::*;
use crate::cli::{
    ContextPageArgs, LaneAction, LaneCmd, PendingAction, PendingCmd, WorkPackageAction,
    WorkPackageCmd,
};
use rayman::context::{
    ContextDeliveryItem, ContextDeliveryLineRange, ContextDeliveryProvenance,
    ContextDeliveryRecord, ContextDeliveryUnresolved, ContextDeliveryV1,
};

#[derive(Debug, Clone)]
struct CapturedGoalDocument {
    path: String,
    sha256: Option<String>,
    line_end: usize,
    goal: Option<goal::Goal>,
    error: Option<String>,
}

fn goal_document_signature(
    documents: &[CapturedGoalDocument],
) -> Vec<(String, Option<String>, Option<String>)> {
    documents
        .iter()
        .map(|document| {
            (
                document.path.clone(),
                document.sha256.clone(),
                document.error.clone(),
            )
        })
        .collect()
}

fn goal_navigation_contract_error(goal: &goal::Goal) -> Option<String> {
    if goal.loaded_from_legacy {
        return Some(
            "legacy Goal schema is audit history and is not eligible for current-contract budgeted navigation"
                .into(),
        );
    }
    goal.current_schema_error()
        .or_else(|| goal::work_package_graph_error(goal))
        .or_else(|| goal::lane_ledger_error(goal))
}

fn capture_goal_documents_once(root: &Path) -> Result<Vec<CapturedGoalDocument>> {
    let Some(dir) = rayman::state_paths::managed_state_dir(root, Path::new("goals"), false)? else {
        return Ok(Vec::new());
    };
    let mut names = std::fs::read_dir(&dir)
        .with_context(|| format!("failed to enumerate goal state: {}", dir.display()))?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    names.sort();
    let mut documents = Vec::new();
    for name in names {
        if Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
            != Some("json")
        {
            continue;
        }
        let Some(name) = name.to_str() else {
            bail!("goal state contains a non-Unicode JSON file name");
        };
        let display_path = format!(".RaymanCodingSkill/goals/{name}");
        let relative_path = Path::new("goals").join(name);
        match rayman::state_paths::read_managed_state_file(
            root,
            &relative_path,
            "goal navigation member",
        ) {
            Ok(bytes) => {
                let sha256 = rayman::hash::sha256_bytes(&bytes);
                let newline_count = bytes.iter().filter(|byte| **byte == b'\n').count();
                let line_end = (newline_count + usize::from(!bytes.ends_with(b"\n"))).max(1);
                match goal::goal_from_captured_bytes(&bytes) {
                    Ok(goal) => {
                        let error = if name != format!("{}.json", goal.id) {
                            Some(format!(
                                "goal file name does not match embedded id: file={name} id={}",
                                goal.id
                            ))
                        } else {
                            goal_navigation_contract_error(&goal).map(|error| {
                                format!("goal document violates the current contract: {error}")
                            })
                        };
                        documents.push(CapturedGoalDocument {
                            path: display_path,
                            sha256: Some(sha256),
                            line_end,
                            goal: error.is_none().then_some(goal),
                            error,
                        });
                    }
                    Err(error) => documents.push(CapturedGoalDocument {
                        path: display_path,
                        sha256: Some(sha256),
                        line_end,
                        goal: None,
                        error: Some(format!("invalid goal document: {error:#}")),
                    }),
                }
            }
            Err(error) => documents.push(CapturedGoalDocument {
                path: display_path,
                sha256: None,
                line_end: 1,
                goal: None,
                error: Some(format!("goal document stable read failed: {error:#}")),
            }),
        }
    }
    Ok(documents)
}

fn capture_goal_documents(root: &Path) -> Result<Vec<CapturedGoalDocument>> {
    let first = capture_goal_documents_once(root)?;
    let second = capture_goal_documents_once(root)?;
    if goal_document_signature(&first) != goal_document_signature(&second) {
        bail!("goal navigation snapshot changed during capture; restart the query");
    }
    Ok(second)
}

fn select_captured_goal(
    documents: &[CapturedGoalDocument],
    id: &str,
) -> Result<CapturedGoalDocument> {
    if id.is_empty()
        || !id.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
    {
        bail!("invalid goal id: {id}");
    }
    let path = format!(".RaymanCodingSkill/goals/{id}.json");
    documents
        .iter()
        .find(|document| document.path == path)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("目标不存在: {id}"))
        .and_then(|document| {
            if let Some(error) = document.error.as_deref() {
                bail!("cannot build navigation from {path}: {error}");
            }
            Ok(document)
        })
}

fn captured_goal(root: &Path, id: &str) -> Result<CapturedGoalDocument> {
    let documents = capture_goal_documents(root)?;
    select_captured_goal(&documents, id)
}

fn goal_record(
    document: &CapturedGoalDocument,
    reason: impl Into<String>,
    evidence: impl Into<String>,
    attributes: serde_json::Value,
) -> Result<ContextDeliveryRecord> {
    let sha256 = document
        .sha256
        .clone()
        .context("resolved goal document is missing its raw hash")?;
    let serde_json::Value::Object(attributes) = attributes else {
        bail!("goal delivery attributes must be an object");
    };
    Ok(ContextDeliveryRecord::Resolved(ContextDeliveryItem {
        path: document.path.clone(),
        sha256: sha256.clone(),
        line_range: ContextDeliveryLineRange {
            start: 1,
            end: document.line_end,
        },
        reason: reason.into(),
        provenance: vec![ContextDeliveryProvenance {
            kind: "goal_state".into(),
            evidence: evidence.into(),
            source_path: Some(document.path.clone()),
            source_sha256: Some(sha256),
        }],
        attributes: attributes.into_iter().collect(),
    }))
}

fn unresolved_goal_record(document: &CapturedGoalDocument) -> ContextDeliveryRecord {
    ContextDeliveryRecord::Unresolved(ContextDeliveryUnresolved {
        reference: document.path.clone(),
        reason: document
            .error
            .clone()
            .unwrap_or_else(|| "goal document could not be resolved".into()),
        provenance: vec![ContextDeliveryProvenance {
            kind: "goal_state_capture".into(),
            evidence: "directory member was present in the bound snapshot".into(),
            source_path: document.sha256.as_ref().map(|_| document.path.clone()),
            source_sha256: document.sha256.clone(),
        }],
    })
}

fn page_controls(page: ContextPageArgs) -> (usize, Option<String>, usize) {
    (
        page.limit.unwrap_or(DEFAULT_CONTEXT_PAGE_LIMIT),
        page.cursor,
        page.budget_bytes.unwrap_or(DEFAULT_CONTEXT_BUDGET_BYTES),
    )
}

fn build_goal_page(
    records: &[ContextDeliveryRecord],
    snapshot: &serde_json::Value,
    query: &serde_json::Value,
    sort: &str,
    page: ContextPageArgs,
) -> Result<ContextDeliveryV1> {
    let (limit, cursor, budget_bytes) = page_controls(page);
    rayman::context::build_context_delivery_page(
        records,
        rayman::context::context_delivery_identity(snapshot)?,
        rayman::context::context_delivery_identity(query)?,
        rayman::context::context_delivery_identity(&sort)?,
        cursor.as_deref(),
        limit,
        budget_bytes,
    )
}

pub fn run_show(store: &goal::GoalStore, json: bool, id: String) -> Result<()> {
    let Some(goal) = store.get(&id)? else {
        bail!("目标不存在: {id}");
    };
    if json {
        print(&serde_json::to_value(&goal)?);
    } else {
        println!(
            "{} [{}/{}] {}",
            goal.id, goal.lifecycle, goal.status, goal.title
        );
        // Text only: the JSON form is the goal document and callers parse that
        // shape. Repeating the host defect here is what survives compaction,
        // because `goal show` is the command an agent reruns to reorient.
        crate::print_host_patch_probe(&rayman::codex_host::patch_probe(None));
        for req in goal.requirements {
            println!(
                "  {} [{}/{}] {}{}",
                req.id,
                req.kind,
                req.status,
                req.text,
                req.evidence
                    .map(|evidence| format!("  证据: {evidence}"))
                    .unwrap_or_default()
            );
            for validation in &req.validations {
                println!("    validated: {}", validation.command);
            }
            for impact in &req.impacts {
                println!(
                    "    impact: {} deps={} dependents={} candidate_tests={} recommended_checks={}",
                    impact.changed_path,
                    impact.direct_dependencies.len(),
                    impact.direct_dependents.len(),
                    impact.candidate_tests.len(),
                    impact.recommended_checks.len()
                );
            }
        }
    }
    Ok(())
}
pub fn run_summary(store: &goal::GoalStore, json: bool, id: String) -> Result<()> {
    let Some(goal) = store.get(&id)? else {
        bail!("目标不存在: {id}");
    };
    let summary = goal.summary();
    if json {
        print(&serde_json::to_value(&summary)?);
    } else {
        println!(
            "{} [{}] must={}/{} packages={}/{} progress={} validation={} authority={}",
            summary.goal_id,
            summary.status,
            summary.done_must,
            summary.done_must + summary.open_must,
            summary.completed_packages,
            summary.work_packages,
            summary.progress_receipts,
            summary.validation_receipts,
            summary.authority_receipts
        );
        println!(
            "  plan: paths={} extensions={}",
            summary.planned_paths, summary.plan_extensions
        );
        for warning in summary.warnings {
            println!("  warning: {warning}");
        }
    }
    Ok(())
}

fn normalize_goal_filter(
    mut values: Vec<String>,
    allowed: &[&str],
    label: &str,
) -> Result<Vec<String>> {
    values = values
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    values.sort();
    values.dedup();
    if let Some(value) = values
        .iter()
        .find(|value| !allowed.contains(&value.as_str()))
    {
        bail!(
            "unknown goal {label} filter `{value}`; available values: {}",
            allowed.join(",")
        );
    }
    Ok(values)
}

pub fn run_budgeted_list(
    root: &Path,
    lifecycle: Vec<String>,
    status: Vec<String>,
    page: ContextPageArgs,
) -> Result<()> {
    let lifecycle = normalize_goal_filter(
        lifecycle,
        &["current", "archived", "superseded"],
        "lifecycle",
    )?;
    let status = normalize_goal_filter(
        status,
        &["active", "success", "partial", "blocked"],
        "status",
    )?;
    let documents = capture_goal_documents(root)?;
    let snapshot = json!(goal_document_signature(&documents));
    let mut ordered = Vec::new();
    for document in &documents {
        let Some(goal) = document.goal.as_ref() else {
            ordered.push((
                format!("1:{}", document.path),
                unresolved_goal_record(document),
            ));
            continue;
        };
        if !lifecycle.is_empty()
            && lifecycle
                .binary_search(&goal.lifecycle.as_str().to_string())
                .is_err()
        {
            continue;
        }
        if !status.is_empty()
            && status
                .binary_search(&goal.status.as_str().to_string())
                .is_err()
        {
            continue;
        }
        ordered.push((
            format!("0:{}:{}", goal.created_at, goal.id),
            goal_record(
                document,
                "goal list match",
                format!("created_at={} id={}", goal.created_at, goal.id),
                serde_json::to_value(super::compact_goal_record(goal))?,
            )?,
        ));
    }
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let records = ordered
        .into_iter()
        .map(|(_, record)| record)
        .collect::<Vec<_>>();
    let envelope = build_goal_page(
        &records,
        &snapshot,
        &json!({
            "command":"goal list",
            "lifecycle":lifecycle,
            "status":status,
        }),
        "goal-list:created_at-asc,id-asc,unresolved-path-asc:v1",
        page,
    )?;
    emit_context_delivery(envelope)
}

fn safe_path_arguments(paths: &[String]) -> Option<String> {
    (!paths.is_empty()
        && paths.iter().all(|path| {
            !path.is_empty()
                && path.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
                })
        }))
    .then(|| paths.join(" "))
}

fn next_goal_command(
    root: &Path,
    goal: &goal::Goal,
    current: &goal::WorkspaceBaseline,
    delta: &goal::GoalPlanDelta,
    frontier: &goal::FrontierReport,
    missing_requirements: &[&goal::Requirement],
    all_goals: &[goal::Goal],
) -> (String, bool) {
    if frontier.consultation == goal::FrontierConsultation::Ready {
        return ("rayman goal pending render --current".into(), false);
    }
    if matches!(
        frontier.execution,
        goal::FrontierExecution::PausedForUser | goal::FrontierExecution::WaitExternal
    ) {
        return (format!("rayman goal frontier {}", goal.id), false);
    }
    if !delta.plan_recorded && delta.plan_required {
        let paths = safe_path_arguments(&delta.actual_changed_paths);
        return (
            format!(
                "rayman goal plan {} {} --check",
                goal.id,
                paths.as_deref().unwrap_or("<paths...>")
            ),
            paths.is_none(),
        );
    }
    if !delta.unplanned_changed_paths.is_empty() {
        let paths = safe_path_arguments(&delta.unplanned_changed_paths);
        return (
            format!(
                "rayman goal plan {} {} --check --extend",
                goal.id,
                paths.as_deref().unwrap_or("<paths...>")
            ),
            paths.is_none(),
        );
    }
    if let Some(package) = goal
        .work_packages
        .iter()
        .find(|package| package.required && package.status == goal::WorkPackageStatus::Open)
    {
        if let Some(progress) = goal.progress_receipts.iter().rev().find(|receipt| {
            receipt.package_id == package.id
                && receipt.workspace_fingerprint_after == current.workspace_fingerprint
        }) {
            return (
                format!(
                    "rayman goal package complete {} {} --progress {}",
                    goal.id, package.id, progress.id
                ),
                false,
            );
        }
        return (
            format!(
                "rayman goal progress {} --package {} --message <message> --command <command>",
                goal.id, package.id
            ),
            true,
        );
    }
    if let Some(requirement) = missing_requirements.first() {
        return (
            format!(
                "rayman goal validate {} --req {} --message <message> --changed <path> --command <command>",
                goal.id, requirement.id
            ),
            true,
        );
    }
    let high_priority = goal
        .plan_receipts
        .first()
        .is_some_and(|receipt| receipt.effective_review_priority() == "high");
    if high_priority
        && !goal
            .review_receipts
            .iter()
            .any(|review| review.source_fingerprint == current.workspace_fingerprint)
    {
        return (
            format!(
                "rayman goal review {} --reviewer <reviewer> --message <review>",
                goal.id
            ),
            true,
        );
    }
    if !goal::has_current_stable_authority_receipt_with_baseline(goal, all_goals, root, current) {
        let requirement = goal
            .requirements
            .iter()
            .find(|requirement| requirement.kind == goal::RequirementKind::Must)
            .map(|requirement| requirement.id.as_str())
            .unwrap_or("<requirement>");
        return (
            format!(
                "rayman goal validate {} --req {} --message <evidence> --changed <path> --command <workspace-gate> --authority --repeat 2",
                goal.id, requirement
            ),
            true,
        );
    }
    match goal.status {
        goal::GoalStatus::Active => (
            format!("rayman goal close {} --status success", goal.id),
            false,
        ),
        goal::GoalStatus::Success if frontier.decision == goal::FrontierDecision::Complete => {
            (format!("rayman finish --goal {}", goal.id), false)
        }
        goal::GoalStatus::Success => (format!("rayman goal frontier {}", goal.id), false),
        goal::GoalStatus::Partial | goal::GoalStatus::Blocked => {
            (format!("rayman goal frontier {}", goal.id), false)
        }
    }
}

pub fn run_brief(root: &Path, id: String, page: ContextPageArgs) -> Result<()> {
    let documents = capture_goal_documents(root)?;
    let document = select_captured_goal(&documents, &id)?;
    let goal = document
        .goal
        .as_ref()
        .context("captured goal document did not parse")?;
    let current = goal::workspace_baseline(root)?;
    let delta = goal::goal_plan_delta(goal, &current)?;
    let pending = goal::PendingStore::new(root);
    let frontier = pending.frontier(goal)?;
    let terminal_documents = capture_goal_documents(root)?;
    let terminal_document = select_captured_goal(&terminal_documents, &id)?;
    let terminal_goal = terminal_document
        .goal
        .as_ref()
        .context("terminal captured goal document did not parse")?;
    let terminal_current = goal::workspace_baseline(root)?;
    let terminal_frontier = pending.frontier(terminal_goal)?;
    if goal_document_signature(&documents) != goal_document_signature(&terminal_documents)
        || document.sha256 != terminal_document.sha256
        || current.workspace_fingerprint != terminal_current.workspace_fingerprint
        || frontier != terminal_frontier
    {
        bail!("goal brief inputs changed during capture; restart the query");
    }
    let snapshot = json!({
        "goal_document_sha256":document.sha256,
        "goal_documents":goal_document_signature(&documents),
        "workspace_fingerprint":current.workspace_fingerprint,
        "frontier":frontier,
    });
    let mut records = Vec::new();
    records.push(goal_record(
        &document,
        "goal overview",
        "compact recovery projection",
        json!({
            "record_type":"overview",
            "goal_id":goal.id,
            "title":goal.title,
            "lifecycle":goal.lifecycle,
            "status":goal.status,
            "updated_at":goal.updated_at,
            "current_fingerprint":current.workspace_fingerprint,
            "open_requirements":goal.requirements.iter().filter(|requirement| requirement.status == goal::RequirementStatus::Open).count(),
            "packages":goal.work_packages.len(),
            "recent_progress_count":goal.progress_receipts.len().min(5),
        }),
    )?);
    for requirement in goal
        .requirements
        .iter()
        .filter(|requirement| requirement.status == goal::RequirementStatus::Open)
    {
        records.push(goal_record(
            &document,
            "open requirement",
            requirement.id.clone(),
            json!({
                "record_type":"open_requirement",
                "requirement_id":requirement.id,
                "text":requirement.text,
                "kind":requirement.kind,
                "proof_kind":requirement.proof_kind,
                "status":requirement.status,
            }),
        )?);
    }
    for (position, package) in goal.work_packages.iter().enumerate() {
        records.push(goal_record(
            &document,
            "work package DAG node",
            package.id.clone(),
            json!({
                "record_type":"package",
                "position":position,
                "package_id":package.id,
                "title":package.title,
                "parent_id":package.parent_id,
                "requirement_ids":package.requirement_ids,
                "required":package.required,
                "status":package.status,
                "progress_receipt_ids":package.progress_receipt_ids,
            }),
        )?);
    }
    records.push(goal_record(
        &document,
        "current plan delta summary",
        "fresh workspace baseline comparison",
        json!({
            "record_type":"delta_summary",
            "current_fingerprint":delta.current_fingerprint,
            "actual_changed_count":delta.actual_changed_paths.len(),
            "planned_changed_count":delta.planned_changed_paths.len(),
            "unplanned_changed_count":delta.unplanned_changed_paths.len(),
            "plan_recorded":delta.plan_recorded,
            "plan_required":delta.plan_required,
            "covered":delta.covered,
        }),
    )?);
    for path in &delta.actual_changed_paths {
        records.push(goal_record(
            &document,
            "current changed path",
            path.clone(),
            json!({
                "record_type":"delta_path",
                "changed_path":path,
                "planned":delta.planned_changed_paths.binary_search(path).is_ok(),
                "unplanned":delta.unplanned_changed_paths.binary_search(path).is_ok(),
            }),
        )?);
    }
    let mut recent = goal.progress_receipts.iter().collect::<Vec<_>>();
    recent.sort_by(|left, right| {
        right
            .recorded_at
            .cmp(&left.recorded_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    for receipt in recent.into_iter().take(5) {
        records.push(goal_record(
            &document,
            "recent progress receipt",
            receipt.id.clone(),
            json!({
                "record_type":"recent_progress",
                "progress_id":receipt.id,
                "package_id":receipt.package_id,
                "recorded_at":receipt.recorded_at,
                "message":receipt.message,
                "command":receipt.command,
                "source_fingerprint":receipt.workspace_fingerprint_after,
                "authoritative":false,
            }),
        )?);
    }
    let missing_requirements = goal
        .requirements
        .iter()
        .filter(|requirement| {
            requirement.kind == goal::RequirementKind::Must
                && !requirement.validations.iter().any(|validation| {
                    goal::validation_has_current_receipt_with_baseline(
                        validation,
                        goal,
                        requirement,
                        root,
                        &current,
                    )
                })
        })
        .collect::<Vec<_>>();
    for requirement in &missing_requirements {
        records.push(goal_record(
            &document,
            "missing current validation",
            requirement.id.clone(),
            json!({
                "record_type":"missing_validation",
                "requirement_id":requirement.id,
                "text":requirement.text,
                "proof_kind":requirement.proof_kind,
                "reason":"no successful receipt bound to the current workspace fingerprint was observed; navigation only",
            }),
        )?);
    }
    records.push(goal_record(
        &document,
        "current execution frontier",
        frontier.reason.clone(),
        json!({
            "record_type":"frontier",
            "decision":frontier.decision,
            "execution":frontier.execution,
            "consultation":frontier.consultation,
            "ask_user_allowed":frontier.ask_user_allowed,
            "background_execution_allowed":frontier.background_execution_allowed,
            "reason":frontier.reason,
            "blocker_ids":frontier.blockers.iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
        }),
    )?);
    let (next_command, arguments_required) = next_goal_command(
        root,
        goal,
        &current,
        &delta,
        &frontier,
        &missing_requirements,
        &documents
            .iter()
            .filter_map(|document| document.goal.clone())
            .collect::<Vec<_>>(),
    );
    records.push(goal_record(
        &document,
        "next command",
        next_command.clone(),
        json!({
            "record_type":"next_command",
            "command":next_command,
            "arguments_required":arguments_required,
        }),
    )?);
    let envelope = build_goal_page(
        &records,
        &snapshot,
        &json!({"command":"goal brief","goal_id":id}),
        "goal-brief:overview;open-requirement-goal-order;package-vector-order;delta-summary-then-path-order;recent-progress-recorded-at-desc-id-desc;missing-validation-goal-order;frontier;next-command:v1",
        page,
    )?;
    emit_context_delivery(envelope)
}

fn package_record(
    document: &CapturedGoalDocument,
    package: &goal::WorkPackage,
    position: usize,
) -> Result<ContextDeliveryRecord> {
    goal_record(
        document,
        "work package DAG node",
        package.id.clone(),
        json!({
            "record_type":"package",
            "position":position,
            "package_id":package.id,
            "title":package.title,
            "parent_id":package.parent_id,
            "requirement_ids":package.requirement_ids,
            "required":package.required,
            "status":package.status,
            "progress_receipt_ids":package.progress_receipt_ids,
            "completed_at":package.completed_at,
        }),
    )
}

fn run_package_list(root: &Path, id: String, page: ContextPageArgs) -> Result<()> {
    let document = captured_goal(root, &id)?;
    let goal = document
        .goal
        .as_ref()
        .context("goal document did not parse")?;
    let records = goal
        .work_packages
        .iter()
        .enumerate()
        .map(|(position, package)| package_record(&document, package, position))
        .collect::<Result<Vec<_>>>()?;
    let envelope = build_goal_page(
        &records,
        &json!({"goal_document_sha256":document.sha256}),
        &json!({"command":"goal package list","goal_id":id}),
        "goal-package-list:vector-position,id:v1",
        page,
    )?;
    emit_context_delivery(envelope)
}

fn run_package_show(
    root: &Path,
    id: String,
    package_id: String,
    page: ContextPageArgs,
) -> Result<()> {
    let document = captured_goal(root, &id)?;
    let goal = document
        .goal
        .as_ref()
        .context("goal document did not parse")?;
    let (position, package) = goal
        .work_packages
        .iter()
        .enumerate()
        .find(|(_, package)| package.id == package_id)
        .ok_or_else(|| anyhow::anyhow!("goal {id} work package does not exist: {package_id}"))?;
    let mut records = vec![package_record(&document, package, position)?];
    for requirement_id in &package.requirement_ids {
        let requirement = goal
            .requirements
            .iter()
            .find(|requirement| &requirement.id == requirement_id)
            .context("work package references a missing requirement")?;
        records.push(goal_record(
            &document,
            "package requirement",
            requirement.id.clone(),
            json!({
                "record_type":"package_requirement",
                "package_id":package.id,
                "requirement_id":requirement.id,
                "text":requirement.text,
                "kind":requirement.kind,
                "proof_kind":requirement.proof_kind,
                "status":requirement.status,
            }),
        )?);
    }
    for progress_id in &package.progress_receipt_ids {
        let receipt = goal
            .progress_receipts
            .iter()
            .find(|receipt| &receipt.id == progress_id)
            .context("work package references a missing progress receipt")?;
        records.push(goal_record(
            &document,
            "package progress receipt",
            receipt.id.clone(),
            json!({
                "record_type":"package_progress",
                "package_id":package.id,
                "progress_id":receipt.id,
                "recorded_at":receipt.recorded_at,
                "message":receipt.message,
                "command":receipt.command,
                "source_fingerprint":receipt.workspace_fingerprint_after,
                "authoritative":false,
            }),
        )?);
    }
    for (child_position, child) in goal
        .work_packages
        .iter()
        .enumerate()
        .filter(|(_, child)| child.parent_id.as_deref() == Some(package.id.as_str()))
    {
        records.push(package_record(&document, child, child_position)?);
    }
    let envelope = build_goal_page(
        &records,
        &json!({"goal_document_sha256":document.sha256}),
        &json!({"command":"goal package show","goal_id":id,"package_id":package_id}),
        "goal-package-show:package,requirements,progress,children:v1",
        page,
    )?;
    emit_context_delivery(envelope)
}

pub fn run_plan(
    root: &Path,
    store: &goal::GoalStore,
    json: bool,
    id: String,
    paths: Vec<String>,
    check: bool,
    extend: bool,
) -> Result<()> {
    if extend {
        // A legitimate extension normally happens after already planned files changed,
        // which makes the old index stale by design.
        context::refresh(root)?;
    }
    let project_map = map::build_readonly(root)?;
    let report = map::change_plan(&project_map, &paths)?;
    if !report.ready {
        let mode = if check { " --check" } else { "" };
        bail!("goal plan{mode} blocked: {}", report.blockers.join("; "));
    }
    let submission = goal::PlanReceiptSubmission {
        changed_paths: report.changed_paths,
        review_priority: report.review_priority,
        impacted_paths: report
            .impacted_files
            .iter()
            .map(|file| file.path.clone())
            .collect(),
        recommended_checks: report.recommended_checks,
    };
    let goal = if extend {
        store.extend_plan(&id, submission)?
    } else {
        store.record_plan(&id, submission)?
    };
    if json {
        print(&serde_json::to_value(&goal)?);
    } else {
        let receipt = goal.plan_receipts.last().expect("recorded plan");
        println!(
            "goal {} plan receipt recorded: priority={} changed={} sha256={} extensions={}",
            goal.id,
            receipt.effective_review_priority(),
            receipt.effective_changed_paths().len(),
            receipt.effective_plan_sha256(),
            receipt.extensions.len()
        );
    }
    Ok(())
}

pub fn run_review(
    store: &goal::GoalStore,
    json: bool,
    id: String,
    reviewer: String,
    message: String,
) -> Result<()> {
    let goal = store.record_review(&id, &reviewer, &message)?;
    if json {
        print(&serde_json::to_value(&goal)?);
    } else {
        let receipt = goal.review_receipts.last().expect("recorded review");
        println!(
            "goal {} review receipt recorded: reviewer={} source_fingerprint={}",
            goal.id, receipt.reviewer, receipt.source_fingerprint
        );
    }
    Ok(())
}

pub fn run_package(
    root: &Path,
    store: &goal::GoalStore,
    json: bool,
    command: WorkPackageCmd,
) -> Result<()> {
    match command.action {
        WorkPackageAction::List { goal: id, page } => run_package_list(root, id, page)?,
        WorkPackageAction::Show {
            goal: id,
            id: package_id,
            page,
        } => run_package_show(root, id, package_id, page)?,
        WorkPackageAction::Add {
            goal: id,
            id: package_id,
            title,
            parent,
            requirements,
            optional,
        } => {
            let goal = store.add_work_package(
                &id,
                &package_id,
                &title,
                parent.as_deref(),
                requirements,
                !optional,
            )?;
            if json {
                print(&serde_json::to_value(goal.summary())?);
            } else {
                println!("goal {id} work package {package_id} 已创建");
            }
        }
        WorkPackageAction::Complete {
            goal: id,
            id: package_id,
            progress,
        } => {
            let goal = store.complete_work_package(&id, &package_id, &progress)?;
            if json {
                print(&serde_json::to_value(goal.summary())?);
            } else {
                println!("goal {id} work package {package_id} 已完成");
            }
        }
    }
    Ok(())
}

pub fn run_lane(store: &goal::GoalStore, json: bool, command: LaneCmd) -> Result<()> {
    match command.action {
        LaneAction::Open {
            goal: id,
            id: lane_id,
            mode,
            allowed_paths,
        } => {
            let mode = goal::LaneMode::parse(&mode)?;
            let goal = store.open_lane(&id, &lane_id, mode, allowed_paths)?;
            if json {
                print(&serde_json::to_value(goal.summary())?);
            } else {
                println!("goal {id} lane {lane_id} 已打开（mode={mode:?}）");
            }
        }
        LaneAction::Close {
            goal: id,
            id: lane_id,
        } => {
            let goal = store.close_lane(&id, &lane_id)?;
            let lane = goal
                .lanes
                .iter()
                .find(|lane| lane.id == lane_id)
                .expect("closed lane exists");
            if json {
                print(&serde_json::to_value(lane)?);
            } else {
                println!(
                    "goal {id} lane {lane_id} 已关闭：delta={} authoritative=false",
                    lane.delta_paths.len()
                );
            }
        }
    }
    Ok(())
}

pub fn run_progress(
    root: &Path,
    store: &goal::GoalStore,
    json: bool,
    id: String,
    package: String,
    message: String,
    command: String,
) -> Result<()> {
    let parsed = goal::parse_validation_command(&command)?;
    // `goal validate` runs this before spawning anything (it refuses shells,
    // control operators and command substitution). progress spawns the very same
    // way, so skipping it here made the weaker twin path the easy one. Only the
    // containment half applies: progress records explicitly non-authoritative
    // evidence, so the receipt-grade "a test command must execute tests" rule
    // would reject legitimate progress commands such as a collect-only run.
    goal::validate_command_containment(root, &parsed)?;
    let before = goal::workspace_fingerprint(root)?;
    let output = run_validation_command(root, &parsed)?;
    let after = goal::workspace_fingerprint(root)?;
    if !output.status.success() {
        bail!(
            "progress 命令失败（exit={}）；不会写入 receipt",
            output.status.code().unwrap_or(-1)
        );
    }
    if before != after {
        bail!("progress 命令修改了源码快照；不会写入 receipt");
    }
    let goal = store.record_progress_receipt(
        &id,
        &package,
        goal::ProgressReceiptSubmission {
            message,
            command,
            exit_code: output.status.code().unwrap_or(0),
            cwd: root.display().to_string(),
            workspace_fingerprint_before: before,
            workspace_fingerprint_after: after,
            stdout_sha256: sha256_hex(&output.stdout),
            stderr_sha256: sha256_hex(&output.stderr),
        },
    )?;
    let receipt = goal.progress_receipts.last().expect("recorded progress");
    if json {
        print(&serde_json::to_value(receipt)?);
    } else {
        println!(
            "goal {} package {} progress receipt {} 已记录（non-authoritative）",
            id, package, receipt.id
        );
    }
    Ok(())
}

pub fn run_pending(
    store: &goal::GoalStore,
    pending: &goal::PendingStore,
    json: bool,
    command: PendingCmd,
) -> Result<()> {
    match command.action {
        PendingAction::Add {
            title,
            message,
            goal: goal_id,
            owner,
            kind,
            attempts,
            evidence_paths,
            minimum_input,
            recommended,
            alternatives,
            risk,
            resume_command,
            auto_resume_condition,
            consultation_timing,
            background_mechanism,
            background_authority_evidence,
            background_isolation_evidence,
            capability_key,
            boundary_class,
        } => {
            if let Some(goal_id) = goal_id.as_deref()
                && store.get(goal_id)?.is_none()
            {
                bail!("pending 绑定的 goal 不存在: {goal_id}");
            }
            let owner = goal::PendingOwner::parse(&owner)?;
            let submission = goal::PendingSubmission {
                title,
                detail: message,
                goal_id,
                owner,
                kind: goal::PendingKind::parse(&kind)?,
                attempts,
                evidence_paths,
                minimum_input,
                recommended_action: recommended,
                alternatives,
                risk,
                resume_command,
                auto_resume_condition,
                consultation_timing: goal::ConsultationTiming::parse(&consultation_timing)?,
                background_mechanism,
                background_authority_evidence,
                background_isolation_evidence,
            };
            let capability_bound = owner != goal::PendingOwner::Agent
                || capability_key.is_some()
                || boundary_class.is_some();
            let item = if capability_bound {
                pending.add_capability_bound(submission, capability_key, boundary_class)?
            } else {
                pending.add_structured(submission)?
            };
            if json {
                print(&serde_json::to_value(&item)?);
            } else {
                println!("已记录待完成项 {}", item.id);
            }
        }
        PendingAction::List => {
            let items = pending.list()?;
            if json {
                print(&serde_json::to_value(&items)?);
            } else if items.is_empty() {
                println!("无待完成项。");
            } else {
                for item in items {
                    println!("{}  {}  {}", item.id, item.title, item.detail);
                }
            }
        }
        PendingAction::Render {
            goal: goal_id,
            current,
        } => {
            let selected = match (goal_id, current) {
                (Some(goal_id), false) => {
                    let Some(selected) = store.get(&goal_id)? else {
                        bail!("pending 绑定的 goal 不存在: {goal_id}");
                    };
                    vec![selected]
                }
                (None, true) => {
                    let (goals, issues) = store.list_with_issues()?;
                    if !issues.is_empty() {
                        bail!(
                            "pending render --current 发现损坏的 goal state: {}",
                            issues
                                .iter()
                                .map(|issue| format!("{}: {}", issue.path, issue.error))
                                .collect::<Vec<_>>()
                                .join("; ")
                        );
                    }
                    let mut ready = Vec::new();
                    for goal in goals
                        .into_iter()
                        .filter(|goal| goal.lifecycle == goal::GoalLifecycle::Current)
                    {
                        if pending.frontier(&goal)?.consultation
                            == goal::FrontierConsultation::Ready
                        {
                            ready.push(goal);
                        }
                    }
                    if ready.is_empty() {
                        bail!("pending render --current 没有当前可咨询的 goal");
                    }
                    ready
                }
                _ => bail!("pending render 必须且只能指定 --goal <id> 或 --current"),
            };
            let rendered = pending.render_for_goals(&selected)?;
            if json {
                print(&serde_json::to_value(&rendered)?);
            } else {
                // This is client-neutral protocol text, not a localizable
                // status line. Host adapters may compare the exact bytes.
                std::println!("{}", rendered.text);
            }
        }
        PendingAction::Migrate {
            id,
            goal: goal_id,
            legacy_package_sha256,
            capability_key,
            boundary_class,
        } => {
            if store.get(&goal_id)?.is_none() {
                bail!("pending 绑定的 goal 不存在: {goal_id}");
            }
            let item = pending.migrate_legacy(
                &id,
                &goal_id,
                &legacy_package_sha256,
                &capability_key,
                &boundary_class,
            )?;
            if json {
                print(&serde_json::to_value(&item)?);
            } else {
                println!("已迁移 legacy pending {}。", item.id);
            }
        }
        PendingAction::Present {
            id: _,
            goal: _,
            package_sha256: _,
            channel: _,
            reference: _,
        } => {
            bail!(
                "`goal pending present` 已退役：代理不能自证用户展示。使用 `rayman goal pending render --current` 生成 client-neutral workspace aggregate；渲染本身不构成展示、送达或完成证据"
            );
        }
        PendingAction::Resolve { id } => {
            // 与 goal show 同理：解决一个不存在的待完成项必须非零退出，
            // 否则调用方会把"没找到"当成"已解决"。
            if !pending.resolve(&id)? {
                bail!("待完成项不存在: {id}");
            }
            if json {
                print(&json!({ "resolved": true, "id": id }));
            } else {
                println!("已解决待完成项。");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_and_summary_reject_unknown_goal_ids() {
        let root = tempfile::tempdir().unwrap();
        let store = goal::GoalStore::new(root.path());
        assert!(run_show(&store, true, "missing".into()).is_err());
        assert!(run_summary(&store, true, "missing".into()).is_err());
    }
}
