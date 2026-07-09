//! 工作树 checkpoint：把当前工作区（gitignore 感知）+ rayman 任务状态整树拷贝到
//! 用户级 checkpoint 根，便于断电/切换 AI 工具（codex/claude/copilot）后快速恢复。
//! 每次保存后按 `keep` 数清理旧快照——保存是“先写暂存目录再原子 rename”，中断不会污染最新快照。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state_store::{self, display_path};
use crate::walk;

/// 默认保留的快照数（滚动，多留几个以防某次保存中途损坏）。
pub const DEFAULT_KEEP: usize = 3;

const TREE_SUBDIR: &str = "tree";
const MANIFEST_NAME: &str = "manifest.json";
const STAGING_PREFIX: &str = ".staging-";

/// 单个快照的元数据。
#[derive(Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub created_at: String,
    pub workspace_root: String,
    pub file_count: usize,
    pub skipped_count: usize,
    pub total_bytes: u64,
    pub tool_version: String,
}

/// 一次保存的结果摘要。
pub struct SaveOutcome {
    pub id: String,
    pub path: PathBuf,
    pub file_count: usize,
    pub skipped_count: usize,
    pub total_bytes: u64,
    pub pruned: usize,
}

/// 列出时的单条快照信息。
pub struct CheckpointInfo {
    pub id: String,
    pub path: PathBuf,
    pub manifest: Option<Manifest>,
}

/// 恢复的结果摘要。
pub struct RestoreOutcome {
    pub id: String,
    pub restored: usize,
    pub failed: usize,
}

/// checkpoint 根：`override_dir` 优先，否则用户级默认目录。
pub fn checkpoints_root(override_dir: Option<&Path>) -> Result<PathBuf> {
    if let Some(dir) = override_dir {
        return Ok(dir.to_path_buf());
    }
    Ok(user_data_root()?.join("Rayman").join("checkpoints"))
}

fn user_data_root() -> Result<PathBuf> {
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        return Ok(PathBuf::from(local));
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(xdg));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".local").join("share"));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(profile));
    }
    bail!(
        "无法确定用户数据目录（未设置 LOCALAPPDATA/XDG_DATA_HOME/HOME/USERPROFILE），请用 --dir 指定"
    );
}

/// 每个工作区一个子目录：可读名 + 规范路径哈希，避免不同工作区互相覆盖。
pub fn workspace_key(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let name = canonical
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    format!("{safe}-{}", &hash[..8])
}

fn workspace_dir(root: &Path, override_dir: Option<&Path>) -> Result<PathBuf> {
    Ok(checkpoints_root(override_dir)?.join(workspace_key(root)))
}

/// 纳入快照的文件：工作树（gitignore 感知、剪枝重目录）+ rayman 任务状态（`.RaymanCodingSkill`，排除易变的 `tmp/`）。
fn files_to_capture(root: &Path) -> Vec<PathBuf> {
    let mut files = walk::workspace_files(root);
    // walk 刻意跳过 .RaymanCodingSkill；把任务状态（目标/待办/上下文索引）补回来，但排除 tmp/。
    let state = root.join(".RaymanCodingSkill");
    if state.is_dir() {
        collect_state_files(&state, &mut files);
    }
    files.sort();
    files.dedup();
    files
}

