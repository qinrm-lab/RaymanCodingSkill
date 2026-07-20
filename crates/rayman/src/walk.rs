//! 唯一的工作区遍历入口：gitignore 感知、剪枝式（不下钻被忽略目录）、单一忽略清单。
//! 取代旧代码里 7 份分歧的硬编码忽略列表以及 filter-after-yield 导致的 node_modules 下钻。
//!
//! 所有调用方都必须使用 [`workspace_files_checked`] 或 [`workspace_walk`]：遍历
//! 错误是安全边界，不能被当作"扫描完整"吞掉。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::file_io::is_link_or_reparse;
use anyhow::{Result, bail};
use ignore::WalkBuilder;

/// 运行时状态目录：任何工作区里都不参与索引。
const STATE_IGNORE: &[&str] = &[".git", ".RaymanCodingSkill"];

/// 按名字兜底跳过的重目录——**仅当其中没有任何被 git 跟踪的内容时**。
///
/// 两个方向都会出错，判据必须是"是否含被跟踪内容"，不能是别的：
/// 无条件剪枝会让被跟踪的 `build/deploy.ps1` 同时从 context 索引、goal 基线差量
/// 门禁和 checkpoint 快照里消失，即本模块声称要防的"谎称遍历完整"；而反过来只看
/// `.git` 是否存在就整体放行，会把未被 .gitignore 覆盖的 target/node_modules
/// 拖进 baseline，使产物每次构建都产生无法声明的 unplanned 差量，goal 门禁
/// 变为不可满足。"没被 .gitignore 匹配"并不等于"被跟踪"——untracked 且
/// unignored 是 git 的常规状态。
const VENDOR_FALLBACK_IGNORE: &[&str] = &[
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];

/// `.RaymanCodingSkill` is runtime state and remains pruned, except for the one
/// repository policy file whose bytes must participate in context freshness and
/// workspace fingerprints.
const INDEXED_STATE_POLICY: &str = ".RaymanCodingSkill/quality.json";

/// 遍历期间的一个错误。错误不可被安全敏感调用方静默忽略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkIssue {
    pub error: String,
}

/// 工作区遍历的完整结果；保留已发现文件，同时显式携带不可访问目录等错误。
#[derive(Debug, Clone, Default)]
pub struct WorkspaceWalk {
    pub files: Vec<PathBuf>,
    pub errors: Vec<WalkIssue>,
}

impl WorkspaceWalk {
    /// 仅当遍历完整时取回文件。checkpoint 等会形成恢复证据的调用方必须使用它。
    pub fn into_files(self) -> Result<Vec<PathBuf>> {
        if self.errors.is_empty() {
            return Ok(self.files);
        }
        let detail = self
            .errors
            .iter()
            .take(3)
            .map(|issue| issue.error.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        bail!(
            "工作区遍历不完整（{} 个错误），拒绝把不完整结果当作完整文件集: {}",
            self.errors.len(),
            detail
        );
    }
}

/// 遍历工作区并显式报告错误。尊重 .gitignore/.ignore，剪枝式跳过
/// [`STATE_IGNORE`]（以及非 git 工作区里的 [`VENDOR_FALLBACK_IGNORE`]），
/// 不跟随符号链接。
pub fn workspace_walk(root: &Path) -> WorkspaceWalk {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false) // 不因“隐藏”自动跳过；由下面的 filter/ignore 精确控制
        .follow_links(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false) // 即使工作区还不是 git 仓库，也尊重 .gitignore
        .parents(false);
    let tracked = tracked_directories(root);
    let owned_root = root.to_path_buf();
    builder.filter_entry(move |entry| {
        // 只剪枝目录：名为 build/dist 的普通源码文件不应从索引里无声消失。
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            return true;
        }
        // 工作区根本身永远保留，哪怕它恰好叫 build。
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        if STATE_IGNORE.contains(&name.as_ref()) {
            return false;
        }
        if !VENDOR_FALLBACK_IGNORE.contains(&name.as_ref()) {
            return true;
        }
        match &tracked {
            // git 可用：只保留确实含被跟踪内容的 vendor 名目录。
            Some(tracked) => entry
                .path()
                .strip_prefix(&owned_root)
                .ok()
                .is_some_and(|rel| tracked.contains(&rel.to_string_lossy().replace('\\', "/"))),
            // 非 git 工作区或 git 不可用：无从判断跟踪状态，按名字兜底剪枝。
            None => false,
        }
    });

    let mut report = WorkspaceWalk::default();
    for result in builder.build() {
        match result {
            Ok(entry) => match entry.file_type() {
                Some(file_type) if file_type.is_dir() => {}
                Some(file_type) if file_type.is_file() => {
                    report.files.push(entry.into_path());
                }
                // `follow_links(false)` means a symlink (or other special
                // entry) is neither a dir nor a file here. Silently skipping
                // it would make `into_files()` claim a complete traversal
                // while quietly missing content — exactly the false
                // completeness this type exists to prevent.
                Some(file_type) => {
                    let kind = if file_type.is_symlink() {
                        "符号链接"
                    } else {
                        "非常规文件"
                    };
                    report.errors.push(WalkIssue {
                        error: format!("{kind}不会被跟随，遍历不完整: {}", entry.path().display()),
                    });
                }
                None => report.errors.push(WalkIssue {
                    error: format!("无法确定文件类型: {}", entry.path().display()),
                }),
            },
            Err(error) => report.errors.push(WalkIssue {
                error: error.to_string(),
            }),
        }
    }
    append_indexed_state_policy(root, &mut report);
    report.files.sort();
    report.files.dedup();
    report
}

