use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use walkdir::WalkDir;

use crate::{display_path, ensure_within, now_iso};

#[derive(Debug, Clone)]
pub struct BackupManager {
    pub repository: PathBuf,
    pub backup_dir: PathBuf,
    pub retention_days: i64,
    pub max_backups_per_target: usize,
}

impl BackupManager {
    pub fn new(repository: impl Into<PathBuf>) -> Result<Self> {
        let repository = repository
            .into()
            .canonicalize()
            .context("无法解析仓库路径")?;
        let backup_dir = ensure_within(
            &repository.join(".RaymanCodingSkill").join("backups"),
            &repository,
            "备份目录必须位于仓库内",
        )?;
        Ok(Self {
            repository,
            backup_dir,
            retention_days: 30,
            max_backups_per_target: 20,
        })
    }

    pub fn create_backup(&self, paths: &[String], comment: &str, source: &str) -> Result<Value> {
        if comment.trim().is_empty() {
            bail!("创建备份必须提供注释");
        }
        if paths.is_empty() {
            bail!("至少需要一个备份路径");
        }
        let targets: Vec<PathBuf> = paths
            .iter()
            .map(|path| self.resolve_target(path))
            .collect::<Result<_>>()?;
        let backup_id = self.new_backup_id(&targets, comment);
        let backup_root = self.backup_dir.join(&backup_id);
        let snapshot_root = backup_root.join("files");
        fs::create_dir_all(&snapshot_root)?;
        let mut target_meta = Vec::new();
        for target in &targets {
            target_meta.push(self.copy_target(target, &snapshot_root)?);
        }
        let size = target_meta
            .iter()
            .filter_map(|target| target.get("size").and_then(Value::as_u64))
            .sum::<u64>();
        let metadata = json!({
            "id": backup_id,
            "created_at": now_iso(),
            "comment": comment.trim(),
            "source": source,
            "repository_path": display_path(&self.repository),
            "original_path": target_meta.first().and_then(|v| v.get("original_path")).cloned().unwrap_or(Value::Null),
            "targets": target_meta,
            "checksum": "",
            "size": size,
            "retention": {
                "retention_days": self.retention_days,
                "max_backups_per_target": self.max_backups_per_target,
            }
        });
        fs::write(
            backup_root.join("index.json"),
            serde_json::to_string_pretty(&metadata)?,
        )?;
        self.with_stale_status(metadata)
    }

    pub fn list_backups(&self) -> Result<Vec<Value>> {
        if !self.backup_dir.exists() {
            return Ok(Vec::new());
        }
        let mut backups = Vec::new();
        for entry in fs::read_dir(&self.backup_dir)? {
            let entry = entry?;
            let index = entry.path().join("index.json");
            if index.exists()
                && let Ok(text) = fs::read_to_string(index)
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                backups.push(self.with_stale_status(value)?);
            }
        }
        backups.sort_by_key(|item| {
            item.get("created_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
        backups.reverse();
        Ok(mark_count_stale(backups, self.max_backups_per_target))
    }

    pub fn restore_backup(&self, backup_id: &str) -> Result<Value> {
        let metadata = self.read_backup(backup_id)?;
        let snapshot_root = self.backup_path(backup_id)?.join("files");
        let mut restore_pairs = Vec::new();
        let mut restore_empty_dirs = Vec::new();
        let mut existing_targets = Vec::new();
        for target in metadata
            .get("targets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            for file_info in target
                .get("files")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let relative = file_info
                    .get("relative_path")
                    .and_then(Value::as_str)
                    .context("备份元数据缺少 relative_path")?;
                let source = ensure_within(
                    &snapshot_root.join(relative),
                    &snapshot_root,
                    "备份快照路径越界",
                )?;
                let destination = ensure_within(
                    &self.repository.join(relative),
                    &self.repository,
                    "备份目标路径越界",
                )?;
                verify_snapshot_checksum(&file_info, &source)?;
                if destination.exists() {
                    existing_targets.push(display_path(&destination));
                }
                restore_pairs.push((source, destination));
            }
            for dir_info in target
                .get("empty_dirs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let relative = dir_info
                    .as_str()
                    .context("备份元数据 empty_dirs 缺少路径")?;
                let destination = ensure_within(
                    &self.repository.join(relative),
                    &self.repository,
                    "备份空目录目标路径越界",
                )?;
                restore_empty_dirs.push(destination);
            }
        }
        let safety_backup = if existing_targets.is_empty() {
            Value::Null
        } else {
            self.create_backup(
                &existing_targets,
                &format!("safety backup before restoring {backup_id}"),
                "restore_safety",
            )?
        };
        let mut restored_dirs = Vec::new();
        for destination in restore_empty_dirs {
            fs::create_dir_all(&destination)?;
            restored_dirs.push(display_path(&destination));
        }
        let mut restored = Vec::new();
        for (source, destination) in restore_pairs {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &destination)?;
            restored.push(display_path(&destination));
        }
        Ok(json!({
            "id": backup_id,
            "restored_files": restored,
            "restored_empty_dirs": restored_dirs,
            "safety_backup": safety_backup
        }))
    }

