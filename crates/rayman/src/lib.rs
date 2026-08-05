//! rayman：RaymanCodingSkill 的精简 v2 核心。
//! 只保留 load-bearing 能力：上下文索引、最小目标契约、只读资产扫描、托管临时目录。

pub mod assets;
pub const CLI_CONTRACT: &str = "rayman-cli-contract-v15";
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod autosave;
pub mod checkpoint;
pub mod codex_hook;
pub mod codex_host;
pub mod context;
pub mod goal;
pub mod hash;
pub mod map;
pub mod source_state;
pub mod state_lock;
pub mod state_paths;
pub mod temp;
pub mod toolchain;
pub mod walk;
pub mod workspace;

mod file_io;
pub mod pathfmt;
pub mod timefmt;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 工作区根：从当前目录向上查找含 `.RaymanCodingSkill/` 或 `.git` 的最近祖先，
/// 找不到则回退到当前目录。修复“从子目录运行会另建一份状态、分裂工作区”的问题。
/// `.git` 用 exists()：git worktree / submodule 的 `.git` 是文件而非目录。
pub fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("无法读取当前目录")?;
    let mut dir = cwd.as_path();
    loop {
        if is_workspace_marker(dir)? {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(cwd),
        }
    }
}

fn is_workspace_marker(dir: &Path) -> Result<bool> {
    // A linked state root must not become the authority boundary that selects
    // a workspace.  `managed_state_root` verifies both symlinks and Windows
    // reparse points before reporting a marker.
    Ok(state_paths::managed_state_root(dir, false)?.is_some() || dir.join(".git").exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_marker_rejects_an_invalid_state_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".RaymanCodingSkill"), "not a directory").unwrap();
        assert!(is_workspace_marker(dir.path()).is_err());
    }
}
