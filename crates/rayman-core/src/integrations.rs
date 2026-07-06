use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::ContextKernel;
use crate::control::ControlPlaneManager;
use crate::evidence::{EvidenceCheckOptions, check_workspace_evidence};
use crate::gate::{GateManager, GateOptions};
use crate::goal::GoalManager;
use crate::model_catalog::ModelCatalogManager;
use crate::risk::{RiskManager, RiskScanOptions};
use crate::subagent::SubagentLedgerManager;
use crate::{display_path, ensure_within, now_iso, write_text};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolDescriptor {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpResourceDescriptor {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: String,
    pub display_name: String,
    pub version: String,
    pub description: String,
    pub mcp: Value,
    pub tools: Vec<String>,
    pub resources: Vec<String>,
    pub exported_at: String,
}

pub struct IntegrationManager {
    workspace: PathBuf,
    state_dir: PathBuf,
}

impl IntegrationManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace.into().canonicalize()?;
        let state_dir = ensure_within(
            &workspace.join(".RaymanCodingSkill").join("integrations"),
            &workspace,
            "integration state must stay inside workspace",
        )?;
        Ok(Self {
            workspace,
            state_dir,
        })
    }

    pub fn mcp_tools(&self) -> Vec<McpToolDescriptor> {
        [
            (
                "rayman.goal.status",
                "Goal status",
                "Read current Rayman goal state and evidence blockers.",
            ),
            (
                "rayman.context.status",
                "Context status",
                "Read Context Index and Context OS freshness.",
            ),
            (
                "rayman.gate.status",
                "Gate status",
                "Read the broad Rayman readiness gate.",
            ),
            (
                "rayman.evidence.check",
                "Evidence check",
                "Check workspace, goal, session, or research evidence ledgers.",
            ),
            (
                "rayman.risk.scan",
                "Risk scan",
                "Run or inspect proactive risk findings.",
            ),
            (
                "rayman.subagent.status",
                "Subagent status",
                "Read Codex host-subagent ledger and dispatch closeout state.",
            ),
            (
                "rayman.models.status",
                "Model catalog status",
                "Read route model catalog freshness and blockers.",
            ),
            (
                "rayman.control.status",
                "Control snapshot",
                "Read a compact control-plane snapshot across core gates.",
            ),
        ]
        .into_iter()
        .map(|(name, title, description)| McpToolDescriptor {
            name: name.into(),
            title: title.into(),
            description: description.into(),
            input_schema: json!({"type": "object", "additionalProperties": true}),
            output_schema: json!({"type": "object", "additionalProperties": true}),
        })
        .collect()
    }

    pub fn mcp_resources(&self) -> Vec<McpResourceDescriptor> {
        [
            (
                "rayman://context",
                "Context",
                "Workspace Context Kernel state",
            ),
            ("rayman://gate", "Gate", "Readiness gate report"),
            (
                "rayman://evidence",
                "Evidence",
                "Evidence and claim ledgers",
            ),
            (
                "rayman://subagents",
                "Subagents",
                "Host-subagent ledger state",
            ),
            ("rayman://models", "Models", "Model catalog status"),
        ]
        .into_iter()
        .map(|(uri, name, description)| McpResourceDescriptor {
            uri: uri.into(),
            name: name.into(),
            description: description.into(),
            mime_type: "application/json".into(),
        })
        .collect()
    }

    pub fn schema(&self) -> Value {
        json!({
            "workspace_path": display_path(&self.workspace),
            "generated_at": now_iso(),
            "protocol": "mcp",
            "tools": self.mcp_tools(),
            "resources": self.mcp_resources(),
            "source_policy": "MCP and plugin output is an integration surface; current files, validation commands, and Rayman evidence remain authoritative."
        })
    }

    pub fn mcp_rpc_response(&self, request: Value) -> Option<Value> {
        let error_id = json_rpc_request_id(&request);
        match self.try_mcp_rpc_response(request) {
            Ok(response) => response,
            Err(error) => Some(json_rpc_error(error_id, -32603, &error.to_string())),
        }
    }

    fn try_mcp_rpc_response(&self, request: Value) -> Result<Option<Value>> {
        if let Value::Array(items) = request {
            if items.is_empty() {
                return Ok(Some(json_rpc_error(
                    Value::Null,
                    -32600,
                    "empty JSON-RPC batch",
                )));
            }
            let responses = items
                .into_iter()
                .filter_map(|item| self.mcp_rpc_response(item))
                .collect::<Vec<_>>();
            return Ok((!responses.is_empty()).then_some(Value::Array(responses)));
        }

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .context("JSON-RPC request missing string method")?;
        if method.starts_with("notifications/") {
            return Ok(None);
        }

        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {
                    "tools": {"listChanged": false},
                    "resources": {"listChanged": false}
                },
                "serverInfo": {
                    "name": "raymancodingskill",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Rayman MCP is a read-only integration surface. Current files, validation commands, and Rayman evidence remain authoritative."
            }),
            "ping" => json!({}),
            "tools/list" => json!({
                "tools": self.mcp_tools()
                    .into_iter()
                    .map(mcp_tool_protocol_json)
                    .collect::<Vec<_>>()
            }),
            "tools/call" => {
                let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .context("tools/call params.name is required")?;
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let value = self.call_mcp_tool(name, &arguments)?;
                json!({
                    "content": [{
                        "type": "text",
                        "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&value)?
                    }],
                    "isError": false
                })
            }
            "resources/list" => json!({
                "resources": self.mcp_resources()
                    .into_iter()
                    .map(mcp_resource_protocol_json)
                    .collect::<Vec<_>>()
            }),
            "resources/read" => {
                let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .context("resources/read params.uri is required")?;
                let value = self.read_mcp_resource(uri)?;
                json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": serde_json::to_string_pretty(&value)?
                    }]
                })
            }
            _ => return Ok(Some(json_rpc_error(id, -32601, "method not found"))),
        };

        Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        })))
    }

    fn call_mcp_tool(&self, name: &str, arguments: &Value) -> Result<Value> {
        match name {
            "rayman.goal.status" => {
                let goal_id = arguments.get("goal_id").and_then(Value::as_str);
                GoalManager::new(&self.workspace)?.status(goal_id)
            }
            "rayman.context.status" => ContextKernel::new(&self.workspace)?.status(),
            "rayman.gate.status" => Ok(serde_json::to_value(
                GateManager::new(&self.workspace)?.status(GateOptions::default())?,
            )?),
            "rayman.evidence.check" => {
                let scope = arguments
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("workspace")
                    .to_string();
                let goal_id = arguments
                    .get("goal_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let include_advisory = arguments
                    .get("include_advisory")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(serde_json::to_value(check_workspace_evidence(
                    self.workspace.clone(),
                    EvidenceCheckOptions {
                        scope,
                        goal_id,
                        include_advisory,
                    },
                )?)?)
            }
            "rayman.risk.scan" => Ok(serde_json::to_value(
                RiskManager::new(&self.workspace)?.scan(RiskScanOptions {
                    write_ledger: false,
                    include_expensive: false,
                })?,
            )?),
            "rayman.subagent.status" => SubagentLedgerManager::new(&self.workspace)?.status(),
            "rayman.models.status" => Ok(serde_json::to_value(
                ModelCatalogManager::new(&self.workspace)?.status()?,
            )?),
            "rayman.control.status" => Ok(serde_json::to_value(
                ControlPlaneManager::new(&self.workspace)?.snapshot()?,
            )?),
            _ => bail!("unknown Rayman MCP tool: {name}"),
        }
    }

    fn read_mcp_resource(&self, uri: &str) -> Result<Value> {
        match uri {
            "rayman://context" => ContextKernel::new(&self.workspace)?.status(),
            "rayman://gate" => Ok(serde_json::to_value(
                GateManager::new(&self.workspace)?.status(GateOptions::default())?,
            )?),
            "rayman://evidence" => Ok(serde_json::to_value(check_workspace_evidence(
                self.workspace.clone(),
                EvidenceCheckOptions {
                    scope: "workspace".into(),
                    goal_id: None,
                    include_advisory: false,
                },
            )?)?),
            "rayman://subagents" => SubagentLedgerManager::new(&self.workspace)?.status(),
            "rayman://models" => Ok(serde_json::to_value(
                ModelCatalogManager::new(&self.workspace)?.status()?,
            )?),
            _ => bail!("unknown Rayman MCP resource: {uri}"),
        }
    }

    pub fn plugin_manifest(&self) -> PluginManifest {
        let tools = self
            .mcp_tools()
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        let resources = self
            .mcp_resources()
            .into_iter()
            .map(|resource| resource.uri)
            .collect::<Vec<_>>();
        PluginManifest {
            name: "raymancodingskill".into(),
            display_name: "RaymanCodingSkill".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Evidence-first local coding-agent control plane exposed through MCP-compatible descriptors.".into(),
            mcp: json!({
                "transport": ["stdio", "http"],
                "schema_command": "rayman mcp schema",
                "serve_stdio_command": "rayman mcp serve --stdio",
                "serve_http_command": "rayman mcp serve --http"
            }),
            tools,
            resources,
            exported_at: now_iso(),
        }
    }

    pub fn export_plugin(&self) -> Result<Value> {
        fs::create_dir_all(&self.state_dir)?;
        let manifest = self.plugin_manifest();
        let manifest_path = self.state_dir.join("plugin-manifest.json");
        let tools_path = self.state_dir.join("mcp-tools.json");
        write_text(&manifest_path, &serde_json::to_string_pretty(&manifest)?)?;
        write_text(&tools_path, &serde_json::to_string_pretty(&self.schema())?)?;
        Ok(json!({
            "status": "exported",
            "manifest_path": display_path(&manifest_path),
            "tools_path": display_path(&tools_path),
            "manifest": manifest
        }))
    }
}

