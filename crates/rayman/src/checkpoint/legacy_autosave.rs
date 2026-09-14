//! Legacy snapshot compatibility only. No scheduler, tick or state writer.
use crate::{file_io::is_link_or_reparse, pathfmt::display_path, state_paths};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const AUTOSAVE_LOCK_NAME: &str = "autosave.lock";
const AUTOSAVE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Serialize restore against an older CLI's legacy writer. Keeping the
/// lock file stable (rather than deleting it after unlock) prevents different
/// processes from locking different file identities after an unlink/recreate
/// race.
pub(super) struct AutosaveLock {
    file: fs::File,
}

/// Retain the old lock identity for snapshot compatibility.
///
/// `checkpoint restore` republishes `autosave.json`, which is guarded by this
/// lock rather than by a per-file state lock. An older installed CLI may still
/// hold it during an upgrade; this module cannot start that writer.
pub(super) fn acquire_lock(root: &Path) -> Result<AutosaveLock> {
    AutosaveLock::acquire(root)
}

impl AutosaveLock {
    fn acquire(root: &Path) -> Result<Self> {
        Self::acquire_with_timeout(root, AUTOSAVE_LOCK_TIMEOUT)
    }

    fn acquire_with_timeout(root: &Path, timeout: Duration) -> Result<Self> {
        let path = state_paths::managed_state_file(root, Path::new(AUTOSAVE_LOCK_NAME), true)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("无法打开 autosave 独占锁: {}", display_path(&path)))?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("无法复查 autosave 独占锁: {}", display_path(&path)))?;
        if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
            bail!("autosave 独占锁不是安全普通文件: {}", display_path(&path));
        }
        if !file
            .metadata()
            .with_context(|| format!("无法读取 autosave 锁句柄: {}", display_path(&path)))?
            .file_type()
            .is_file()
        {
            bail!("autosave 锁句柄不是普通文件: {}", display_path(&path));
        }

        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if is_lock_busy(&error) && started.elapsed() < timeout => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) if is_lock_busy(&error) => {
                    bail!("等待 autosave 独占锁超过 {} 秒", timeout.as_secs_f64());
                }
                Err(error) => return Err(error).context("无法取得 autosave 独占锁"),
            }
        }
    }
}

impl Drop for AutosaveLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn is_lock_busy(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock || matches!(error.raw_os_error(), Some(32) | Some(33))
}

/// Preserve the old custom-store hint for manual snapshot discovery.
/// It is optional metadata, never authority to restart the retired scheduler.
pub fn configured_legacy_dir(root: &Path) -> Option<PathBuf> {
    let path = state_paths::managed_state_file(root, Path::new("autosave.json"), false).ok()?;
    let value: serde_json::Value = crate::file_io::read_json(&path).ok().flatten()?;
    value.get("dir")?.as_str().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_checkpoint_hint_preserves_existing_bytes() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        assert!(configured_legacy_dir(root).is_none());
        assert!(!root.join(".RaymanCodingSkill").exists());
        fs::create_dir(root.join(".RaymanCodingSkill")).unwrap();
        let path = root.join(".RaymanCodingSkill/autosave.json");
        let bytes = br#"{"dir":"old-custom-store","active":true}"#;
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            configured_legacy_dir(root),
            Some(PathBuf::from("old-custom-store"))
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
        fs::write(&path, b"{broken").unwrap();
        assert!(configured_legacy_dir(root).is_none());
        assert_eq!(fs::read(&path).unwrap(), b"{broken");
    }

    #[test]
    fn legacy_autosave_lock_remains_compatible_with_restoration() {
        let workspace = tempfile::tempdir().unwrap();
        let first = acquire_lock(workspace.path()).unwrap();
        let path = workspace.path().join(".RaymanCodingSkill/autosave.lock");
        assert!(path.is_file());
        assert!(
            AutosaveLock::acquire_with_timeout(workspace.path(), Duration::from_millis(50))
                .is_err()
        );
        drop(first);
        let second = acquire_lock(workspace.path()).unwrap();
        drop(second);
        assert!(path.is_file());
    }
}
