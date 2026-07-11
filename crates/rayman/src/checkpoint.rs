//! 工作树 checkpoint：把当前工作区（gitignore 感知）和 v2 任务状态快照到用户级目录，
//! 便于断电/切换 AI 工具后恢复。
//!
//! 快照不是尽力而为的日志：复制、遍历或校验失败会产生标记为 `partial` 的取证快照并
//! 返回错误，绝不会把它当作可恢复的最新快照；只有完成的 v2 manifest 才能恢复或参与轮换。

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state_store::{self, display_path};
use crate::{hash, walk};

/// 默认保留的完整快照数（滚动，多留几个以防某次保存中途损坏）。
pub const DEFAULT_KEEP: usize = 3;
pub const MANIFEST_SCHEMA: &str = "rayman.checkpoint.v2";
pub const MANIFEST_VERSION: u32 = 2;

const TREE_SUBDIR: &str = "tree";
const MANIFEST_NAME: &str = "manifest.json";
const STAGING_PREFIX: &str = ".staging-";

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
    fs::create_dir_all(&ckpt_root)
        .with_context(|| format!("无法创建 checkpoint 目录: {}", display_path(&ckpt_root)))?;
    ensure_real_directory_chain(&ckpt_root)?;
    ensure_real_directory(&ckpt_root)?;
    let ckpt_root = ckpt_root
        .canonicalize()
        .with_context(|| format!("无法规范化 checkpoint 目录: {}", display_path(&ckpt_root)))?;
    let ws_dir = ckpt_root.join(workspace_key(&root));
    fs::create_dir_all(&ws_dir)
        .with_context(|| format!("无法创建工作区 checkpoint 目录: {}", display_path(&ws_dir)))?;
    ensure_real_directory(&ws_dir)?;

    let timestamp = state_store::now_iso();
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
    state_store::write_json(&staging.join(MANIFEST_NAME), manifest)?;
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
            // 只清理真实目录，绝不 remove_dir_all 一个可能指向外部的链接。
            let _ = fs::remove_dir_all(&path);
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
    let manifest = state_store::read_json::<Manifest>(&snapshot.join(MANIFEST_NAME))?
        .ok_or_else(|| anyhow::anyhow!("checkpoint 缺少 manifest: {}", display_path(snapshot)))?;
    if manifest.schema != MANIFEST_SCHEMA || manifest.version != MANIFEST_VERSION {
        bail!(
            "checkpoint manifest 不是受支持的 v2 schema: schema={:?} version={}",
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
    state_store::read_json::<Manifest>(&snapshot.join(MANIFEST_NAME))?
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
        let relative = manifest_relative_path(&expected.path)?;
        let key = normalized_path_key(&relative)?;
        if !seen.insert(key) {
            bail!("manifest 包含重复路径: {}", expected.path);
        }
        let actual = integrity_for_file(tree, &relative)?;
        if actual.size != expected.size || actual.sha256 != expected.sha256 {
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

/// 把某个完整快照（默认最近完整快照）叠加恢复到工作区。
/// 覆盖同名文件，但不删除工作区里多出来的文件；所有源文件和目标路径先验证，
/// 之后才写入第一个文件。
pub fn restore(
    root: &Path,
    override_dir: Option<&Path>,
    which: Option<&str>,
) -> Result<RestoreOutcome> {
    let checkpoints = list(root, override_dir)?;
    if checkpoints.is_empty() {
        bail!("没有可恢复的 checkpoint");
    }
    let target = match which {
        None | Some("latest") => checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.status == SnapshotStatus::Complete)
            .ok_or_else(|| anyhow::anyhow!("没有完整且已验证的 checkpoint"))?,
        Some(id) => checkpoints
            .iter()
            .find(|checkpoint| checkpoint.id == id)
            .ok_or_else(|| anyhow::anyhow!("找不到 checkpoint: {id}"))?,
    };
    if target.status != SnapshotStatus::Complete {
        bail!(
            "拒绝恢复非完整 checkpoint {}（状态：{:?}）",
            target.id,
            target.status
        );
    }
    let manifest = verify_snapshot(&target.path)?;
    ensure_real_directory(root)?;
    let tree = target.path.join(TREE_SUBDIR);

    let mut files = manifest.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    // 先验证每个目标目录和目标文件，避免写到一半才发现符号链接/reparse traversal。
    for expected in &files {
        let relative = manifest_relative_path(&expected.path)?;
        prepare_destination_file(root, &relative)?;
    }

    let mut restored = 0;
    for expected in &files {
        let relative = manifest_relative_path(&expected.path)?;
        let source = integrity_for_file(&tree, &relative)?;
        if source.size != expected.size || source.sha256 != expected.sha256 {
            bail!("恢复前源文件完整性发生变化: {}", expected.path);
        }
        let destination = root.join(&relative);
        prepare_destination_file(root, &relative)?;
        // Re-check source and destination immediately around the write.  This is
        // deliberately fail-closed for symlink/reparse substitutions; a hostile
        // same-user process can still race these path-based checks (documented).
        ensure_safe_file_under(&tree, &relative)?;
        prepare_destination_file(root, &relative)?;
        fs::copy(tree.join(&relative), &destination).with_context(|| {
            format!(
                "无法恢复 checkpoint 文件: {} -> {}",
                display_path(&tree.join(&relative)),
                display_path(&destination)
            )
        })?;
        ensure_safe_file_under(&tree, &relative)?;
        prepare_destination_file(root, &relative)?;
        let written = integrity_for_file(root, &relative)?;
        if written.size != expected.size || written.sha256 != expected.sha256 {
            bail!("恢复后目标文件完整性不匹配: {}", expected.path);
        }
        restored += 1;
    }
    Ok(RestoreOutcome {
        id: target.id.clone(),
        restored,
        failed: 0,
    })
}

fn manifest_relative_path(text: &str) -> Result<PathBuf> {
    let relative = PathBuf::from(text);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("checkpoint manifest 含不安全相对路径: {text}");
    }
    Ok(relative)
}

fn normalized_path_key(relative: &Path) -> Result<String> {
    let text = relative
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("checkpoint 路径不是有效 UTF-8: {}", relative.display()))?
        .replace('\\', "/");
    #[cfg(windows)]
    {
        Ok(text.to_ascii_lowercase())
    }
    #[cfg(not(windows))]
    {
        Ok(text)
    }
}

