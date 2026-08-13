//! 工作区上下文索引：单次遍历 + 每文件指纹缓存。
//!
//! `refresh` 对每个文件重算内容摘要，并在遍历或读取失败时拒绝写出索引；缓存仅用于
//! 报告复用统计，不能作为跳过内容证明的依据。

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::file_io::is_link_or_reparse;
use crate::file_io::{read_json, write_json};
use crate::hash::{sha256_bytes, sha256_file};
use crate::pathfmt::display_path;
use crate::state_paths;
use crate::timefmt::now_iso;
use crate::walk::{relative_key, workspace_files_checked};

#[cfg(test)]
const INDEX_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/index.json";
const INDEX_STATE_RELATIVE: &str = "context/index.json";
pub const CONTEXT_SCHEMA_VERSION: u32 = 2;
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "py", "go", "java", "cs", "cpp", "c", "h", "hpp", "rb", "php",
    "swift", "kt", "scala",
];
const TEST_MARKERS: &[&str] = &["test", "tests", "spec"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub mtime_ns: u128,
    pub sha256: String,
    pub kind: String,
    pub lines: usize,
    #[serde(default)]
    pub symbols: Vec<Symbol>,
    /// 本进程从不写入这个字段：`build_entry` 在读取/哈希失败时直接返回 `Err`，
    /// `refresh` 把错误向上抛，索引根本不会落盘。字段只用于**读取**外部写入或
    /// 被篡改的索引文件——发现它有值就说明那份索引不可信，必须拒绝。
    #[serde(default)]
    pub read_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextIndex {
    #[serde(default)]
    pub schema_version: u32,
    pub generated_at: String,
    pub workspace: String,
    #[serde(default)]
    pub workspace_identity: String,
    pub files: Vec<FileEntry>,
}

/// 一次刷新的统计，用来向用户报告到底做了多少实际工作。
#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub total: usize,
    pub reused: usize,
    pub rehashed: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

/// 相对当前工作区状态的新鲜度，stat-only 计算，不做整树哈希。
#[derive(Debug, Clone, Serialize)]
pub struct FreshnessReport {
    pub status: String, // ready | stale | missing | incomplete
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
    pub errors: Vec<String>,
}

fn index_path(root: &Path, create_parents: bool) -> Result<std::path::PathBuf> {
    state_paths::managed_state_file(root, Path::new(INDEX_STATE_RELATIVE), create_parents)
}

pub fn workspace_identity(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    workspace_identity_from_canonical_root(&canonical)
}

/// Hash a root already canonicalized by a caller that owns a stable capture.
/// This avoids reopening that path in capture-only readiness decisions.
pub(crate) fn workspace_identity_from_canonical_root(root: &Path) -> String {
    sha256_bytes(root.to_string_lossy().as_bytes())
}

pub fn load(root: &Path) -> Result<Option<ContextIndex>> {
    read_json::<ContextIndex>(&index_path(root, false)?)
}

fn mtime_ns(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// 载入缓存索引；只有缺失返回 `None`，损坏或 I/O 错误必须向上传播。
fn load_cached(root: &Path) -> Result<Option<ContextIndex>> {
    // 只有文件确实不存在才视为首次运行。损坏、权限或其它 I/O 错误
    // 必须继续向上传递，避免 refresh 静默覆盖取证状态。
    read_json::<ContextIndex>(&index_path(root, false)?)
}

/// 按工作区相对路径对文件分类；只吃相对路径，避免祖先目录含 "test" 造成整清单误判。
fn classify(rel: &str, extension: &str) -> String {
    let in_test_dir = rel
        .split('/')
        .any(|component| component == "test" || component == "tests" || component == "__tests__");
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let is_source = SOURCE_EXTENSIONS.contains(&extension);
    let name_marks_test = is_source && file_name_marks_test(file_name);
    if is_source && (in_test_dir || name_marks_test) {
        "test".into()
    } else if ["md", "mdx", "rst", "txt"].contains(&extension) {
        "docs".into()
    } else if ["yaml", "yml", "json", "toml", "ini", "env"].contains(&extension) {
        "config".into()
    } else if ["sh", "ps1", "bat", "cmd"].contains(&extension) {
        "script".into()
    } else if is_source {
        "source".into()
    } else {
        "asset".into()
    }
}

/// 文件名级测试判定：整词/词缀匹配 TEST_MARKERS，不用裸子串——
/// latest.rs、inspect.rs、attest.rs 都含 "test"/"spec" 子串但不是测试。
fn file_name_marks_test(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or(&lower);
    TEST_MARKERS.iter().any(|marker| {
        stem == *marker
            || stem.starts_with(&format!("{marker}_"))
            || stem.ends_with(&format!("_{marker}"))
            || lower.contains(&format!(".{marker}."))
    })
}

fn extract_symbols(text: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim_start();
        if line.starts_with("//") || line.starts_with('#') || line.starts_with('*') {
            continue;
        }
        if let Some(route) = between(line, ".route(\"", "\"") {
            symbols.push(Symbol {
                name: route,
                kind: "route".into(),
                line: index + 1,
            });
            continue;
        }
        if let Some(name) = reexport_name(line) {
            symbols.push(Symbol {
                name,
                kind: "reexport".into(),
                line: index + 1,
            });
            continue;
        }
        let mut rest = line;
        // 可见性前缀：pub、pub(crate)、pub(super)、pub(in path) 统一剥掉。
        if let Some(after) = rest.strip_prefix("pub ") {
            rest = after;
        } else if let Some(after) = rest.strip_prefix("pub(")
            && let Some(close) = after.find(')')
        {
            rest = after[close + 1..].trim_start();
        }
        for prefix in ["async ", "unsafe ", "default "] {
            if let Some(stripped) = rest.strip_prefix(prefix) {
                rest = stripped;
            }
        }
        for (prefix, kind) in [
            ("fn ", "function"),
            ("struct ", "type"),
            ("enum ", "type"),
            ("trait ", "type"),
            ("mod ", "module"),
            ("class ", "type"),
            ("def ", "function"),
        ] {
            if let Some(tail) = rest.strip_prefix(prefix) {
                let name = tail
                    .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() {
                    symbols.push(Symbol {
                        name,
                        kind: kind.into(),
                        line: index + 1,
                    });
                }
                break;
            }
        }
    }
    symbols
}

fn reexport_name(line: &str) -> Option<String> {
    let tail = line.strip_prefix("pub use ")?;
    let tail = tail.trim_end_matches(';').trim();
    let name = tail
        .rsplit("::")
        .next()
        .unwrap_or(tail)
        .trim_matches(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'));
    (!name.is_empty()).then(|| name.to_string())
}

fn between(text: &str, start: &str, end: &str) -> Option<String> {
    let begin = text.find(start)? + start.len();
    let rest = &text[begin..];
    let stop = rest.find(end)?;
    Some(rest[..stop].to_string())
}

/// 超过此大小的文件只做流式 hash 与流式行数统计，不进内存做符号解析。
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

/// Count lines without holding the file in memory. Matches `str::lines()`:
/// a trailing newline does not add an empty final line.
fn count_lines_streaming(path: &Path) -> Result<usize> {
    use std::io::{BufRead, BufReader};

    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut buffer = Vec::new();
    let mut lines = 0usize;
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            break;
        }
        lines += 1;
    }
    Ok(lines)
}

