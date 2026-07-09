//! 工作区上下文索引：单次遍历 + 每文件指纹缓存。
//!
//! 与旧实现的关键区别：`refresh` 用 stat-only 遍历，(size, mtime) 未变的文件直接复用缓存里的
//! sha/符号，**只重算变更文件**——修复“每次调用从头重建整份索引、缓存只用于事后报告 stale”的性能问题。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::project_store::{display_path, now_iso, read_json, sha256_file, write_json};
use crate::walk::{relative_key, workspace_files};

const INDEX_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/index.json";
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextIndex {
    pub generated_at: String,
    pub workspace: String,
    pub files: Vec<FileEntry>,
}

/// 一次刷新的统计，用来向用户报告到底做了多少实际工作。
#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub total: usize,
    pub reused: usize,
    pub rehashed: usize,
    pub removed: usize,
}

/// 相对当前工作区状态的新鲜度，stat-only 计算，不做整树哈希。
#[derive(Debug, Clone, Serialize)]
pub struct FreshnessReport {
    pub status: String, // ready | stale | missing
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub added: Vec<String>,
}

fn index_path(root: &Path) -> std::path::PathBuf {
    root.join(INDEX_RELATIVE_PATH)
}

pub fn load(root: &Path) -> Result<Option<ContextIndex>> {
    read_json::<ContextIndex>(&index_path(root))
}

fn mtime_ns(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

/// 载入缓存索引；缺失或**损坏**都返回 `None`（降级为重建，不让命令报错）。
fn load_cached(root: &Path) -> Option<ContextIndex> {
    // read_json 对损坏文件返回 Err；unwrap_or_default 把缺失(None)与损坏(Err)统一降级为 None。
    read_json::<ContextIndex>(&index_path(root)).unwrap_or_default()
}

/// 按工作区相对路径对文件分类；只吃相对路径，避免祖先目录含 "test" 造成整清单误判。
fn classify(rel: &str, extension: &str) -> String {
    let in_test_dir = rel
        .split('/')
        .any(|component| component == "test" || component == "tests");
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let is_source = SOURCE_EXTENSIONS.contains(&extension);
    let name_marks_test = is_source && TEST_MARKERS.iter().any(|marker| file_name.contains(marker));
    if in_test_dir || name_marks_test {
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
        for prefix in ["pub ", "async ", "unsafe ", "default "] {
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

fn build_entry(root: &Path, path: &Path, size: u64, mtime: u128) -> Result<FileEntry> {
    let rel = relative_key(root, path);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = std::fs::read_to_string(path).unwrap_or_default();
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
        sha256: sha256_file(path)?,
        kind,
        lines,
        symbols,
    })
}

/// 刷新索引：复用未变文件的指纹与符号，只重算变更文件。
pub fn refresh(root: &Path) -> Result<(ContextIndex, RefreshReport)> {
    let cached = load_cached(root)
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

    for path in workspace_files(root) {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let size = metadata.len();
        let mtime = mtime_ns(&metadata);
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        match cached.get(&rel) {
            Some(entry) if entry.size == size && entry.mtime_ns == mtime && mtime != 0 => {
                reused += 1;
                files.push(entry.clone());
            }
            _ => {
                rehashed += 1;
                files.push(build_entry(root, &path, size, mtime)?);
            }
        }
    }

    let removed = cached.keys().filter(|key| !present.contains(*key)).count();
    let report = RefreshReport {
        total: files.len(),
        reused,
        rehashed,
        removed,
    };
    let index = ContextIndex {
        generated_at: now_iso(),
        workspace: display_path(root),
        files,
    };
    write_json(&index_path(root), &index)?;
    Ok((index, report))
}

/// 只做 stat-only 新鲜度检查，不重建、不整树哈希。缓存损坏或缺失时报 `missing`。
pub fn freshness(root: &Path) -> FreshnessReport {
    let Some(cached) = load_cached(root) else {
        return FreshnessReport {
            status: "missing".into(),
            changed: Vec::new(),
            removed: Vec::new(),
            added: Vec::new(),
        };
    };
    let cached_map: BTreeMap<_, _> = cached
        .files
        .into_iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect();

    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut present = std::collections::BTreeSet::new();
    for path in workspace_files(root) {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let rel = relative_key(root, &path);
        present.insert(rel.clone());
        match cached_map.get(&rel) {
            Some(entry) => {
                if entry.size != metadata.len() || entry.mtime_ns != mtime_ns(&metadata) {
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

    let status = if changed.is_empty() && added.is_empty() && removed.is_empty() {
        "ready"
    } else {
        "stale"
    };
    FreshnessReport {
        status: status.into(),
        changed,
        removed,
        added,
    }
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
    fn corrupt_cache_degrades_to_missing_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("src/a.rs"), "fn a() {}");
        refresh(root).unwrap();
        fs::write(root.join(INDEX_RELATIVE_PATH), "{ corrupt").unwrap();
        // 不 panic、不 Err：降级为 missing，随后可重建。
        assert_eq!(freshness(root).status, "missing");
        assert!(refresh(root).is_ok());
    }
}
