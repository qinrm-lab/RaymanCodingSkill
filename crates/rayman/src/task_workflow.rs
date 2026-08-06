use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::json;

use crate::cli::TaskWorkflowCmd;

#[derive(Debug, Serialize)]
struct CommandInvocation {
    program: &'static str,
    args: Vec<String>,
}

#[derive(Debug, Serialize)]
struct PrepareReadinessSnapshot {
    scope: &'static str,
    workspace_fingerprint: String,
    goal_state_sha256: String,
    verified_at: String,
    stability: &'static str,
}

fn plan_invocation(goal_id: &str, changed_paths: &[String], extend: bool) -> CommandInvocation {
    let mut args = vec!["goal".into(), "plan".into(), goal_id.into()];
    args.extend(changed_paths.iter().cloned());
    if extend {
        args.push("--extend".into());
    }
    args.push("--check".into());
    CommandInvocation {
        program: "rayman",
        args,
    }
}

fn checkpoint_restore_invocation(store: Option<&Path>, checkpoint_id: &str) -> CommandInvocation {
    let mut args = vec!["checkpoint".into()];
    if let Some(store) = store {
        args.push("--dir".into());
        args.push(rayman::pathfmt::display_path(store));
    }
    args.extend(["restore".into(), checkpoint_id.into(), "--yes".into()]);
    CommandInvocation {
        program: "rayman",
        args,
    }
}

fn goal_state_sha256(goal: &rayman::goal::Goal) -> Result<String> {
    Ok(rayman::hash::sha256_bytes(&serde_json::to_vec(goal)?))
}

fn ensure_prepare_plan_covered(goal_id: &str, delta: &rayman::goal::GoalPlanDelta) -> Result<()> {
    if delta.covered {
        return Ok(());
    }

    if delta.plan_recorded {
        let invocation = plan_invocation(goal_id, &delta.unplanned_changed_paths, true);
        bail!(
            "prepare 发现未计划的实际变更: {}。prepare 不会自动扩展 plan；先将这些路径恢复到 goal baseline，再按 program/args 逐参数调用: {}",
            serde_json::to_string(&delta.unplanned_changed_paths)?,
            serde_json::to_string(&invocation)?
        );
    }
    let invocation = plan_invocation(goal_id, &delta.actual_changed_paths, false);
    bail!(
        "prepare 发现实际变更 {} 个文件但缺少首次修改前的 goal plan receipt: {}。prepare 不会事后补 plan；先将这些路径恢复到 goal baseline，再按 program/args 逐参数调用: {}",
        delta.actual_changed_paths.len(),
        serde_json::to_string(&delta.actual_changed_paths)?,
        serde_json::to_string(&invocation)?
    );
}

