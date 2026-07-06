use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::ContextKernel;
use crate::{display_path, ensure_within, now_iso, read_text, sha256_file, write_text};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticIndexRecord {
    pub path: String,
    pub sha256: String,
    pub terms: Vec<String>,
    pub summary: String,
    pub indexed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SemanticIndexStatus {
    pub workspace_path: String,
    pub index_path: String,
    pub status: String,
    pub record_count: usize,
    pub stale_count: usize,
    pub blockers: Vec<String>,
    pub source_policy: String,
}

pub struct SemanticContextManager {
    workspace: PathBuf,
    index_path: PathBuf,
}

impl SemanticContextManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into().canonicalize()?;
        let index_path = ensure_within(
            &workspace
                .join(".RaymanCodingSkill")
                .join("context")
                .join("semantic")
                .join("index.jsonl"),
            &workspace,
            "semantic context index must stay inside workspace",
        )?;
        Ok(Self {
            workspace,
            index_path,
        })
    }

    pub fn build(&self) -> Result<Value> {
        let index = ContextKernel::new(&self.workspace)?.refresh_index()?;
        let mut records = Vec::new();
        for file in index.file_inventory.files {
            let path = self.workspace.join(&file.path);
            if !path.exists() || file.size > 256_000 {
                continue;
            }
            let text = match read_text(&path) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let terms = extract_terms(&text);
            if terms.is_empty() {
                continue;
            }
            records.push(SemanticIndexRecord {
                path: file.path,
                sha256: file.sha256,
                terms: terms.into_iter().collect(),
                summary: text.lines().take(8).collect::<Vec<_>>().join("\n"),
                indexed_at: now_iso(),
            });
        }
        if let Some(parent) = self.index_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        for record in &records {
            out.push_str(&serde_json::to_string(record)?);
            out.push('\n');
        }
        write_text(&self.index_path, &out)?;
        Ok(json!({
            "status": "built",
            "index_path": display_path(&self.index_path),
            "record_count": records.len(),
            "source_policy": semantic_source_policy()
        }))
    }

    pub fn status(&self) -> SemanticIndexStatus {
        match self.records() {
            Ok(records) => {
                let stale = records
                    .iter()
                    .filter(|record| {
                        let path = self.workspace.join(&record.path);
                        !path.exists()
                            || sha256_file(&path)
                                .map(|hash| hash != record.sha256)
                                .unwrap_or(true)
                    })
                    .count();
                SemanticIndexStatus {
                    workspace_path: display_path(&self.workspace),
                    index_path: display_path(&self.index_path),
                    status: if stale == 0 { "passed" } else { "blocked" }.into(),
                    record_count: records.len(),
                    stale_count: stale,
                    blockers: if stale == 0 {
                        Vec::new()
                    } else {
                        vec!["semantic index is stale; run rayman context semantic build".into()]
                    },
                    source_policy: semantic_source_policy().into(),
                }
            }
            Err(error) => SemanticIndexStatus {
                workspace_path: display_path(&self.workspace),
                index_path: display_path(&self.index_path),
                status: "blocked".into(),
                record_count: 0,
                stale_count: 1,
                blockers: vec![format!("semantic_index_parse_error: {error}")],
                source_policy: semantic_source_policy().into(),
            },
        }
    }

    pub fn query(&self, query: &str) -> Result<Value> {
        let status = self.status();
        let terms = extract_terms(query);
        let records = self.records()?;
        let mut matches = records
            .into_iter()
            .map(|record| {
                let score = record
                    .terms
                    .iter()
                    .filter(|term| terms.contains(*term))
                    .count();
                (score, record)
            })
            .filter(|(score, _)| *score > 0)
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.path.cmp(&right.1.path))
        });
        Ok(json!({
            "query": query,
            "status": status.status,
            "stale_count": status.stale_count,
            "source_policy": semantic_source_policy(),
            "matches": matches.into_iter().take(20).map(|(score, record)| json!({
                "score": score,
                "path": record.path,
                "sha256": record.sha256,
                "summary": record.summary,
                "evidence_status": if status.stale_count == 0 { "navigation_only" } else { "stale_navigation_only" }
            })).collect::<Vec<_>>(),
            "blockers": status.blockers
        }))
    }

    fn records(&self) -> Result<Vec<SemanticIndexRecord>> {
        if !self.index_path.exists() {
            return Ok(Vec::new());
        }
        let text = read_text(&self.index_path)
            .with_context(|| format!("无法读取 semantic index: {}", self.index_path.display()))?;
        let mut records = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            records.push(
                serde_json::from_str::<SemanticIndexRecord>(line)
                    .with_context(|| format!("invalid semantic record at line {}", index + 1))?,
            );
        }
        Ok(records)
    }
}

fn extract_terms(text: &str) -> BTreeSet<String> {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .map(str::trim)
        .filter(|term| term.chars().count() >= 3)
        .map(|term| term.to_ascii_lowercase())
        .take(200)
        .collect()
}

fn semantic_source_policy() -> &'static str {
    "Semantic context is navigation only. Each hit is hash-bound to a current file and cannot satisfy completion evidence without rereading current files or validation output."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_status_blocks_when_hash_is_stale() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "alpha context marker").unwrap();
        let manager = SemanticContextManager::new(temp.path()).unwrap();
        manager.build().unwrap();
        fs::write(temp.path().join("README.md"), "changed marker").unwrap();

        let status = manager.status();

        assert_eq!(status.status, "blocked");
        assert_eq!(status.stale_count, 1);
    }

    #[test]
    fn semantic_query_is_navigation_only() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "alpha context marker").unwrap();
        let manager = SemanticContextManager::new(temp.path()).unwrap();
        manager.build().unwrap();

        let result = manager.query("alpha").unwrap();

        assert_eq!(result["matches"][0]["evidence_status"], "navigation_only");
        assert!(
            result["source_policy"]
                .as_str()
                .unwrap()
                .contains("navigation only")
        );
    }
}
