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
        let prompt = read(&dir.join("prompt.md"))?;
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
    copy_dir(&task.fixture_dir, dest)
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).with_context(|| format!("无法创建目录: {}", dest.display()))?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .with_context(|| format!("无法复制 {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}
