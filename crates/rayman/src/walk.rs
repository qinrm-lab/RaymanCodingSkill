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
        // core.excludesFile（全局忽略）是 git 自己的忽略来源之一。不认它，会让
        // `git status` 干净、`git check-ignore` 命中的文件（`*.log`、`.idea/`
        // 这类主流全局配置）进入索引、goal baseline 与 checkpoint 快照——这些
        // 文件不断变化又永远无法被 receipt 声明，于是 `goal close --status
        // success` 变得不可满足。
        .git_global(true)
        .git_exclude(true)
        .require_git(false) // 即使工作区还不是 git 仓库，也尊重 .gitignore
        // **不要**打开 `parents`：git 只在同一个工作树内部向上找 .gitignore，
        // 到工作树顶层就停；而 `ignore` crate 会一路走到盘符根，把工作区之外
        // 的 .gitignore 也应用上（`require_git(false)` 还会关掉它自身的 .git
        // 边界短路）。那比 git 更严格——恰好是反方向的错误：工作区上层目录里
        // 任何一个 .gitignore（嵌套 checkout、本身是 dotfiles 仓库的项目目录）
        // 都会让 `git status` 明明列为 `??` 的未提交文件从快照里消失，而
        // manifest 仍写 status=complete、errors=[]，正是本模块声称要防的
        // "谎称遍历完整"。代价是：工作区若是某个 git 仓库的子目录，仓库根的
        // .gitignore 不会被认——`ignore` 没有"到工作树顶层为止"的选项，宁可
        // 少忽略也不能静默丢用户的东西。
        .parents(false);
    let tracked = tracked_paths(root);
    let owned_root = root.to_path_buf();
    let filter_root = owned_root.clone();
    let filter_tracked = tracked
        .as_ref()
        .map(|tracked| tracked.directories_cmp.clone());
    builder.filter_entry(move |entry| {
        // 工作区根本身永远保留，哪怕它恰好叫 build。
        if entry.depth() == 0 {
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        // STATE_IGNORE 是按名字剪枝，且与条目是目录还是文件无关：linked
        // worktree / submodule 的 `.git` 是一个 gitdir 指针**文件**，此前只剪
        // 目录，于是它进了索引与 goal fingerprint，checkpoint restore 还会用
        // 快照里的旧指针覆盖当前指针，直接打断 worktree 的 git 链接。
        if name_matches(STATE_IGNORE, &name) {
            return false;
        }
        // 其余只剪枝目录：名为 build/dist 的普通源码文件不应从索引里无声消失。
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            return true;
        }
        if !name_matches(VENDOR_FALLBACK_IGNORE, &name) {
            return true;
        }
        match &filter_tracked {
            // git 可用：只保留确实含被跟踪内容的 vendor 名目录。
            Some(directories) => entry
                .path()
                .strip_prefix(&filter_root)
                .ok()
                .is_some_and(|rel| {
                    directories
                        .contains(&tracked_cmp_key(&rel.to_string_lossy().replace('\\', "/")))
                }),
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
                    // 判据是 git 自己的判据，不是文件所在目录的名字。
                    //
                    // 能走到这里的文件都没有被任何 gitignore 来源忽略，也就是
                    // `git status` 会显示为已跟踪或 `??` 的内容。此前这里在
                    // vendor 名目录**之内**额外要求"必须被跟踪"，于是用户刚写好
                    // 还没 add 的 `build/new-script.ps1` 被静默丢弃：
                    // `checkpoint save` 会跳过它却照样报告成功——正是本模块声称
                    // 要防的"谎称遍历完整"，而且丢的是未提交的工作。
                    //
                    // 未跟踪的构建产物仍然应当排除，但那要靠 gitignore（git 也
                    // 会把它们列成 `??` 抱怨），而不是靠名字猜测。目录层的
                    // vendor 剪枝保持不变：不含任何被跟踪内容的 vendor 目录整棵
                    // 都不会进来。
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
    // .gitignore 命中的条目在 ignore walker 内部就被丢弃，上面的逐文件跟踪
    // rescue 根本看不到它们：被 `git add -f` 强制跟踪的忽略路径文件（模块顶部
    // 文档里的 build/deploy.ps1 正是这一类）会同时从索引、goal 差量门禁和
    // checkpoint 快照消失——恰是本模块声称要防的"谎称遍历完整"。这里按跟踪
    // 清单补回遍历没见到的文件。判据仍是"是否被跟踪"，与顶部文档一致。
    if let Some(tracked) = &tracked {
        let walked_cmp: BTreeSet<String> = report
            .files
            .iter()
            .filter_map(|path| path.strip_prefix(&owned_root).ok())
            .map(|rel| tracked_cmp_key(&rel.to_string_lossy().replace('\\', "/")))
            .collect();
        for relative in &tracked.files {
            // STATE_IGNORE 目录在遍历里是任意深度剪枝的（filter_entry 按目录名），
            // 补回的豁免必须同样按任意深度匹配，否则嵌套的 .RaymanCodingSkill
            // 运行时状态（被跟踪的子工作区状态）会被补回进索引。
            if walked_cmp.contains(&tracked_cmp_key(relative))
                || relative
                    .split('/')
                    .any(|component| name_matches(STATE_IGNORE, component))
            {
                continue;
            }
            // Windows 名字解析会剥掉每段结尾的 '.' 和 ' '：这样的索引条目 stat
            // 会命中剥离后的真实文件，把幻影路径塞进索引与快照（真实文件本身
            // 已由正常遍历收录）。这种名字在本盘面上不可能作为独立文件存在。
            if cfg!(windows)
                && relative
                    .split('/')
                    .any(|component| component.ends_with('.') || component.ends_with(' '))
            {
                continue;
            }
            let candidate = owned_root.join(relative);
            match std::fs::symlink_metadata(&candidate) {
                // A symlink only reaches the rescue when the walker never
                // yielded it — some ignore source excluded it. Raising a fatal
                // walk error made `.gitignore` stop being an escape for a
                // tracked symlink and locked every walk-based command out; the
                // walker's own silent exclusion of ignored content is what to
                // match. Test the link bit specifically, NOT `is_link_or_reparse`:
                // that predicate is also true for ordinary files carrying
                // FILE_ATTRIBUTE_REPARSE_POINT (OneDrive placeholders,
                // AppExecLink stubs), which the main walk loop indexes as plain
                // files — dropping them here would silently disagree with the
                // walker about the same file.
                Ok(metadata) if metadata.file_type().is_symlink() => {}
                Ok(metadata) if metadata.is_file() => report.files.push(candidate),
                // 目录（子模块 gitlink 条目）：内容由 --recurse-submodules 单独列出。
                Ok(_) => {}
                // 已从盘面删除但仍在索引：遍历语义是枚举盘面，跳过与主循环一致。
                // 同类：Windows 非法文件名（<>|" 等，ERROR_INVALID_NAME=123）在
                // 本盘面上不可能存在，不算"遍历不完整"，否则一条坏索引条目会把
                // context/checkpoint/goal 的所有遍历永久卡死。
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) if cfg!(windows) && error.raw_os_error() == Some(123) => {}
                Err(error) => report.errors.push(WalkIssue {
                    error: format!("无法检查被跟踪文件 {}: {error}", candidate.display()),
                }),
            }
        }
    }
    append_indexed_state_policy(root, &mut report);
    report.files.sort();
    report.files.dedup();
    report
}

/// 目录名与兜底清单的匹配。Windows 文件系统大小写不敏感，`Build` 与 `build`
/// 是同一个目录，按字节精确比较会让判定随盘面大小写漂移；非 Windows 保持精确。
fn name_matches(list: &[&str], name: &str) -> bool {
    if cfg!(windows) {
        list.iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    } else {
        list.contains(&name)
    }
}

/// 跟踪路径的比较键。Windows 下文件系统与 git（core.ignorecase=true）都大小写
/// 不敏感，盘面与索引的大小写漂移（重命名工具、解压、Explorer）不得让被跟踪
/// 文件被误判为未跟踪而静默消失；非 Windows 保持字节精确。
fn tracked_cmp_key(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

/// git 跟踪的路径集合与其祖先目录集合。
///
/// 返回 `None` 表示无从判断——不是 git 仓库、git 不可用、或命令失败。调用方在
/// 那种情况下必须回退到按名字剪枝，而不是当作"没有跟踪内容"，否则一次 git 故障
/// 就会静默改变索引范围。
///
/// 优先用 `--recurse-submodules`：子模块在普通 `ls-files` 里只是一条不带尾斜杠的
/// 裸目录记录，其内容一条都不出现，于是挂在 vendor 名路径下的子模块会被整棵剪掉。
struct TrackedPaths {
    /// 原样大小写，用于补回缺失文件时拼接真实路径。
    files: BTreeSet<String>,
    /// 比较键集合（见 [`tracked_cmp_key`]）。
    directories_cmp: BTreeSet<String>,
}

fn tracked_paths(root: &Path) -> Option<TrackedPaths> {
    let listing = git_ls_files(root, &["ls-files", "-z", "--recurse-submodules"])
        .or_else(|| git_ls_files(root, &["ls-files", "-z"]))?;

    let mut files = BTreeSet::new();
    let mut directories_cmp = BTreeSet::new();
    for entry in listing.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let path = String::from_utf8_lossy(entry).into_owned();
        // gitlink 条目本身就是一个被跟踪的目录，所以整条路径也要进目录集合。
        directories_cmp.insert(tracked_cmp_key(&path));
        let mut components: Vec<&str> = path.split('/').collect();
        components.pop();
        let mut prefix = String::new();
        for component in components {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            directories_cmp.insert(tracked_cmp_key(&prefix));
        }
        files.insert(path);
    }
    Some(TrackedPaths {
        files,
        directories_cmp,
    })
}

fn git_ls_files(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
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

    /// vendor 名目录**之内**的判据是 gitignore，不是"是否已被 git add"。
    ///
    /// 两个方向都必须成立：被 gitignore 的构建产物不得进索引（否则产物每次构建
    /// 都产生无法声明的 unplanned 差量，goal 门禁不可满足）；而未跟踪**且未被
    /// 忽略**的文件是用户刚写、还没 add 的工作，静默丢弃它会让 `checkpoint save`
    /// 漏掉未提交内容却照样报成功——本模块声称要防的"谎称遍历完整"。
    #[test]
    fn vendor_directory_contents_follow_gitignore_not_staging_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("build")).unwrap();
        fs::write(root.join("build/deploy.ps1"), "Write-Host x").unwrap();
        fs::write(root.join("README.md"), "# x").unwrap();
        // 产物被显式忽略，这是排除它们的唯一判据。
        fs::write(root.join(".gitignore"), "build/out/\nbuild/*.tmp\n").unwrap();

        git(root, &["init", "--quiet"]);
        git(
            root,
            &["add", "build/deploy.ps1", "README.md", ".gitignore"],
        );

        fs::create_dir_all(root.join("build/out")).unwrap();
        fs::write(root.join("build/out/bundle.bin"), "artifact").unwrap();
        fs::write(root.join("build/cache.tmp"), "artifact").unwrap();
        // 刚写好、还没 add 的用户脚本：git status 会显示为 `??`。
        fs::write(root.join("build/new-script.ps1"), "Write-Host new").unwrap();

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(
            keys.contains(&"build/deploy.ps1".to_string()),
            "tracked content must stay indexed: {keys:?}"
        );
        assert!(
            !keys.iter().any(|key| key.starts_with("build/out")),
            "gitignored artifacts must stay out of the index: {keys:?}"
        );
        assert!(
            !keys.contains(&"build/cache.tmp".to_string()),
            "gitignored artifacts must stay out of the index: {keys:?}"
        );
        assert!(
            keys.contains(&"build/new-script.ps1".to_string()),
            "untracked but un-ignored work must never be dropped silently: {keys:?}"
        );
    }

    /// 子模块在普通 `git ls-files` 里只是一条裸目录记录、内容一条都不出现，
    /// 于是挂在 vendor 名路径下的子模块会被整棵剪掉——正是本模块声称要防的
    /// "谎称遍历完整"。
    #[test]
    fn a_submodule_under_a_vendor_named_path_is_not_pruned() {
        let outer = tempfile::tempdir().unwrap();
        let inner = tempfile::tempdir().unwrap();

        let inner_root = inner.path();
        fs::write(inner_root.join("lib.txt"), "vendored source").unwrap();
        git(inner_root, &["init", "--quiet"]);
        git(inner_root, &["add", "lib.txt"]);
        git(
            inner_root,
            &[
                "-c",
                "user.email=t@example.com",
                "-c",
                "user.name=t",
                "commit",
                "--quiet",
                "-m",
                "seed",
            ],
        );

        let root = outer.path();
        fs::write(root.join("README.md"), "# x").unwrap();
        git(root, &["init", "--quiet"]);
        git(root, &["add", "README.md"]);
        let inner_url = inner_root.to_string_lossy().replace('\\', "/");
        let added = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "--quiet",
                &inner_url,
                "dist",
            ])
            .status()
            .expect("git must be available");
        if !added.success() {
            // 某些环境禁用 file 协议的 submodule；此时跳过而不是给出假绿。
            return;
        }

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();

        assert!(
            keys.contains(&"dist/lib.txt".to_string()),
            "submodule content under a vendor-named path must stay visible: {keys:?}"
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

    /// STATE_IGNORE 是按名字剪枝，与条目是目录还是文件无关：linked worktree /
    /// submodule 的 `.git` 是 gitdir 指针**文件**，此前只剪目录，于是它进了索引
    /// 与 goal fingerprint，checkpoint restore 还会拿快照里的旧指针覆盖当前指针，
    /// 直接打断 worktree 的 git 链接。
    #[test]
    fn a_gitdir_pointer_file_never_participates_in_indexing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(
            root.join(".git"),
            "gitdir: ../main-repo/.git/worktrees/wt\n",
        )
        .unwrap();
        fs::write(root.join("src.rs"), "fn main() {}").unwrap();

        let keys: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();
        assert!(keys.contains(&"src.rs".to_string()), "{keys:?}");
        assert!(!keys.contains(&".git".to_string()), "{keys:?}");
    }

    /// 被 .gitignore 覆盖但被 `git add -f` 跟踪的文件曾在 ignore walker 内部
    /// 就被丢弃——索引、goal 差量门禁和 checkpoint"完整"快照一起对它失明。
    /// 判据必须是 git 跟踪状态（见模块顶部文档），补回逻辑锁死这一点。
    #[test]
    fn tracked_files_survive_gitignore_and_case_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "walk@test.local"]);
        git(root, &["config", "user.name", "walk-test"]);
        fs::write(root.join(".gitignore"), "deploy/\n").unwrap();
        fs::create_dir_all(root.join("deploy")).unwrap();
        fs::write(root.join("deploy/release.ps1"), "release").unwrap();
        fs::write(root.join("deploy/untracked.log"), "noise").unwrap();
        fs::write(root.join("README.md"), "# x").unwrap();
        git(root, &["add", "README.md", ".gitignore"]);
        git(root, &["add", "-f", "deploy/release.ps1"]);
        git(root, &["commit", "-qm", "init"]);

        let rels: Vec<String> = workspace_files_checked(root)
            .unwrap()
            .iter()
            .map(|path| relative_key(root, path))
            .collect();
        assert!(
            rels.contains(&"deploy/release.ps1".to_string()),
            "被 .gitignore 覆盖但被跟踪的文件必须参与索引: {rels:?}"
        );
        assert!(
            !rels.contains(&"deploy/untracked.log".to_string()),
            "未跟踪的忽略文件仍然排除: {rels:?}"
        );

        // Windows：盘面大小写漂移（git core.ignorecase 视为无变化）不得让
        // vendor 名目录下被跟踪的文件消失。
        #[cfg(windows)]
        {
            fs::create_dir_all(root.join("build")).unwrap();
            fs::write(root.join("build/TOOL.md"), "tool").unwrap();
            git(root, &["add", "build/TOOL.md"]);
            git(root, &["commit", "-qm", "vendor tracked"]);
            fs::rename(root.join("build/TOOL.md"), root.join("build/tmp.md")).unwrap();
            fs::rename(root.join("build/tmp.md"), root.join("build/tool.md")).unwrap();

            let rels: Vec<String> = workspace_files_checked(root)
                .unwrap()
                .iter()
                .map(|path| relative_key(root, path))
                .collect();
            assert!(
                rels.iter()
                    .any(|rel| rel.eq_ignore_ascii_case("build/tool.md")),
                "大小写漂移不得让被跟踪的 vendor 目录文件消失: {rels:?}"
            );
            assert_eq!(
                rels.iter()
                    .filter(|rel| rel.eq_ignore_ascii_case("build/tool.md"))
                    .count(),
                1,
                "补回逻辑不得因大小写差异重复计入同一文件: {rels:?}"
            );
        }
    }
}
