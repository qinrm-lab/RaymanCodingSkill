//! 精简版托管临时目录：运行期临时文件放工作区本地 `.RaymanCodingSkill/tmp/`，
//! 不用系统临时目录、不记忆跨项目位置。提供创建、递归审计与清理；清理只删自己管辖的目录。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::file_io::is_link_or_reparse;
use crate::pathfmt::display_path;
use crate::state_paths;

const TEMP_ROOT: &str = ".RaymanCodingSkill/tmp";
const MAX_REPORTED_ERRORS: usize = 64;
const LEASES_RELATIVE: &str = "tmp/leases";
const LEASE_MANIFEST: &str = "lease.json";
static LEASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn temp_root(root: &Path) -> PathBuf {
    root.join(TEMP_ROOT)
}

/// 在托管临时根下创建（若不存在）一个具名子目录并返回其路径。
pub fn scratch_dir(root: &Path, label: &str) -> Result<PathBuf> {
    let safe = safe_label(label);
    state_paths::managed_state_dir(root, &Path::new("tmp").join(&safe), true)?
        .ok_or_else(|| anyhow::anyhow!("无法创建受管临时目录"))
}

fn safe_label(label: &str) -> String {
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
    if safe.is_empty() {
        "scratch".into()
    } else {
        safe
    }
}

fn safe_lease_id_label(label: &str) -> String {
    let safe = safe_label(label)
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches(['-', '.']).to_string();
    if safe.is_empty() {
        "pytest".into()
    } else {
        safe
    }
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

/// A workspace-local, source-excluded pytest run root. The manifest makes
/// ownership explicit so cleanup never accepts an arbitrary caller path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PytestLease {
    pub schema: String,
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub root: String,
    pub basetemp: String,
    pub cache_dir: String,
    pub temp_dir: String,
    pub pycache_dir: String,
    pub environment: BTreeMap<String, String>,
    pub pytest_args: Vec<String>,
}

fn lease_relative(id: &str) -> Result<PathBuf> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        anyhow::bail!("无效 pytest lease id: {id}");
    }
    Ok(Path::new(LEASES_RELATIVE).join(id))
}