fn collect_state_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // tmp/ 是运行期临时文件，不进快照。
            if entry.file_name() == "tmp" {
                continue;
            }
            collect_state_files(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// 保存当前工作树快照，然后按 `keep` 清理旧快照。
pub fn save(root: &Path, override_dir: Option<&Path>, keep: usize) -> Result<SaveOutcome> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let ckpt_root = checkpoints_root(override_dir)?;
    let ws_dir = ckpt_root.join(workspace_key(&root));
    fs::create_dir_all(&ws_dir)
        .with_context(|| format!("无法创建 checkpoint 目录: {}", display_path(&ws_dir)))?;

    let timestamp = state_store::now_iso();
    let id = fs_safe_id(&timestamp);
    let staging = ws_dir.join(format!("{STAGING_PREFIX}{}-{id}", std::process::id()));
    let tree = staging.join(TREE_SUBDIR);
    fs::create_dir_all(&tree)
        .with_context(|| format!("无法创建暂存目录: {}", display_path(&tree)))?;

    let files = files_to_capture(&root);
    let mut file_count = 0usize;
    let mut skipped = 0usize;
    let mut total_bytes = 0u64;
    for src in &files {
        // 绝不把 checkpoint 目录自己拷进去（用户若把 --dir 指到工作区内也安全）。
        if src.starts_with(&ckpt_root) {
            continue;
        }
        let Ok(rel) = src.strip_prefix(&root) else {
            continue;
        };
        let dest = tree.join(rel);
        if let Some(parent) = dest.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::copy(src, &dest) {
            Ok(bytes) => {
                file_count += 1;
                total_bytes += bytes;
            }
            // 锁定/无权限的文件跳过而非整次失败——部分快照也比没有强。
            Err(_) => skipped += 1,
        }
    }

    let manifest = Manifest {
        created_at: timestamp,
        workspace_root: display_path(&root),
        file_count,
        skipped_count: skipped,
        total_bytes,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    state_store::write_json(&staging.join(MANIFEST_NAME), &manifest)?;

    let final_dir = ws_dir.join(&id);
    if final_dir.exists() {
        let _ = fs::remove_dir_all(&final_dir);
    }
    fs::rename(&staging, &final_dir).with_context(|| {
        format!(
            "无法提交 checkpoint: {} -> {}",
            display_path(&staging),
            display_path(&final_dir)
        )
    })?;

    let pruned = prune(&ws_dir, keep)?;
    Ok(SaveOutcome {
        id,
        path: final_dir,
        file_count,
        skipped_count: skipped,
        total_bytes,
        pruned,
    })
}

/// RFC3339 时间戳转成文件系统安全的目录名（去掉 ':'）。字典序仍等于时间序。
fn fs_safe_id(timestamp: &str) -> String {
    timestamp.replace(':', "-")
}

fn prune(ws_dir: &Path, keep: usize) -> Result<usize> {
    let mut checkpoints: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(ws_dir)?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(STAGING_PREFIX) {
            // 清理上次中断残留的暂存目录。
            let _ = fs::remove_dir_all(&path);
            continue;
        }
        if name.starts_with('.') {
            continue;
        }
        checkpoints.push(path);
    }
    checkpoints.sort(); // 时间戳目录名字典序 = 时间序
    let mut pruned = 0;
    if checkpoints.len() > keep {
        for path in checkpoints.iter().take(checkpoints.len() - keep) {
            if fs::remove_dir_all(path).is_ok() {
                pruned += 1;
            }
        }
    }
    Ok(pruned)
}

