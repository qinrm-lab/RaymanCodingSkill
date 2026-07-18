use std::path::Path;

use anyhow::{Result, bail};
use serde_json::json;

use crate::cli::TaskWorkflowCmd;

pub(crate) fn run_prepare(root: &Path, json_output: bool, cmd: TaskWorkflowCmd) -> Result<()> {
    let store = rayman::goal::GoalStore::new(root);
    let (goal_id, goal_status, refresh, source) =
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
                refresh,
                rayman::source_state::inspect(root),
            ))
        })?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ready": true,
                "goal_id": goal_id,
                "goal_status": goal_status,
                "context_refresh": refresh,
                "source": source,
            }))?
        );
    } else {
        println!("任务准备完成: {} (status={})", goal_id, goal_status);
        println!(
            "  context: total={} reused={} rehashed={} removed={}",
            refresh.total, refresh.reused, refresh.rehashed, refresh.removed
        );
        crate::print_source_state(&source);
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