fn build_entry(root: &Path, path: &Path, size: u64, mtime: u128) -> Result<FileEntry> {
    ensure_source_file(root, path)?;
    let rel = relative_key(root, path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 读取或哈希失败不能归约为空内容：refresh 必须拒绝写出可能被误判为完整的索引。
    let (bytes, streamed_hash, streamed_lines) = if size > MAX_TEXT_BYTES {
        let hash = sha256_file(path)
            .with_context(|| format!("上下文索引无法哈希文件: {}", display_path(path)))?;
        // Reporting 0 lines here inverted the `large_file` quality gate: the
        // rule blocks above a line threshold, so the very largest files were
        // the only ones it could never fire on. Line count is streamed instead;
        // symbols stay empty because that needs the whole text in memory.
        let lines = count_lines_streaming(path)
            .with_context(|| format!("上下文索引无法统计文件行数: {}", display_path(path)))?;
        (Vec::new(), Some(hash), Some(lines))
    } else {
        let bytes = std::fs::read(path)
            .with_context(|| format!("上下文索引无法读取文件: {}", display_path(path)))?;
        (bytes, None, None)
    };
    let after = std::fs::metadata(path)
        .with_context(|| format!("上下文索引无法复查文件元数据: {}", display_path(path)))?;
    ensure_source_file(root, path)?;
    if after.len() != size || mtime_ns(&after) != mtime {
        bail!("上下文索引读取期间文件发生变化: {}", display_path(path));
    }
    let sha256 = streamed_hash.unwrap_or_else(|| sha256_bytes(&bytes));
    let text = String::from_utf8_lossy(&bytes);
    let lines = streamed_lines.unwrap_or_else(|| text.lines().count());
    let kind = classify(&rel, &extension);
    let symbols = if kind == "source" || kind == "test" {
        extract_symbols(&text)
    } else {
        Vec::new()
    };
    Ok(FileEntry {
        path: rel,
        size,
        mtime_ns: mtime,
        sha256,
        kind,
        lines,
        symbols,
        read_error: None,
    })
}

/// Build the exact context entry for bytes already captured through a stable
/// no-follow handle. Readiness uses this to project one workspace capture into
/// context freshness, assets, maps, and goal baselines without another walk.
pub(crate) fn build_entry_from_captured_bytes(
    root: &Path,
    path: &Path,
    size: u64,
    mtime: u128,
    bytes: &[u8],
) -> Result<FileEntry> {
    if bytes.len() as u64 != size {
        bail!(
            "captured context file size mismatch: {} ({} != {})",
            display_path(path),
            bytes.len(),
            size
        );
    }
    let rel = relative_key(root, path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = classify(&rel, &extension);
    let lines = if bytes.is_empty() {
        0
    } else {
        bytes.iter().filter(|byte| **byte == b'\n').count()
            + usize::from(bytes.last() != Some(&b'\n'))
    };
    let symbols = if (kind == "source" || kind == "test") && size <= MAX_TEXT_BYTES {
        extract_symbols(&String::from_utf8_lossy(bytes))
    } else {
        Vec::new()
    };
    Ok(FileEntry {
        path: rel,
        size,
        mtime_ns: mtime,
        sha256: sha256_bytes(bytes),
        kind,
        lines,
        symbols,
        read_error: None,
    })
}

fn incomplete_freshness(error: impl Into<String>) -> FreshnessReport {
    FreshnessReport {
        status: "incomplete".into(),
        changed: Vec::new(),
        removed: Vec::new(),
        added: Vec::new(),
        errors: vec![error.into()],
    }
}

/// Validate a cached index against caller-captured complete entries. The exact
/// cached object is returned only when every persisted and derived field
/// matches; callers must not reopen the cache after this decision.
pub(crate) fn verify_index_from_capture(
    root: &Path,
    cached: Result<Option<ContextIndex>>,
    current: &[FileEntry],
) -> (FreshnessReport, Option<ContextIndex>) {
    let index = match cached {
        Ok(Some(index)) => index,
        Ok(None) => {
            return (
                FreshnessReport {
                    status: "missing".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors: Vec::new(),
                },
                None,
            );
        }
        Err(error) => {
            return (
                incomplete_freshness(format!("无法安全读取 context 状态: {error:#}")),
                None,
            );
        }
    };
    if index.schema_version != CONTEXT_SCHEMA_VERSION
        || index.workspace_identity != workspace_identity(root)
    {
        return (
            FreshnessReport {
                status: "missing".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec!["context schema/workspace identity 不匹配".into()],
            },
            None,
        );
    }

    let mut errors = Vec::new();
    let mut cached_by_path = BTreeMap::new();
    for entry in &index.files {
        if cached_by_path.insert(entry.path.as_str(), entry).is_some() {
            errors.push(format!("context 索引包含重复路径: {}", entry.path));
        }
        if entry.read_error.is_some() {
            errors.push(format!("context 索引包含读取失败条目: {}", entry.path));
        }
    }
    let mut current_by_path = BTreeMap::new();
    for entry in current {
        if current_by_path.insert(entry.path.as_str(), entry).is_some() {
            errors.push(format!(
                "当前 workspace capture 包含重复路径: {}",
                entry.path
            ));
        }
    }

    let mut changed = Vec::new();
    let mut added = Vec::new();
    for (path, entry) in &current_by_path {
        match cached_by_path.get(path) {
            Some(cached) if **cached == **entry => {}
            Some(_) => changed.push((*path).to_string()),
            None => added.push((*path).to_string()),
        }
    }
    let mut removed = cached_by_path
        .keys()
        .filter(|path| !current_by_path.contains_key(**path))
        .map(|path| (*path).to_string())
        .collect::<Vec<_>>();
    changed.sort();
    added.sort();
    removed.sort();
    let status = if !errors.is_empty() {
        "incomplete"
    } else if changed.is_empty() && added.is_empty() && removed.is_empty() {
        "ready"
    } else {
        "stale"
    };
    let ready = status == "ready";
    (
        FreshnessReport {
            status: status.into(),
            changed,
            removed,
            added,
            errors,
        },
        ready.then_some(index),
    )
}

/// Publish a context index from an already complete workspace capture. This is
/// the readiness `--refresh-context` path: refresh itself is the first decision
/// walk rather than an extra fifth walk before a two-round finish.
pub(crate) fn refresh_from_capture(
    root: &Path,
    files: Vec<FileEntry>,
    captured_cached: Result<Option<ContextIndex>>,
) -> Result<(ContextIndex, RefreshReport)> {
    let identity = workspace_identity(root);
    let cached = captured_cached?
        .filter(|index| {
            index.schema_version == CONTEXT_SCHEMA_VERSION && index.workspace_identity == identity
        })
        .map(|index| {
            index
                .files
                .into_iter()
                .map(|entry| (entry.path.clone(), entry))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let current_paths = files
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let reused = files
        .iter()
        .filter(|entry| {
            cached.get(&entry.path).is_some_and(|old| {
                old.sha256 == entry.sha256 && old.read_error.is_none() && entry.read_error.is_none()
            })
        })
        .count();
    let removed = cached
        .keys()
        .filter(|path| !current_paths.contains(path.as_str()))
        .count();
    let report = RefreshReport {
        total: files.len(),
        reused,
        rehashed: files.len().saturating_sub(reused),
        removed,
        errors: Vec::new(),
    };
    let index = ContextIndex {
        schema_version: CONTEXT_SCHEMA_VERSION,
        generated_at: now_iso(),
        workspace: display_path(root),
        workspace_identity: identity,
        files,
    };
    write_json(&index_path(root, true)?, &index)?;
    Ok((index, report))
}

/// 在读取/哈希前后确认候选文件仍是工作区内的普通文件。遍历器不跟随
/// 链接，但路径可能在遍历完成后被替换；拒绝链接/reparse，避免把工作区外
/// 内容纳入上下文索引。
pub(crate) fn ensure_source_file(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "上下文索引文件不属于工作区: {} under {}",
            display_path(path),
            display_path(root)
        )
    })?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("上下文索引拒绝不安全相对路径: {}", display_path(path));
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("上下文索引拒绝不安全相对路径: {}", display_path(path));
        };
        current.push(name);
        let metadata = std::fs::symlink_metadata(&current)
            .with_context(|| format!("上下文索引无法读取文件元数据: {}", display_path(&current)))?;
        if is_link_or_reparse(&metadata) {
            bail!(
                "上下文索引拒绝链接/reparse 路径: {}",
                display_path(&current)
            );
        }
        if index + 1 == components.len() {
            if !metadata.file_type().is_file() {
                bail!("上下文索引拒绝非普通文件: {}", display_path(&current));
            }
        } else if !metadata.file_type().is_dir() {
            bail!("上下文索引路径组件不是目录: {}", display_path(&current));
        }
    }
    let workspace = root
        .canonicalize()
        .with_context(|| format!("无法规范化工作区根: {}", display_path(root)))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("无法规范化上下文文件: {}", display_path(path)))?;
    if !canonical.starts_with(&workspace) {
        bail!(
            "上下文索引文件逃逸工作区: {} -> {}",
            display_path(path),
            display_path(&canonical)
        );
    }
    Ok(())
}

