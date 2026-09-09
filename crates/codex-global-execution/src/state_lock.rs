use anyhow::{Result, bail};
use fs2::FileExt;
use std::{
    fs::File,
    path::Path,
    time::{Duration, Instant},
};

pub struct StateLock(File);
impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
pub fn acquire_state_lock(target: &Path) -> Result<StateLock> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("lock parent missing"))?;
    crate::state_paths::ensure_real_directory(parent)?;
    let leaf = target
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("lock name invalid"))?;
    let lock = parent.join(format!(".{leaf}.rayman.lock"));
    if let Ok(metadata) = std::fs::symlink_metadata(&lock)
        && (!metadata.is_file() || crate::file_io::is_link_or_reparse(&metadata))
    {
        bail!("state lock is not an ordinary file");
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock)?;
    let start = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(StateLock(file)),
            Err(e)
                if (e.kind() == std::io::ErrorKind::WouldBlock
                    || matches!(e.raw_os_error(), Some(32 | 33)))
                    && start.elapsed() < Duration::from_millis(2500) =>
            {
                std::thread::sleep(Duration::from_millis(25))
            }
            Err(e) => return Err(e.into()),
        }
    }
}
