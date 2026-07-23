//! Deterministic Codex lifecycle integration for Owner Mode.
//!
//! The Stop hook is read-only for workspace state and reuses the existing goal
//! and frontier contracts. Installation changes only Rayman's managed handler
//! in the user's hooks.json.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::file_io::{is_link_or_reparse, write_atomic};
use crate::goal::{
    FrontierExecution, GoalLifecycle, GoalStatus, GoalStore, PendingStore, goal_gate_verdict,
    workspace_fingerprint,
};

const MANAGED_STATUS: &str = "Rayman Owner Mode completion guard";
const HOOKS_FILE: &str = "hooks.json";

#[derive(Debug, Deserialize)]
struct StopHookInput {
    hook_event_name: Option<String>,
    #[allow(dead_code)]
    stop_hook_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StopHookResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub should_continue: Option<bool>,
}

impl StopHookResponse {
    fn allow() -> Self {
        Self {
            decision: None,
            reason: None,
            should_continue: Some(true),
        }
    }

    fn block(reason: impl Into<String>) -> Self {
        Self {
            decision: Some("block".into()),
            reason: Some(reason.into()),
            should_continue: None,
        }
    }

    pub fn blocks_stop(&self) -> bool {
        self.decision.as_deref() == Some("block")
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HookInstallReport {
    pub hooks_path: String,
    pub installed: bool,
    pub changed: bool,
    pub command: Option<String>,
}

/// Inactive workspaces are outside Rayman's authority. Once an activation
/// contract exists, malformed activation or task state fails closed.
pub fn evaluate_stop(root: &Path) -> StopHookResponse {
    let activation = match crate::workspace::activation_status(root) {
        Ok(report) => report,
        Err(error) => {
            return StopHookResponse::block(format!(
                "Rayman Stop guard could not validate workspace activation: {error:#}. Repair the activation/state contract before ending the turn."
            ));
        }
    };
    if !activation.config_present {
        return StopHookResponse::allow();
    }
    if !activation.active {
        if !activation.enabled {
            return StopHookResponse::allow();
        }
        return StopHookResponse::block(format!(
            "Rayman activation exists but is invalid (status={}, issues={}). Run `rayman workspace status` and repair it before ending the turn.",
            activation.status,
            activation.issues.join("; ")
        ));
    }

    let store = GoalStore::new(root);
    let (goals, issues) = match store.list_with_issues() {
        Ok(result) => result,
        Err(error) => {
            return StopHookResponse::block(format!(
                "Rayman Stop guard could not read goals: {error:#}. Repair goal state before ending the turn."
            ));
        }
    };
    if !issues.is_empty() {
        let detail = issues
            .iter()
            .map(|issue| format!("{}: {}", issue.path, issue.error))
            .collect::<Vec<_>>()
            .join("; ");
        return StopHookResponse::block(format!(
            "Rayman goal state is incomplete or corrupt: {detail}. Repair it before ending the turn."
        ));
    }

    let current = goals
        .iter()
        .filter(|goal| goal.lifecycle == GoalLifecycle::Current)
        .collect::<Vec<_>>();
    // Trivial tasks intentionally require no Rayman goal.
    if current.is_empty() {
        return StopHookResponse::allow();
    }

    let fingerprint = match workspace_fingerprint(root) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            return StopHookResponse::block(format!(
                "Rayman Stop guard could not fingerprint the workspace: {error:#}. Continue recovery before ending the turn."
            ));
        }
    };
    let pending = PendingStore::new(root);
    let mut unfinished = Vec::new();
    for goal in current {
        let frontier = match pending.frontier(goal) {
            Ok(frontier) => frontier,
            Err(error) => {
                return StopHookResponse::block(format!(
                    "Rayman Stop guard could not validate frontier for {}: {error:#}. Repair pending state before ending the turn.",
                    goal.id
                ));
            }
        };
        match frontier.execution {
            FrontierExecution::PausedForUser
            | FrontierExecution::WaitExternal
            | FrontierExecution::ContinueBackground => {}
            FrontierExecution::Complete => {
                let verdict = goal_gate_verdict(goal, &goals, root, Some(&fingerprint));
                if !verdict.blockers.is_empty() || goal.status != GoalStatus::Success {
                    unfinished.push(format!(
                        "{} has invalid completion evidence: {}",
                        goal.id,
                        verdict.blockers.join("; ")
                    ));
                }
            }
            FrontierExecution::ContinueForeground => unfinished.push(format!(
                "{} remains {}: {}",
                goal.id, goal.status, frontier.reason
            )),
        }
    }

    if unfinished.is_empty() {
        StopHookResponse::allow()
    } else {
        StopHookResponse::block(format!(
            "Rayman Owner Mode forbids premature handoff. {}. Continue safe foreground work, update the goal contract for newly added requirements, then run `rayman goal frontier <id>` and `rayman finish --goal <id>` before ending the turn.",
            unfinished.join(" | ")
        ))
    }
}