/// Read a file named by an already strongly verified index and bind the exact
/// bytes consumed by a downstream map to that same [`FileEntry`].  This closes
/// the gap where a caller validated the cache, then reopened a changed source or
/// followed a newly substituted parent link while deriving trusted conclusions.
pub(crate) fn read_verified_file(root: &Path, entry: &FileEntry) -> Result<Vec<u8>> {
    let relative = Path::new(&entry.path);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("上下文索引条目含不安全路径: {}", entry.path);
    }
    let path = root.join(relative);
    ensure_source_file(root, &path)?;
    let before = std::fs::symlink_metadata(&path)
        .with_context(|| format!("无法读取已验证上下文文件元数据: {}", entry.path))?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("无法读取已验证上下文文件: {}", entry.path))?;
    ensure_source_file(root, &path)?;
    let after = std::fs::symlink_metadata(&path)
        .with_context(|| format!("无法复查已验证上下文文件元数据: {}", entry.path))?;
    let actual_hash = sha256_bytes(&bytes);
    if before.len() != after.len()
        || bytes.len() as u64 != entry.size
        || actual_hash != entry.sha256
    {
        bail!(
            "上下文文件在索引验证后发生变化: {} (size {} != {} or sha256 {} != {})",
            entry.path,
            bytes.len(),
            entry.size,
            actual_hash,
            entry.sha256
        );
    }
    Ok(bytes)
}

