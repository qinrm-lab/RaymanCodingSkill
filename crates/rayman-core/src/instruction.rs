use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::read_text;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionHit {
    pub path: PathBuf,
    pub line: usize,
    pub marker: String,
    pub snippet: String,
}

const RETIRED_MARKERS: &[&str] = &[
    concat!("OL", "D_"),
    concat!("LEG", "ACY_"),
    concat!("deprecated", " rule"),
    concat!("obsolete", " rule"),
    concat!("过时", "规则"),
    concat!("旧", "规则"),
];

pub fn scan_retired_instructions(root: &Path) -> Result<Vec<InstructionHit>> {
    let mut hits = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip(entry.path()) {
            continue;
        }
        let Some(ext) = entry.path().extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if !["md", "yaml", "yml", "toml", "json"].contains(&ext) {
            continue;
        }
        let Ok(text) = read_text(entry.path()) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            for marker in RETIRED_MARKERS {
                if line.contains(marker) {
                    hits.push(InstructionHit {
                        path: entry.path().to_path_buf(),
                        line: index + 1,
                        marker: marker.to_string(),
                        snippet: line.trim().chars().take(200).collect(),
                    });
                }
            }
        }
    }
    Ok(hits)
}

pub fn assert_stale_instructions_released(root: &Path) -> Result<()> {
    let hits = scan_retired_instructions(root)?;
    if hits.is_empty() {
        return Ok(());
    }
    let summary = hits
        .iter()
        .map(|hit| format!("{}:{} {}", hit.path.display(), hit.line, hit.marker))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::bail!("发现退役指令资产:\n{summary}");
}

fn should_skip(path: &Path) -> bool {
    path.components().any(|part| {
        let value = part.as_os_str().to_string_lossy();
        [".git", "target", ".RaymanCodingSkill", "logs"].contains(&value.as_ref())
    })
}
