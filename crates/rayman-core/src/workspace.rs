use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::mapping_get;
use crate::yaml::{Mapping, Value};
use crate::{display_path, now_iso, sha256_file, yaml};

#[derive(Debug, Clone)]
pub struct WorkspaceActivationManager {
    pub workspace: PathBuf,
    pub skill_file: PathBuf,
    pub state_path: PathBuf,
}

impl WorkspaceActivationManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let workspace_skill = workspace.join("SKILL.md");
        let skill_file = if workspace_skill.exists() {
            workspace_skill
        } else {
            canonical_skill_file().unwrap_or(workspace_skill)
        };
        let state_path = workspace
            .join(".RaymanCodingSkill")
            .join("workspace_skill.yaml");
        Ok(Self {
            workspace,
            skill_file,
            state_path,
        })
    }

    pub fn enable(&self, reason: &str, source: &str) -> Result<Value> {
        let previous = self.read_state()?;
        let mut state = self.base_state(Some(&previous), true, reason, source)?;
        let updated = mapping_get(&state, "updated_at")
            .cloned()
            .unwrap_or(Value::Null);
        let map = state_mapping_mut(&mut state)?;
        map.insert(Value::String("enabled_at".into()), updated);
        map.remove(Value::String("disabled_at".into()));
        self.write_state(&state)?;
        self.status()
    }

    pub fn disable(&self, reason: &str, source: &str) -> Result<Value> {
        let previous = self.read_state()?;
        let mut state = self.base_state(Some(&previous), false, reason, source)?;
        if let Some(enabled_at) = mapping_get(&previous, "enabled_at") {
            state_mapping_mut(&mut state)?
                .insert(Value::String("enabled_at".into()), enabled_at.clone());
        }
        let updated = mapping_get(&state, "updated_at")
            .cloned()
            .unwrap_or(Value::Null);
        state_mapping_mut(&mut state)?.insert(Value::String("disabled_at".into()), updated);
        self.write_state(&state)?;
        self.status()
    }

    pub fn record_use(&self, reason: &str, source: &str) -> Result<Value> {
        let previous = self.read_state()?;
        let disabled = mapping_get(&previous, "enabled")
            .and_then(Value::as_bool)
            .map(|enabled| !enabled)
            .unwrap_or(false);
        let mut state = self.base_state(Some(&previous), !disabled, reason, source)?;
        if let Some(enabled_at) = mapping_get(&previous, "enabled_at") {
            state_mapping_mut(&mut state)?
                .insert(Value::String("enabled_at".into()), enabled_at.clone());
        } else if !disabled {
            let updated = mapping_get(&state, "updated_at")
                .cloned()
                .unwrap_or(Value::Null);
            state_mapping_mut(&mut state)?.insert(Value::String("enabled_at".into()), updated);
        }
        if disabled && let Some(disabled_at) = mapping_get(&previous, "disabled_at") {
            state_mapping_mut(&mut state)?
                .insert(Value::String("disabled_at".into()), disabled_at.clone());
        } else {
            state_mapping_mut(&mut state)?.remove(Value::String("disabled_at".into()));
        }
        self.write_state(&state)?;
        self.status()
    }

    pub fn status(&self) -> Result<Value> {
        let state = self.read_state()?;
        let enabled = mapping_get(&state, "enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let current_hash = if self.skill_file.exists() {
            Some(sha256_file(&self.skill_file)?)
        } else {
            None
        };
        let recorded_hash = mapping_get(&state, "skill_sha256")
            .and_then(Value::as_str)
            .map(str::to_string);
        let uses_latest = current_hash
            .as_deref()
            .zip(recorded_hash.as_deref())
            .map(|(current, recorded)| current == recorded)
            .unwrap_or(false);
        let mut map = Mapping::new();
        map.insert(
            Value::String("skill".into()),
            Value::String("raymancodingskill".into()),
        );
        map.insert(Value::String("enabled".into()), Value::Bool(enabled));
        map.insert(Value::String("auto_use".into()), Value::Bool(enabled));
        map.insert(
            Value::String("scope".into()),
            Value::String("workspace".into()),
        );
        map.insert(
            Value::String("workspace_path".into()),
            Value::String(display_path(&self.workspace)),
        );
        map.insert(
            Value::String("state_path".into()),
            Value::String(display_path(&self.state_path)),
        );
        map.insert(
            Value::String("skill_file".into()),
            Value::String(display_path(&self.skill_file)),
        );
        map.insert(
            Value::String("current_skill_sha256".into()),
            current_hash.map(Value::String).unwrap_or(Value::Null),
        );
        map.insert(
            Value::String("recorded_skill_sha256".into()),
            recorded_hash.map(Value::String).unwrap_or(Value::Null),
        );
        map.insert(
            Value::String("uses_latest_skill_file".into()),
            Value::Bool(uses_latest),
        );
        for key in [
            "reason",
            "source",
            "updated_at",
            "enabled_at",
            "disabled_at",
        ] {
            map.insert(
                Value::String(key.into()),
                mapping_get(&state, key).cloned().unwrap_or(Value::Null),
            );
        }
        Ok(Value::Mapping(map))
    }

    fn base_state(
        &self,
        previous: Option<&Value>,
        enabled: bool,
        reason: &str,
        source: &str,
    ) -> Result<Value> {
        let mut map = previous
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_else(Mapping::new);
        map.insert(
            Value::String("skill".into()),
            Value::String("raymancodingskill".into()),
        );
        map.insert(Value::String("enabled".into()), Value::Bool(enabled));
        map.insert(Value::String("auto_use".into()), Value::Bool(enabled));
        map.insert(
            Value::String("scope".into()),
            Value::String("workspace".into()),
        );
        map.insert(
            Value::String("workspace_path".into()),
            Value::String(display_path(&self.workspace)),
        );
        map.insert(
            Value::String("skill_file".into()),
            Value::String(display_path(&self.skill_file)),
        );
        map.insert(
            Value::String("skill_sha256".into()),
            if self.skill_file.exists() {
                Value::String(sha256_file(&self.skill_file)?)
            } else {
                Value::Null
            },
        );
        map.insert(
            Value::String("latest_policy".into()),
            Value::String(
                "read current workspace SKILL.md when present, otherwise canonical RaymanCodingSkill SKILL.md"
                    .into(),
            ),
        );
        map.insert(Value::String("reason".into()), Value::String(reason.into()));
        map.insert(Value::String("source".into()), Value::String(source.into()));
        map.insert(Value::String("updated_at".into()), Value::String(now_iso()));
        Ok(Value::Mapping(map))
    }

    fn read_state(&self) -> Result<Value> {
        if !self.state_path.exists() {
            return Ok(Value::Mapping(Mapping::new()));
        }
        let text = fs::read_to_string(&self.state_path)
            .with_context(|| format!("无法读取工作区状态: {}", self.state_path.display()))?;
        yaml::from_str(&text).context("无法解析工作区状态")
    }

    fn write_state(&self, state: &Value) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.state_path, yaml::to_string(state)?)?;
        Ok(())
    }
}