/// 刷新索引：复用未变文件的指纹与符号，只重算变更文件。
pub fn refresh(root: &Path) -> Result<(ContextIndex, RefreshReport)> {
    let identity = workspace_identity(root);
    let cached = load_cached(root)?
        .filter(|index| {
            index.schema_version == CONTEXT_SCHEMA_VERSION && index.workspace_identity == identity
        })
        .map(|index| {
            index
                .files
                .into_iter()
                .map(|entry| (entry.path.clone(), entry))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let mut files = Vec::new();
    let mut reused = 0usize;
    let mut rehashed = 0usize;
    let mut present = std::collections::BTreeSet::new();

    for path in workspace_files_checked(root)? {
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("上下文索引无法读取文件元数据: {}", display_path(&path)))?;
        let size = metadata.len();
        let mtime = mtime_ns(&metadata);
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        // 标准 refresh 为内容证明：即使攻击者保持 size/mtime 不变也必须重算 hash。
        // cached entry 仅用于统计同内容复用，不作为跳过读取的依据。
        let entry = build_entry(root, &path, size, mtime)?;
        if cached.get(&rel).is_some_and(|old| {
            old.sha256 == entry.sha256 && old.read_error.is_none() && entry.read_error.is_none()
        }) {
            reused += 1;
        } else {
            rehashed += 1;
        }
        files.push(entry);
    }

    let removed = cached.keys().filter(|key| !present.contains(*key)).count();
    let report = RefreshReport {
        total: files.len(),
        reused,
        rehashed,
        removed,
        errors: Vec::new(),
    };
    let index = ContextIndex {
        schema_version: CONTEXT_SCHEMA_VERSION,
        generated_at: now_iso(),
        workspace: display_path(root),
        workspace_identity: identity,
        files,
    };
    write_json(&index_path(root, true)?, &index)?;
    Ok((index, report))
}

/// 只做 stat-only 新鲜度检查，不重建、不整树哈希。缓存损坏或缺失时报 `missing`。
pub fn freshness(root: &Path) -> FreshnessReport {
    let cached = match load_cached(root) {
        Ok(Some(cached)) => cached,
        Ok(None) => {
            return FreshnessReport {
                status: "missing".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: Vec::new(),
            };
        }
        Err(error) => {
            return FreshnessReport {
                status: "incomplete".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec![format!("无法安全读取 context 状态: {error:#}")],
            };
        }
    };
    if cached.schema_version != CONTEXT_SCHEMA_VERSION
        || cached.workspace_identity != workspace_identity(root)
    {
        return FreshnessReport {
            status: "missing".into(),
            changed: Vec::new(),
            removed: Vec::new(),
            added: Vec::new(),
            errors: vec!["context schema/workspace identity 不匹配".into()],
        };
    }
    let cached_map: BTreeMap<_, _> = cached
        .files
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();

    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut present = std::collections::BTreeSet::new();
    let mut errors = Vec::new();
    let paths = match workspace_files_checked(root) {
        Ok(paths) => paths,
        Err(error) => {
            return FreshnessReport {
                status: "incomplete".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec![format!("工作区遍历失败: {error:#}")],
            };
        }
    };
    for path in paths {
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(format!("{}: {error}", display_path(&path)));
                continue;
            }
        };
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        match cached_map.get(&rel) {
            Some(entry) => {
                if entry.read_error.is_some()
                    || entry.size != metadata.len()
                    || entry.mtime_ns != mtime_ns(&metadata)
                {
                    changed.push(rel);
                }
            }
            None => added.push(rel),
        }
    }
    let removed: Vec<String> = cached_map
        .keys()
        .filter(|key| !present.contains(*key))
        .cloned()
        .collect();

    let status =
        if !errors.is_empty() || cached_map.values().any(|entry| entry.read_error.is_some()) {
            "incomplete"
        } else if changed.is_empty() && added.is_empty() && removed.is_empty() {
            "ready"
        } else {
            "stale"
        };
    FreshnessReport {
        status: status.into(),
        changed,
        removed,
        added,
        errors,
    }
}

