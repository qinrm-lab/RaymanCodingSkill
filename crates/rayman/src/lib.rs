//! rayman：RaymanCodingSkill 的精简 v2 核心。
//! 只保留 load-bearing 能力：上下文索引、最小目标契约、只读资产扫描、托管临时目录。

pub mod assets;
pub mod autosave;
pub mod checkpoint;
pub mod context;
pub mod fsutil;
pub mod goal;
pub mod map;
pub mod temp;
pub mod walk;

mod file_io;
mod hash;
mod path_guard;
mod pathfmt;
mod project_store;
mod state_store;
mod timefmt;
mod ui_paths;

use std::path::PathBuf;

use anyhow::{Context, Result};

/// 工作区根：从当前目录向上查找含 `.RaymanCodingSkill/` 或 `.git/` 的最近祖先，
/// 找不到则回退到当前目录。修复“从子目录运行会另建一份状态、分裂工作区”的问题。
pub fn workspace_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("无法读取当前目录")?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join(".RaymanCodingSkill").is_dir() || dir.join(".git").is_dir() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return Ok(cwd),
        }
    }
}
