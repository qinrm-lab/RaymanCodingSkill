//! 唯一的工作区遍历入口：gitignore 感知、剪枝式（不下钻被忽略目录）、单一忽略清单。
//! 取代旧代码里 7 份分歧的硬编码忽略列表以及 filter-after-yield 导致的 node_modules 下钻。
//!
//! 所有调用方都必须使用 [`workspace_files_checked`] 或 [`workspace_walk`]：遍历
//! 错误是安全边界，不能被当作"扫描完整"吞掉。

use std::path::{Path, PathBuf};

use crate::file_io::is_link_or_reparse;
use anyhow::{Result, bail};
use ignore::WalkBuilder;

/// 运行时状态目录：任何工作区里都不参与索引。
const STATE_IGNORE: &[&str] = &[".git", ".RaymanCodingSkill"];

/// 仅在 .gitignore 不可用时兜底跳过的重目录。
///
/// 这些名字**不能**在 git 工作区里无条件剪枝：能走到 filter_entry 的目录说明它
/// 没有被 .gitignore 忽略，也就是被跟踪的第一方内容。真正的产物目录本就会被
/// gitignore 拦在前面，所以按名字剪枝只在它错的时候才起作用——被跟踪的
/// `build/deploy.ps1` 会同时从 context 索引、goal 基线差量门禁和 checkpoint 快照
/// 里消失，正是本模块声称要防的"谎称遍历完整"。
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
    // git 工作区里 .gitignore 是唯一权威的忽略来源，兜底清单只在没有 git 时生效。
    let vendor_fallback = std::fs::symlink_metadata(root.join(".git")).is_err();
    builder.filter_entry(move |entry| {
        // 只剪枝目录：名为 build/dist 的普通源码文件不应从索引里无声消失。
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        if STATE_IGNORE.contains(&name.as_ref()) {
            return false;
        }
        !(vendor_fallback && VENDOR_FALLBACK_IGNORE.contains(&name.as_ref()))
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

    #[test]
    fn walk_indexes_git_tracked_files_inside_vendor_named_directories() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 有 .git 时 .gitignore 是权威来源；能走到这里的 build/ 就是被跟踪的
        // 第一方内容，按名字剪枝会让它对索引、goal 差量门禁和 checkpoint 隐形。
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/deploy.ps1"), "Write-Host x").unwrap();
        fs::create_dir_all(root.join("dist")).unwrap();
        fs::write(root.join("dist/bundle.js"), "x").unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/artifact.bin"), "x").unwrap();

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(
            keys.contains(&"build/deploy.ps1".to_string()),
            "keys={keys:?}"
        );
        assert!(
            keys.contains(&"dist/bundle.js".to_string()),
            "keys={keys:?}"
        );
        // gitignore 仍然权威：真正的产物目录照常被排除。
        assert!(
            !keys.iter().any(|key| key.starts_with("target")),
            "keys={keys:?}"
        );
        assert!(
            !keys.iter().any(|key| key.starts_with(".git/")),
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
