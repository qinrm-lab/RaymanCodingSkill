use super::*;
use crate::cli::{
    LaneAction, LaneCmd, PendingAction, PendingCmd, WorkPackageAction, WorkPackageCmd,
};

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

pub fn run_package(store: &goal::GoalStore, json: bool, command: WorkPackageCmd) -> Result<()> {
    match command.action {
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