/// 强新鲜度：用于 map、standard/release 检查。它从当前工作区遍历结果重新构造
/// 每个完整 [`FileEntry`]，同时复核内容摘要和 kind/lines/symbols 等派生字段。
///
/// 缓存里的 `path` 只是待比较的数据，绝不能反过来驱动文件读取：状态文件可被
/// 手工编辑，直接 `root.join(cached.path)` 会形成工作区外读取，也会让伪造的派生
/// 字段在 hash 不变时进入 map/quality 信任链。
/// Return the exact cached index object that passed the strong content check.
///
/// Callers that derive trusted conclusions from the index must use this API
/// instead of checking [`strong_freshness`] and then reopening the state file:
/// the latter creates a validate-then-reopen window in which a different cache
/// can be substituted after validation.
pub fn verified_index(root: &Path) -> Result<ContextIndex> {
    let (report, index) = strong_freshness_with_index(root);
    if report.status != "ready" {
        let mut details = Vec::new();
        if !report.changed.is_empty() {
            details.push(format!("changed={:?}", report.changed));
        }
        if !report.added.is_empty() {
            details.push(format!("added={:?}", report.added));
        }
        if !report.removed.is_empty() {
            details.push(format!("removed={:?}", report.removed));
        }
        if !report.errors.is_empty() {
            details.push(format!("errors={:?}", report.errors));
        }
        bail!(
            "上下文索引不是 ready（当前: {}）。先运行 `rayman context refresh`。{}",
            report.status,
            if details.is_empty() {
                String::new()
            } else {
                format!(" {}", details.join(" "))
            }
        );
    }
    index.ok_or_else(|| anyhow::anyhow!("上下文索引缺失。先运行 `rayman context refresh`。"))
}

pub fn strong_freshness(root: &Path) -> FreshnessReport {
    strong_freshness_with_index(root).0
}