/// Compare the exact hashes consumed by context refresh with the freshly
/// hashed snapshot used for goal-plan reconciliation.  A second tree walk by
/// itself is not enough: without this binding prepare could report a current
/// delta alongside an index produced from different source bytes.
fn context_snapshot_mismatches(
    index: &rayman::context::ContextIndex,
    current: &rayman::goal::WorkspaceBaseline,
) -> Vec<String> {
    let mut indexed = BTreeMap::new();
    let mut mismatches = Vec::new();
    for entry in &index.files {
        if indexed
            .insert(entry.path.clone(), entry.sha256.clone())
            .is_some()
        {
            mismatches.push(entry.path.clone());
        }
    }
    let paths = indexed
        .keys()
        .chain(current.files.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    mismatches.extend(
        paths
            .into_iter()
            .filter(|path| indexed.get(path) != current.files.get(path)),
    );
    mismatches.sort();
    mismatches.dedup();
    mismatches
}

/// The newest verified snapshot across every store this workspace actually
/// uses: the default user-profile root, the store autosave was configured with,
/// and the workspace-local one the state audit allowlists (which the workflow
/// reference tells agents to use in workspace-only sandboxes). Consulting only
/// the default root reported `checkpoint: none` to an agent resuming from a
/// snapshot that exists.
/// Also returns the store the snapshot came from. Reporting only an id left the
/// agent unable to act on it: `checkpoint status`/`restore` resolve `--dir`
/// independently and default to the user-profile root, so a snapshot found in
/// the workspace-local store was invisible to every command that could use it.
fn latest_checkpoint_across_stores(
    root: &Path,
) -> Result<
    Option<(
        rayman::checkpoint::CheckpointInfo,
        Option<std::path::PathBuf>,
    )>,
> {
    let mut stores: Vec<Option<std::path::PathBuf>> = vec![None];
    if let Some(dir) = rayman::autosave::configured_checkpoint_dir(root) {
        stores.push(Some(dir));
    }
    let workspace_local = std::path::PathBuf::from(".RaymanCodingSkill/checkpoints");
    if !stores
        .iter()
        .any(|store| store.as_deref() == Some(workspace_local.as_path()))
    {
        stores.push(Some(workspace_local));
    }

    let mut best: Option<(
        rayman::checkpoint::CheckpointInfo,
        Option<std::path::PathBuf>,
    )> = None;
    for store in &stores {
        // A store that cannot be read (absent, denied) must not mask the others.
        let Ok(Some(candidate)) = rayman::checkpoint::latest(root, store.as_deref()) else {
            continue;
        };
        let candidate_created = candidate
            .manifest
            .as_ref()
            .map(|manifest| manifest.created_at.clone())
            .unwrap_or_default();
        let better = best.as_ref().is_none_or(|(current, _)| {
            current
                .manifest
                .as_ref()
                .map(|manifest| manifest.created_at.clone())
                .unwrap_or_default()
                < candidate_created
        });
        if better {
            best = Some((candidate, store.clone()));
        }
    }
    Ok(best)
}

pub(crate) fn run_prepare(root: &Path, json_output: bool, cmd: TaskWorkflowCmd) -> Result<()> {
    run_prepare_with_phase_hook(root, json_output, cmd, || {})
}

fn run_prepare_with_phase_hook(
    root: &Path,
    json_output: bool,
    cmd: TaskWorkflowCmd,
    after_snapshot_verified: impl FnOnce(),
) -> Result<()> {
    let store = rayman::goal::GoalStore::new(root);
    let (goal_id, goal_status, summary, refresh, goal_delta, captured_goal_state_sha256) =
        store.with_locked_goal(&cmd.goal, |selected| {
            if selected.lifecycle != rayman::goal::GoalLifecycle::Current
                || selected.status != rayman::goal::GoalStatus::Active
            {
                bail!(
                    "prepare 要求 current active goal；{} 当前 lifecycle={} status={}",
                    selected.id,
                    selected.lifecycle,
                    selected.status
                );
            }

            let before = rayman::goal::workspace_baseline(root)?;
            let before_delta = rayman::goal::goal_plan_delta(selected, &before)?;
            ensure_prepare_plan_covered(&selected.id, &before_delta)?;

            let (index, refresh) = rayman::context::refresh(root)?;
            let current = rayman::goal::workspace_baseline(root)?;
            let goal_delta = rayman::goal::goal_plan_delta(selected, &current)?;
            ensure_prepare_plan_covered(&selected.id, &goal_delta)?;
            let mismatches = context_snapshot_mismatches(&index, &current);
            if !mismatches.is_empty() {
                bail!(
                    "prepare 期间源码发生变化，context index 与 goal delta 不属于同一快照: {}；请在源码稳定后重试",
                    mismatches.join(", ")
                );
            }
            Ok((
                selected.id.clone(),
                selected.status,
                selected.summary(),
                refresh,
                goal_delta,
                goal_state_sha256(selected)?,
            ))
        })?;
    after_snapshot_verified();

    // These are informational enrichments, not readiness inputs. Keep them
    // outside the goal lock so a slow Git/checkpoint probe cannot widen the
    // trusted snapshot interval.
    let source = rayman::source_state::inspect(root);
    let latest_checkpoint = latest_checkpoint_across_stores(root)?;
    let latest_checkpoint = latest_checkpoint.as_ref().map(|(checkpoint, store)| {
        json!({
            "id": checkpoint.id,
            "status": checkpoint.status,
            "created_at": checkpoint.manifest.as_ref().map(|manifest| manifest.created_at.clone()),
            "file_count": checkpoint.manifest.as_ref().map(|manifest| manifest.file_count),
            // Without the store, `checkpoint restore <id>` cannot find it.
            "store_dir": store.as_ref().map(|dir| rayman::pathfmt::display_path(dir)),
            "restore_invocation": checkpoint_restore_invocation(store.as_deref(), &checkpoint.id),
        })
    });

    // A prepare report is an attested snapshot, never a lease. Re-read both
    // independently mutable authorities immediately before publication and
    // refuse to publish if either changed after the locked verification.
    let final_workspace = rayman::goal::workspace_baseline(root)?;
    let final_goal = store
        .get(&goal_id)?
        .ok_or_else(|| anyhow::anyhow!("prepare 最终重验时 goal 已不存在: {goal_id}"))?;
    let final_goal_state_sha256 = goal_state_sha256(&final_goal)?;
    if final_workspace.workspace_fingerprint != goal_delta.current_fingerprint
        || final_goal_state_sha256 != captured_goal_state_sha256
    {
        bail!(
            "prepare 核心验证后 workspace 或 goal 状态发生变化；snapshot readiness 已失效，请重试（workspace {} -> {}；goal {} -> {}）",
            goal_delta.current_fingerprint,
            final_workspace.workspace_fingerprint,
            captured_goal_state_sha256,
            final_goal_state_sha256
        );
    }
    let readiness = PrepareReadinessSnapshot {
        scope: "goal_workspace_snapshot",
        workspace_fingerprint: goal_delta.current_fingerprint.clone(),
        goal_state_sha256: captured_goal_state_sha256,
        verified_at: rayman::timefmt::now_iso(),
        stability: "snapshot_not_lease_invalidated_by_any_workspace_or_goal_change",
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "readiness": readiness,
                "goal_id": goal_id,
                "goal_status": goal_status,
                "summary": summary,
                "latest_verified_checkpoint": latest_checkpoint,
                "context_refresh": refresh,
                "goal_delta": goal_delta,
                "source_scope": "informational_git_head_only",
                "source": source,
            }))?
        );
    } else {
        println!("任务准备完成: {} (status={})", goal_id, goal_status);
        println!(
            "  summary: must={}/{} packages={}/{} progress={} validation={} authority={}",
            summary.done_must,
            summary.done_must + summary.open_must,
            summary.completed_packages,
            summary.work_packages,
            summary.progress_receipts,
            summary.validation_receipts,
            summary.authority_receipts
        );
        for warning in &summary.warnings {
            println!("  warning: {warning}");
        }
        match latest_checkpoint {
            Some(checkpoint) => {
                println!("  checkpoint: {}", checkpoint["id"]);
                // The id alone is not actionable when the snapshot lives in a
                // store the default `checkpoint restore` never looks at.
                if !checkpoint["restore_invocation"].is_null() {
                    println!(
                        "    restore invocation: {}",
                        serde_json::to_string(&checkpoint["restore_invocation"])?
                    );
                }
            }
            None => println!("  checkpoint: none"),
        }
        println!(
            "  context: total={} reused={} rehashed={} removed={}",
            refresh.total, refresh.reused, refresh.rehashed, refresh.removed
        );
        println!(
            "  plan: covered={} recorded={} required={} actual={} planned={}",
            goal_delta.covered,
            goal_delta.plan_recorded,
            goal_delta.plan_required,
            goal_delta.actual_changed_paths.len(),
            goal_delta.planned_changed_paths.len()
        );
        if goal_delta.actual_changed_paths.is_empty() {
            println!("    actual_changed_paths: none");
        } else {
            println!("    actual_changed_paths:");
            for path in &goal_delta.actual_changed_paths {
                println!("      {}", serde_json::to_string(path)?);
            }
        }
        println!(
            "  readiness: scope={} workspace_fingerprint={} goal_state_sha256={} verified_at={} ({})",
            readiness.scope,
            readiness.workspace_fingerprint,
            readiness.goal_state_sha256,
            readiness.verified_at,
            readiness.stability
        );
        println!("  source-scope: informational Git/HEAD only; goal_delta is the plan authority");
        crate::print_source_state(&source);
    }
    Ok(())
}

