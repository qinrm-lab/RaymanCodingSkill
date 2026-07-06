use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{display_path, ensure_within, now_iso};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraceEvent {
    pub id: String,
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    pub input_hash: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub event_count: usize,
    pub deterministic: bool,
    pub transcript_hash: String,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraceStatus {
    pub workspace_path: String,
    pub events_path: String,
    pub event_count: usize,
    pub status: String,
    pub blockers: Vec<String>,
    pub last_event: Option<TraceEvent>,
}

pub struct TraceManager {
    workspace: PathBuf,
    events_path: PathBuf,
}

impl TraceManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into().canonicalize()?;
        let events_path = ensure_within(
            &workspace
                .join(".RaymanCodingSkill")
                .join("traces")
                .join("events.jsonl"),
            &workspace,
            "trace state must stay inside workspace",
        )?;
        Ok(Self {
            workspace,
            events_path,
        })
    }

    pub fn record(
        &self,
        kind: &str,
        message: &str,
        evidence_refs: &[String],
    ) -> Result<TraceEvent> {
        if kind.trim().is_empty() || message.trim().is_empty() {
            bail!("trace kind and message are required");
        }
        if let Some(parent) = self.events_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let created_at = now_iso();
        let input = json!({
            "kind": kind.trim(),
            "message": message.trim(),
            "evidence_refs": evidence_refs,
            "created_at": created_at,
        });
        let input_hash = hash_text(&input.to_string());
        let event = TraceEvent {
            id: format!("trace_{}", &input_hash[..12]),
            kind: kind.trim().into(),
            message: message.trim().into(),
            evidence_refs: evidence_refs
                .iter()
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect(),
            input_hash,
            created_at,
        };
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events_path)?;
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        Ok(event)
    }

    pub fn status(&self) -> TraceStatus {
        match self.events() {
            Ok(events) => TraceStatus {
                workspace_path: display_path(&self.workspace),
                events_path: display_path(&self.events_path),
                event_count: events.len(),
                status: "passed".into(),
                blockers: Vec::new(),
                last_event: events.last().cloned(),
            },
            Err(error) => TraceStatus {
                workspace_path: display_path(&self.workspace),
                events_path: display_path(&self.events_path),
                event_count: 0,
                status: "blocked".into(),
                blockers: vec![format!("trace_parse_error: {error}")],
                last_event: None,
            },
        }
    }

    pub fn replay(&self) -> Result<ReplayReport> {
        let events = self.events()?;
        let transcript = events
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join("\n");
        Ok(ReplayReport {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: "passed".into(),
            event_count: events.len(),
            deterministic: true,
            transcript_hash: hash_text(&transcript),
            blockers: Vec::new(),
        })
    }

    pub fn events(&self) -> Result<Vec<TraceEvent>> {
        if !self.events_path.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.events_path)
            .with_context(|| format!("无法读取 trace 事件: {}", self.events_path.display()))?;
        let mut events = Vec::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event = serde_json::from_str::<TraceEvent>(line)
                .with_context(|| format!("invalid trace event at line {}", index + 1))?;
            events.push(event);
        }
        Ok(events)
    }
}

pub fn trace_eval_gate_status(workspace: &Path) -> Result<Value> {
    let manager = TraceManager::new(workspace)?;
    let status = manager.status();
    let replay = manager.replay().ok();
    let report_dir = workspace
        .join(".RaymanCodingSkill")
        .join("evals")
        .join("reports");
    let passed_eval_reports = count_passed_eval_reports(&report_dir)?;
    let trace_replay_passed = status.status == "passed"
        && replay
            .as_ref()
            .is_none_or(|report| report.status == "passed");
    let passed = trace_replay_passed && passed_eval_reports > 0;
    let mut required_actions: Vec<String> = Vec::new();
    if !trace_replay_passed {
        required_actions.push("repair trace JSONL or rerun rayman trace replay".into());
    }
    if passed_eval_reports == 0 {
        required_actions.push(
            "run rayman eval dataset run --dataset <path> --grader contains and keep at least one passed report".into(),
        );
    }
    Ok(json!({
        "status": if passed { "passed" } else { "blocked" },
        "trace": status,
        "replay": replay,
        "passed_eval_reports": passed_eval_reports,
        "required_actions": required_actions
    }))
}

fn count_passed_eval_reports(report_dir: &Path) -> Result<usize> {
    if !report_dir.exists() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in fs::read_dir(report_dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let value = serde_json::from_str::<Value>(&fs::read_to_string(entry.path())?)?;
        if value.get("status").and_then(Value::as_str) == Some("passed") {
            count += 1;
        }
    }
    Ok(count)
}

pub fn write_eval_report(workspace: &Path, report: &Value) -> Result<PathBuf> {
    let reports_dir = ensure_within(
        &workspace
            .join(".RaymanCodingSkill")
            .join("evals")
            .join("reports"),
        workspace,
        "eval reports must stay inside workspace",
    )?;
    fs::create_dir_all(&reports_dir)?;
    let id = report
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("dataset_eval");
    let path = reports_dir.join(format!("{id}.json"));
    crate::write_text(&path, &serde_json::to_string_pretty(report)?)?;
    Ok(path)
}

pub fn summarize_trace_kinds(events: &[TraceEvent]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for event in events {
        *out.entry(event.kind.clone()).or_insert(0) += 1;
    }
    out
}

fn hash_text(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_replay_is_deterministic() {
        let temp = tempfile::tempdir().unwrap();
        let manager = TraceManager::new(temp.path()).unwrap();
        manager
            .record("tool", "rayman context status", &["context checked".into()])
            .unwrap();

        let left = manager.replay().unwrap();
        let right = manager.replay().unwrap();

        assert_eq!(left.status, "passed");
        assert_eq!(left.transcript_hash, right.transcript_hash);
        assert!(left.deterministic);
    }

    #[test]
    fn malformed_trace_blocks_status() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp
            .path()
            .join(".RaymanCodingSkill")
            .join("traces")
            .join("events.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{bad json").unwrap();

        let status = TraceManager::new(temp.path()).unwrap().status();

        assert_eq!(status.status, "blocked");
        assert!(status.blockers[0].contains("trace_parse_error"));
    }

    #[test]
    fn trace_eval_gate_requires_passed_eval_report() {
        let temp = tempfile::tempdir().unwrap();
        TraceManager::new(temp.path())
            .unwrap()
            .record("tool", "rayman trace replay", &[])
            .unwrap();

        let report = trace_eval_gate_status(temp.path()).unwrap();

        assert_eq!(report["status"], "blocked");
        assert_eq!(report["passed_eval_reports"], 0);
        assert!(
            report["required_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action.as_str().unwrap().contains("eval dataset run"))
        );
    }
}
