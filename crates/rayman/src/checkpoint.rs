//! 工作树 checkpoint：把当前工作区（gitignore 感知）和 v2 任务状态快照到用户级目录，
//! 便于断电/切换 AI 工具后恢复。
//!
//! 快照不是尽力而为的日志：复制、遍历或校验失败会产生标记为 `partial` 的取证快照并
//! 返回错误，绝不会把它当作可恢复的最新快照；只有完成的 v2 manifest 才能恢复或参与轮换。

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::file_io::is_link_or_reparse;
use crate::pathfmt::display_path;
use crate::{hash, walk};

/// 默认保留的完整快照数（滚动，多留几个以防某次保存中途损坏）。
pub const DEFAULT_KEEP: usize = 3;
pub const MANIFEST_SCHEMA: &str = "rayman.checkpoint.v3";
pub const MANIFEST_VERSION: u32 = 3;
const LEGACY_MANIFEST_SCHEMA: &str = "rayman.checkpoint.v2";
const LEGACY_MANIFEST_VERSION: u32 = 2;

const TREE_SUBDIR: &str = "tree";
const MANIFEST_NAME: &str = "manifest.json";
const STAGING_PREFIX: &str = ".staging-";
const LOCK_NAME: &str = ".checkpoint.lock";
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const RESTORE_TRANSACTION_PREFIX: &str = ".restore-transaction-";
const RESTORE_JOURNAL_NAME: &str = "journal.json";
const RESTORE_JOURNAL_SCHEMA: &str = "rayman.restore-transaction.v1";
const RESTORE_JOURNAL_VERSION: u32 = 1;

struct CheckpointLock {
    file: fs::File,
}

impl CheckpointLock {
    fn acquire(ws_dir: &Path) -> Result<Self> {
        ensure_real_directory(ws_dir)?;
        let path = ws_dir.join(LOCK_NAME);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() => {
                bail!("checkpoint 锁不是安全普通文件: {}", display_path(&path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法检查 checkpoint 锁: {}", display_path(&path)));
            }
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("无法打开 checkpoint 锁: {}", display_path(&path)))?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("无法复查 checkpoint 锁: {}", display_path(&path)))?;
        if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
            bail!("checkpoint 锁被替换为非普通文件: {}", display_path(&path));
        }
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error)
                    if is_checkpoint_lock_busy(&error) && started.elapsed() < LOCK_TIMEOUT =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) if is_checkpoint_lock_busy(&error) => {
                    bail!("等待 checkpoint 锁超过 {} 秒", LOCK_TIMEOUT.as_secs());
                }
                Err(error) => return Err(error).context("无法取得 checkpoint 独占锁"),
            }
        }
    }
}

fn is_checkpoint_lock_busy(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        // Windows LockFileEx commonly maps contention to ERROR_LOCK_VIOLATION
        // rather than WouldBlock; sharing violations are the same retryable state.
        || matches!(error.raw_os_error(), Some(32) | Some(33))
}

impl Drop for CheckpointLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// 当前 v2 允许随快照携带的状态文件。不要把整个 `.RaymanCodingSkill/` 当作备份源：
/// 其中可能有旧版本状态、评测运行物或数 GB 的托管临时文件。
const V2_STATE_FILES: &[&str] = &[
    "pending.json",
    "autosave.json",
    "context/index.json",
    "context/project_map.json",
];
const V2_GOALS_DIR: &str = "goals";

/// 快照的可恢复性状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStatus {
    #[default]
    Complete,
    Partial,
    Corrupt,
}

/// 一份被写入树中的文件的不可变完整性记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIntegrity {
    /// 相对 `tree/` 的正斜杠路径。
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// `None` only occurs when deserializing a legacy v2 manifest.  Such a
    /// snapshot remains inspectable, but verification/restore refuses it: byte
    /// identity alone cannot safely reconstruct platform permission metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    /// Unix permission/special bits (`st_mode & 0o7777`).  A snapshot created
    /// on a platform that cannot provide them is deliberately not restorable on
    /// Unix, where silently inventing executable/write bits would be unsafe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unix_mode: Option<u32>,
}

/// 单个快照的 v2 元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub status: SnapshotStatus,
    pub created_at: String,
    pub workspace_root: String,
    pub file_count: usize,
    /// 仅为兼容旧 CLI 输出保留；完整 v2 快照必须为零。
    #[serde(default)]
    pub skipped_count: usize,
    pub total_bytes: u64,
    pub tool_version: String,
    #[serde(default)]
    pub files: Vec<FileIntegrity>,
    /// partial/corrupt 状态的具体失败原因；完整快照必须为空。
    #[serde(default)]
    pub errors: Vec<String>,
}

