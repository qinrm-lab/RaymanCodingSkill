use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{load_yaml, mapping_get};
use crate::yaml::Value as YamlValue;
use crate::{display_path, ensure_within, now_iso};

const DEFAULT_TEMP_ROOT: &str = ".RaymanCodingSkill/tmp";
const DEFAULT_RETENTION_HOURS: i64 = 24;
const RUNS_DIR: &str = "runs";
const METADATA_FILE: &str = "metadata.json";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeTempConfig {
    pub root: PathBuf,
    pub retention_hours: i64,
    pub preserve_failed: bool,
}

#[derive(Debug, Clone)]
pub struct TempManager {
    pub workspace: PathBuf,
    pub config: RuntimeTempConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempRunMetadata {
    pub command: String,
    pub workspace: String,
    pub pid: u32,
    pub created_at: String,
    pub updated_at: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct TempRun {
    pub path: PathBuf,
    pub metadata_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempEntryStatus {
    #[serde(default = "default_temp_entry_kind")]
    pub kind: String,
    pub path: PathBuf,
    pub status: String,
    pub command: Option<String>,
    pub pid: Option<u32>,
    pub created_at: Option<String>,
    pub stale: bool,
    pub managed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempStatus {
    pub workspace: PathBuf,
    pub temp_root: PathBuf,
    pub retention_hours: i64,
    pub preserve_failed: bool,
    pub entries: Vec<TempEntryStatus>,
    pub active: usize,
    pub completed: usize,
    pub failed: usize,
    pub stale: usize,
    pub foreign: usize,
    pub cargo_target_caches: usize,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempCleanupOptions {
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub all_failed: bool,
    #[serde(default)]
    pub cargo_targets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempCleanupReport {
    pub temp_root: PathBuf,
    pub removed: Vec<PathBuf>,
    pub skipped_active: Vec<PathBuf>,
    pub skipped_foreign: Vec<PathBuf>,
    pub failed: Vec<TempCleanupFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempCleanupFailure {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempDoctorReport {
    pub workspace: PathBuf,
    pub temp_root: PathBuf,
    pub checks: Vec<TempDoctorCheck>,
    pub status: String,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TempDoctorCheck {
    pub name: String,
    pub status: String,
    pub detail: String,
}

impl Default for RuntimeTempConfig {
    fn default() -> Self {
        Self {
            root: PathBuf::from(DEFAULT_TEMP_ROOT),
            retention_hours: DEFAULT_RETENTION_HOURS,
            preserve_failed: true,
        }
    }
}

impl TempManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let config = RuntimeTempConfig::load(&workspace)?;
        let root = if config.root.is_absolute() {
            config.root.clone()
        } else {
            workspace.join(&config.root)
        };
        let root = ensure_within(&root, &workspace, "临时目录必须位于工作区内")?;
        Ok(Self {
            workspace,
            config: RuntimeTempConfig { root, ..config },
        })
    }

    pub fn root(&self) -> &Path {
        &self.config.root
    }

    pub fn run_dir(&self, command: &str) -> Result<TempRun> {
        let runs_root = self.config.root.join(RUNS_DIR);
        fs::create_dir_all(&runs_root)
            .with_context(|| format!("无法创建临时运行目录: {}", runs_root.display()))?;
        let id = unique_id();
        let path = ensure_within(
            &runs_root.join(id),
            &self.workspace,
            "临时运行目录必须位于工作区内",
        )?;
        fs::create_dir(&path)
            .with_context(|| format!("无法创建临时运行目录: {}", path.display()))?;
        let metadata_path = path.join(METADATA_FILE);
        let metadata = TempRunMetadata {
            command: command.to_string(),
            workspace: display_path(&self.workspace),
            pid: std::process::id(),
            created_at: now_iso(),
            updated_at: now_iso(),
            status: "active".into(),
        };
        write_metadata(&metadata_path, &metadata)?;
        Ok(TempRun {
            path,
            metadata_path,
        })
    }

    pub fn atomic_temp_for(target: &Path, label: &str) -> PathBuf {
        atomic_temp_path(target, label)
    }

    pub fn status(&self) -> Result<TempStatus> {
        let entries = self.entries()?;
        let active = entries
            .iter()
            .filter(|entry| entry.managed && entry.status == "active")
            .count();
        let completed = entries
            .iter()
            .filter(|entry| entry.managed && entry.status == "completed")
            .count();
        let failed = entries
            .iter()
            .filter(|entry| entry.managed && entry.status == "failed")
            .count();
        let stale = entries
            .iter()
            .filter(|entry| self.stale_cleanup_eligible(entry))
            .count();
        let foreign = entries.iter().filter(|entry| !entry.managed).count();
        let cargo_target_caches = entries
            .iter()
            .filter(|entry| entry.kind == "cargo_target_cache")
            .count();
        let mut next_actions = Vec::new();
        if completed > 0 {
            next_actions.push(
                "Run rayman temp cleanup --completed to remove completed managed temp runs before task close.".into(),
            );
        }
        if stale > 0 {
            next_actions.push(
                "Run rayman temp cleanup --stale to remove expired managed temp runs.".into(),
            );
        }
        if failed > 0 {
            next_actions.push(
                "Inspect failed temp runs, then run rayman temp cleanup --all-failed when safe."
                    .into(),
            );
        }
        if cargo_target_caches > 0 {
            next_actions.push(
                "Run rayman temp cleanup --cargo-targets to remove recognized CARGO_TARGET_DIR caches after successful validation; preserve failed validation caches until inspected."
                    .into(),
            );
        }
        if foreign > 0 {
            next_actions.push(
                "Foreign entries are not removed automatically; inspect them manually.".into(),
            );
        }
        if next_actions.is_empty() {
            next_actions.push("No temp cleanup action is currently required.".into());
        }
        Ok(TempStatus {
            workspace: self.workspace.clone(),
            temp_root: self.config.root.clone(),
            retention_hours: self.config.retention_hours,
            preserve_failed: self.config.preserve_failed,
            entries,
            active,
            completed,
            failed,
            stale,
            foreign,
            cargo_target_caches,
            next_actions,
        })
    }

    pub fn success_blockers(&self) -> Result<Vec<String>> {
        let status = self.status()?;
        let mut blockers = Vec::new();
        if status.active > 0 {
            blockers.push(format!(
                "managed_temp_active: {} active managed temp runs remain; complete or fail them before success",
                status.active
            ));
        }
        if status.completed > 0 {
            blockers.push(format!(
                "managed_temp_completed: {} completed managed temp runs remain; run rayman temp cleanup --completed before success",
                status.completed
            ));
        }
        if status.stale > 0 {
            blockers.push(format!(
                "managed_temp_stale: {} stale managed temp runs remain; run rayman temp cleanup --stale before success",
                status.stale
            ));
        }
        if status.failed > 0 {
            blockers.push(format!(
                "managed_temp_failed: {} failed managed temp runs remain; inspect them, then run rayman temp cleanup --all-failed or close partial/blocked",
                status.failed
            ));
        }
        if status.cargo_target_caches > 0 {
            blockers.push(format!(
                "managed_temp_cargo_target_cache: {} retained CARGO_TARGET_DIR caches remain under .RaymanCodingSkill/tmp; run rayman temp cleanup --cargo-targets after successful validation or keep them only for non-success diagnosis",
                status.cargo_target_caches
            ));
        }
        if status.foreign > 0 {
            blockers.push(format!(
                "managed_temp_foreign: {} unmanaged temp entries under .RaymanCodingSkill/tmp/runs require manual inspection; Rayman will not delete unknown files",
                status.foreign
            ));
        }
        Ok(blockers)
    }

    pub fn cleanup(&self, options: &TempCleanupOptions) -> Result<TempCleanupReport> {
        let mut report = TempCleanupReport {
            temp_root: self.config.root.clone(),
            removed: Vec::new(),
            skipped_active: Vec::new(),
            skipped_foreign: Vec::new(),
            failed: Vec::new(),
        };
        for entry in self.entries()? {
            if !entry.managed {
                report.skipped_foreign.push(entry.path);
                continue;
            }
            if entry.kind == "cargo_target_cache" {
                if !options.cargo_targets {
                    continue;
                }
                match fs::remove_dir_all(&entry.path) {
                    Ok(()) => report.removed.push(entry.path),
                    Err(error) => report.failed.push(TempCleanupFailure {
                        path: entry.path,
                        error: error.to_string(),
                    }),
                }
                continue;
            }
            if entry.status == "active" {
                report.skipped_active.push(entry.path);
                continue;
            }
            let should_remove = (options.stale && self.stale_cleanup_eligible(&entry))
                || (options.completed && entry.status == "completed")
                || (options.all_failed && entry.status == "failed");
            if !should_remove {
                continue;
            }
            match fs::remove_dir_all(&entry.path) {
                Ok(()) => report.removed.push(entry.path),
                Err(error) => report.failed.push(TempCleanupFailure {
                    path: entry.path,
                    error: error.to_string(),
                }),
            }
        }
        Ok(report)
    }

    pub fn doctor(&self) -> TempDoctorReport {
        let mut checks = Vec::new();
        let mut next_actions = Vec::new();
        checks.push(TempDoctorCheck {
            name: "workspace_scope".into(),
            status: if self.config.root.starts_with(&self.workspace) {
                "passed".into()
            } else {
                next_actions.push("Set runtime_temp.root to a workspace-local path.".into());
                "failed".into()
            },
            detail: format!(
                "{} is under {}",
                display_path(&self.config.root),
                display_path(&self.workspace)
            ),
        });

        match fs::create_dir_all(&self.config.root) {
            Ok(()) => checks.push(TempDoctorCheck {
                name: "create_root".into(),
                status: "passed".into(),
                detail: format!("created or found {}", display_path(&self.config.root)),
            }),
            Err(error) => {
                next_actions.push("Fix workspace temp directory permissions.".into());
                checks.push(TempDoctorCheck {
                    name: "create_root".into(),
                    status: "failed".into(),
                    detail: error.to_string(),
                });
            }
        }

        let write_path = self
            .config
            .root
            .join(format!(".doctor-{}.tmp", unique_id()));
        match fs::write(&write_path, "rayman temp doctor") {
            Ok(()) => {
                let rename_path = write_path.with_extension("renamed");
                match fs::rename(&write_path, &rename_path) {
                    Ok(()) => {
                        let _ = fs::remove_file(&rename_path);
                        checks.push(TempDoctorCheck {
                            name: "same_directory_rename".into(),
                            status: "passed".into(),
                            detail: "same-directory temp write and rename succeeded".into(),
                        });
                    }
                    Err(error) => {
                        let _ = fs::remove_file(&write_path);
                        next_actions.push(
                            "Check file locks or antivirus rules for the workspace temp root."
                                .into(),
                        );
                        checks.push(TempDoctorCheck {
                            name: "same_directory_rename".into(),
                            status: "failed".into(),
                            detail: error.to_string(),
                        });
                    }
                }
            }
            Err(error) => {
                next_actions.push("Fix write permission for the workspace temp root.".into());
                checks.push(TempDoctorCheck {
                    name: "write_root".into(),
                    status: "failed".into(),
                    detail: error.to_string(),
                });
            }
        }

        let temp_root_display = display_path(&self.config.root);
        let temp_root_character_count = temp_root_display.chars().count();
        if cfg!(windows) && temp_root_character_count > 200 {
            next_actions.push(
                "Shorten runtime_temp.root or workspace path to reduce Windows path-length risk."
                    .into(),
            );
            checks.push(TempDoctorCheck {
                name: "path_length".into(),
                status: "warning".into(),
                detail: format!("temp root is {} characters", temp_root_character_count),
            });
        } else {
            checks.push(TempDoctorCheck {
                name: "path_length".into(),
                status: "passed".into(),
                detail: "temp root length is within the conservative threshold".into(),
            });
        }

        if next_actions.is_empty() {
            next_actions.push("Temp workspace is usable.".into());
        }
        let status = if checks.iter().any(|check| check.status == "failed") {
            "failed"
        } else if checks.iter().any(|check| check.status == "warning") {
            "warning"
        } else {
            "passed"
        };
        TempDoctorReport {
            workspace: self.workspace.clone(),
            temp_root: self.config.root.clone(),
            checks,
            status: status.into(),
            next_actions,
        }
    }

    fn entries(&self) -> Result<Vec<TempEntryStatus>> {
        if !self.config.root.exists() {
            return Ok(Vec::new());
        }
        let runs_root = self.config.root.join(RUNS_DIR);
        let cutoff = Utc::now() - Duration::hours(self.config.retention_hours.max(1));
        let mut entries = Vec::new();
        if runs_root.exists() {
            for entry in fs::read_dir(&runs_root)
                .with_context(|| format!("无法读取临时运行目录: {}", runs_root.display()))?
            {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let path = entry.path();
                let metadata_path = path.join(METADATA_FILE);
                let Ok(text) = fs::read_to_string(&metadata_path) else {
                    entries.push(TempEntryStatus {
                        kind: "foreign_run".into(),
                        path,
                        status: "foreign".into(),
                        command: None,
                        pid: None,
                        created_at: None,
                        stale: false,
                        managed: false,
                    });
                    continue;
                };
                let Ok(metadata) = serde_json::from_str::<TempRunMetadata>(&text) else {
                    entries.push(TempEntryStatus {
                        kind: "foreign_run".into(),
                        path,
                        status: "invalid_metadata".into(),
                        command: None,
                        pid: None,
                        created_at: None,
                        stale: false,
                        managed: false,
                    });
                    continue;
                };
                let created = DateTime::parse_from_rfc3339(&metadata.created_at)
                    .map(|value| value.with_timezone(&Utc))
                    .ok();
                let stale = created.is_some_and(|value| value < cutoff);
                entries.push(TempEntryStatus {
                    kind: "managed_run".into(),
                    path,
                    status: metadata.status,
                    command: Some(metadata.command),
                    pid: Some(metadata.pid),
                    created_at: Some(metadata.created_at),
                    stale,
                    managed: true,
                });
            }
        }
        for entry in fs::read_dir(&self.config.root)
            .with_context(|| format!("无法读取临时根目录: {}", self.config.root.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path();
            if path.file_name().and_then(|name| name.to_str()) == Some(RUNS_DIR) {
                continue;
            }
            if !is_cargo_target_cache(&path) {
                continue;
            }
            entries.push(TempEntryStatus {
                kind: "cargo_target_cache".into(),
                path,
                status: "retained".into(),
                command: Some("CARGO_TARGET_DIR".into()),
                pid: None,
                created_at: None,
                stale: false,
                managed: true,
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn stale_cleanup_eligible(&self, entry: &TempEntryStatus) -> bool {
        entry.managed
            && entry.stale
            && entry.status != "active"
            && (entry.status != "failed" || !self.config.preserve_failed)
    }
}

fn default_temp_entry_kind() -> String {
    "managed_run".into()
}

fn is_cargo_target_cache(path: &Path) -> bool {
    path.is_dir()
        && (path.join(".rustc_info.json").is_file() || path.join("CACHEDIR.TAG").is_file())
        && (path.join("debug").is_dir() || path.join("release").is_dir())
}

impl RuntimeTempConfig {
    fn load(workspace: &Path) -> Result<Self> {
        let mut config = Self::default();
        let config_path = workspace.join("config").join("default_config.yaml");
        if !config_path.exists() {
            return Ok(config);
        }
        let yaml = load_yaml(&config_path)?;
        let Some(runtime_temp) = mapping_get(&yaml, "runtime_temp") else {
            return Ok(config);
        };
        if let Some(root) = mapping_get(runtime_temp, "root").and_then(YamlValue::as_str) {
            config.root = PathBuf::from(root);
        }
        if let Some(retention) =
            mapping_get(runtime_temp, "retention_hours").and_then(YamlValue::as_i64)
        {
            config.retention_hours = retention.max(1);
        }
        if let Some(preserve_failed) =
            mapping_get(runtime_temp, "preserve_failed").and_then(YamlValue::as_bool)
        {
            config.preserve_failed = preserve_failed;
        }
        Ok(config)
    }
}

impl TempRun {
    pub fn complete(&self) -> Result<()> {
        self.set_status("completed")
    }

    pub fn fail(&self) -> Result<()> {
        self.set_status("failed")
    }

    fn set_status(&self, status: &str) -> Result<()> {
        let text = fs::read_to_string(&self.metadata_path)
            .with_context(|| format!("无法读取临时运行元数据: {}", self.metadata_path.display()))?;
        let mut metadata: TempRunMetadata =
            serde_json::from_str(&text).context("无法解析临时运行元数据")?;
        metadata.status = status.into();
        metadata.updated_at = now_iso();
        write_metadata(&self.metadata_path, &metadata)
    }
}

pub fn atomic_temp_path(target: &Path, label: &str) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("rayman");
    let label = label
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect::<String>();
    target.with_file_name(format!(
        ".{file_name}.rayman-{}-{}.tmp",
        if label.is_empty() { "atomic" } else { &label },
        unique_id()
    ))
}

pub fn write_atomic(target: &Path, text: &str, label: &str) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }
    let temp = atomic_temp_path(target, label);
    fs::write(&temp, text).with_context(|| format!("无法写入临时文件: {}", temp.display()))?;
    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        bail!(
            "无法原子替换文件: {} -> {}: {}",
            temp.display(),
            target.display(),
            error
        );
    }
    Ok(())
}

fn write_metadata(path: &Path, metadata: &TempRunMetadata) -> Result<()> {
    let value: Value = json!(metadata);
    fs::write(path, serde_json::to_string_pretty(&value)?)
        .with_context(|| format!("无法写入临时运行元数据: {}", path.display()))
}

fn unique_id() -> String {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}_{}_{}",
        Utc::now()
            .format("%Y%m%dT%H%M%S%fZ")
            .to_string()
            .trim_end_matches('0'),
        std::process::id(),
        counter
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_root_defaults_inside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().canonicalize().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();

        assert!(manager.root().starts_with(&workspace));
        assert!(manager.root().ends_with(".RaymanCodingSkill/tmp"));
    }

    #[test]
    fn run_dirs_are_unique_and_managed() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();

        let first = manager.run_dir("first").unwrap();
        let second = manager.run_dir("second").unwrap();

        assert_ne!(first.path, second.path);
        assert!(first.metadata_path.exists());
        assert_eq!(manager.status().unwrap().active, 2);
    }

    #[test]
    fn status_reports_completed_temp_cleanup_before_success() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let run = manager.run_dir("completed").unwrap();
        run.complete().unwrap();

        let status = manager.status().unwrap();
        let blockers = manager.success_blockers().unwrap();

        assert_eq!(status.completed, 1);
        assert!(
            status
                .next_actions
                .iter()
                .any(|action| { action.contains("rayman temp cleanup --completed") })
        );
        assert!(
            blockers
                .iter()
                .any(|blocker| { blocker.contains("managed_temp_completed") })
        );
    }

    #[test]
    fn stale_status_does_not_claim_active_or_preserved_failed_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let active = manager.run_dir("old active").unwrap();
        make_stale(&active);
        let failed = manager.run_dir("old failed").unwrap();
        failed.fail().unwrap();
        make_stale(&failed);

        let status = manager.status().unwrap();
        let blockers = manager.success_blockers().unwrap();
        let cleanup = manager
            .cleanup(&TempCleanupOptions {
                completed: false,
                stale: true,
                all_failed: false,
                cargo_targets: false,
            })
            .unwrap();

        assert_eq!(status.active, 1);
        assert_eq!(status.failed, 1);
        assert_eq!(status.stale, 0);
        assert_eq!(status.entries.iter().filter(|entry| entry.stale).count(), 2);
        assert!(
            !status
                .next_actions
                .iter()
                .any(|action| action.contains("rayman temp cleanup --stale"))
        );
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.contains("managed_temp_active"))
        );
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.contains("managed_temp_failed"))
        );
        assert!(
            !blockers
                .iter()
                .any(|blocker| blocker.contains("managed_temp_stale"))
        );
        assert!(cleanup.removed.is_empty());
        assert!(cleanup.skipped_active.contains(&active.path));
        assert!(failed.path.exists());
    }

    #[test]
    fn success_blockers_cover_active_completed_stale_failed_and_foreign() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let _active = manager.run_dir("active").unwrap();
        let completed = manager.run_dir("completed").unwrap();
        completed.complete().unwrap();
        let stale = manager.run_dir("stale completed").unwrap();
        stale.complete().unwrap();
        make_stale(&stale);
        let failed = manager.run_dir("failed").unwrap();
        failed.fail().unwrap();
        fs::create_dir_all(manager.root().join(RUNS_DIR).join("foreign")).unwrap();

        let blockers = manager.success_blockers().unwrap().join("\n");

        assert!(blockers.contains("managed_temp_active"));
        assert!(blockers.contains("managed_temp_completed"));
        assert!(blockers.contains("managed_temp_stale"));
        assert!(blockers.contains("managed_temp_failed"));
        assert!(blockers.contains("managed_temp_foreign"));
    }

    #[test]
    fn cleanup_skips_active_and_foreign_entries() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let run = manager.run_dir("active").unwrap();
        let foreign = manager.root().join(RUNS_DIR).join("foreign");
        fs::create_dir_all(&foreign).unwrap();

        let report = manager
            .cleanup(&TempCleanupOptions {
                completed: true,
                stale: true,
                all_failed: true,
                cargo_targets: false,
            })
            .unwrap();

        assert!(report.skipped_active.contains(&run.path));
        assert!(report.skipped_foreign.contains(&foreign));
        assert!(run.path.exists());
        assert!(foreign.exists());
    }

    #[test]
    fn cleanup_removes_failed_runs_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let run = manager.run_dir("failed").unwrap();
        run.fail().unwrap();

        let report = manager
            .cleanup(&TempCleanupOptions {
                completed: false,
                stale: false,
                all_failed: true,
                cargo_targets: false,
            })
            .unwrap();

        assert!(report.removed.contains(&run.path));
        assert!(!run.path.exists());
    }

    #[test]
    fn cleanup_removes_stale_completed_runs_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let run = manager.run_dir("stale completed").unwrap();
        run.complete().unwrap();
        make_stale(&run);

        let report = manager
            .cleanup(&TempCleanupOptions {
                completed: false,
                stale: true,
                all_failed: false,
                cargo_targets: false,
            })
            .unwrap();

        assert!(report.removed.contains(&run.path));
        assert!(!run.path.exists());
    }

    #[test]
    fn cleanup_removes_completed_runs_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let run = manager.run_dir("completed").unwrap();
        run.complete().unwrap();

        let report = manager
            .cleanup(&TempCleanupOptions {
                completed: true,
                stale: false,
                all_failed: false,
                cargo_targets: false,
            })
            .unwrap();

        assert!(report.removed.contains(&run.path));
        assert!(!run.path.exists());
    }

    #[test]
    fn status_reports_retained_cargo_target_caches_before_success() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let target = create_cargo_target_cache(&manager, "manual-target-tests");

        let status = manager.status().unwrap();
        let blockers = manager.success_blockers().unwrap();

        assert_eq!(status.cargo_target_caches, 1);
        assert!(
            status
                .entries
                .iter()
                .any(|entry| entry.path == target && entry.kind == "cargo_target_cache")
        );
        assert!(
            status
                .next_actions
                .iter()
                .any(|action| action.contains("rayman temp cleanup --cargo-targets"))
        );
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.contains("managed_temp_cargo_target_cache"))
        );
    }

    #[test]
    fn cleanup_preserves_cargo_target_cache_without_explicit_flag() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let target = create_cargo_target_cache(&manager, "manual-target-tests");

        let report = manager
            .cleanup(&TempCleanupOptions {
                completed: true,
                stale: true,
                all_failed: true,
                cargo_targets: false,
            })
            .unwrap();

        assert!(report.removed.is_empty());
        assert!(target.exists());
    }

    #[test]
    fn cleanup_removes_cargo_target_cache_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        let target = create_cargo_target_cache(&manager, "manual-target-tests");

        let report = manager
            .cleanup(&TempCleanupOptions {
                completed: false,
                stale: false,
                all_failed: false,
                cargo_targets: true,
            })
            .unwrap();

        assert!(report.removed.contains(&target));
        assert!(!target.exists());
    }

    #[test]
    fn cargo_deny_advisory_cache_is_not_a_cargo_target_cache() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();
        fs::create_dir_all(manager.root().join("cargo-deny").join("advisory-db")).unwrap();

        let status = manager.status().unwrap();
        let blockers = manager.success_blockers().unwrap();

        assert_eq!(status.cargo_target_caches, 0);
        assert_eq!(status.foreign, 0);
        assert!(blockers.is_empty());
    }

    #[test]
    fn atomic_temp_is_next_to_target() {
        let target = PathBuf::from("workspace/state.json");
        let temp = TempManager::atomic_temp_for(&target, "stats");

        assert_eq!(temp.parent(), target.parent());
        assert!(
            temp.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("stats")
        );
    }

    #[test]
    fn doctor_reports_usable_temp_root() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TempManager::new(temp.path()).unwrap();

        let report = manager.doctor();

        assert_eq!(report.status, "passed");
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "same_directory_rename")
        );
    }

    fn make_stale(run: &TempRun) {
        let text = fs::read_to_string(&run.metadata_path).unwrap();
        let mut metadata: TempRunMetadata = serde_json::from_str(&text).unwrap();
        let old = (Utc::now() - Duration::hours(48)).to_rfc3339();
        metadata.created_at = old.clone();
        metadata.updated_at = old;
        write_metadata(&run.metadata_path, &metadata).unwrap();
    }

    fn create_cargo_target_cache(manager: &TempManager, name: &str) -> PathBuf {
        let target = manager.root().join(name);
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::write(target.join(".rustc_info.json"), "{}").unwrap();
        fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        target
    }
}