/// 含被 git 跟踪内容的目录集合（工作区相对、正斜杠分隔）。
///
/// 返回 `None` 表示无从判断——不是 git 仓库、git 不可用、或命令失败。调用方在
/// 那种情况下必须回退到按名字剪枝，而不是当作"没有跟踪内容"，否则一次 git 故障
/// 就会静默改变索引范围。
fn tracked_directories(root: &Path) -> Option<BTreeSet<String>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut directories = BTreeSet::new();
    for entry in output.stdout.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(entry);
        let mut components: Vec<&str> = path.split('/').collect();
        components.pop(); // 去掉文件名，只累积祖先目录
        let mut prefix = String::new();
        for component in components {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            directories.insert(prefix.clone());
        }
    }
    Some(directories)
}

fn append_indexed_state_policy(root: &Path, report: &mut WorkspaceWalk) {
    let state_dir = root.join(".RaymanCodingSkill");
    let policy_path = root.join(INDEXED_STATE_POLICY);
    let state_metadata = match std::fs::symlink_metadata(&state_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            report.errors.push(WalkIssue {
                error: format!("无法检查共享 policy 目录 {}: {error}", state_dir.display()),
            });
            return;
        }
    };
    if !state_metadata.file_type().is_dir() || is_link_or_reparse(&state_metadata) {
        report.errors.push(WalkIssue {
            error: format!(
                "共享 quality policy 的父目录必须是工作区内真实目录，不能是链接/reparse: {}",
                state_dir.display()
            ),
        });
        return;
    }

    let metadata = match std::fs::symlink_metadata(&policy_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            report.errors.push(WalkIssue {
                error: format!(
                    "无法检查共享 quality policy {}: {error}",
                    policy_path.display()
                ),
            });
            return;
        }
    };
    if !metadata.file_type().is_file() || is_link_or_reparse(&metadata) {
        report.errors.push(WalkIssue {
            error: format!(
                "共享 quality policy 必须是工作区内普通文件，不能是链接/reparse/非常规文件: {}",
                policy_path.display()
            ),
        });
        return;
    }

    let containment = root.canonicalize().and_then(|canonical_root| {
        let canonical_state = state_dir.canonicalize()?;
        let canonical_policy = policy_path.canonicalize()?;
        Ok((canonical_root, canonical_state, canonical_policy))
    });
    match containment {
        Ok((canonical_root, canonical_state, canonical_policy))
            if canonical_state.starts_with(&canonical_root)
                && canonical_policy.parent() == Some(canonical_state.as_path()) =>
        {
            report.files.push(policy_path);
        }
        Ok((_, _, canonical_policy)) => report.errors.push(WalkIssue {
            error: format!(
                "共享 quality policy 逃逸工作区或不在精确 policy 目录: {} -> {}",
                policy_path.display(),
                canonical_policy.display()
            ),
        }),
        Err(error) => report.errors.push(WalkIssue {
            error: format!(
                "无法验证共享 quality policy containment {}: {error}",
                policy_path.display()
            ),
        }),
    }
}