/// 一次保存的结果摘要。仅 `complete` 保存会返回此结构。
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
    pub status: SnapshotStatus,
}

/// 恢复的结果摘要。成功返回时 `failed` 恒为 0；失败会直接返回 Err，避免把部分恢复误报为成功。
pub struct RestoreOutcome {
    pub id: String,
    pub restored: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RestoreTransactionEntry {
    expected: FileIntegrity,
    #[serde(default)]
    original: Option<FileIntegrity>,
    #[serde(default)]
    destination_prepared: bool,
    #[serde(default)]
    publish_attempted: bool,
    #[serde(default)]
    rollback_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RestorePhase {
    Preparing,
    Publishing,
    RollingBack,
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RestoreDirectoryState {
    Planned,
    Created,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RestoreCreatedDirectory {
    path: String,
    state: RestoreDirectoryState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RestoreJournal {
    schema: String,
    version: u32,
    workspace_root: String,
    phase: RestorePhase,
    entries: Vec<RestoreTransactionEntry>,
    #[serde(default)]
    created_directories: Vec<RestoreCreatedDirectory>,
}

struct RestoreTransaction {
    path: PathBuf,
    staged: PathBuf,
    backups: PathBuf,
    journal: RestoreJournal,
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

/// 纳入快照的文件：工作树（gitignore 感知）+ v2 状态白名单。
fn files_to_capture(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = walk::workspace_files_checked(root)?;
    files.extend(v2_state_files(root)?);
    files.sort();
    files.dedup();
    Ok(files)
}

fn v2_state_files(root: &Path) -> Result<Vec<PathBuf>> {
    let state = root.join(".RaymanCodingSkill");
    match fs::symlink_metadata(&state) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取状态目录: {}", display_path(&state)));
        }
        Ok(_) => ensure_real_directory(&state)?,
    }

    let mut files = Vec::new();
    for relative in V2_STATE_FILES {
        capture_optional_state_file(&state, Path::new(relative), &mut files)?;
    }
    collect_goal_state_files(&state, &mut files)?;
    Ok(files)
}

fn capture_optional_state_file(
    state: &Path,
    relative: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    let path = state.join(relative);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("无法读取 v2 状态文件: {}", display_path(&path)))
        }
        Ok(metadata) => {
            if is_link_or_reparse(&metadata) {
                bail!("拒绝链接/reparse 状态文件: {}", display_path(&path));
            }
            if !metadata.file_type().is_file() {
                bail!("v2 状态路径不是普通文件: {}", display_path(&path));
            }
            ensure_safe_file_under(state, relative)?;
            out.push(path);
            Ok(())
        }
    }
}

fn collect_goal_state_files(state: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let goals = state.join(V2_GOALS_DIR);
    match fs::symlink_metadata(&goals) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法读取目标状态目录: {}", display_path(&goals)));
        }
        Ok(_) => ensure_real_directory(&goals)?,
    }
    let entries = fs::read_dir(&goals)
        .with_context(|| format!("无法遍历目标状态目录: {}", display_path(&goals)))?;
    for entry in entries {
        let entry =
            entry.with_context(|| format!("无法读取目标状态目录条目: {}", display_path(&goals)))?;
        let name = entry.file_name();
        if Path::new(&name)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("json")
        {
            continue;
        }
        let relative = Path::new(V2_GOALS_DIR).join(&name);
        capture_optional_state_file(state, &relative, out)?;
    }
    Ok(())
}

