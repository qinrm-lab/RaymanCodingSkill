//! 任务与 fixture 加载：每个任务 = 一段给 agent 的提示 + 起始工作区 + 一条隐藏的评分命令。
//!
//! Fixture 是评测输入的一部分，不能把其中的 symlink 当作普通文件递归跟随；否则一个
//! 看似局部的任务可以在复制时把宿主机任意目录带进 trial。这里一律拒绝 symlink。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::grade;

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub prompt: String,
    prompt_path: PathBuf,
    pub fixture_dir: PathBuf,
    /// 隐藏评分命令：在 agent 完成后的工作区里运行，退出 0 记为成功。agent 看不到它。
    pub grade_cmd: String,
    grade_path: PathBuf,
}

/// 写入 run manifest 的任务输入身份。`task_sha256` 绑定任务名、提示、评分命令和 fixture tree。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskProvenance {
    pub name: String,
    pub prompt_sha256: String,
    pub grade_sha256: String,
    pub fixture_sha256: String,
    pub task_sha256: String,
}

impl Task {
    pub fn provenance(&self) -> Result<TaskProvenance> {
        // Re-read all task source inputs for every integrity check. The task keeps the
        // startup strings used by the agent/grade, but a later host-shell mutation of
        // prompt.md or grade.txt must still invalidate the run rather than leave the
        // original hashes looking current.
        let prompt_sha256 = sha256_bytes(read(&self.prompt_path)?.trim().as_bytes());
        let grade_sha256 = sha256_bytes(read(&self.grade_path)?.trim().as_bytes());
        let fixture_sha256 = hash_tree(&self.fixture_dir)?;
        let task_sha256 = sha256_parts(&[
            self.name.as_bytes(),
            prompt_sha256.as_bytes(),
            grade_sha256.as_bytes(),
            fixture_sha256.as_bytes(),
        ]);
        Ok(TaskProvenance {
            name: self.name.clone(),
            prompt_sha256,
            grade_sha256,
            fixture_sha256,
            task_sha256,
        })
    }
}

/// 从 `tasks_root` 加载所有任务；`filter` 非空时只保留名字匹配的那个。
pub fn load_tasks(tasks_root: &Path, filter: Option<&str>) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();
    let entries = fs::read_dir(tasks_root)
        .with_context(|| format!("无法读取任务目录: {}", tasks_root.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("无法枚举任务目录: {}", tasks_root.display()))?;
        let dir = entry.path();
        let metadata = fs::symlink_metadata(&dir)
            .with_context(|| format!("无法检查任务目录: {}", dir.display()))?;
        if is_link_or_reparse(&metadata) {
            bail!("拒绝任务目录 symlink: {}", dir.display());
        }
        if !metadata.is_dir() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if let Some(filter) = filter
            && filter != name
        {
            continue;
        }
        // 杂散目录（IDE 缓存、临时文件夹等）没有 prompt.md：跳过并提示，别让整轮评测挂掉。
        let prompt_path = dir.join("prompt.md");
        if !prompt_path.is_file() {
            eprintln!("⚠ 跳过缺 prompt.md 的目录（非任务）: {}", dir.display());
            continue;
        }
        let prompt = read(&prompt_path)?;
        let grade_path = dir.join("grade.txt");
        let grade_cmd = read(&grade_path)?.trim().to_string();
        let fixture_dir = dir.join("fixture");
        ensure_real_dir(&fixture_dir, "任务 fixture")?;
        // On Windows `cmd` resolves a bare command from its CWD before PATH. A top-level
        // rayman wrapper would leak availability into control before a trial begins.
        if let Err(error) = grade::ensure_no_top_level_rayman_command(&fixture_dir) {
            bail!("任务 {name} 的 fixture 顶层包含不允许的 rayman 命令/包装器: {error}");
        }
        if grade_cmd.is_empty() {
            bail!("任务 {name} 的 grade.txt 为空");
        }
        // 在创建任何 trial 之前就遍历一次，确保 provenance 和复制看到的是同一类安全输入。
        let _ = hash_tree(&fixture_dir)?;
        tasks.push(Task {
            name,
            prompt: prompt.trim().to_string(),
            prompt_path,
            fixture_dir,
            grade_cmd,
            grade_path,
        });
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tasks)
}