pub fn run_stop_from_stdin() -> StopHookResponse {
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        return StopHookResponse::block(format!(
            "Rayman Stop guard could not read the Codex event: {error}"
        ));
    }
    let event = match serde_json::from_str::<StopHookInput>(&input) {
        Ok(event) => event,
        Err(error) => {
            return StopHookResponse::block(format!(
                "Rayman Stop guard received invalid Codex Stop JSON: {error}"
            ));
        }
    };
    if event.hook_event_name.as_deref() != Some("Stop") {
        return StopHookResponse::block(
            "Rayman Stop guard was invoked for an event other than Stop",
        );
    }
    match crate::workspace_root() {
        Ok(root) => evaluate_stop(&root),
        Err(error) => StopHookResponse::block(format!(
            "Rayman Stop guard could not resolve the workspace: {error:#}"
        )),
    }
}

fn default_codex_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .ok_or_else(|| anyhow::anyhow!("cannot resolve Codex home"))?;
    Ok(PathBuf::from(home).join(".codex"))
}

fn hooks_path(codex_home: Option<&Path>) -> Result<PathBuf> {
    let home = codex_home
        .map(Path::to_path_buf)
        .map_or_else(default_codex_home, Ok)?;
    let home = if home.is_absolute() {
        home
    } else {
        env::current_dir()?.join(home)
    };
    crate::file_io::ensure_real_directory_labeled(&home, "Codex home")?;
    let path = home.join(HOOKS_FILE);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if is_link_or_reparse(&metadata) => {
            bail!(
                "refusing linked/reparse Codex hooks file: {}",
                path.display()
            )
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("Codex hooks path is not a regular file: {}", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect Codex hooks file"),
    }
    Ok(path)
}

fn hook_command(executable: &Path) -> Result<String> {
    let text = executable
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("rayman executable path is not Unicode"))?;
    #[cfg(windows)]
    {
        if text.contains(['\r', '\n', '"', '%', '!', '&', '|', '<', '>', '^', '$', '`']) {
            bail!("rayman executable path contains unsafe Windows hook command characters");
        }
        Ok(format!("\"{text}\" codex-hook stop"))
    }
    #[cfg(not(windows))]
    {
        if text.contains(['\r', '\n']) {
            bail!("rayman executable path contains a newline");
        }
        let escaped = text.replace('\'', "'\\''");
        Ok(format!("'{escaped}' codex-hook stop"))
    }
}

fn read_hooks(path: &Path) -> Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let value: Value =
                serde_json::from_str(&text).context("Codex hooks.json is invalid JSON")?;
            if !value.is_object() {
                bail!("Codex hooks.json root must be an object");
            }
            Ok(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error).context("cannot read Codex hooks.json"),
    }
}

fn is_managed_handler(value: &Value) -> bool {
    value.get("statusMessage").and_then(Value::as_str) == Some(MANAGED_STATUS)
}

fn remove_managed_handlers(root: &mut Value) -> Result<bool> {
    let object = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json root must be an object"))?;
    let Some(hooks) = object.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json `hooks` must be an object"))?;
    let Some(stop) = hooks.get_mut("Stop") else {
        return Ok(false);
    };
    let stop = stop
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json `hooks.Stop` must be an array"))?;
    let mut changed = false;
    for group in stop.iter_mut() {
        let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        let before = handlers.len();
        handlers.retain(|handler| !is_managed_handler(handler));
        changed |= handlers.len() != before;
    }
    let before = stop.len();
    stop.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|handlers| !handlers.is_empty())
    });
    Ok(changed || stop.len() != before)
}

fn install_value(root: &mut Value, command: &str) -> Result<()> {
    remove_managed_handlers(root)?;
    let object = root.as_object_mut().expect("validated object");
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json `hooks` must be an object"))?;
    let stop = hooks
        .entry("Stop")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("Codex hooks.json `hooks.Stop` must be an array"))?;
    stop.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 30,
            "statusMessage": MANAGED_STATUS
        }]
    }));
    Ok(())
}

