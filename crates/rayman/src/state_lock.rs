use std::fs::{self, OpenOptions};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use fs2::FileExt;

use crate::pathfmt::display_path;
use crate::state_paths;

/// Inter-process mutual exclusion for read-modify-write state transactions.
///
/// A stable regular file carries an OS advisory lock. The kernel releases the
/// lock when a process exits, so crash recovery never guesses from mtime or
/// deletes a lock path. Permission/ACL failures remain distinct from contention.
pub struct StateLock {
    file: fs::File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub fn is_state_lock_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        // Windows LockFileEx maps real contention to sharing/lock violations.
        // ERROR_ACCESS_DENIED (5) is deliberately not retryable.
        || matches!(error.raw_os_error(), Some(32 | 33))
}

fn state_lock_contention_timeout(
    error: std::io::Error,
    target: &Path,
    timeout: Duration,
) -> anyhow::Error {
    anyhow::Error::new(error).context(format!(
        "状态正在被另一个 rayman 进程修改: {}；等待锁超过 {} 秒",
        display_path(target),
        timeout.as_secs_f64()
    ))
}

pub fn acquire_state_lock(target: &Path) -> Result<StateLock> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    state_paths::ensure_real_directory(parent)?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let lock_path = parent.join(format!(".{name}.rayman.lock"));

    match fs::symlink_metadata(&lock_path) {
        // Use the shared predicate: a bare `is_symlink()` check misses Windows
        // reparse points, which every other managed-state path already refuses.
        Ok(metadata) if crate::file_io::is_link_or_reparse(&metadata) || !metadata.is_file() => {
            bail!("状态锁不是安全普通文件: {}", display_path(&lock_path));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).context(format!("无法检查状态锁: {}", display_path(&lock_path)));
        }
    }
    // Opening the lock file races other rayman processes doing the same. On
    // Windows that surfaces as a sharing violation (32/33), which the flock step
    // below already treats as retryable contention — reporting it here as an ACL
    // denial named the wrong cause and gave up without waiting.
    const OPEN_TIMEOUT: Duration = Duration::from_millis(2500);
    let open_started = Instant::now();
    let file = loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => break file,
            Err(error)
                if is_state_lock_contention(&error) && open_started.elapsed() < OPEN_TIMEOUT =>
            {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) if is_state_lock_contention(&error) => {
                return Err(error).context(format!(
                    "状态锁正被另一个 rayman 进程占用: {}",
                    display_path(&lock_path)
                ));
            }
            Err(error) => {
                return Err(error).context(format!(
                    "无法打开状态锁（权限或 ACL 拒绝）: {}",
                    display_path(&lock_path)
                ));
            }
        }
    };
    let metadata = fs::symlink_metadata(&lock_path)
        .with_context(|| format!("无法复查状态锁: {}", display_path(&lock_path)))?;
    if crate::file_io::is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!("状态锁被替换为非普通文件: {}", display_path(&lock_path));
    }

    const LOCK_TIMEOUT: Duration = Duration::from_millis(2500);
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(StateLock { file }),
            Err(error) if is_state_lock_contention(&error) && started.elapsed() < LOCK_TIMEOUT => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) if is_state_lock_contention(&error) => {
                return Err(state_lock_contention_timeout(error, target, LOCK_TIMEOUT));
            }
            Err(error) => {
                return Err(error).context(format!(
                    "无法取得状态独占锁（权限或 ACL 拒绝）: {}",
                    display_path(&lock_path)
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_timeout_preserves_the_structured_io_error() {
        let error = state_lock_contention_timeout(
            std::io::Error::from(std::io::ErrorKind::WouldBlock),
            Path::new("goal.json"),
            Duration::from_millis(2500),
        );

        assert!(error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(is_state_lock_contention)
        }));
    }
}
