//! 任务与 fixture 加载：每个任务 = 一段给 agent 的提示 + 起始工作区 + 一条隐藏的评分命令。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub prompt: String,
    pub fixture_dir: PathBuf,
    /// 隐藏评分命令：在 agent 完成后的工作区里运行，退出 0 记为成功。agent 看不到它。
    pub grade_cmd: String,
}

/// 从 `tasks_root` 加载所有任务；`filter` 非空时只保留名字匹配的那个。
pub fn load_tasks(tasks_root: &Path, filter: Option<&str>) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();
    let entries = std::fs::read_dir(tasks_root)
        .with_context(|| format!("无法读取任务目录: {}", tasks_root.display()))?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
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
        let grade_cmd = read(&dir.join("grade.txt"))?.trim().to_string();
        let fixture_dir = dir.join("fixture");
        if !fixture_dir.is_dir() {
            bail!("任务 {name} 缺少 fixture/ 目录");
        }
        if grade_cmd.is_empty() {
            bail!("任务 {name} 的 grade.txt 为空");
        }
        tasks.push(Task {
            name,
            prompt: prompt.trim().to_string(),
            fixture_dir,
            grade_cmd,
        });
    }
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tasks)
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("无法读取: {}", path.display()))
}

/// 把任务的 fixture/ 递归复制到全新的工作区目录。
pub fn setup_workspace(task: &Task, dest: &Path) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .with_context(|| format!("无法清理旧工作区: {}", dest.display()))?;
    }
    copy_dir(&task.fixture_dir, dest)?;
    // 关键：给每个 fixture 一个 .RaymanCodingSkill/ 标记，让 rayman 把这个副本当作工作区根，
    // 否则它会沿目录向上找到真实仓库的 .git，把整个仓库当工作区（污染 + 超时）。
    std::fs::create_dir_all(dest.join(".RaymanCodingSkill"))
        .with_context(|| format!("无法创建工作区标记: {}", dest.display()))?;
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("无法创建目录: {}", dest.display()))?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let name = entry.file_name();
        // 构建产物/元数据不属于任务起点，复制会拖慢评测并污染工作区。
        if from.is_dir()
            && matches!(
                name.to_string_lossy().as_ref(),
                "target" | ".git" | "node_modules" | ".RaymanCodingSkill"
            )
        {
            continue;
        }
        let to = dest.join(name);
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("无法复制 {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
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
    fn setup_workspace_copies_fixture_and_marks_rayman_root() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = dir.path().join("fixture");
        write(&fixture.join("src/lib.rs"), "pub fn ok() {}\n");
        let task = Task {
            name: "sample".into(),
            prompt: "fix it".into(),
            fixture_dir: fixture,
            grade_cmd: "cargo test".into(),
        };
        let dest = dir.path().join("workspace");

        setup_workspace(&task, &dest).unwrap();

        assert!(dest.join("src/lib.rs").exists());
        assert!(dest.join(".RaymanCodingSkill").is_dir());
    }
}
