//! 精简版托管临时目录：运行期临时文件放工作区本地 `.RaymanCodingSkill/tmp/`，
//! 不用系统临时目录、不记忆跨项目位置。提供创建、递归审计与清理；清理只删自己管辖的目录。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::file_io::is_link_or_reparse;
use crate::pathfmt::display_path;
use crate::state_paths;

const TEMP_ROOT: &str = ".RaymanCodingSkill/tmp";
const MAX_REPORTED_ERRORS: usize = 64;

pub fn temp_root(root: &Path) -> PathBuf {
    root.join(TEMP_ROOT)
}

/// 在托管临时根下创建（若不存在）一个具名子目录并返回其路径。
pub fn scratch_dir(root: &Path, label: &str) -> Result<PathBuf> {
    let mut safe: String = label
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                ch
            }
        })
        .collect();
    // Apply Windows' trailing-dot/space rule everywhere so checkpoints remain portable.
    while safe.ends_with(' ') || safe.ends_with('.') {
        safe.pop();
        safe.push('_');
    }
    // Windows 保留设备名（con/nul/aux/com1…）作目录名非法，会报一条与输入无关的 OS 错误。
    if is_windows_reserved_name(&safe) {
        safe.push('_');
    }
    let label = if safe.is_empty() { "scratch" } else { &safe };
    state_paths::managed_state_dir(root, &Path::new("tmp").join(label), true)?
        .ok_or_else(|| anyhow::anyhow!("无法创建受管临时目录"))
}

fn is_windows_reserved_name(name: &str) -> bool {
    // Device names remain reserved with an extension (for example NUL.txt).
    let lower = name
        .trim_end_matches([' ', '.'])
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || (lower.len() == 4
            && (lower.starts_with("com") || lower.starts_with("lpt"))
            && lower.as_bytes()[3].is_ascii_digit())
}

#[derive(Debug, Clone)]
pub struct TempStatus {
    pub root: String,
    pub exists: bool,
    /// 向后兼容：托管根下的直接条目数。
    pub entry_count: usize,
    /// 递归文件数（不含符号链接/reparse point）。
    pub file_count: usize,
    /// 递归子目录数（不含根目录）。
    pub directory_count: usize,
    pub total_bytes: u64,
    pub traversal_error_count: usize,
    pub traversal_errors: Vec<String>,
}

/// 递归审计托管临时目录。所有不可遍历或链接/reparse 条目都会被计数并报告，
/// 而不是被默默当作零字节或零文件。
pub fn audit(root: &Path) -> TempStatus {
    let display_root = temp_root(root);
    let mut status = TempStatus {
        root: display_path(&display_root),
        exists: false,
        entry_count: 0,
        file_count: 0,
        directory_count: 0,
        total_bytes: 0,
        traversal_error_count: 0,
        traversal_errors: Vec::new(),
    };
    let dir = match state_paths::managed_state_dir(root, Path::new("tmp"), false) {
        Ok(None) => return status,
        Ok(Some(dir)) => {
            status.exists = true;
            dir
        }
        Err(error) => {
            record_error(
                &mut status,
                format!("{}: {error:#}", display_path(&display_root)),
            );
            return status;
        }
    };
    if let Err(error) = collect_metrics(&dir, &mut status, true) {
        record_error(&mut status, format!("{}: {error}", display_path(&dir)));
    }
    status
}

/// 兼容旧调用；现在返回的结构同时携带递归指标。
pub fn status(root: &Path) -> TempStatus {
    audit(root)
}