fn read(path: &Path) -> Result<String> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("无法检查: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!("拒绝非普通任务文件: {}", path.display());
    }
    fs::read_to_string(path).with_context(|| format!("无法读取: {}", path.display()))
}

fn ensure_real_dir(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 {label}: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        bail!("{label} 必须是非 symlink 目录: {}", path.display());
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// 把任务的 fixture/ 递归复制到一个全新的工作区。既有目标是错误，而不是可删除的旧 run。
pub fn setup_workspace(task: &Task, dest: &Path) -> Result<()> {
    if fs::symlink_metadata(dest).is_ok() {
        bail!("拒绝覆写既有 trial 工作区: {}", dest.display());
    }
    copy_dir(&task.fixture_dir, dest)?;
    // 给每个 fixture 一个 .RaymanCodingSkill/ 标记，让 rayman 把这个副本当作工作区根，
    // 否则它会沿目录向上找到真实仓库的 .git，把整个仓库当工作区（污染 + 超时）。
    fs::create_dir(dest.join(".RaymanCodingSkill"))
        .with_context(|| format!("无法创建工作区标记: {}", dest.display()))?;
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    ensure_real_dir(src, "fixture 目录")?;
    match fs::create_dir(dest) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            bail!("拒绝覆写既有目录: {}", dest.display());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法创建目录: {}", dest.display()));
        }
    }
    let entries = fs::read_dir(src).with_context(|| format!("无法读取目录: {}", src.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("无法枚举目录: {}", src.display()))?;
        let from = entry.path();
        let name = entry.file_name();
        let metadata = fs::symlink_metadata(&from)
            .with_context(|| format!("无法检查 fixture 输入: {}", from.display()))?;
        if is_link_or_reparse(&metadata) {
            bail!("拒绝 fixture 内 symlink: {}", from.display());
        }
        // 构建产物/元数据不属于任务起点，复制会拖慢评测并污染工作区。
        if metadata.is_dir()
            && matches!(
                name.to_string_lossy().as_ref(),
                "target" | ".git" | "node_modules" | ".RaymanCodingSkill"
            )
        {
            continue;
        }
        let to = dest.join(name);
        if metadata.is_dir() {
            copy_dir(&from, &to)?;
        } else if metadata.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("无法复制 {} -> {}", from.display(), to.display()))?;
        } else {
            bail!("拒绝 fixture 内非普通文件: {}", from.display());
        }
    }
    Ok(())
}

