use std::path::Path;

use anyhow::{Context, Result, bail};
use rayman::{goal, temp};
use serde_json::json;

/// `.<allowed-state-file>.rayman.lock` — the advisory lock `acquire_state_lock`
/// puts beside a state file. The kernel releases the lock on exit, so the file
/// is deliberately never deleted; leaving it off the allowlist made
/// `state audit --check` (and therefore the whole repository audit, which runs
/// it) fail permanently in any workspace that had used `goal pending add`,
/// while the contract forbids deleting state to make the gate pass.
fn is_managed_state_lock(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|rest| rest.strip_suffix(".rayman.lock"))
        .is_some_and(|target| STATE_LOCK_TARGETS.contains(&target))
}

const STATE_LOCK_TARGETS: &[&str] = &["pending.json", "autosave.json"];

pub(crate) fn run_state_audit(root: &Path, json: bool, check: bool) -> Result<()> {
    const V2_ALLOWED: &[&str] = &[
        "goals",
        "pending.json",
        "context",
        "autosave.json",
        "autosave.lock",
        "tmp",
        "checkpoints",
        "workspace_skill.yaml",
        "quality.json",
        "release-closeout-evidence.json",
    ];
    let state = root.join(".RaymanCodingSkill");
    let mut retired = Vec::new();
    let mut errors = Vec::new();
    match rayman::state_paths::managed_state_root(root, false) {
        Ok(None) => {}
        Ok(Some(verified_state)) => match std::fs::read_dir(&verified_state) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if !V2_ALLOWED.contains(&name.as_str()) && !is_managed_state_lock(&name)
                            {
                                retired.push(name);
                            } else if let Err(error) = audit_allowed_state_entry(root, &name) {
                                errors
                                    .push(format!("允许的状态项 `{name}` 不安全或无效: {error:#}"));
                            }
                        }
                        Err(error) => errors.push(error.to_string()),
                    }
                }
            }
            Err(error) => errors.push(error.to_string()),
        },
        Err(error) => errors.push(format!("无法安全读取受管状态: {error:#}")),
    }
    retired.sort();
    let temp_status = temp::audit(root);
    let clean = retired.is_empty() && errors.is_empty() && temp_status.traversal_error_count == 0;
    if json {
        crate::print(&json!({
            "state_root": state,
            "allowed_v2_entries": V2_ALLOWED,
            "retired_entries": retired,
            "errors": errors,
            "temp": {
                "root": temp_status.root,
                "files": temp_status.file_count,
                "directories": temp_status.directory_count,
                "bytes": temp_status.total_bytes,
                "traversal_errors": temp_status.traversal_errors,
            },
            "clean": clean,
            "destructive_action": "none; inspect and migrate or remove retired state only after explicit user approval",
        }));
    } else {
        println!("受管状态审计: clean={clean}");
        println!(
            "  temp: files={} dirs={} {:.1} MB",
            temp_status.file_count,
            temp_status.directory_count,
            temp_status.total_bytes as f64 / 1_048_576.0
        );
        if !retired.is_empty() {
            println!("  retired entries: {}", retired.join(", "));
        }
        for error in errors.iter().chain(temp_status.traversal_errors.iter()) {
            println!("  error: {error}");
        }
        println!("  no files were deleted");
    }
    if check && !clean {
        bail!("受管状态包含退役条目或遍历错误；先审阅 `rayman state audit` 输出")
    }
    Ok(())
}