/// 保存当前工作树快照，然后仅按 `keep` 清理旧的完整快照。
///
/// 若复制、遍历或验证有任何错误，会保留一份 `partial` 快照供取证并返回 Err；
/// 此路径不轮换任何完整快照，因此至少最近的完整恢复点不会被失败保存删除。
pub fn save(root: &Path, override_dir: Option<&Path>, keep: usize) -> Result<SaveOutcome> {
    // keep=0 会在 prune 时把刚保存的快照一起删光，安全网形同虚设。
    let keep = keep.max(1);
    ensure_real_directory(root)?;
    let root = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;

    let ckpt_root = checkpoints_root(override_dir)?;
    ensure_real_directory_chain(&ckpt_root)?;
    fs::create_dir_all(&ckpt_root)
        .with_context(|| format!("无法创建 checkpoint 目录: {}", display_path(&ckpt_root)))?;
    ensure_real_directory_chain(&ckpt_root)?;
    ensure_real_directory(&ckpt_root)?;
    let ckpt_root = ckpt_root
        .canonicalize()
        .with_context(|| format!("无法规范化 checkpoint 目录: {}", display_path(&ckpt_root)))?;
    let ws_dir = ckpt_root.join(workspace_key(&root));
    ensure_real_directory_chain(&ws_dir)?;
    fs::create_dir_all(&ws_dir)
        .with_context(|| format!("无法创建工作区 checkpoint 目录: {}", display_path(&ws_dir)))?;
    ensure_real_directory(&ws_dir)?;
    let _lock = CheckpointLock::acquire(&ws_dir)?;
    // A saver must never snapshot a workspace left halfway through a crashed
    // restore.  Recovery runs under the same per-workspace lock as restore.
    recover_orphaned_restore_transactions(&root, &ws_dir)?;

    let timestamp = crate::timefmt::now_iso();
    let id = fs_safe_id(&timestamp);
    let staging = ws_dir.join(format!("{STAGING_PREFIX}{}-{id}", std::process::id()));
    if staging.exists() {
        bail!(
            "checkpoint 暂存目录已存在，拒绝覆盖: {}",
            display_path(&staging)
        );
    }
    fs::create_dir(&staging)
        .with_context(|| format!("无法创建 checkpoint 暂存目录: {}", display_path(&staging)))?;
    ensure_real_directory(&staging)?;
    let tree = staging.join(TREE_SUBDIR);
    fs::create_dir(&tree)
        .with_context(|| format!("无法创建 checkpoint 树目录: {}", display_path(&tree)))?;
    ensure_real_directory(&tree)?;
    let final_dir = ws_dir.join(&id);

    let files = match files_to_capture(&root) {
        Ok(files) => files,
        Err(error) => {
            return finish_partial(
                &staging,
                &final_dir,
                partial_manifest(&timestamp, &root, Vec::new(), vec![error.to_string()]),
            );
        }
    };

    let mut integrity = Vec::new();
    let mut errors = Vec::new();
    for source in files {
        // 用户若把 --dir 指到工作区内，绝不把 checkpoint 自己拷进去。
        if source.starts_with(&ckpt_root) {
            continue;
        }
        let relative = match source.strip_prefix(&root) {
            Ok(relative) => relative,
            Err(_) => {
                errors.push(format!(
                    "发现不属于工作区根的候选文件: {}",
                    display_path(&source)
                ));
                continue;
            }
        };
        match copy_and_measure(&root, relative, &tree) {
            Ok(record) => integrity.push(record),
            Err(error) => errors.push(format!("{}: {error}", display_path(&source))),
        }
    }

    let manifest = Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        version: MANIFEST_VERSION,
        status: SnapshotStatus::Complete,
        created_at: timestamp,
        workspace_root: display_path(&root),
        file_count: integrity.len(),
        skipped_count: 0,
        total_bytes: integrity.iter().map(|entry| entry.size).sum(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        files: integrity,
        errors: Vec::new(),
    };
    if !errors.is_empty() {
        let mut partial = manifest;
        partial.status = SnapshotStatus::Partial;
        partial.skipped_count = errors.len();
        partial.errors = errors;
        return finish_partial(&staging, &final_dir, partial);
    }
    if let Err(error) = verify_manifest_tree(&tree, &manifest) {
        let mut partial = manifest;
        partial.status = SnapshotStatus::Partial;
        partial.skipped_count = 1;
        partial.errors = vec![format!("保存后完整性验证失败: {error}")];
        return finish_partial(&staging, &final_dir, partial);
    }

    commit_snapshot(&staging, &final_dir, &manifest)?;
    let pruned = prune(&ws_dir, keep)?;
    Ok(SaveOutcome {
        id,
        path: final_dir,
        file_count: manifest.file_count,
        skipped_count: 0,
        total_bytes: manifest.total_bytes,
        pruned,
    })
}

fn partial_manifest(
    timestamp: &str,
    root: &Path,
    files: Vec<FileIntegrity>,
    errors: Vec<String>,
) -> Manifest {
    Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        version: MANIFEST_VERSION,
        status: SnapshotStatus::Partial,
        created_at: timestamp.to_string(),
        workspace_root: display_path(root),
        file_count: files.len(),
        skipped_count: errors.len(),
        total_bytes: files.iter().map(|entry| entry.size).sum(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        files,
        errors,
    }
}

fn finish_partial(staging: &Path, final_dir: &Path, manifest: Manifest) -> Result<SaveOutcome> {
    let id = final_dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let failure_count = manifest.errors.len();
    commit_snapshot(staging, final_dir, &manifest)?;
    bail!(
        "checkpoint 保存不完整（{} 个错误）；已保留 partial 快照 {} 供取证，不会替代最近完整快照",
        failure_count,
        id
    )
}