/// 列出当前工作区的所有快照（按时间升序）。
pub fn list(root: &Path, override_dir: Option<&Path>) -> Result<Vec<CheckpointInfo>> {
    let ws_dir = workspace_dir(root, override_dir)?;
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&ws_dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let manifest = state_store::read_json::<Manifest>(&path.join(MANIFEST_NAME))
            .ok()
            .flatten();
        out.push(CheckpointInfo {
            id: name,
            path,
            manifest,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// 最近一次快照（无则 None）。
pub fn latest(root: &Path, override_dir: Option<&Path>) -> Result<Option<CheckpointInfo>> {
    Ok(list(root, override_dir)?.pop())
}

/// 把某个快照（默认最近）叠加恢复到工作区。覆盖同名文件，但不删除工作区里多出来的文件。
pub fn restore(
    root: &Path,
    override_dir: Option<&Path>,
    which: Option<&str>,
) -> Result<RestoreOutcome> {
    let checkpoints = list(root, override_dir)?;
    if checkpoints.is_empty() {
        bail!("没有可恢复的 checkpoint");
    }
    let target = match which {
        None | Some("latest") => checkpoints.last().unwrap(),
        Some(id) => checkpoints
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("找不到 checkpoint: {id}"))?,
    };
    let tree = target.path.join(TREE_SUBDIR);
    if !tree.is_dir() {
        bail!("checkpoint 缺少 tree/ 目录: {}", display_path(&target.path));
    }
    let dest_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut restored = 0;
    let mut failed = 0;
    restore_tree(&tree, &tree, &dest_root, &mut restored, &mut failed);
    Ok(RestoreOutcome {
        id: target.id.clone(),
        restored,
        failed,
    })
}

fn restore_tree(
    base: &Path,
    dir: &Path,
    dest_root: &Path,
    restored: &mut usize,
    failed: &mut usize,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            restore_tree(base, &path, dest_root, restored, failed);
        } else if path.is_file() {
            let Ok(rel) = path.strip_prefix(base) else {
                continue;
            };
            let dest = dest_root.join(rel);
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match fs::copy(&path, &dest) {
                Ok(_) => *restored += 1,
                Err(_) => *failed += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn save_captures_tree_and_state_but_not_tmp_then_restores() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        let dir = Some(store.path());

        write(&root.join("src/main.rs"), "fn main() {}");
        write(&root.join(".gitignore"), "ignored.txt\n");
        write(&root.join("ignored.txt"), "secret");
        write(&root.join("node_modules/pkg/index.js"), "x");
        write(
            &root.join(".RaymanCodingSkill/goals/g1.json"),
            "{\"id\":\"g1\"}",
        );
        write(
            &root.join(".RaymanCodingSkill/tmp/scratch.txt"),
            "transient",
        );

        let outcome = save(root, dir, DEFAULT_KEEP).unwrap();
        assert!(outcome.file_count >= 2, "应至少抓到 main.rs 与 goal 状态");

        let tree = outcome.path.join(TREE_SUBDIR);
        assert!(tree.join("src/main.rs").exists());
        assert!(tree.join(".RaymanCodingSkill/goals/g1.json").exists());
        // gitignore 命中、vendor 目录、易变 tmp/ 都不应进快照。
        assert!(!tree.join("ignored.txt").exists());
        assert!(!tree.join("node_modules/pkg/index.js").exists());
        assert!(!tree.join(".RaymanCodingSkill/tmp/scratch.txt").exists());

        // 删掉工作区文件后恢复。
        fs::remove_file(root.join("src/main.rs")).unwrap();
        let restored = restore(root, dir, None).unwrap();
        assert!(restored.restored >= 2);
        assert_eq!(
            fs::read_to_string(root.join("src/main.rs")).unwrap(),
            "fn main() {}"
        );
    }

    #[test]
    fn prune_keeps_only_newest() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        let dir = Some(store.path());
        write(&root.join("a.txt"), "a");

        // 直接造 5 个不同 id 的快照目录，验证 prune 只留最新 2 个。
        let ws_dir = workspace_dir(root, dir).unwrap();
        fs::create_dir_all(&ws_dir).unwrap();
        for stamp in [
            "2026-01-01",
            "2026-02-01",
            "2026-03-01",
            "2026-04-01",
            "2026-05-01",
        ] {
            fs::create_dir_all(ws_dir.join(stamp).join(TREE_SUBDIR)).unwrap();
        }
        let pruned = prune(&ws_dir, 2).unwrap();
        assert_eq!(pruned, 3);
        let remaining = list(root, dir).unwrap();
        let ids: Vec<_> = remaining.iter().map(|c| c.id.clone()).collect();
        assert_eq!(
            ids,
            vec!["2026-04-01".to_string(), "2026-05-01".to_string()]
        );
    }

    #[test]
    fn staging_dirs_are_ignored_by_list_and_cleaned_by_prune() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        let dir = Some(store.path());
        let ws_dir = workspace_dir(root, dir).unwrap();
        fs::create_dir_all(ws_dir.join(format!("{STAGING_PREFIX}123-abc"))).unwrap();
        fs::create_dir_all(ws_dir.join("2026-06-01").join(TREE_SUBDIR)).unwrap();

        assert_eq!(list(root, dir).unwrap().len(), 1, "暂存目录不算快照");
        prune(&ws_dir, DEFAULT_KEEP).unwrap();
        assert!(!ws_dir.join(format!("{STAGING_PREFIX}123-abc")).exists());
    }
}
