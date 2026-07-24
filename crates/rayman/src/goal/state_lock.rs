use std::fs::{self, OpenOptions};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use fs2::FileExt;

use crate::state_paths;

/// Inter-process mutual exclusion for read-modify-write state transactions.
///
/// A stable regular file carries an OS advisory lock. The kernel releases the
/// lock when a process exits, so crash recovery never guesses from mtime or
/// deletes a lock path. Permission/ACL failures remain distinct from contention.
pub(super) struct StateLock {
    file: fs::File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(super) fn is_state_lock_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        // Windows LockFileEx maps real contention to sharing/lock violations.
        // ERROR_ACCESS_DENIED (5) is deliberately not retryable.
        || matches!(error.raw_os_error(), Some(32 | 33))
}

pub(super) fn acquire_state_lock(target: &Path) -> Result<StateLock> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    state_paths::ensure_real_directory(parent)?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let lock_path = parent.join(format!(".{name}.rayman.lock"));

    match fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            bail!("状态锁不是安全普通文件: {}", lock_path.display());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).context(format!("无法检查状态锁: {}", lock_path.display()));
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("无法打开状态锁（权限或 ACL 拒绝）: {}", lock_path.display()))?;
    let metadata = fs::symlink_metadata(&lock_path)
        .with_context(|| format!("无法复查状态锁: {}", lock_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        bail!("状态锁被替换为非普通文件: {}", lock_path.display());
    }

    const LOCK_TIMEOUT: Duration = Duration::from_millis(2500);
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(StateLock { file }),
            Err(error) if is_state_lock_contention(&error) && started.elapsed() < LOCK_TIMEOUT => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) if is_state_lock_contention(&error) => bail!(
                "状态正在被另一个 rayman 进程修改: {}；等待锁超过 {} 秒",
                target.display(),
                LOCK_TIMEOUT.as_secs_f64()
            ),
            Err(error) => {
                return Err(error).context(format!(
                    "无法取得状态独占锁（权限或 ACL 拒绝）: {}",
                    lock_path.display()
                ));
            }
        }
    }
}
