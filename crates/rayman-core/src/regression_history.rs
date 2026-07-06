use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{display_path, ensure_within, now_iso};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegressionRunRecord {
    pub id: String,
    pub profile: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u128,
    pub steps: Vec<RegressionStepRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegressionStepRecord {
    pub name: String,
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegressionHistoryReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub history_path: String,
    pub limit: usize,
    pub total_returned: usize,
    pub records: Vec<RegressionRunRecord>,
}

pub struct RegressionHistoryManager {
    workspace: PathBuf,
    history_path: PathBuf,
}

impl RegressionHistoryManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let history_path = ensure_within(
            &workspace
                .join(".RaymanCodingSkill")
                .join("regression")
                .join("history.jsonl"),
            &workspace,
            "回归历史必须位于工作区内",
        )?;
        Ok(Self {
            workspace,
            history_path,
        })
    }

    pub fn history_path(&self) -> &Path {
        &self.history_path
    }

    pub fn append(&self, record: &RegressionRunRecord) -> Result<()> {
        if let Some(parent) = self.history_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建回归历史目录: {}", parent.display()))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.history_path)
            .with_context(|| format!("无法打开回归历史: {}", self.history_path.display()))?;
        writeln!(file, "{}", serde_json::to_string(record)?)
            .with_context(|| format!("无法写入回归历史: {}", self.history_path.display()))
    }

    pub fn history(&self, limit: usize) -> Result<RegressionHistoryReport> {
        let mut records = self.read_all()?;
        records.reverse();
        records.truncate(limit);
        let total_returned = records.len();
        Ok(RegressionHistoryReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            history_path: display_path(&self.history_path),
            limit,
            total_returned,
            records,
        })
    }

    pub fn latest(&self) -> Result<Option<RegressionRunRecord>> {
        Ok(self.read_all()?.into_iter().last())
    }

    pub fn latest_passed(&self) -> Result<Option<RegressionRunRecord>> {
        Ok(self
            .latest()?
            .filter(|record| record.status.eq_ignore_ascii_case("passed")))
    }

    fn read_all(&self) -> Result<Vec<RegressionRunRecord>> {
        if !self.history_path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.history_path)
            .with_context(|| format!("无法读取回归历史: {}", self.history_path.display()))?;
        parse_history_records(&text)
            .with_context(|| format!("无法解析回归历史: {}", self.history_path.display()))
    }
}

fn parse_history_records(text: &str) -> Result<Vec<RegressionRunRecord>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Ok(record) = serde_json::from_value::<RegressionRunRecord>(value.clone()) {
            return Ok(vec![record]);
        }
        if let Some(records) = value.get("records") {
            return serde_json::from_value::<Vec<RegressionRunRecord>>(records.clone())
                .context("无法解析 records 回归历史数组");
        }
        if let Some(latest) = value.get("latest_regression") {
            if latest.is_null() {
                return Ok(Vec::new());
            }
            return serde_json::from_value::<RegressionRunRecord>(latest.clone())
                .map(|record| vec![record])
                .context("无法解析 latest_regression 回归历史记录");
        }
    }

    // 回归历史是 release/gate 的 fail-closed 证据来源：单行损坏必须报错，由上层（release evidence）
    // 降级为 partial/blocked，而不是在这里静默跳过让损坏历史看起来“正常”。
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<RegressionRunRecord>(line)
                .with_context(|| format!("无法解析回归历史 JSONL 第 {} 行: {line}", index + 1))
        })
        .collect()
}

pub fn regression_run_id(profile: &str, started_at: &str) -> String {
    let compact_time = started_at
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    format!("regression_{profile}_{compact_time}")
}