fn collect_metrics(dir: &Path, status: &mut TempStatus, is_root: bool) -> Result<()> {
    ensure_real_directory(dir)?;
    let entries =
        fs::read_dir(dir).with_context(|| format!("无法读取临时目录: {}", dir.display()))?;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record_error(status, format!("{}: {error}", display_path(dir)));
                continue;
            }
        };
        if is_root {
            status.entry_count += 1;
        }
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                record_error(status, format!("{}: {error}", display_path(&path)));
                continue;
            }
        };
        if is_link_or_reparse(&metadata) {
            record_error(
                status,
                format!("拒绝遍历链接/reparse 临时条目: {}", display_path(&path)),
            );
            continue;
        }
        if metadata.file_type().is_dir() {
            status.directory_count += 1;
            if let Err(error) = collect_metrics(&path, status, false) {
                record_error(status, format!("{}: {error}", display_path(&path)));
            }
        } else if metadata.file_type().is_file() {
            status.file_count += 1;
            status.total_bytes = status.total_bytes.saturating_add(metadata.len());
        } else {
            record_error(
                status,
                format!("不支持的临时条目类型: {}", display_path(&path)),
            );
        }
    }
    Ok(())
}

fn record_error(status: &mut TempStatus, error: String) {
    status.traversal_error_count += 1;
    if status.traversal_errors.len() < MAX_REPORTED_ERRORS {
        status.traversal_errors.push(error);
    }
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    crate::file_io::ensure_real_directory_labeled(path, "临时目录")
}

/// 清理整个托管临时根。只删 `.RaymanCodingSkill/tmp`，绝不触碰其它用户数据。
pub fn cleanup(root: &Path) -> Result<bool> {
    let Some(dir) = state_paths::managed_state_dir(root, Path::new("tmp"), false)? else {
        return Ok(false);
    };
    fs::remove_dir_all(&dir).with_context(|| format!("无法清理临时目录: {}", dir.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_label_avoids_windows_reserved_names() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(dir.path(), "nul").unwrap();
        assert_ne!(scratch.file_name().unwrap(), "nul");
        assert!(scratch.is_dir());
    }

    #[test]
    fn scratch_label_preserves_safe_unicode_and_sanitizes_path_syntax() {
        let dir = tempfile::tempdir().unwrap();
        let unicode = scratch_dir(dir.path(), "中文-資料-🙂").unwrap();
        assert_eq!(unicode.file_name().unwrap(), "中文-資料-🙂");

        let unsafe_label = scratch_dir(dir.path(), "../危险\\路径:*?").unwrap();
        let name = unsafe_label.file_name().unwrap().to_string_lossy();
        assert!(!name.contains('/') && !name.contains('\\'));
        assert!(!name.contains(':') && !name.contains('*') && !name.contains('?'));
    }

    #[test]
    fn scratch_label_rejects_reserved_device_stems_with_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let scratch = scratch_dir(dir.path(), "NUL.txt").unwrap();
        assert_ne!(scratch.file_name().unwrap(), "NUL.txt");
        assert!(scratch.is_dir());
    }

    #[test]
    fn scratch_create_status_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let scratch = scratch_dir(root, "build cache").unwrap();
        fs::create_dir_all(scratch.join("nested")).unwrap();
        fs::write(scratch.join("f.txt"), "x").unwrap();
        fs::write(scratch.join("nested/g.txt"), "xyz").unwrap();
        let report = status(root);
        assert!(report.exists);
        assert_eq!(report.entry_count, 1);
        assert_eq!(report.file_count, 2);
        assert_eq!(report.directory_count, 2, "scratch + nested");
        assert_eq!(report.total_bytes, 4);
        assert_eq!(report.traversal_error_count, 0);
        assert!(cleanup(root).unwrap());
        assert!(!status(root).exists);
    }

    #[cfg(unix)]
    #[test]
    fn audit_and_cleanup_refuse_a_symlinked_temp_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".RaymanCodingSkill")).unwrap();
        symlink(outside.path(), temp_root(dir.path())).unwrap();

        let report = audit(dir.path());
        assert_eq!(report.traversal_error_count, 1);
        assert!(cleanup(dir.path()).is_err());
        assert!(outside.path().exists(), "cleanup must not follow the link");
    }
}