fn commit_snapshot(staging: &Path, final_dir: &Path, manifest: &Manifest) -> Result<()> {
    ensure_real_directory(staging)?;
    if final_dir.exists() {
        bail!(
            "checkpoint 目标目录已存在，拒绝覆盖: {}",
            display_path(final_dir)
        );
    }
    crate::file_io::write_json(&staging.join(MANIFEST_NAME), manifest)?;
    ensure_safe_file_under(staging, Path::new(MANIFEST_NAME))?;
    fs::rename(staging, final_dir).with_context(|| {
        format!(
            "无法提交 checkpoint: {} -> {}",
            display_path(staging),
            display_path(final_dir)
        )
    })?;
    Ok(())
}

fn copy_and_measure(source_root: &Path, relative: &Path, tree: &Path) -> Result<FileIntegrity> {
    let source_before = integrity_for_file(source_root, relative)?;
    let destination = prepare_destination_file(tree, relative)?;
    let source = source_root.join(relative);
    // Re-check both sides immediately before the copy.  The checks cannot remove a
    // same-user TOCTOU race, but they fail closed on ordinary symlink/reparse swaps.
    ensure_safe_file_under(source_root, relative)?;
    prepare_destination_file(tree, relative)?;
    fs::copy(&source, &destination).with_context(|| {
        format!(
            "无法复制 checkpoint 文件: {} -> {}",
            display_path(&source),
            display_path(&destination)
        )
    })?;

    ensure_safe_file_under(source_root, relative)?;
    ensure_safe_file_under(tree, relative)?;
    let destination_after = integrity_for_file(tree, relative)?;
    let source_after = integrity_for_file(source_root, relative)?;
    if source_before != source_after {
        bail!("源文件在 checkpoint 复制期间发生变化");
    }
    if source_before != destination_after {
        bail!("复制后的文件与源文件完整性不一致");
    }
    Ok(destination_after)
}

/// RFC3339 时间戳转成文件系统安全的目录名（去掉 ':'）。字典序仍等于时间序。
fn fs_safe_id(timestamp: &str) -> String {
    timestamp.replace(':', "-")
}