fn hash_tree(root: &Path) -> Result<String> {
    ensure_real_dir(root, "fixture")?;
    let mut hasher = Sha256::new();
    hash_tree_into(root, root, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

fn hash_tree_into(root: &Path, path: &Path, hasher: &mut Sha256) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 fixture 输入: {}", path.display()))?;
    if is_link_or_reparse(&metadata) {
        bail!("拒绝 fixture 内 symlink: {}", path.display());
    }
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if metadata.is_dir() {
        hasher.update(b"dir\0");
        hasher.update(relative.as_bytes());
        hasher.update(b"\0");
        let mut entries: Vec<_> = fs::read_dir(path)
            .with_context(|| format!("无法读取 fixture 目录: {}", path.display()))?
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let child = entry.path();
            let child_metadata = fs::symlink_metadata(&child)
                .with_context(|| format!("无法检查 fixture 输入: {}", child.display()))?;
            if child_metadata.is_dir()
                && matches!(
                    entry.file_name().to_string_lossy().as_ref(),
                    "target" | ".git" | "node_modules" | ".RaymanCodingSkill"
                )
            {
                continue;
            }
            hash_tree_into(root, &child, hasher)?;
        }
    } else if metadata.is_file() {
        hasher.update(b"file\0");
        hasher.update(relative.as_bytes());
        hasher.update(b"\0");
        let bytes =
            fs::read(path).with_context(|| format!("无法读取 fixture 文件: {}", path.display()))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    } else {
        bail!("拒绝 fixture 内非普通文件: {}", path.display());
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn sha256_parts(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

// `hex` 的格式化逻辑很小，但不值得为了它再引入一条依赖树。
mod hex {
    use std::fmt::Write;

    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let mut out = String::with_capacity(bytes.as_ref().len() * 2);
        for byte in bytes.as_ref() {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn load_tasks_filters_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path();
        for name in ["zeta", "alpha"] {
            let task_dir = tasks.join(name);
            write(&task_dir.join("prompt.md"), &format!("fix {name}\n"));
            write(&task_dir.join("grade.txt"), "cargo test\n");
            write(&task_dir.join("fixture/src/lib.rs"), "pub fn ok() {}\n");
        }

        let all = load_tasks(tasks, None).unwrap();
        assert_eq!(
            all.iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );

        let filtered = load_tasks(tasks, Some("zeta")).unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "zeta");
        assert_eq!(filtered[0].prompt, "fix zeta");
    }

    #[test]
    fn load_tasks_skips_stray_dirs_without_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path();
        let task_dir = tasks.join("real");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "cargo test\n");
        write(&task_dir.join("fixture/src/lib.rs"), "pub fn ok() {}\n");
        // 杂散目录：没有 prompt.md，应被跳过而非报错。
        write(&tasks.join("stray/junk.txt"), "not a task\n");

        let all = load_tasks(tasks, None).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "real");
    }

    #[test]
    fn copy_dir_skips_build_and_vcs_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("fixture");
        write(&src.join("src/lib.rs"), "pub fn ok() {}\n");
        for skipped in ["target", ".git", "node_modules", ".RaymanCodingSkill"] {
            write(&src.join(skipped).join("junk"), "x");
        }
        let dest = dir.path().join("dest");

        copy_dir(&src, &dest).unwrap();

        assert!(dest.join("src/lib.rs").exists());
        for skipped in ["target", ".git", "node_modules", ".RaymanCodingSkill"] {
            assert!(!dest.join(skipped).exists(), "{skipped} 不应被复制");
        }
    }

    #[test]
    fn setup_workspace_copies_fixture_and_refuses_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn ok() {}\n");
        let task = Task {
            name: "sample".into(),
            prompt: "fix it".into(),
            prompt_path: dir.path().join("prompt.md"),
            fixture_dir: fixture,
            grade_cmd: "cargo test".into(),
            grade_path: dir.path().join("grade.txt"),
        };
        let dest = dir.path().join("workspace");

        setup_workspace(&task, &dest).unwrap();

        assert!(dest.join("src/lib.rs").exists());
        assert!(dest.join(".RaymanCodingSkill").is_dir());
        assert!(setup_workspace(&task, &dest).is_err());
    }

    #[test]
    fn provenance_hash_binds_fixture_content() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn ok() {}\n");
        let prompt_path = dir.path().join("prompt.md");
        let grade_path = dir.path().join("grade.txt");
        write(&prompt_path, "fix it\n");
        write(&grade_path, "cargo test\n");
        let task = Task {
            name: "sample".into(),
            prompt: "fix it".into(),
            prompt_path: prompt_path.clone(),
            fixture_dir: fixture.clone(),
            grade_cmd: "cargo test".into(),
            grade_path: grade_path.clone(),
        };
        let first = task.provenance().unwrap();
        write(&fixture.join("src/lib.rs"), "pub fn changed() {}\n");
        let second = task.provenance().unwrap();
        assert_ne!(first.fixture_sha256, second.fixture_sha256);
        assert_ne!(first.task_sha256, second.task_sha256);

        write(&prompt_path, "fix a different issue\n");
        let third = task.provenance().unwrap();
        assert_ne!(second.prompt_sha256, third.prompt_sha256);
        assert_ne!(second.task_sha256, third.task_sha256);

        write(&grade_path, "cargo test --all\n");
        let fourth = task.provenance().unwrap();
        assert_ne!(third.grade_sha256, fourth.grade_sha256);
        assert_ne!(third.task_sha256, fourth.task_sha256);
    }

    #[cfg(windows)]
    #[test]
    fn load_tasks_rejects_top_level_custom_rayman_wrapper_in_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let task_dir = dir.path().join("task");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "cargo test\n");
        write(
            &task_dir.join("fixture/RAYMAN.custom.extension"),
            "@echo off",
        );

        let error = load_tasks(dir.path(), None).unwrap_err().to_string();
        assert!(error.contains("rayman"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn fixture_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("fixture");
        write(&src.join("normal.txt"), "ok");
        let outside = dir.path().join("outside.txt");
        write(&outside, "secret");
        symlink(&outside, src.join("escape.txt")).unwrap();

        assert!(copy_dir(&src, &dir.path().join("dest")).is_err());
        assert!(hash_tree(&src).is_err());
    }
}
