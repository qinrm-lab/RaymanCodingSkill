//! 只读的过时资产与未完成标记扫描（report-only，绝不自动删除）。
//! 目的是死代码/残留卫生提示，把“要不要删”留给人。

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::walk::{relative_key, workspace_files_checked};

const OBSOLETE_SUFFIXES: &[&str] = &[".bak", ".old", ".orig", ".deprecated", "~"];
const OBSOLETE_STEM_MARKERS: &[&str] =
    &["_deprecated", "_old", "_backup", "-old", "-backup", ".copy"];
const WORK_MARKERS: &[&str] = &["TODO", "FIXME", "XXX", "HACK", "未完成", "待完成", "待办"];
const MAX_SCAN_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Serialize)]
pub struct AssetFinding {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarkerFinding {
    pub path: String,
    pub line: usize,
    pub marker: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetReport {
    pub obsolete: Vec<AssetFinding>,
    pub markers: Vec<MarkerFinding>,
}

impl AssetReport {
    pub fn is_clean(&self) -> bool {
        self.obsolete.is_empty() && self.markers.is_empty()
    }
}

fn looks_obsolete(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if let Some(suffix) = OBSOLETE_SUFFIXES
        .iter()
        .find(|suffix| lower.ends_with(**suffix))
    {
        return Some(format!("疑似过时文件名后缀 `{suffix}`"));
    }
    if let Some(marker) = OBSOLETE_STEM_MARKERS
        .iter()
        .find(|marker| lower.contains(**marker))
    {
        return Some(format!("疑似过时命名标记 `{marker}`"));
    }
    None
}

/// 扫描工作区，返回过时资产候选与工作标记（均为提示，不做任何删除）。
///
/// 资产结果会进入 readiness 输出，因此遍历或可读取文件的读取失败必须传给调用方；
/// 不能把不完整扫描误报为干净。大文件仍按既有上限跳过正文扫描。
pub fn scan(root: &Path) -> Result<AssetReport> {
    let mut obsolete = Vec::new();
    let mut markers = Vec::new();

    for path in workspace_files_checked(root)? {
        let rel = relative_key(root, &path);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if let Some(reason) = looks_obsolete(name) {
            obsolete.push(AssetFinding {
                path: rel.clone(),
                reason,
            });
        }

        let too_big = std::fs::metadata(&path)
            .with_context(|| format!("资产扫描无法读取文件元数据: {}", path.display()))?
            .len()
            > MAX_SCAN_BYTES;
        if too_big {
            continue;
        }
        // 非 UTF-8 不是读取失败；用无损错误替换继续扫描，避免 GBK 等文件让扫描失效。
        let bytes = std::fs::read(&path)
            .with_context(|| format!("资产扫描无法读取文件: {}", path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        for (index, line) in text.lines().enumerate() {
            if is_marker_catalog_or_fixture_line(&rel, line) {
                continue;
            }
            for marker in WORK_MARKERS
                .iter()
                .filter(|marker| line_has_work_marker(line, marker))
            {
                markers.push(MarkerFinding {
                    path: rel.clone(),
                    line: index + 1,
                    marker: (*marker).into(),
                    text: line.trim().chars().take(120).collect(),
                });
            }
        }
    }
    Ok(AssetReport { obsolete, markers })
}

fn is_marker_catalog_or_fixture_line(path: &str, line: &str) -> bool {
    let trimmed = line.trim_start();
    let scanner_test_line = path == "crates/rayman/src/assets.rs" || path == "src/assets.rs";
    let rust_test_fixture_line =
        path.ends_with(".rs") && path.contains("/tests/") && trimmed.starts_with('"');
    trimmed.contains("WORK_MARKERS")
        || trimmed.contains("markers.contains(&")
        || trimmed.contains("finding.marker ==")
        || trimmed.contains("report.markers")
        || ((scanner_test_line || rust_test_fixture_line) && trimmed.ends_with("\\n\","))
        || (scanner_test_line && trimmed.starts_with('"'))
}

fn line_has_work_marker(line: &str, marker: &str) -> bool {
    if marker_requires_delimiter(marker) {
        return line.find(marker).is_some_and(|index| {
            let tail = &line[index + marker.len()..];
            tail.starts_with(':') || tail.starts_with('：') || tail.starts_with(char::is_whitespace)
        });
    }
    line.contains(marker)
}

fn marker_requires_delimiter(marker: &str) -> bool {
    matches!(marker, "未完成" | "待完成" | "待办")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scan_reports_obsolete_names_and_work_markers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() {} // TODO: 未完成 wire up\n",
        )
        .unwrap();
        fs::write(root.join("src/old_helper.rs.bak"), "dead").unwrap();
        fs::write(root.join("src/config_deprecated.rs"), "old").unwrap();

        let report = scan(root).unwrap();
        assert!(!report.is_clean());
        assert!(
            report
                .obsolete
                .iter()
                .any(|finding| finding.path.ends_with(".bak"))
        );
        assert!(
            report
                .obsolete
                .iter()
                .any(|finding| finding.path.contains("deprecated"))
        );
        assert!(
            report
                .markers
                .iter()
                .any(|finding| finding.marker == "TODO")
        );
        assert!(
            report
                .markers
                .iter()
                .any(|finding| finding.marker == "未完成")
        );
    }

    #[test]
    fn scan_ignores_marker_catalogs_and_pending_vocabulary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/main.rs"),
            "const WORK_MARKERS: &[&str] = &[\"TODO\", \"未完成\"];\n\
             println!(\"待完成项: 0\");\n\
             println!(\"未完成标记: 0\");\n\
             // 未完成: wire this later\n",
        )
        .unwrap();

        let report = scan(root).unwrap();
        assert_eq!(report.markers.len(), 1);
        assert_eq!(report.markers[0].marker, "未完成");
    }

    #[test]
    fn scan_ignores_rust_test_fixture_string_markers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("crates/rayman/tests")).unwrap();
        fs::write(
            root.join("crates/rayman/tests/cli.rs"),
            "\"fn main() {} // TODO: 未完成 wire up\\n\",\n\
             // TODO: real follow-up in test code\n",
        )
        .unwrap();

        let report = scan(root).unwrap();
        assert_eq!(report.markers.len(), 1);
        assert_eq!(report.markers[0].line, 2);
        assert_eq!(report.markers[0].marker, "TODO");
    }

    #[test]
    fn scan_refuses_an_incomplete_workspace_walk() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan(&dir.path().join("missing-workspace")).is_err());
    }
}