fn state_mapping_mut(state: &mut Value) -> Result<&mut Mapping> {
    state
        .as_mapping_mut()
        .context("workspace skill state must be a mapping")
}

fn canonical_skill_file() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)?
        .to_path_buf();
    let skill = root.join("SKILL.md");
    skill.exists().then_some(skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_record_use_does_not_reenable_stopped_workspace() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        let manager = WorkspaceActivationManager::new(temp.path()).unwrap();
        manager.disable("stop", "test").unwrap();
        let status = manager.record_use("used", "test").unwrap();
        assert_eq!(
            crate::config::mapping_get(&status, "enabled").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn workspace_status_reports_stale_skill_hash() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("SKILL.md");
        fs::write(&skill, "# skill").unwrap();
        let manager = WorkspaceActivationManager::new(temp.path()).unwrap();
        manager.enable("enable", "test").unwrap();
        fs::write(&skill, "# changed skill").unwrap();

        let status = manager.status().unwrap();

        assert_eq!(
            crate::config::mapping_get(&status, "uses_latest_skill_file").and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn workspace_state_preserves_unknown_fields() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        let manager = WorkspaceActivationManager::new(temp.path()).unwrap();
        let state = serde_yaml::from_str("custom_field: keep-me\n").unwrap();
        manager.write_state(&state).unwrap();

        manager.enable("enable", "test").unwrap();
        let state = manager.read_state().unwrap();

        assert_eq!(
            crate::config::mapping_get(&state, "custom_field").and_then(Value::as_str),
            Some("keep-me")
        );
    }

    #[test]
    fn workspace_state_mapping_helper_reports_non_mapping() {
        let mut state = Value::Sequence(Vec::new());

        let error = state_mapping_mut(&mut state).unwrap_err().to_string();

        assert!(error.contains("workspace skill state must be a mapping"));
    }

    #[test]
    fn customer_workspace_without_local_skill_uses_canonical_skill() {
        let temp = tempfile::tempdir().unwrap();
        let manager = WorkspaceActivationManager::new(temp.path()).unwrap();

        let status = manager.enable("enable", "test").unwrap();

        assert_eq!(
            crate::config::mapping_get(&status, "uses_latest_skill_file").and_then(Value::as_bool),
            Some(true)
        );
        let skill_file = crate::config::mapping_get(&status, "skill_file")
            .and_then(Value::as_str)
            .unwrap();
        assert!(
            skill_file.ends_with("RaymanCodingSkill\\SKILL.md")
                || skill_file.ends_with("RaymanCodingSkill/SKILL.md")
        );
    }

    #[test]
    fn enabled_workspace_subagent_authorization_matches_explicit_phrase() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        let manager = WorkspaceActivationManager::new(temp.path()).unwrap();
        let status = manager.enable("enable", "test").unwrap();
        assert_eq!(
            crate::config::mapping_get(&status, "enabled").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            crate::config::mapping_get(&status, "auto_use").and_then(Value::as_bool),
            Some(true)
        );

        let task = "review architecture and validation strategy";
        assert!(!task.contains("subagent"));
        assert!(!task.contains("开启subagent"));

        let plan = crate::subagent::SubagentLedgerManager::new(temp.path())
            .unwrap()
            .plan(crate::subagent::SubagentPlanRequest {
                task: task.into(),
                paths: vec![PathBuf::from("crates/rayman-core/src/goal.rs")],
                read_only: true,
                max_lanes: 2,
            })
            .unwrap();

        assert_eq!(plan["auto_start_ready"].as_bool(), Some(true));
        assert_eq!(
            plan["auto_start_contract"]["authorization_mode"].as_str(),
            Some("standing_workspace_authorization")
        );
        assert_eq!(
            plan["auto_start_contract"]["per_use_prompt_required"].as_bool(),
            Some(false)
        );
        assert_eq!(
            plan["auto_start_contract"]["explicit_subagent_phrase_required"].as_bool(),
            Some(false)
        );
    }
}