fn lock_hooks(path: &Path) -> Result<fs::File> {
    let lock_path = path.with_extension("json.rayman.lock");
    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if is_link_or_reparse(&metadata) => {
            bail!("refusing linked/reparse hook lock: {}", lock_path.display())
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!("hook lock is not a regular file: {}", lock_path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("cannot inspect hook lock"),
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("cannot open hook lock: {}", lock_path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("cannot lock Codex hooks: {}", path.display()))?;
    Ok(file)
}

pub fn install(codex_home: Option<&Path>, executable: &Path) -> Result<HookInstallReport> {
    let path = hooks_path(codex_home)?;
    let _lock = lock_hooks(&path)?;
    let command = hook_command(executable)?;
    let mut value = read_hooks(&path)?;
    let before = serde_json::to_vec(&value)?;
    install_value(&mut value, &command)?;
    let changed = before != serde_json::to_vec(&value)?;
    if changed {
        let mut text = serde_json::to_string_pretty(&value)?;
        text.push('\n');
        write_atomic(&path, &text)?;
    }
    Ok(HookInstallReport {
        hooks_path: path.display().to_string(),
        installed: true,
        changed,
        command: Some(command),
    })
}

pub fn uninstall(codex_home: Option<&Path>) -> Result<HookInstallReport> {
    let path = hooks_path(codex_home)?;
    let _lock = lock_hooks(&path)?;
    let mut value = read_hooks(&path)?;
    let changed = remove_managed_handlers(&mut value)?;
    if changed {
        let mut text = serde_json::to_string_pretty(&value)?;
        text.push('\n');
        write_atomic(&path, &text)?;
    }
    Ok(HookInstallReport {
        hooks_path: path.display().to_string(),
        installed: false,
        changed,
        command: None,
    })
}

pub fn status(codex_home: Option<&Path>) -> Result<HookInstallReport> {
    let path = hooks_path(codex_home)?;
    let value = read_hooks(&path)?;
    let command = value
        .get("hooks")
        .and_then(|hooks| hooks.get("Stop"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .find(|handler| is_managed_handler(handler))
        .and_then(|handler| handler.get("command"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(HookInstallReport {
        hooks_path: path.display().to_string(),
        installed: command.is_some(),
        changed: false,
        command,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::{ConsultationTiming, PendingKind, PendingOwner, PendingSubmission};

    fn activate(root: &Path) {
        let skill = root.join("SKILL.md");
        fs::write(&skill, "test skill\n").unwrap();
        crate::workspace::activate(root, &skill).unwrap();
    }

    #[test]
    fn inactive_or_goal_less_workspace_may_stop() {
        let root = tempfile::tempdir().unwrap();
        assert!(!evaluate_stop(root.path()).blocks_stop());
        activate(root.path());
        assert!(!evaluate_stop(root.path()).blocks_stop());
        crate::workspace::deactivate(root.path()).unwrap();
        assert!(!evaluate_stop(root.path()).blocks_stop());
    }

    #[test]
    fn active_open_goal_blocks_stop_after_mid_turn_addition() {
        let root = tempfile::tempdir().unwrap();
        activate(root.path());
        let goal = GoalStore::new(root.path())
            .start(
                "whole program",
                &[
                    ("original feature".into(), true),
                    ("mid-turn addition".into(), true),
                ],
            )
            .unwrap();
        let response = evaluate_stop(root.path());
        assert!(response.blocks_stop());
        assert!(response.reason.unwrap().contains(&goal.id));
    }

    #[test]
    fn complete_human_solution_package_allows_pause() {
        let root = tempfile::tempdir().unwrap();
        activate(root.path());
        let goal = GoalStore::new(root.path())
            .start("human boundary", &[("requires owner choice".into(), true)])
            .unwrap();
        PendingStore::new(root.path())
            .add_structured(PendingSubmission {
                title: "owner choice".into(),
                detail: "two materially different directions".into(),
                goal_id: Some(goal.id),
                owner: PendingOwner::Human,
                kind: PendingKind::HumanInput,
                attempts: vec!["completed independent work".into()],
                evidence_paths: vec!["decision.md".into()],
                minimum_input: Some("choose A or B".into()),
                recommended_action: Some("choose A".into()),
                alternatives: vec!["choose B".into()],
                risk: Some("behavior differs".into()),
                resume_command: Some("rayman prepare --goal goal".into()),
                auto_resume_condition: Some("owner records choice".into()),
                consultation_timing: ConsultationTiming::Immediate,
                background_mechanism: None,
                background_authority_evidence: None,
                background_isolation_evidence: None,
            })
            .unwrap();
        assert!(!evaluate_stop(root.path()).blocks_stop());
    }

    #[test]
    fn install_is_idempotent_and_preserves_other_handlers() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join(HOOKS_FILE),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other","statusMessage":"Other"}]}]}}"#,
        )
        .unwrap();
        let executable = env::current_exe().unwrap();
        assert!(install(Some(home.path()), &executable).unwrap().changed);
        assert!(!install(Some(home.path()), &executable).unwrap().changed);
        let value = read_hooks(&home.path().join(HOOKS_FILE)).unwrap();
        let text = value.to_string();
        assert!(text.contains("Other"));
        assert_eq!(text.matches(MANAGED_STATUS).count(), 1);
        assert!(uninstall(Some(home.path())).unwrap().changed);
        assert!(
            read_hooks(&home.path().join(HOOKS_FILE))
                .unwrap()
                .to_string()
                .contains("Other")
        );
    }

    #[test]
    fn malformed_activation_or_hooks_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".RaymanCodingSkill")).unwrap();
        fs::write(
            root.path().join(".RaymanCodingSkill/workspace_skill.yaml"),
            "enabled: true\n",
        )
        .unwrap();
        assert!(evaluate_stop(root.path()).blocks_stop());

        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join(HOOKS_FILE), "[]").unwrap();
        assert!(install(Some(home.path()), &env::current_exe().unwrap()).is_err());
    }
}
