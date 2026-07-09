//! 精简版托管临时目录：运行期临时文件放工作区本地 `.RaymanCodingSkill/tmp/`，
//! 不用系统临时目录、不记忆跨项目位置。提供创建与清理，清理只删自己管辖的目录。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ui_paths::display_path;

const TEMP_ROOT: &str = ".RaymanCodingSkill/tmp";

pub fn temp_root(root: &Path) -> PathBuf {
    root.join(TEMP_ROOT)
}

/// 在托管临时根下创建（若不存在）一个具名子目录并返回其路径。
pub fn scratch_dir(root: &Path, label: &str) -> Result<PathBuf> {
    let safe: String = label
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    let dir = temp_root(root).join(if safe.is_empty() {
        "scratch".into()
    } else {
        safe
    });
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("无法创建临时目录: {}", dir.display()))?;
    Ok(dir)
}

#[derive(Debug, Clone)]
pub struct TempStatus {
    pub root: String,
    pub exists: bool,
    pub entry_count: usize,
}

pub fn status(root: &Path) -> TempStatus {
    let dir = temp_root(root);
    let entry_count = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    TempStatus {
        root: display_path(&dir),
        exists: dir.exists(),
        entry_count,
    }
}

/// 清理整个托管临时根。只删 `.RaymanCodingSkill/tmp`，绝不触碰其它用户数据。
pub fn cleanup(root: &Path) -> Result<bool> {
    let dir = temp_root(root);
    if !dir.exists() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&dir)
        .with_context(|| format!("无法清理临时目录: {}", dir.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_create_status_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let scratch = scratch_dir(root, "build cache").unwrap();
        std::fs::write(scratch.join("f.txt"), "x").unwrap();
        assert!(status(root).exists);
        assert_eq!(status(root).entry_count, 1);
        assert!(cleanup(root).unwrap());
        assert!(!status(root).exists);
    }
}