fn integrity_for_file(root: &Path, relative: &Path) -> Result<FileIntegrity> {
    ensure_safe_file_under(root, relative)?;
    let path = root.join(relative);
    let before = fs::symlink_metadata(&path)
        .with_context(|| format!("无法读取文件元数据: {}", display_path(&path)))?;
    if is_link_or_reparse(&before) || !before.file_type().is_file() {
        bail!("拒绝非普通文件: {}", display_path(&path));
    }
    let sha256 = hash::sha256_file(&path)?;
    let after = fs::symlink_metadata(&path)
        .with_context(|| format!("无法复查文件元数据: {}", display_path(&path)))?;
    if is_link_or_reparse(&after) || !after.file_type().is_file() || after.len() != before.len() {
        bail!("文件在读取期间变更或变为链接: {}", display_path(&path));
    }
    Ok(FileIntegrity {
        path: normalized_path_key(relative)?,
        size: after.len(),
        sha256,
    })
}

fn prepare_destination_file(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure_real_directory(root)?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("不安全的 checkpoint 目标路径: {}", relative.display());
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("不安全的 checkpoint 目标路径: {}", relative.display());
        };
        current.push(name);
        if index + 1 == components.len() {
            match fs::symlink_metadata(&current) {
                Ok(metadata) if is_link_or_reparse(&metadata) => {
                    bail!("拒绝链接/reparse 恢复目标: {}", display_path(&current));
                }
                Ok(metadata) if metadata.file_type().is_dir() => {
                    bail!("恢复目标是目录而非文件: {}", display_path(&current));
                }
                Ok(metadata) if !metadata.file_type().is_file() => {
                    bail!("恢复目标不是普通文件: {}", display_path(&current));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("无法读取恢复目标: {}", display_path(&current)));
                }
            }
        } else {
            ensure_or_create_real_directory(&current)?;
        }
    }
    Ok(current)
}

fn ensure_or_create_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_real_directory(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(create_error) => {
                    return Err(create_error)
                        .with_context(|| format!("无法创建恢复目标目录: {}", display_path(path)));
                }
            }
            ensure_real_directory(path)
        }
        Err(error) => {
            Err(error).with_context(|| format!("无法读取恢复目标目录: {}", display_path(path)))
        }
    }
}

