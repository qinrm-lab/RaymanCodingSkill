//! 只读的过时资产与未完成标记扫描（report-only，绝不自动删除）。
//! 目的是死代码/残留卫生提示，把“要不要删”留给人。

use std::path::Path;

use serde::Serialize;

use crate::walk::{relative_key, workspace_files};

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

/// 扫描工作区，返回过时资产候选与未完成标记（均为提示，不做任何删除）。
pub fn scan(root: &Path) -> AssetReport {
    let mut obsolete = Vec::new();
    let mut markers = Vec::new();

    for path in workspace_files(root) {
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
            .map(|metadata| metadata.len() > MAX_SCAN_BYTES)
            .unwrap_or(true);
        if too_big {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            for marker in WORK_MARKERS.iter().filter(|marker| line.contains(**marker)) {
                markers.push(MarkerFinding {
                    path: rel.clone(),
                    line: index + 1,
                    marker: (*marker).into(),
                    text: line.trim().chars().take(120).collect(),
                });
            }
        }
    }
    AssetReport { obsolete, markers }
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

        let report = scan(root);
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
}