    pub fn cleanup_stale(&self) -> Result<Value> {
        let stale: Vec<Value> = self
            .list_backups()?
            .into_iter()
            .filter(|backup| {
                backup
                    .get("stale")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .collect();
        let mut removed = Vec::new();
        for backup in stale {
            let Some(id) = backup.get("id").and_then(Value::as_str) else {
                continue;
            };
            let path = self.backup_path(id)?;
            if path.exists() {
                fs::remove_dir_all(path)?;
                removed.push(id.to_string());
            }
        }
        Ok(json!({"removed_count": removed.len(), "removed": removed}))
    }

    pub fn stale_hint(&self) -> Result<Option<String>> {
        let count = self
            .list_backups()?
            .iter()
            .filter(|backup| {
                backup
                    .get("stale")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            })
            .count();
        Ok((count > 0)
            .then(|| format!("发现 {count} 份过时备份，建议运行: rayman backup cleanup --stale")))
    }

    fn resolve_target(&self, path: &str) -> Result<PathBuf> {
        let path = PathBuf::from(path);
        let candidate = if path.is_absolute() {
            path
        } else {
            self.repository.join(path)
        };
        let target = ensure_within(&candidate, &self.repository, "备份路径必须位于仓库内")?;
        if !target.exists() {
            bail!("备份路径不存在: {}", target.display());
        }
        if target.starts_with(&self.backup_dir) {
            bail!("不能备份备份目录自身");
        }
        Ok(target)
    }

    fn copy_target(&self, target: &Path, snapshot_root: &Path) -> Result<Value> {
        let relative = target
            .strip_prefix(&self.repository)?
            .to_string_lossy()
            .replace('\\', "/");
        let mut files = Vec::new();
        let mut empty_dirs = Vec::new();
        if target.is_file() {
            files.push(self.copy_file(target, snapshot_root)?);
        } else {
            for entry in WalkDir::new(target)
                .into_iter()
                .filter_entry(|entry| !entry.path().starts_with(&self.backup_dir))
                .filter_map(|entry| entry.ok())
            {
                if entry.file_type().is_file() {
                    files.push(self.copy_file(entry.path(), snapshot_root)?);
                } else if entry.file_type().is_dir() && is_empty_dir(entry.path())? {
                    empty_dirs.push(
                        entry
                            .path()
                            .strip_prefix(&self.repository)?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        empty_dirs.sort();
        let size = files
            .iter()
            .filter_map(|file| file.get("size").and_then(Value::as_u64))
            .sum::<u64>();
        Ok(json!({
            "original_path": display_path(target),
            "relative_path": relative,
            "type": if target.is_dir() { "directory" } else { "file" },
            "files": files,
            "empty_dirs": empty_dirs,
            "checksum": "",
            "size": size,
        }))
    }

    fn copy_file(&self, path: &Path, snapshot_root: &Path) -> Result<Value> {
        let relative = path
            .strip_prefix(&self.repository)?
            .to_string_lossy()
            .replace('\\', "/");
        let destination = snapshot_root.join(&relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(path, &destination)?;
        Ok(json!({
            "relative_path": relative,
            "checksum": file_checksum(path)?,
            "size": path.metadata()?.len(),
        }))
    }

    fn read_backup(&self, backup_id: &str) -> Result<Value> {
        let path = self.backup_path(backup_id)?.join("index.json");
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    fn backup_path(&self, backup_id: &str) -> Result<PathBuf> {
        if backup_id.is_empty()
            || backup_id.contains('/')
            || backup_id.contains('\\')
            || backup_id.contains("..")
        {
            bail!("无效备份 ID: {backup_id}");
        }
        ensure_within(
            &self.backup_dir.join(backup_id),
            &self.backup_dir,
            "备份路径越界",
        )
    }

    fn new_backup_id(&self, targets: &[PathBuf], comment: &str) -> String {
        let mut digest = Sha1::new();
        for target in targets {
            digest.update(display_path(target));
        }
        digest.update(comment);
        digest.update(now_iso());
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%6fZ");
        format!("bkp_{}_{}", stamp, &format!("{:x}", digest.finalize())[..8])
    }

    fn with_stale_status(&self, mut metadata: Value) -> Result<Value> {
        let created = metadata
            .get("created_at")
            .and_then(Value::as_str)
            .unwrap_or("");
        let stale = chrono::DateTime::parse_from_rfc3339(&created.replace('Z', "+00:00"))
            .map(|created| {
                (chrono::Utc::now() - created.with_timezone(&chrono::Utc)).num_days()
                    > self.retention_days
            })
            .unwrap_or(false);
        metadata["stale"] = Value::Bool(false);
        metadata["stale_reasons"] = json!([]);
        if stale {
            add_stale_reason(
                &mut metadata,
                &format!("超过保留天数 {} 天", self.retention_days),
            );
        }
        Ok(metadata)
    }
}

fn mark_count_stale(mut backups: Vec<Value>, max_per_target: usize) -> Vec<Value> {
    if max_per_target == 0 {
        for backup in &mut backups {
            add_stale_reason(backup, "超过同一目标最多保留份数");
        }
        return backups;
    }
    let mut seen_by_target: HashMap<String, usize> = HashMap::new();
    for backup in &mut backups {
        let target_keys = backup_target_keys(backup);
        let over_limit = target_keys.into_iter().any(|target| {
            let seen = seen_by_target.entry(target).or_insert(0);
            *seen += 1;
            *seen > max_per_target
        });
        if over_limit {
            add_stale_reason(backup, "超过同一目标最多保留份数");
        }
    }
    backups
}

fn backup_target_keys(backup: &Value) -> Vec<String> {
    let mut keys = backup
        .get("targets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|target| {
            target
                .get("relative_path")
                .or_else(|| target.get("original_path"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    if keys.is_empty()
        && let Some(path) = backup.get("original_path").and_then(Value::as_str)
    {
        keys.push(path.to_string());
    }
    keys.sort();
    keys.dedup();
    keys
}

fn verify_snapshot_checksum(file_info: &Value, source: &Path) -> Result<()> {
    let expected = file_info
        .get("checksum")
        .and_then(Value::as_str)
        .unwrap_or("");
    if expected.is_empty() {
        return Ok(());
    }
    let actual = file_checksum(source)?;
    if actual != expected {
        bail!(
            "备份快照校验失败: {} expected={} actual={}",
            source.display(),
            expected,
            actual
        );
    }
    Ok(())
}

fn add_stale_reason(backup: &mut Value, reason: &str) {
    backup["stale"] = Value::Bool(true);
    if !backup
        .get("stale_reasons")
        .map(Value::is_array)
        .unwrap_or(false)
    {
        backup["stale_reasons"] = json!([]);
    }
    if let Some(reasons) = backup["stale_reasons"].as_array_mut()
        && !reasons.iter().any(|item| item.as_str() == Some(reason))
    {
        reasons.push(Value::String(reason.to_string()));
    }
}

fn file_checksum(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn is_empty_dir(path: &Path) -> Result<bool> {
    Ok(fs::read_dir(path)?.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_rejects_outside_path() {
        let temp = tempfile::tempdir().unwrap();
        let manager = BackupManager::new(temp.path()).unwrap();
        assert!(
            manager
                .create_backup(&[String::from("missing")], "x", "test")
                .is_err()
        );
    }

    #[test]
    fn stale_reason_replaces_malformed_reason_list_without_panic() {
        let mut backup = json!({
            "stale": false,
            "stale_reasons": "not an array"
        });

        add_stale_reason(&mut backup, "过期");

        assert_eq!(backup["stale"], true);
        assert_eq!(backup["stale_reasons"], json!(["过期"]));
    }

    #[test]
    fn restore_creates_safety_backup_before_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("config.txt");
        fs::write(&file, "before").unwrap();
        let manager = BackupManager::new(temp.path()).unwrap();
        let backup = manager
            .create_backup(&["config.txt".into()], "snapshot", "test")
            .unwrap();
        fs::write(&file, "current").unwrap();

        let restored = manager
            .restore_backup(backup["id"].as_str().unwrap())
            .unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "before");
        assert_ne!(restored["safety_backup"], Value::Null);
        assert_eq!(restored["restored_files"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn restore_rejects_corrupt_snapshot_checksum() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("config.txt");
        fs::write(&file, "before").unwrap();
        let manager = BackupManager::new(temp.path()).unwrap();
        let backup = manager
            .create_backup(&["config.txt".into()], "snapshot", "test")
            .unwrap();
        fs::write(&file, "current").unwrap();
        let backup_id = backup["id"].as_str().unwrap();
        fs::write(
            temp.path()
                .join(".RaymanCodingSkill")
                .join("backups")
                .join(backup_id)
                .join("files")
                .join("config.txt"),
            "corrupt",
        )
        .unwrap();

        let result = manager.restore_backup(backup_id);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "current");
    }

    #[test]
    fn directory_backup_restores_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        let empty_dir = temp.path().join("data").join("empty");
        fs::create_dir_all(&empty_dir).unwrap();
        let manager = BackupManager::new(temp.path()).unwrap();
        let backup = manager
            .create_backup(&["data".into()], "snapshot", "test")
            .unwrap();
        fs::remove_dir_all(&empty_dir).unwrap();

        let restored = manager
            .restore_backup(backup["id"].as_str().unwrap())
            .unwrap();

        assert!(empty_dir.is_dir());
        assert_eq!(restored["restored_empty_dirs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn stale_count_is_tracked_per_target() {
        let backups = vec![
            json!({"id": "a1", "targets": [{"relative_path": "a.txt"}], "stale": false, "stale_reasons": []}),
            json!({"id": "b1", "targets": [{"relative_path": "b.txt"}], "stale": false, "stale_reasons": []}),
            json!({"id": "a2", "targets": [{"relative_path": "a.txt"}], "stale": false, "stale_reasons": []}),
        ];

        let marked = mark_count_stale(backups, 1);

        assert_eq!(marked[0]["stale"].as_bool(), Some(false));
        assert_eq!(marked[1]["stale"].as_bool(), Some(false));
        assert_eq!(marked[2]["stale"].as_bool(), Some(true));
    }
}