/// 仅轮换完整快照；partial/corrupt 快照是故障取证，不会被自动删掉。
fn prune(ws_dir: &Path, keep: usize) -> Result<usize> {
    ensure_real_directory(ws_dir)?;
    let mut complete = Vec::new();
    let entries = fs::read_dir(ws_dir)
        .with_context(|| format!("无法列出 checkpoint 目录: {}", display_path(ws_dir)))?;
    for entry in entries {
        let entry = entry
            .with_context(|| format!("无法读取 checkpoint 目录条目: {}", display_path(ws_dir)))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.file_type().is_dir() || is_link_or_reparse(&metadata) {
            continue;
        }
        if name.starts_with(STAGING_PREFIX) {
            // Staging may belong to another live saver.  The workspace lock
            // serializes new saves, but never reap an uncommitted directory
            // implicitly: a crashed staging tree is forensic evidence and can
            // only be removed by an explicit maintenance action.
            continue;
        }
        if name.starts_with('.') {
            continue;
        }
        if inspect_snapshot(&path).1 == SnapshotStatus::Complete {
            complete.push(path);
        }
    }
    complete.sort(); // 时间戳目录名字典序 = 时间序
    let mut pruned = 0;
    if complete.len() > keep {
        for path in complete.iter().take(complete.len() - keep) {
            fs::remove_dir_all(path)
                .with_context(|| format!("无法清理旧 checkpoint: {}", display_path(path)))?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

/// 列出当前工作区的所有快照（按时间升序），并标记完整/partial/corrupt 状态。
pub fn list(root: &Path, override_dir: Option<&Path>) -> Result<Vec<CheckpointInfo>> {
    let ws_dir = workspace_dir(root, override_dir)?;
    let mut out = Vec::new();
    let entries = match fs::read_dir(&ws_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("无法列出 checkpoint 目录: {}", display_path(&ws_dir)));
        }
    };
    for entry in entries {
        let entry = entry
            .with_context(|| format!("无法读取 checkpoint 目录条目: {}", display_path(&ws_dir)))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.file_type().is_dir() {
            continue;
        }
        let (manifest, status) = inspect_snapshot(&path);
        out.push(CheckpointInfo {
            id: name,
            path,
            manifest,
            status,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// 最近一次经完整性验证的完整快照（无则 None）。
pub fn latest(root: &Path, override_dir: Option<&Path>) -> Result<Option<CheckpointInfo>> {
    Ok(list(root, override_dir)?
        .into_iter()
        .rev()
        .find(|checkpoint| checkpoint.status == SnapshotStatus::Complete))
}

/// 验证单个快照的 manifest、文件数、所有路径、大小和 SHA-256。
///
/// 该 API 不信任快照目录内容；任何缺失、篡改、链接/reparse traversal 或 schema 漂移都会返回错误。
pub fn verify_snapshot(snapshot: &Path) -> Result<Manifest> {
    ensure_real_directory(snapshot)?;
    ensure_safe_file_under(snapshot, Path::new(MANIFEST_NAME))?;
    let manifest = crate::file_io::read_json::<Manifest>(&snapshot.join(MANIFEST_NAME))?
        .ok_or_else(|| anyhow::anyhow!("checkpoint 缺少 manifest: {}", display_path(snapshot)))?;
    if manifest.schema == LEGACY_MANIFEST_SCHEMA && manifest.version == LEGACY_MANIFEST_VERSION {
        bail!(
            "checkpoint 使用旧 v2 content-only manifest，缺少权限完整性证明，不能安全恢复；请用当前 Rayman 新建 v3 checkpoint"
        );
    }
    if manifest.schema != MANIFEST_SCHEMA || manifest.version != MANIFEST_VERSION {
        bail!(
            "checkpoint manifest 不是受支持的 v3 schema: schema={:?} version={}",
            manifest.schema,
            manifest.version
        );
    }
    if manifest.status != SnapshotStatus::Complete {
        bail!("checkpoint 不是完整快照（状态：{:?}）", manifest.status);
    }
    if manifest.skipped_count != 0 || !manifest.errors.is_empty() {
        bail!("完整 checkpoint manifest 含有跳过项或错误记录");
    }
    let tree = snapshot.join(TREE_SUBDIR);
    verify_manifest_tree(&tree, &manifest)?;
    Ok(manifest)
}

fn inspect_snapshot(snapshot: &Path) -> (Option<Manifest>, SnapshotStatus) {
    let manifest = match load_manifest_untrusted(snapshot) {
        Ok(manifest) => manifest,
        Err(_) => return (None, SnapshotStatus::Corrupt),
    };
    if manifest.schema != MANIFEST_SCHEMA || manifest.version != MANIFEST_VERSION {
        return (Some(manifest), SnapshotStatus::Corrupt);
    }
    match manifest.status {
        SnapshotStatus::Complete => match verify_snapshot(snapshot) {
            Ok(_) => (Some(manifest), SnapshotStatus::Complete),
            Err(_) => (Some(manifest), SnapshotStatus::Corrupt),
        },
        SnapshotStatus::Partial => (Some(manifest), SnapshotStatus::Partial),
        SnapshotStatus::Corrupt => (Some(manifest), SnapshotStatus::Corrupt),
    }
}

fn load_manifest_untrusted(snapshot: &Path) -> Result<Manifest> {
    ensure_real_directory(snapshot)?;
    ensure_safe_file_under(snapshot, Path::new(MANIFEST_NAME))?;
    crate::file_io::read_json::<Manifest>(&snapshot.join(MANIFEST_NAME))?
        .ok_or_else(|| anyhow::anyhow!("checkpoint 缺少 manifest"))
}

fn verify_manifest_tree(tree: &Path, manifest: &Manifest) -> Result<()> {
    ensure_real_directory(tree)?;
    if manifest.file_count != manifest.files.len() {
        bail!(
            "manifest file_count={} 与完整性条目数={} 不一致",
            manifest.file_count,
            manifest.files.len()
        );
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for expected in &manifest.files {
        if !is_valid_sha256(&expected.sha256) {
            bail!("manifest 含无效 SHA-256: {}", expected.path);
        }
        validate_integrity_record(expected)?;
        let relative = manifest_relative_path(&expected.path)?;
        let key = normalized_path_key(&relative)?;
        if !seen.insert(key) {
            bail!("manifest 包含重复路径: {}", expected.path);
        }
        let actual = integrity_for_file(tree, &relative)?;
        if actual != *expected {
            bail!("checkpoint 文件完整性不匹配: {}", expected.path);
        }
        total = total
            .checked_add(actual.size)
            .ok_or_else(|| anyhow::anyhow!("checkpoint 总字节数溢出"))?;
    }
    if total != manifest.total_bytes {
        bail!(
            "manifest total_bytes={} 与文件完整性总和={} 不一致",
            manifest.total_bytes,
            total
        );
    }
    Ok(())
}

mod restore;

pub use restore::*;

#[cfg(test)]
mod tests;
