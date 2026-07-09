//! File hashing helpers.

use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("无法打开文件: {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("无法读取文件: {}", path.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}