fn lease_path(root: &Path, id: &str, create: bool) -> Result<Option<PathBuf>> {
    state_paths::managed_state_dir(root, &lease_relative(id)?, create)
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

/// Create a unique pytest lease and prove every directory can be traversed,
/// written, read and cleaned by the current execution token.
pub fn create_pytest_lease(root: &Path, label: &str) -> Result<PytestLease> {
    let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = crate::timefmt::now_iso()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    let id = format!(
        "{}-{}-{}-{}",
        safe_lease_id_label(label),
        timestamp,
        std::process::id(),
        sequence
    );
    let lease_root =
        lease_path(root, &id, true)?.ok_or_else(|| anyhow::anyhow!("无法创建 pytest lease"))?;
    let basetemp = lease_root.join("basetemp");
    let cache_dir = lease_root.join("cache");
    let temp_dir = lease_root.join("temp");
    let pycache_dir = lease_root.join("pycache");
    for path in [&basetemp, &cache_dir, &temp_dir, &pycache_dir] {
        fs::create_dir_all(path)
            .with_context(|| format!("无法创建 pytest lease 子目录: {}", display_path(path)))?;
        ensure_real_directory(path)?;
        probe_directory(path)?;
    }

    let mut environment = BTreeMap::new();
    environment.insert("TMP".into(), path_text(&temp_dir));
    environment.insert("TEMP".into(), path_text(&temp_dir));
    environment.insert("TMPDIR".into(), path_text(&temp_dir));
    environment.insert("PYTHONPYCACHEPREFIX".into(), path_text(&pycache_dir));
    environment.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    let lease = PytestLease {
        schema: "rayman.pytest-lease.v1".into(),
        id: id.clone(),
        label: label.trim().to_string(),
        created_at: crate::timefmt::now_iso(),
        root: path_text(&lease_root),
        basetemp: path_text(&basetemp),
        cache_dir: path_text(&cache_dir),
        temp_dir: path_text(&temp_dir),
        pycache_dir: path_text(&pycache_dir),
        environment,
        pytest_args: vec![
            "--basetemp".into(),
            path_text(&basetemp),
            "-o".into(),
            format!("cache_dir={}", path_text(&cache_dir)),
        ],
    };
    crate::file_io::write_json(&lease_root.join(LEASE_MANIFEST), &lease)?;
    verify_pytest_lease(root, &id)
}

fn probe_directory(path: &Path) -> Result<()> {
    let sequence = LEASE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe = path.join(format!(
        ".rayman-probe-{}-{sequence}.tmp",
        std::process::id()
    ));
    fs::write(&probe, b"rayman-temp-probe")
        .with_context(|| format!("pytest lease 写探针失败: {}", display_path(&probe)))?;
    let bytes = fs::read(&probe)
        .with_context(|| format!("pytest lease 读探针失败: {}", display_path(&probe)))?;
    if bytes != b"rayman-temp-probe" {
        anyhow::bail!("pytest lease 探针内容不一致: {}", display_path(&probe));
    }
    fs::remove_file(&probe)
        .with_context(|| format!("pytest lease 清理探针失败: {}", display_path(&probe)))?;
    Ok(())
}

pub fn verify_pytest_lease(root: &Path, id: &str) -> Result<PytestLease> {
    let lease_root =
        lease_path(root, id, false)?.ok_or_else(|| anyhow::anyhow!("pytest lease 不存在: {id}"))?;
    let lease = crate::file_io::read_json::<PytestLease>(&lease_root.join(LEASE_MANIFEST))?
        .ok_or_else(|| anyhow::anyhow!("pytest lease 缺少 manifest: {id}"))?;
    let basetemp = lease_root.join("basetemp");
    let cache_dir = lease_root.join("cache");
    let temp_dir = lease_root.join("temp");
    let pycache_dir = lease_root.join("pycache");
    if lease.schema != "rayman.pytest-lease.v1"
        || lease.id != id
        || Path::new(&lease.root) != lease_root
        || Path::new(&lease.basetemp) != basetemp
        || Path::new(&lease.cache_dir) != cache_dir
        || Path::new(&lease.temp_dir) != temp_dir
        || Path::new(&lease.pycache_dir) != pycache_dir
        || lease.environment.get("TMP") != Some(&path_text(&temp_dir))
        || lease.environment.get("TEMP") != Some(&path_text(&temp_dir))
        || lease.environment.get("TMPDIR") != Some(&path_text(&temp_dir))
        || lease.environment.get("PYTHONPYCACHEPREFIX") != Some(&path_text(&pycache_dir))
        || lease.environment.get("PYTHONDONTWRITEBYTECODE") != Some(&"1".to_string())
        || lease.pytest_args
            != vec![
                "--basetemp".to_string(),
                path_text(&basetemp),
                "-o".to_string(),
                format!("cache_dir={}", path_text(&cache_dir)),
            ]
    {
        anyhow::bail!("pytest lease manifest 与受管路径不一致: {id}");
    }
    for path in [&basetemp, &cache_dir, &temp_dir, &pycache_dir] {
        ensure_real_directory(path)?;
        probe_directory(path)?;
    }
    Ok(lease)
}

/// Remove exactly one manifest-owned lease. Arbitrary paths are never
/// accepted and a missing/corrupt manifest fails closed.
pub fn release_pytest_lease(root: &Path, id: &str) -> Result<bool> {
    let lease = verify_pytest_lease(root, id)?;
    let lease_root = PathBuf::from(&lease.root);
    fs::remove_dir_all(&lease_root)
        .with_context(|| format!("无法释放 pytest lease: {}", display_path(&lease_root)))?;
    Ok(true)
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

    #[test]
    fn pytest_lease_is_unique_probed_and_manifest_owned() {
        let dir = tempfile::tempdir().unwrap();
        let first = create_pytest_lease(dir.path(), "focused tests").unwrap();
        let second = create_pytest_lease(dir.path(), "focused tests").unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(first.schema, "rayman.pytest-lease.v1");
        assert!(Path::new(&first.basetemp).is_dir());
        assert_eq!(verify_pytest_lease(dir.path(), &first.id).unwrap(), first);
        assert!(release_pytest_lease(dir.path(), &first.id).unwrap());
        assert!(verify_pytest_lease(dir.path(), &first.id).is_err());
        assert!(verify_pytest_lease(dir.path(), "../outside").is_err());
        assert!(Path::new(&second.root).is_dir());
    }

    #[test]
    fn pytest_lease_rejects_a_tampered_manifest_without_deleting_anything() {
        let dir = tempfile::tempdir().unwrap();
        let lease = create_pytest_lease(dir.path(), "tamper").unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        let manifest = Path::new(&lease.root).join(LEASE_MANIFEST);
        let mut tampered = lease.clone();
        tampered.basetemp = Path::new(&lease.root)
            .join("..")
            .join("outside")
            .display()
            .to_string();
        crate::file_io::write_json(&manifest, &tampered).unwrap();
        assert!(verify_pytest_lease(dir.path(), &lease.id).is_err());
        assert!(release_pytest_lease(dir.path(), &lease.id).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("keep.txt")).unwrap(),
            "keep"
        );
        assert!(Path::new(&lease.root).is_dir());
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