/// 遍历工作区，遇到任何遍历错误即返回错误。
pub fn workspace_files_checked(root: &Path) -> Result<Vec<PathBuf>> {
    workspace_walk(root).into_files()
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
        fs::write(root.join(".RaymanCodingSkill/quality.json"), "{}").unwrap();
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "secret").unwrap();
        fs::write(root.join("README.md"), "# x").unwrap();

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(keys.contains(&"src/main.rs".to_string()));
        assert!(keys.contains(&"README.md".to_string()));
        assert!(!keys.iter().any(|key| key.starts_with("node_modules")));
        assert!(keys.contains(&".RaymanCodingSkill/quality.json".to_string()));
        assert!(!keys.contains(&".RaymanCodingSkill/state.json".to_string()));
        assert!(!keys.contains(&"ignored.txt".to_string()));
    }

    fn git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .status()
            .expect("git must be available");
        assert!(status.success(), "git {args:?} failed");
    }

    /// vendor 名目录的去留取决于**是否含被跟踪内容**，两个方向都必须成立：
    /// 被跟踪的 `build/deploy.ps1` 不能从索引里消失；未跟踪的 `node_modules/`
    /// 也不能被拖进索引，否则产物每次构建都会产生无法声明的 goal 差量。
    #[test]
    fn vendor_pruning_follows_git_tracking_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/deploy.ps1"), "Write-Host x").unwrap();
        fs::write(root.join("README.md"), "# x").unwrap();
        // 未跟踪且未被 .gitignore 覆盖——这正是回归发生的形态。
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();

        git(root, &["init", "--quiet"]);
        git(root, &["add", "build/deploy.ps1", "README.md"]);

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(
            keys.contains(&"build/deploy.ps1".to_string()),
            "tracked build/ content must be indexed: {keys:?}"
        );
        assert!(
            !keys.iter().any(|key| key.starts_with("node_modules")),
            "untracked artifact dir must stay pruned: {keys:?}"
        );
        assert!(
            !keys.iter().any(|key| key.starts_with(".git/")),
            "keys={keys:?}"
        );
    }

    /// 非 git 工作区无从判断跟踪状态，回退到按名字剪枝。
    #[test]
    fn vendor_pruning_falls_back_to_names_without_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("README.md"), "# x").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(keys.contains(&"README.md".to_string()), "keys={keys:?}");
        assert!(
            !keys.iter().any(|key| key.starts_with("node_modules")),
            "keys={keys:?}"
        );
    }

    #[test]
    fn checked_walk_refuses_a_missing_root_instead_of_hiding_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let report = workspace_walk(&missing);
        assert!(!report.errors.is_empty(), "missing root must be reported");
        assert!(workspace_files_checked(&missing).is_err());
    }

    #[test]
    fn walk_refuses_a_non_directory_quality_policy_parent() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".RaymanCodingSkill"), "not a directory").unwrap();

        let report = workspace_walk(root);
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.error.contains("quality policy")
                    && issue.error.contains("父目录")),
            "issues={:?}",
            report.errors
        );
        assert!(workspace_files_checked(root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn walk_reports_a_symlinked_file_as_an_issue_instead_of_silently_dropping_it() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("real.txt"), "content").unwrap();
        symlink(root.join("real.txt"), root.join("linked.txt")).unwrap();

        let report = workspace_walk(root);
        assert!(
            report
                .errors
                .iter()
                .any(|issue| issue.error.contains("linked.txt")),
            "expected a WalkIssue for the symlink, got {:?}",
            report.errors
        );
        // The checked variant must fail closed rather than report a complete
        // traversal that silently omitted the symlinked file.
        assert!(workspace_files_checked(root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn walk_refuses_quality_policy_through_a_symlinked_state_parent() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("quality.json"), "{}").unwrap();
        symlink(outside.path(), workspace.path().join(".RaymanCodingSkill")).unwrap();

        let report = workspace_walk(workspace.path());
        assert!(
            report.errors.iter().any(|issue| {
                issue.error.contains("quality policy") && issue.error.contains("父目录")
            }),
            "issues={:?}",
            report.errors
        );
        assert!(
            !report
                .files
                .iter()
                .any(|path| relative_key(workspace.path(), path) == INDEXED_STATE_POLICY)
        );
        assert!(workspace_files_checked(workspace.path()).is_err());
    }
}