fn mcp_tool_protocol_json(tool: McpToolDescriptor) -> Value {
    json!({
        "name": tool.name,
        "title": tool.title,
        "description": tool.description,
        "inputSchema": tool.input_schema,
        "outputSchema": tool.output_schema,
    })
}

fn mcp_resource_protocol_json(resource: McpResourceDescriptor) -> Value {
    json!({
        "uri": resource.uri,
        "name": resource.name,
        "description": resource.description,
        "mimeType": resource.mime_type,
    })
}

fn json_rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn json_rpc_request_id(request: &Value) -> Value {
    request.get("id").cloned().unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_schema_exposes_core_control_tools() {
        let temp = tempfile::tempdir().unwrap();
        let schema = IntegrationManager::new(temp.path()).unwrap().schema();
        let tools = schema["tools"].as_array().unwrap();
        assert!(
            tools
                .iter()
                .any(|tool| tool["name"] == "rayman.gate.status")
        );
        assert!(
            tools
                .iter()
                .any(|tool| tool["name"] == "rayman.control.status")
        );
    }

    #[test]
    fn plugin_export_writes_workspace_local_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let report = IntegrationManager::new(temp.path())
            .unwrap()
            .export_plugin()
            .unwrap();
        let manifest_path = report["manifest_path"].as_str().unwrap();
        assert!(manifest_path.contains(".RaymanCodingSkill"));
        assert!(std::path::Path::new(manifest_path).exists());
    }

    #[test]
    fn mcp_json_rpc_supports_initialize_and_lists_tools() {
        let temp = tempfile::tempdir().unwrap();
        let manager = IntegrationManager::new(temp.path()).unwrap();
        let init = manager
            .mcp_rpc_response(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize"
            }))
            .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "raymancodingskill");

        let tools = manager
            .mcp_rpc_response(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            }))
            .unwrap();
        assert!(
            tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == "rayman.control.status")
        );
    }

    #[test]
    fn mcp_json_rpc_tool_call_returns_content() {
        let temp = tempfile::tempdir().unwrap();
        let manager = IntegrationManager::new(temp.path()).unwrap();
        let response = manager
            .mcp_rpc_response(json!({
                "jsonrpc": "2.0",
                "id": "call-1",
                "method": "tools/call",
                "params": {
                    "name": "rayman.models.status",
                    "arguments": {}
                }
            }))
            .unwrap();
        assert_eq!(response["id"], "call-1");
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("workspace_path")
        );
    }
}
