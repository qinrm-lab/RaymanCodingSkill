//! 唯一的工作区遍历入口：gitignore 感知、剪枝式（不下钻被忽略目录）、单一忽略清单。
//! 取代旧代码里 7 份分歧的硬编码忽略列表以及 filter-after-yield 导致的 node_modules 下钻。

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// 状态目录与总是应跳过的重目录（在 .gitignore 之外的兜底）。
const ALWAYS_IGNORE: &[&str] = &[
    ".git",
    ".RaymanCodingSkill",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];

/// 遍历工作区，返回文件（不含目录）的绝对路径。
/// 尊重 .gitignore/.ignore，剪枝式跳过 [`ALWAYS_IGNORE`]，不跟随符号链接。
pub fn workspace_files(root: &Path) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // 不因“隐藏”自动跳过；由下面的 filter/ignore 精确控制
        .follow_links(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false) // 即使工作区还不是 git 仓库，也尊重 .gitignore
        .parents(false);
    builder.filter_entry(|entry| {
        // 只剪枝目录：名为 build/dist 的普通源码文件不应从索引里无声消失。
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        !ALWAYS_IGNORE.contains(&name.as_ref())
    });

    let mut files = Vec::new();
    for result in builder.build() {
        let Ok(entry) = result else { continue };
        if entry
            .file_type()
            .map(|file_type| file_type.is_file())
            .unwrap_or(false)
        {
            files.push(entry.into_path());
        }
    }
    files.sort();
    files
}

/// 工作区相对路径（正斜杠分隔），用于稳定的、跨平台一致的索引键。
pub fn relative_key(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn walk_skips_state_and_vendor_dirs_and_respects_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();
        fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
        fs::write(root.join(".RaymanCodingSkill/state.json"), "{}").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "secret").unwrap();
        fs::write(root.join("README.md"), "# x").unwrap();

        let keys: Vec<String> = workspace_files(root)
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(keys.contains(&"src/main.rs".to_string()));
        assert!(keys.contains(&"README.md".to_string()));
        assert!(!keys.iter().any(|key| key.starts_with("node_modules")));
        assert!(!keys.iter().any(|key| key.starts_with(".RaymanCodingSkill")));
        assert!(!keys.contains(&"ignored.txt".to_string()));
    }
}
