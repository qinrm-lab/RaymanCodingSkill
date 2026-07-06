use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::ContextKernel;
use crate::gate::{GateManager, GateOptions};
use crate::model_catalog::ModelCatalogManager;
use crate::risk::{RiskManager, RiskScanOptions};
use crate::semantic::SemanticContextManager;
use crate::subagent::SubagentLedgerManager;
use crate::trace::TraceManager;
use crate::{display_path, now_iso};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlPlaneSnapshot {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub gate: Value,
    pub risk: Value,
    pub trace: Value,
    pub subagents: Value,
    pub models: Value,
    pub context: Value,
    pub required_actions: Vec<String>,
}

pub struct ControlPlaneManager {
    workspace: PathBuf,
}

impl ControlPlaneManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            workspace: workspace.into().canonicalize()?,
        })
    }

    pub fn snapshot(&self) -> Result<ControlPlaneSnapshot> {
        let gate = GateManager::new(&self.workspace)?.status(GateOptions::default())?;
        let risk = RiskManager::new(&self.workspace)?.scan(RiskScanOptions {
            write_ledger: false,
            include_expensive: false,
        })?;
        let trace = TraceManager::new(&self.workspace)?.status();
        let subagents = SubagentLedgerManager::new(&self.workspace)?.status()?;
        let models =
            match ModelCatalogManager::new(&self.workspace).and_then(|manager| manager.status()) {
                Ok(status) => serde_json::to_value(status)?,
                Err(error) => json!({
                    "status": "blocked",
                    "required_actions": [format!("model catalog status unavailable: {error}")],
                }),
            };
        let context = ContextKernel::new(&self.workspace)?.status()?;
        let semantic = SemanticContextManager::new(&self.workspace)?.status();
        let mut required_actions = gate.required_actions.clone();
        required_actions.extend(
            models
                .get("required_actions")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        );
        required_actions.extend(trace.blockers.clone());
        required_actions.extend(semantic.blockers.clone());
        let status = if required_actions.is_empty() {
            "passed"
        } else {
            "blocked"
        };
        Ok(ControlPlaneSnapshot {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: status.into(),
            gate: serde_json::to_value(gate)?,
            risk: serde_json::to_value(risk)?,
            trace: serde_json::to_value(trace)?,
            subagents,
            models,
            context: json!({
                "kernel": context,
                "semantic": semantic
            }),
            required_actions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_snapshot_has_required_sections() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = ControlPlaneManager::new(temp.path())
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(snapshot.gate.is_object());
        assert!(snapshot.trace.is_object());
        assert!(snapshot.models.is_object());
        assert!(snapshot.context.is_object());
    }
}