pub fn tail_text(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    chars[chars.len() - max_chars..].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_history_appends_jsonl_and_reads_latest_first() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RegressionHistoryManager::new(temp.path()).unwrap();
        let first = RegressionRunRecord {
            id: "regression_quick_1".into(),
            profile: "quick".into(),
            status: "passed".into(),
            started_at: "2026-06-08T00:00:00Z".into(),
            finished_at: "2026-06-08T00:00:01Z".into(),
            duration_ms: 1000,
            steps: Vec::new(),
        };
        let second = RegressionRunRecord {
            id: "regression_full_2".into(),
            profile: "full".into(),
            status: "failed".into(),
            started_at: "2026-06-08T00:01:00Z".into(),
            finished_at: "2026-06-08T00:01:01Z".into(),
            duration_ms: 1000,
            steps: Vec::new(),
        };

        manager.append(&first).unwrap();
        manager.append(&second).unwrap();
        let history = manager.history(10).unwrap();

        assert_eq!(history.total_returned, 2);
        assert_eq!(history.records[0].id, "regression_full_2");
        assert_eq!(manager.latest().unwrap().unwrap().id, "regression_full_2");
    }

    #[test]
    fn latest_passed_returns_none_for_failed_latest() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RegressionHistoryManager::new(temp.path()).unwrap();
        manager
            .append(&RegressionRunRecord {
                id: "regression_quick_1".into(),
                profile: "quick".into(),
                status: "passed".into(),
                started_at: "2026-06-08T00:00:00Z".into(),
                finished_at: "2026-06-08T00:00:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();
        manager
            .append(&RegressionRunRecord {
                id: "regression_full_2".into(),
                profile: "full".into(),
                status: "failed".into(),
                started_at: "2026-06-08T00:01:00Z".into(),
                finished_at: "2026-06-08T00:01:01Z".into(),
                duration_ms: 1000,
                steps: Vec::new(),
            })
            .unwrap();

        assert!(manager.latest_passed().unwrap().is_none());
    }

    #[test]
    fn regression_history_reads_history_report_wrapper() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RegressionHistoryManager::new(temp.path()).unwrap();
        let report = serde_json::json!({
            "workspace_path": temp.path(),
            "generated_at": "2026-06-08T00:00:02Z",
            "history_path": manager.history_path(),
            "limit": 10,
            "total_returned": 1,
            "records": [{
                "id": "regression_full_1",
                "profile": "full",
                "status": "passed",
                "started_at": "2026-06-08T00:00:00Z",
                "finished_at": "2026-06-08T00:00:01Z",
                "duration_ms": 1000,
                "steps": []
            }]
        });
        fs::create_dir_all(manager.history_path().parent().unwrap()).unwrap();
        fs::write(
            manager.history_path(),
            serde_json::to_string_pretty(&report).unwrap(),
        )
        .unwrap();

        let latest = manager.latest().unwrap().unwrap();

        assert_eq!(latest.id, "regression_full_1");
    }

    #[test]
    fn regression_history_reads_release_evidence_latest_regression() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RegressionHistoryManager::new(temp.path()).unwrap();
        let release_evidence = serde_json::json!({
            "status": "ready",
            "latest_regression": {
                "id": "regression_quick_1",
                "profile": "quick",
                "status": "passed",
                "started_at": "2026-06-08T00:00:00Z",
                "finished_at": "2026-06-08T00:00:01Z",
                "duration_ms": 1000,
                "steps": []
            }
        });
        fs::create_dir_all(manager.history_path().parent().unwrap()).unwrap();
        fs::write(
            manager.history_path(),
            serde_json::to_string_pretty(&release_evidence).unwrap(),
        )
        .unwrap();

        let latest = manager.latest().unwrap().unwrap();

        assert_eq!(latest.id, "regression_quick_1");
    }

    #[test]
    fn regression_history_reports_jsonl_line_errors() {
        let temp = tempfile::tempdir().unwrap();
        let manager = RegressionHistoryManager::new(temp.path()).unwrap();
        fs::create_dir_all(manager.history_path().parent().unwrap()).unwrap();
        let valid = RegressionRunRecord {
            id: "regression_quick_1".into(),
            profile: "quick".into(),
            status: "passed".into(),
            started_at: "2026-06-08T00:00:00Z".into(),
            finished_at: "2026-06-08T00:00:01Z".into(),
            duration_ms: 1000,
            steps: Vec::new(),
        };
        fs::write(
            manager.history_path(),
            format!("{}\nnot-json\n", serde_json::to_string(&valid).unwrap()),
        )
        .unwrap();

        let error = format!("{:#}", manager.latest().unwrap_err());

        assert!(error.contains("第 2 行"));
    }

    #[test]
    fn tail_text_keeps_short_text_and_trims_long_text() {
        assert_eq!(tail_text("abc", 5), "abc");
        assert_eq!(tail_text("abcdef", 3), "def");
    }
}
