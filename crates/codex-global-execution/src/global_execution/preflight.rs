//! Activation-exempt diagnostics usable from any project. A successful write
//! probe is deliberately not an installed broker or authorization claim.
use crate::{execution_context, file_io, pathfmt, state_paths};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct GlobalPreflight {
    pub schema: &'static str,
    pub execution_context: execution_context::ExecutionContextProbe,
    pub projects: Vec<ProjectPreflight>,
    pub privileged_operations_executed: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectPreflight {
    pub workspace: String,
    pub formal_state: state_paths::StateWriteProbe,
    pub git_marker: &'static str,
    pub commit_ready: bool,
    pub installation_ready: bool,
    pub blockers: Vec<&'static str>,
}

pub fn preflight(workspaces: &[PathBuf], probe_writes: bool) -> Result<GlobalPreflight> {
    if workspaces.is_empty() || workspaces.len() > 256 {
        bail!("preflight requires 1 to 256 explicit workspaces");
    }
    let mut projects = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for workspace in workspaces {
        let root = std::fs::canonicalize(workspace)?;
        if !root.is_dir() {
            bail!("preflight workspace is not a directory");
        }
        // Avoid duplicate probes through alternate spellings of one path.
        let key = pathfmt::display_path(&root);
        let dedup = if cfg!(windows) {
            key.to_lowercase()
        } else {
            key.clone()
        };
        if !seen.insert(dedup) {
            bail!("duplicate preflight workspace");
        }
        let state = if probe_writes {
            state_paths::state_write_probe(&root)
        } else {
            state_paths::state_write_probe_not_requested(&root)
        };
        let mut blockers = vec!["global_worker_not_attested"];
        if state.probed && !state.writable {
            blockers.push("formal_state_not_writable_by_current_token");
        }
        projects.push(ProjectPreflight {
            workspace: key,
            formal_state: state,
            git_marker: git_marker(&root),
            commit_ready: false,
            installation_ready: false,
            blockers,
        });
    }
    Ok(GlobalPreflight {
        schema: "rayman.global-preflight.v1",
        execution_context: execution_context::execution_context_probe(),
        projects,
        privileged_operations_executed: false,
    })
}

fn git_marker(root: &Path) -> &'static str {
    match std::fs::symlink_metadata(root.join(".git")) {
        Ok(m) if file_io::is_link_or_reparse(&m) => "unsafe_reparse",
        Ok(m) if m.is_dir() => "directory_unattested",
        Ok(m) if m.is_file() => "gitfile_unattested",
        Ok(_) => "unsupported_type",
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent",
        Err(_) => "unreadable",
    }
}