/// `finish` is the delivery boundary, not another workspace-health probe. A
/// current authority receipt must show the exact final workspace survived at
/// least two executions of the same project gate without content drift.
pub(crate) fn require_stable_authority(root: &Path, goal_id: &str) -> Result<()> {
    let store = rayman::goal::GoalStore::new(root);
    let Some(goal) = store.get(goal_id)? else {
        bail!("绑定的 goal 不存在: {goal_id}");
    };
    let fingerprint = rayman::goal::workspace_fingerprint(root)?;
    if !rayman::goal::has_current_stable_authority_receipt(&goal, root, &fingerprint) {
        bail!(
            "finish 要求当前稳定 authority receipt；先运行 `rayman goal validate {goal_id} --req <req> --message <evidence> --command <project-gate> --changed <path> --authority --repeat 2`"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn record_plan(store: &rayman::goal::GoalStore, goal_id: &str, paths: &[&str]) {
        store
            .record_plan(
                goal_id,
                rayman::goal::PlanReceiptSubmission {
                    changed_paths: paths.iter().map(|path| (*path).into()).collect(),
                    review_priority: "normal".into(),
                    impacted_paths: Vec::new(),
                    recommended_checks: Vec::new(),
                },
            )
            .unwrap();
    }

    #[test]
    fn prepare_refreshes_context_for_current_active_goal() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();

        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("prepare task", &[("refresh context".into(), true)])
            .unwrap();

        run_prepare(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
        )
        .unwrap();

        assert_eq!(rayman::context::strong_freshness(root).status, "ready");
        let persisted = store.get(&goal.id).unwrap().unwrap();
        assert_eq!(persisted.lifecycle, rayman::goal::GoalLifecycle::Current);
        assert_eq!(persisted.status, rayman::goal::GoalStatus::Active);
    }

    #[test]
    fn prepare_accepts_a_planned_change_without_git() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 41 }\n").unwrap();

        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("planned prepare", &[("reconcile plan".into(), true)])
            .unwrap();
        record_plan(&store, &goal.id, &["src/lib.rs"]);
        std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();

        assert!(!rayman::source_state::inspect(root).available);
        run_prepare(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
        )
        .unwrap();
        assert_eq!(rayman::context::strong_freshness(root).status, "ready");
    }

    #[test]
    fn prepare_rejects_an_unplanned_change_before_refresh() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("planned.txt"), "before\n").unwrap();
        std::fs::write(root.join("outside.txt"), "before\n").unwrap();

        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("reject drift", &[("reconcile plan".into(), true)])
            .unwrap();
        record_plan(&store, &goal.id, &["planned.txt"]);
        std::fs::write(root.join("outside.txt"), "after\n").unwrap();

        let error = run_prepare(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("outside.txt"), "{error}");
        assert!(error.contains("--extend"), "{error}");
        assert_eq!(rayman::context::strong_freshness(root).status, "missing");
    }

    #[test]
    fn prepare_rejects_two_changes_without_a_plan() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("a.txt"), "before\n").unwrap();
        std::fs::write(root.join("b.txt"), "before\n").unwrap();

        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("missing plan", &[("reconcile plan".into(), true)])
            .unwrap();
        std::fs::write(root.join("a.txt"), "after\n").unwrap();
        std::fs::write(root.join("b.txt"), "after\n").unwrap();

        let error = run_prepare(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("2 个文件"), "{error}");
        assert!(
            error.contains("a.txt") && error.contains("b.txt"),
            "{error}"
        );
        assert_eq!(rayman::context::strong_freshness(root).status, "missing");
    }

    #[test]
    fn context_snapshot_comparison_detects_source_drift() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("source.txt"), "first\n").unwrap();
        let (index, _) = rayman::context::refresh(root).unwrap();
        let same = rayman::goal::workspace_baseline(root).unwrap();
        assert!(context_snapshot_mismatches(&index, &same).is_empty());

        std::fs::write(root.join("source.txt"), "second\n").unwrap();
        let changed = rayman::goal::workspace_baseline(root).unwrap();
        assert_eq!(
            context_snapshot_mismatches(&index, &changed),
            ["source.txt"]
        );
    }

    #[test]
    fn prepare_suggestions_preserve_hostile_paths_as_argv_data() {
        let hostile = "src/$([payload])`line\nnext.rs".to_string();
        let delta = rayman::goal::GoalPlanDelta {
            baseline_fingerprint: "baseline".into(),
            current_fingerprint: "current".into(),
            actual_changed_paths: vec![hostile.clone()],
            planned_changed_paths: Vec::new(),
            unplanned_changed_paths: vec![hostile.clone()],
            plan_recorded: true,
            plan_required: true,
            covered: false,
        };

        let error = ensure_prepare_plan_covered("goal-hostile", &delta)
            .unwrap_err()
            .to_string();
        let encoded = error
            .split_once("逐参数调用: ")
            .expect("error should carry structured invocation")
            .1;
        let invocation: serde_json::Value = serde_json::from_str(encoded).unwrap();
        assert_eq!(invocation["program"], "rayman");
        assert_eq!(
            invocation["args"],
            json!([
                "goal",
                "plan",
                "goal-hostile",
                hostile,
                "--extend",
                "--check"
            ])
        );
        assert!(!error.contains("`rayman"), "{error}");
    }

    #[test]
    fn checkpoint_restore_is_an_exact_argv_invocation() {
        let store = Path::new("checkpoints/$([payload])`tick\nnext");
        let invocation = checkpoint_restore_invocation(Some(store), "checkpoint-$([id])");
        let encoded = serde_json::to_value(&invocation).unwrap();

        assert_eq!(encoded["program"], "rayman");
        assert_eq!(
            encoded["args"],
            json!([
                "checkpoint",
                "--dir",
                rayman::pathfmt::display_path(store),
                "restore",
                "checkpoint-$([id])",
                "--yes"
            ])
        );
        assert!(encoded.get("restore_command").is_none());
    }

    #[test]
    fn prepare_rejects_workspace_drift_after_snapshot_verification() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("source.txt"), "before\n").unwrap();
        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("workspace race", &[("prepare".into(), true)])
            .unwrap();

        let error = run_prepare_with_phase_hook(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
            || std::fs::write(root.join("source.txt"), "after\n").unwrap(),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("snapshot readiness 已失效"), "{error}");
        assert!(error.contains("workspace"), "{error}");
    }

    #[test]
    fn prepare_rejects_goal_drift_after_snapshot_verification() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("source.txt"), "stable\n").unwrap();
        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("goal race", &[("prepare".into(), true)])
            .unwrap();
        record_plan(&store, &goal.id, &["source.txt"]);

        let error = run_prepare_with_phase_hook(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
            || {
                store
                    .record_review(&goal.id, "race", "mutated after snapshot")
                    .unwrap();
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("snapshot readiness 已失效"), "{error}");
        assert!(error.contains("goal"), "{error}");
    }

    #[test]
    fn prepare_does_not_reach_enrichment_phase_when_plan_check_fails() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        std::fs::write(root.join("source.txt"), "before\n").unwrap();
        std::fs::write(root.join("other.txt"), "before\n").unwrap();
        let store = rayman::goal::GoalStore::new(root);
        let goal = store
            .start("early failure", &[("prepare".into(), true)])
            .unwrap();
        std::fs::write(root.join("source.txt"), "after\n").unwrap();
        std::fs::write(root.join("other.txt"), "after\n").unwrap();
        let hook_called = Cell::new(false);

        let error = run_prepare_with_phase_hook(
            root,
            true,
            TaskWorkflowCmd {
                goal: goal.id.clone(),
            },
            || hook_called.set(true),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("缺少首次修改前"), "{error}");
        assert!(!hook_called.get());
        assert_eq!(rayman::context::strong_freshness(root).status, "missing");
    }
}