fn ensure_safe_file_under(root: &Path, relative: &Path) -> Result<()> {
    ensure_real_directory(root)?;
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("不安全的 checkpoint 相对路径: {}", relative.display());
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("不安全的 checkpoint 相对路径: {}", relative.display());
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("无法读取 checkpoint 路径: {}", display_path(&current)))?;
        if is_link_or_reparse(&metadata) {
            bail!("拒绝链接/reparse 路径: {}", display_path(&current));
        }
        if index + 1 == components.len() {
            if !metadata.file_type().is_file() {
                bail!("checkpoint 路径不是普通文件: {}", display_path(&current));
            }
        } else if !metadata.file_type().is_dir() {
            bail!("checkpoint 路径组件不是目录: {}", display_path(&current));
        }
    }
    Ok(())
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法读取目录元数据: {}", display_path(path)))?;
    if is_link_or_reparse(&metadata) {
        bail!("拒绝链接/reparse 目录: {}", display_path(path));
    }
    if !metadata.file_type().is_dir() {
        bail!("路径不是目录: {}", display_path(path));
    }
    Ok(())
}

/// Reject symlink/reparse components in a checkpoint root's existing ancestor
/// chain.  The final directory check alone would still allow a symlinked parent
/// to redirect checkpoint writes outside the requested root.
fn ensure_real_directory_chain(path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                bail!(
                    "拒绝链接/reparse checkpoint 路径组件: {}",
                    display_path(candidate)
                );
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("无法读取 checkpoint 路径组件: {}", display_path(candidate))
                });
            }
        }
        current = candidate.parent();
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
    false
}

