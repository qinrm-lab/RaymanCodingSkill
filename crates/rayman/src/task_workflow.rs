use std::path::Path;

use anyhow::{Result, bail};
use serde_json::json;

use crate::cli::TaskWorkflowCmd;

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
) -> Result<Option<(rayman::checkpoint::CheckpointInfo, Option<std::path::PathBuf>)>> {
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

    let mut best: Option<(rayman::checkpoint::CheckpointInfo, Option<std::path::PathBuf>)> = None;
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
    let store = rayman::goal::GoalStore::new(root);
    let (goal_id, goal_status, summary, refresh, source) =
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
            let (_, refresh) = rayman::context::refresh(root)?;
            Ok((
                selected.id.clone(),
                selected.status,
                selected.summary(),
                refresh,
                rayman::source_state::inspect(root),
            ))
        })?;
    let latest_checkpoint = latest_checkpoint_across_stores(root)?;
    let latest_checkpoint = latest_checkpoint.as_ref().map(|(checkpoint, store)| {
        json!({
            "id": checkpoint.id,
            "status": checkpoint.status,
            "created_at": checkpoint.manifest.as_ref().map(|manifest| manifest.created_at.clone()),
            "file_count": checkpoint.manifest.as_ref().map(|manifest| manifest.file_count),
            // Without the store, `checkpoint restore <id>` cannot find it.
            "store_dir": store.as_ref().map(|dir| rayman::pathfmt::display_path(dir)),
            "restore_command": match store {
                // Quote the store path: autosave --dir is routinely a profile
                // or drive path with spaces, and an unquoted one produced a
                // command that cannot run — the opposite of "ready-to-run".
                Some(dir) => format!(
                    "rayman checkpoint --dir \"{}\" restore {} --yes",
                    rayman::pathfmt::display_path(dir),
                    checkpoint.id
                ),
                None => format!("rayman checkpoint restore {} --yes", checkpoint.id),
            },
        })
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ready": true,
                "goal_id": goal_id,
                "goal_status": goal_status,
                "summary": summary,
                "latest_verified_checkpoint": latest_checkpoint,
                "context_refresh": refresh,
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
                if let Some(command) = checkpoint["restore_command"].as_str() {
                    println!("    restore: {command}");
                }
            }
            None => println!("  checkpoint: none"),
        }
        println!(
            "  context: total={} reused={} rehashed={} removed={}",
            refresh.total, refresh.reused, refresh.rehashed, refresh.removed
        );
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
    use super::*;

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
}