fn strong_freshness_with_index(root: &Path) -> (FreshnessReport, Option<ContextIndex>) {
    let index = match load_cached(root) {
        Ok(Some(index)) => index,
        Ok(None) => {
            return (
                FreshnessReport {
                    status: "missing".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors: Vec::new(),
                },
                None,
            );
        }
        Err(error) => {
            return (
                FreshnessReport {
                    status: "incomplete".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors: vec![format!("无法安全读取 context 状态: {error:#}")],
                },
                None,
            );
        }
    };
    if index.schema_version != CONTEXT_SCHEMA_VERSION
        || index.workspace_identity != workspace_identity(root)
    {
        return (
            FreshnessReport {
                status: "missing".into(),
                changed: Vec::new(),
                removed: Vec::new(),
                added: Vec::new(),
                errors: vec!["context schema/workspace identity 不匹配".into()],
            },
            None,
        );
    }

    let mut errors = Vec::new();
    let mut cached = BTreeMap::new();
    for entry in &index.files {
        if cached.insert(entry.path.as_str(), entry).is_some() {
            errors.push(format!("context 索引包含重复路径: {}", entry.path));
        }
        if entry.read_error.is_some() {
            errors.push(format!("context 索引包含读取失败条目: {}", entry.path));
        }
    }
    let paths = match workspace_files_checked(root) {
        Ok(paths) => paths,
        Err(error) => {
            errors.push(format!("工作区遍历失败: {error:#}"));
            return (
                FreshnessReport {
                    status: "incomplete".into(),
                    changed: Vec::new(),
                    removed: Vec::new(),
                    added: Vec::new(),
                    errors,
                },
                None,
            );
        }
    };
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut present = std::collections::BTreeSet::new();
    for path in paths {
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                errors.push(format!("{}: {error}", display_path(&path)));
                continue;
            }
        };
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        match build_entry(root, &path, metadata.len(), mtime_ns(&metadata)) {
            Ok(current)
                if cached
                    .get(rel.as_str())
                    .is_some_and(|entry| **entry == current) => {}
            Ok(_) if cached.contains_key(rel.as_str()) => changed.push(rel),
            Ok(_) => added.push(rel),
            Err(error) => errors.push(format!("{}: {error:#}", display_path(&path))),
        }
    }
    let removed = cached
        .keys()
        .filter(|path| !present.contains(**path))
        .map(|path| (*path).to_string())
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    added.sort();
    added.dedup();
    let status = if !errors.is_empty() {
        "incomplete"
    } else if changed.is_empty() && added.is_empty() && removed.is_empty() {
        "ready"
    } else {
        "stale"
    };
    let ready = status == "ready";
    // `cached` borrows `index`; end that borrow before returning the exact
    // validated object to the caller.
    drop(cached);
    (
        FreshnessReport {
            status: status.into(),
            changed,
            removed,
            added,
            errors,
        },
        ready.then_some(index),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// Files above `MAX_TEXT_BYTES` are hashed without being read into memory.
    /// Reporting them as 0 lines inverted the `large_file` quality gate, which
    /// blocks above a line threshold: the very largest files were the only
    /// ones it could never fire on.
    #[test]
    fn oversized_files_report_their_real_line_count() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let line = "pub fn filler() {} // padding to exceed the in-memory limit\n";
        let repeats = (MAX_TEXT_BYTES as usize / line.len()) + 64;
        touch(&root.join("src/huge.rs"), &line.repeat(repeats));
        touch(&root.join("src/small.rs"), "pub fn a() {}\npub fn b() {}\n");

        let (index, _) = refresh(root).unwrap();
        let huge = index
            .files
            .iter()
            .find(|file| file.path == "src/huge.rs")
            .unwrap();
        assert!(huge.size > MAX_TEXT_BYTES, "fixture must exceed the limit");
        assert_eq!(huge.lines, repeats, "line count must be streamed, not zero");
        // Symbols still need the whole text in memory, so they stay empty.
        assert!(huge.symbols.is_empty());

        let small = index
            .files
            .iter()
            .find(|file| file.path == "src/small.rs")
            .unwrap();
        assert_eq!(small.lines, 2, "the in-memory path is unchanged");
    }

    #[test]
    fn refresh_reuses_unchanged_files_and_only_rehashes_changed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "pub fn a() {}");
        touch(&root.join("src/b.rs"), "pub fn b() {}");

        let (_, first) = refresh(root).unwrap();
        assert_eq!(first.total, 2);
        assert_eq!(first.rehashed, 2);
        assert_eq!(first.reused, 0);

        // 不改任何文件：第二次全部复用，零重算。
        let (_, second) = refresh(root).unwrap();
        assert_eq!(second.reused, 2, "未变文件应全部复用");
        assert_eq!(second.rehashed, 0, "不应重算未变文件");

        // 改一个文件：只有它被重算。
        touch(&root.join("src/a.rs"), "pub fn a() { /* changed */ }");
        let (_, third) = refresh(root).unwrap();
        assert_eq!(third.rehashed, 1);
        assert_eq!(third.reused, 1);
    }

    #[test]
    fn classify_uses_relative_path_and_extracts_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/main.rs"), "pub fn main() {}\nstruct Foo;");
        touch(&root.join("tests/it.rs"), "fn check() {}");
        touch(&root.join("README.md"), "# doc");

        let (index, _) = refresh(root).unwrap();
        let by_path: BTreeMap<_, _> = index
            .files
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        assert_eq!(by_path["src/main.rs"].kind, "source");
        assert_eq!(by_path["tests/it.rs"].kind, "test");
        assert_eq!(by_path["README.md"].kind, "docs");
        assert!(
            by_path["src/main.rs"]
                .symbols
                .iter()
                .any(|symbol| symbol.name == "main" && symbol.kind == "function")
        );
    }

    #[test]
    fn classify_extracts_reexport_symbols() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(
            &root.join("src/lib.rs"),
            "pub use crate::exports::display_path;\n",
        );

        let (index, _) = refresh(root).unwrap();
        let file = index
            .files
            .iter()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        assert!(
            file.symbols
                .iter()
                .any(|symbol| { symbol.name == "display_path" && symbol.kind == "reexport" })
        );
    }

    #[test]
    fn classify_does_not_treat_substring_names_as_tests() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // 含 test/spec 子串但不是测试的普通源文件。
        touch(&root.join("src/latest.rs"), "pub fn latest() {}");
        touch(&root.join("src/inspect.rs"), "pub fn inspect() {}");
        touch(&root.join("src/attest.rs"), "pub fn attest() {}");
        // 真正的测试命名习惯仍要识别。
        touch(&root.join("src/parser_test.rs"), "#[test]\nfn t() {}");
        touch(&root.join("src/test_utils.rs"), "pub fn helper() {}");
        touch(&root.join("src/app.spec.ts"), "it('x', () => {});");

        let (index, _) = refresh(root).unwrap();
        let kind = |path: &str| {
            index
                .files
                .iter()
                .find(|entry| entry.path == path)
                .unwrap()
                .kind
                .clone()
        };
        assert_eq!(kind("src/latest.rs"), "source");
        assert_eq!(kind("src/inspect.rs"), "source");
        assert_eq!(kind("src/attest.rs"), "source");
        assert_eq!(kind("src/parser_test.rs"), "test");
        assert_eq!(kind("src/test_utils.rs"), "test");
        assert_eq!(kind("src/app.spec.ts"), "test");
    }

    #[test]
    fn docs_inside_tests_directory_do_not_create_a_test_anchor() {
        assert_eq!(classify("tests/README.md", "md"), "docs");
        assert_eq!(classify("tests/case.rs", "rs"), "test");
    }

    #[test]
    fn extract_symbols_handles_scoped_visibility() {
        let symbols = extract_symbols(
            "pub(crate) fn helper() {}\npub(super) struct Inner;\npub(in crate::x) enum E {}\n",
        );
        let names: Vec<_> = symbols.iter().map(|symbol| symbol.name.as_str()).collect();
        assert!(names.contains(&"helper"));
        assert!(names.contains(&"Inner"));
        assert!(names.contains(&"E"));
    }

    #[test]
    fn refresh_tolerates_non_utf8_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "pub fn a() {}");
        // GBK 编码字节：非法 UTF-8，不得让 refresh 整体失败。
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/gbk.rs"), [0xD6u8, 0xD0, 0xCE, 0xC4, b'\n']).unwrap();

        let (index, _) = refresh(root).unwrap();
        assert!(index.files.iter().any(|entry| entry.path == "src/gbk.rs"));
    }

    #[test]
    fn freshness_is_missing_then_ready_then_stale() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "fn a() {}");
        assert_eq!(freshness(root).status, "missing");
        refresh(root).unwrap();
        assert_eq!(freshness(root).status, "ready");
        touch(&root.join("src/b.rs"), "fn b() {}");
        let report = freshness(root);
        assert_eq!(report.status, "stale");
        assert_eq!(report.added, vec!["src/b.rs".to_string()]);
    }

    #[test]
    fn strong_freshness_detects_same_stat_content_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("src/a.rs");
        touch(&path, "fn alpha() {}\n");
        refresh(root).unwrap();
        let original = fs::metadata(&path).unwrap().modified().unwrap();
        // Same byte length, then restore mtime: stat-only UI cannot prove identity.
        fs::write(&path, "fn bravo() {}\n").unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(original))
            .unwrap();
        assert_eq!(freshness(root).status, "ready");
        assert_eq!(strong_freshness(root).status, "stale");
    }

    #[test]
    fn strong_freshness_rejects_forged_derived_fields() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        let (mut index, _) = refresh(root).unwrap();
        let entry = index
            .files
            .iter_mut()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        entry.kind = "asset".into();
        entry.lines = 0;
        entry.symbols.clear();
        write_json(&root.join(INDEX_RELATIVE_PATH), &index).unwrap();

        let report = strong_freshness(root);
        assert_eq!(report.status, "stale", "errors={:?}", report.errors);
        assert_eq!(report.changed, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn strong_freshness_never_reads_paths_supplied_only_by_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        let (mut index, _) = refresh(root).unwrap();
        index.files.push(FileEntry {
            path: "../outside-do-not-read.rs".into(),
            size: 1,
            mtime_ns: 1,
            sha256: "0".repeat(64),
            kind: "source".into(),
            lines: 1,
            symbols: Vec::new(),
            read_error: None,
        });
        write_json(&root.join(INDEX_RELATIVE_PATH), &index).unwrap();

        let report = strong_freshness(root);
        assert_eq!(report.status, "stale");
        assert!(report.errors.is_empty(), "errors={:?}", report.errors);
        assert_eq!(
            report.removed,
            vec!["../outside-do-not-read.rs".to_string()]
        );
    }

    #[test]
    fn strong_freshness_rejects_duplicate_cached_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        let (mut index, _) = refresh(root).unwrap();
        index.files.push(index.files[0].clone());
        write_json(&root.join(INDEX_RELATIVE_PATH), &index).unwrap();

        let report = strong_freshness(root);
        assert_eq!(report.status, "incomplete");
        assert!(
            report.errors.iter().any(|error| error.contains("重复路径")),
            "errors={:?}",
            report.errors
        );
    }

    #[test]
    fn verified_index_is_the_same_object_that_passed_validation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        refresh(root).unwrap();

        let validated = verified_index(root).unwrap();
        let mut replacement = load(root).unwrap().unwrap();
        replacement.files[0].path = "../outside.rs".into();
        write_json(&root.join(INDEX_RELATIVE_PATH), &replacement).unwrap();

        assert_eq!(validated.files.len(), 1);
        assert_eq!(validated.files[0].path, "src/lib.rs");
        assert_ne!(strong_freshness(root).status, "ready");
    }

    #[test]
    fn verified_index_stale_error_names_the_changed_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n");
        refresh(root).unwrap();
        touch(&root.join("src/new.rs"), "pub fn new_item() {}\n");

        let error = verified_index(root).unwrap_err().to_string();
        assert!(error.contains("added"), "{error}");
        assert!(error.contains("src/new.rs"), "{error}");
    }

    #[test]
    fn verified_file_read_rejects_post_validation_byte_drift() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn alpha() {}\n");
        let index = refresh(root).unwrap().0;
        let entry = index
            .files
            .iter()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        touch(&root.join("src/lib.rs"), "pub fn bravo() {}\n");

        let error = read_verified_file(root, entry).unwrap_err().to_string();
        assert!(error.contains("索引验证后发生变化"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn verified_file_read_rejects_a_substituted_parent_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/lib.rs"), "pub fn answer() {}\n");
        let index = refresh(root).unwrap().0;
        let entry = index
            .files
            .iter()
            .find(|entry| entry.path == "src/lib.rs")
            .unwrap();
        fs::remove_file(root.join("src/lib.rs")).unwrap();
        fs::remove_dir(root.join("src")).unwrap();
        touch(&outside.path().join("lib.rs"), "pub fn answer() {}\n");
        symlink(outside.path(), root.join("src")).unwrap();

        let error = read_verified_file(root, entry).unwrap_err().to_string();
        assert!(error.contains("链接/reparse"), "{error}");
    }

    #[test]
    fn corrupt_cache_is_preserved_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "fn a() {}");
        refresh(root).unwrap();
        fs::write(root.join(INDEX_RELATIVE_PATH), "{ corrupt").unwrap();
        // 损坏状态不能被静默降级并覆盖；只读状态报告为 incomplete，刷新返回错误。
        assert_eq!(freshness(root).status, "incomplete");
        assert!(refresh(root).is_err());
        assert_eq!(
            fs::read_to_string(root.join(INDEX_RELATIVE_PATH)).unwrap(),
            "{ corrupt"
        );
    }

    #[test]
    fn refresh_refuses_incomplete_workspace_walk() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-workspace");
        assert!(refresh(&missing).is_err());
    }

    #[test]
    fn build_entry_refuses_a_non_file_read_target() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let target = root.join("not-a-file");
        fs::create_dir_all(&target).unwrap();
        let metadata = fs::metadata(&target).unwrap();
        assert!(build_entry(root, &target, metadata.len(), mtime_ns(&metadata)).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn build_entry_refuses_a_symlink_to_a_file_outside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("secret.rs");
        fs::write(&outside_file, "fn secret() {}").unwrap();
        let target = workspace.path().join("secret.rs");
        symlink(&outside_file, &target).unwrap();
        let metadata = fs::metadata(&target).unwrap();
        assert!(
            build_entry(
                workspace.path(),
                &target,
                metadata.len(),
                mtime_ns(&metadata)
            )
            .is_err()
        );
    }
}