fn is_valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn save_with_dir_inside_workspace_excludes_prior_snapshots() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(&root.join("src/main.rs"), "fn main() {}");
        let ckpt_dir = root.join(".ckpts");

        let first = save(root, Some(&ckpt_dir), DEFAULT_KEEP).unwrap();
        let second = save(root, Some(&ckpt_dir), DEFAULT_KEEP).unwrap();

        // 快照目录在工作区内时，旧快照绝不能被递归拷进新快照（体积会几何级膨胀）。
        let tree = second.path.join(TREE_SUBDIR);
        assert!(
            !tree.join(".ckpts").exists(),
            "旧快照被拷进了新快照: {}",
            tree.display()
        );
        assert_eq!(second.file_count, first.file_count);
    }

    #[test]
    fn save_with_keep_zero_still_retains_latest_snapshot() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(&root.join("src/main.rs"), "fn main() {}");

        let outcome = save(root, Some(store.path()), 0).unwrap();
        assert!(
            outcome.path.join(MANIFEST_NAME).exists(),
            "keep=0 不得把刚保存的快照也删掉"
        );
        assert_eq!(
            verify_snapshot(&outcome.path).unwrap().status,
            SnapshotStatus::Complete
        );
    }

    #[test]
    fn save_captures_only_v2_state_whitelist_and_restores() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        let dir = Some(store.path());

        write(&root.join("src/main.rs"), "fn main() {}");
        write(&root.join(".gitignore"), "ignored.txt\n");
        write(&root.join("ignored.txt"), "secret");
        write(&root.join("node_modules/pkg/index.js"), "x");
        write(
            &root.join(".RaymanCodingSkill/goals/g1.json"),
            "{\"id\":\"g1\"}",
        );
        write(
            &root.join(".RaymanCodingSkill/pending.json"),
            "{\"items\":[]}",
        );
        write(
            &root.join(".RaymanCodingSkill/context/index.json"),
            "{\"version\":2}",
        );
        write(
            &root.join(".RaymanCodingSkill/context/project_map.json"),
            "{\"version\":2}",
        );
        write(
            &root.join(".RaymanCodingSkill/autosave.json"),
            "{\"active\":false}",
        );
        write(
            &root.join(".RaymanCodingSkill/tmp/scratch.txt"),
            "transient",
        );
        write(
            &root.join(".RaymanCodingSkill/regression/history.jsonl"),
            "retired",
        );

        let outcome = save(root, dir, DEFAULT_KEEP).unwrap();
        let tree = outcome.path.join(TREE_SUBDIR);
        assert!(tree.join("src/main.rs").exists());
        assert!(tree.join(".RaymanCodingSkill/goals/g1.json").exists());
        assert!(tree.join(".RaymanCodingSkill/pending.json").exists());
        assert!(tree.join(".RaymanCodingSkill/context/index.json").exists());
        assert!(
            tree.join(".RaymanCodingSkill/context/project_map.json")
                .exists()
        );
        assert!(tree.join(".RaymanCodingSkill/autosave.json").exists());
        // gitignore 命中、vendor 目录、易变 tmp 和已退役状态都不应进快照。
        assert!(!tree.join("ignored.txt").exists());
        assert!(!tree.join("node_modules/pkg/index.js").exists());
        assert!(!tree.join(".RaymanCodingSkill/tmp/scratch.txt").exists());
        assert!(
            !tree
                .join(".RaymanCodingSkill/regression/history.jsonl")
                .exists()
        );

        fs::remove_file(root.join("src/main.rs")).unwrap();
        let restored = restore(root, dir, None).unwrap();
        assert!(restored.restored >= 2);
        assert_eq!(
            fs::read_to_string(root.join("src/main.rs")).unwrap(),
            "fn main() {}"
        );
    }

    #[test]
    fn partial_save_is_not_latest_and_does_not_prune_last_complete_snapshot() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(&root.join("a.txt"), "a");
        let complete = save(root, Some(store.path()), 1).unwrap();

        // goals 应为目录；用同名普通文件模拟状态遍历失败，不依赖权限模型。
        write(&root.join(".RaymanCodingSkill/goals"), "not a directory");
        assert!(save(root, Some(store.path()), 1).is_err());

        let checkpoints = list(root, Some(store.path())).unwrap();
        assert!(
            checkpoints
                .iter()
                .any(|checkpoint| checkpoint.status == SnapshotStatus::Partial)
        );
        let latest_complete = latest(root, Some(store.path())).unwrap().unwrap();
        assert_eq!(latest_complete.id, complete.id);
        assert!(
            latest_complete.path.exists(),
            "last complete snapshot must survive"
        );
    }

    #[test]
    fn verify_rejects_tampering_before_restore_writes_any_file() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(&root.join("a.txt"), "a");
        write(&root.join("b.txt"), "b");
        let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();
        fs::write(saved.path.join(TREE_SUBDIR).join("b.txt"), "tampered").unwrap();

        fs::remove_file(root.join("a.txt")).unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        assert!(verify_snapshot(&saved.path).is_err());
        assert!(restore(root, Some(store.path()), Some(&saved.id)).is_err());
        assert!(
            !root.join("a.txt").exists(),
            "restore must verify before its first write"
        );
        assert!(!root.join("b.txt").exists());
    }

    #[test]
    fn prune_only_removes_verified_complete_snapshots() {
        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(&root.join("a.txt"), "a");
        let first = save(root, Some(store.path()), 10).unwrap();
        write(&root.join("a.txt"), "b");
        let second = save(root, Some(store.path()), 10).unwrap();
        write(&root.join("a.txt"), "c");
        let third = save(root, Some(store.path()), 10).unwrap();

        let ws_dir = workspace_dir(root, Some(store.path())).unwrap();
        assert_eq!(prune(&ws_dir, 2).unwrap(), 1);
        assert!(!first.path.exists());
        assert!(second.path.exists());
        assert!(third.path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn save_and_restore_reject_symlink_traversal() {
        use std::os::unix::fs::symlink;

        let ws = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = ws.path();
        write(&root.join("nested/link/a.txt"), "a");
        let saved = save(root, Some(store.path()), DEFAULT_KEEP).unwrap();

        // 恢复目标的父目录若是 symlink，绝不能把内容写到工作区外。
        fs::remove_file(root.join("nested/link/a.txt")).unwrap();
        fs::remove_dir(root.join("nested/link")).unwrap();
        symlink(outside.path(), root.join("nested/link")).unwrap();
        // manifest 仍完整有效，目的路径本身才是被替换的攻击面。
        assert!(verify_snapshot(&saved.path).is_ok());
        assert!(restore(root, Some(store.path()), Some(&saved.id)).is_err());
        assert!(!outside.path().join("a.txt").exists());

        // 状态白名单中的 symlink 也会使保存 fail-closed，而不是跟随链接抓取外部文件。
        let second = tempfile::tempdir().unwrap();
        write(&second.path().join("source.txt"), "x");
        write(&outside.path().join("pending.json"), "outside");
        fs::create_dir_all(second.path().join(".RaymanCodingSkill")).unwrap();
        symlink(
            outside.path().join("pending.json"),
            second.path().join(".RaymanCodingSkill/pending.json"),
        )
        .unwrap();
        assert!(save(second.path(), Some(store.path()), DEFAULT_KEEP).is_err());

        // The checkpoint root itself must not be reached through a symlinked
        // parent, even when the final directory resolves to a real directory.
        let redirected_parent = store.path().join("redirected");
        symlink(outside.path(), &redirected_parent).unwrap();
        assert!(
            save(
                root,
                Some(&redirected_parent.join("checkpoints")),
                DEFAULT_KEEP
            )
            .is_err()
        );
    }
}
