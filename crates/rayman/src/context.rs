//! 工作区上下文索引：单次遍历 + 每文件指纹缓存。
//!
//! `refresh` 对每个文件重算内容摘要，并在遍历或读取失败时拒绝写出索引；缓存仅用于
//! 报告复用统计，不能作为跳过内容证明的依据。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::project_store::{
    display_path, now_iso, read_json, sha256_bytes, sha256_file, write_json,
};
use crate::state_paths;
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
    /// 读取/哈希失败不能伪装成空文件；当前索引因此为 incomplete。
    #[serde(default)]
    pub read_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn workspace_identity(root: &Path) -> String {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    sha256_bytes(canonical.to_string_lossy().as_bytes())
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

/// 超过此大小的文件只做流式 hash，不进内存做文本解析。
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

fn build_entry(root: &Path, path: &Path, size: u64, mtime: u128) -> Result<FileEntry> {
    ensure_source_file(root, path)?;
    let rel = relative_key(root, path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 读取或哈希失败不能归约为空内容：refresh 必须拒绝写出可能被误判为完整的索引。
    let (bytes, streamed_hash) = if size > MAX_TEXT_BYTES {
        let hash = sha256_file(path)
            .with_context(|| format!("上下文索引无法哈希文件: {}", display_path(path)))?;
        (Vec::new(), Some(hash))
    } else {
        let bytes = std::fs::read(path)
            .with_context(|| format!("上下文索引无法读取文件: {}", display_path(path)))?;
        (bytes, None)
    };
    let after = std::fs::metadata(path)
        .with_context(|| format!("上下文索引无法复查文件元数据: {}", display_path(path)))?;
    ensure_source_file(root, path)?;
    if after.len() != size || mtime_ns(&after) != mtime {
        bail!("上下文索引读取期间文件发生变化: {}", display_path(path));
    }
    let sha256 = streamed_hash.unwrap_or_else(|| sha256_bytes(&bytes));
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().count();
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

/// 在读取/哈希前后确认候选文件仍是工作区内的普通文件。遍历器不跟随
/// 链接，但路径可能在遍历完成后被替换；拒绝链接/reparse，避免把工作区外
/// 内容纳入上下文索引。
fn ensure_source_file(root: &Path, path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("上下文索引无法读取文件元数据: {}", display_path(path)))?;
    if is_link_or_reparse(&metadata) || !metadata.file_type().is_file() {
        bail!("上下文索引拒绝非普通文件: {}", display_path(path));
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

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
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

/// 强新鲜度：用于 map、standard/release 检查。它复核每个内容摘要，避免 size+mtime
/// 恰好相同或被保留时的 false-ready。
pub fn strong_freshness(root: &Path) -> FreshnessReport {
    let mut report = freshness(root);
    let index = match load_cached(root) {
        Ok(Some(index)) => index,
        Ok(None) => return report,
        Err(error) => {
            report
                .errors
                .push(format!("无法安全读取 context 状态: {error:#}"));
            report.status = "incomplete".into();
            return report;
        }
    };
    if report.status == "missing" {
        return report;
    }
    for entry in &index.files {
        let path = root.join(&entry.path);
        match sha256_file(&path) {
            Ok(hash) if hash == entry.sha256 && entry.read_error.is_none() => {}
            Ok(_) => report.changed.push(entry.path.clone()),
            Err(error) => report.errors.push(format!("{}: {error}", entry.path)),
        }
    }
    report.changed.sort();
    report.changed.dedup();
    report.status = if !report.errors.is_empty() {
        "incomplete".into()
    } else if report.changed.is_empty() && report.added.is_empty() && report.removed.is_empty() {
        "ready".into()
    } else {
        "stale".into()
    };
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
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