/// An allowlisted name is not automatically safe: `read_dir` exposes a lexical
/// entry, and a link/reparse point or wrong type would otherwise make audit
/// report `clean=true` while another command follows or later fails on it.
/// Reuse the same state-path authority as the readers and writers.
fn audit_allowed_state_entry(root: &Path, name: &str) -> Result<()> {
    match name {
        "goals" => {
            let Some(_) = rayman::state_paths::managed_state_dir(root, Path::new("goals"), false)?
            else {
                bail!("目录在枚举后消失");
            };
            let (_, issues) = goal::GoalStore::new(root).list_with_issues()?;
            if !issues.is_empty() {
                let details = issues
                    .iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                bail!("目标目录含不可安全读取的记录: {details}");
            }
            Ok(())
        }
        // `checkpoints` appears whenever `checkpoint save --dir` targets the
        // workspace, which is exactly what the workflow reference tells an agent
        // to do inside a workspace-only sandbox. Leaving it off the allowlist made
        // that documented remedy break `state audit --check` for good.
        "context" | "tmp" | "checkpoints" => {
            let Some(_) = rayman::state_paths::managed_state_dir(root, Path::new(name), false)?
            else {
                bail!("目录在枚举后消失");
            };
            Ok(())
        }
        name if is_managed_state_lock(name) => {
            let path = rayman::state_paths::managed_state_file(root, Path::new(name), false)?;
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_file() => Ok(()),
                Ok(_) => bail!("状态锁不是安全普通文件: {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    bail!("文件在枚举后消失: {}", path.display())
                }
                Err(error) => {
                    Err(error).with_context(|| format!("无法检查状态锁: {}", path.display()))
                }
            }
        }
        "pending.json"
        | "autosave.json"
        | "autosave.lock"
        | "workspace_skill.yaml"
        | "quality.json"
        | "release-closeout-evidence.json" => {
            let path = rayman::state_paths::managed_state_file(root, Path::new(name), false)?;
            match std::fs::symlink_metadata(&path) {
                Ok(_) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    bail!("文件在枚举后消失: {}", path.display())
                }
                Err(error) => Err(error)
                    .with_context(|| format!("无法读取允许的状态文件: {}", path.display())),
            }
        }
        _ => bail!("未知的允许状态项"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_audit_check_refuses_an_unreadable_or_invalid_state_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".RaymanCodingSkill"), "not a directory").unwrap();
        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    #[test]
    fn state_audit_check_refuses_a_wrong_type_for_an_allowed_state_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".RaymanCodingSkill/pending.json")).unwrap();

        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    #[test]
    fn state_audit_accepts_release_evidence_only_as_an_ordinary_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join(".RaymanCodingSkill");
        std::fs::create_dir_all(&state).unwrap();
        let evidence = state.join("release-closeout-evidence.json");
        std::fs::write(&evidence, "{}\n").unwrap();

        assert!(run_state_audit(dir.path(), false, true).is_ok());

        std::fs::remove_file(&evidence).unwrap();
        std::fs::create_dir(&evidence).unwrap();
        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    /// `goal pending add` leaves `.pending.json.rayman.lock` behind by design,
    /// and `scripts/audit-repository.ps1` runs `state audit --check`, so this
    /// omission made an ordinary documented command permanently red-line the
    /// repository audit.
    #[test]
    fn state_audit_accepts_the_state_locks_the_cli_itself_creates() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join(".RaymanCodingSkill");
        std::fs::create_dir_all(&state).unwrap();
        for lock in [".pending.json.rayman.lock", ".autosave.json.rayman.lock"] {
            std::fs::write(state.join(lock), "").unwrap();
        }
        assert!(run_state_audit(dir.path(), false, true).is_ok());

        // Only locks for known state targets are allowed, and only as files.
        std::fs::write(state.join(".secrets.rayman.lock"), "").unwrap();
        assert!(run_state_audit(dir.path(), false, true).is_err());
        std::fs::remove_file(state.join(".secrets.rayman.lock")).unwrap();

        std::fs::remove_file(state.join(".pending.json.rayman.lock")).unwrap();
        std::fs::create_dir(state.join(".pending.json.rayman.lock")).unwrap();
        assert!(run_state_audit(dir.path(), false, true).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_audit_check_refuses_a_linked_allowed_state_entry() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".RaymanCodingSkill")).unwrap();
        std::fs::create_dir_all(outside.path().join("goals")).unwrap();
        symlink(
            outside.path().join("goals"),
            dir.path().join(".RaymanCodingSkill/goals"),
        )
        .unwrap();

        assert!(run_state_audit(dir.path(), false, true).is_err());
    }
}
