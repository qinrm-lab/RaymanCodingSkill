use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::assets::AssetRetirementReport;
use crate::audit;
use crate::backup::BackupManager;
use crate::customer_deploy::CustomerDeployManager;
use crate::project::{DependencyEdge, ImpactReport, LanguageIndex, ProjectAnalyzer, TestTarget};
use crate::session::SessionManager;
use crate::workspace::WorkspaceActivationManager;
use crate::{display_path, ensure_within, now_iso, sha256_file, write_text};

const CONTEXT_INDEX_VERSION: u32 = 1;
const CONTEXT_OS_VERSION: u32 = 1;
const CONTEXT_INDEX_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/index.json";
const CONTEXT_OS_STATE_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/state.json";
const CONTEXT_OS_EVENTS_RELATIVE_PATH: &str = ".RaymanCodingSkill/context/events.jsonl";
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROJECT_SOURCE_POLICY: &str = "Current workspace files are authoritative. Cached summaries, remembered conclusions, and auxiliary AI output are navigation hints only; reread referenced source files before implementation or review.";
const TASK_SOURCE_POLICY: &str = "Load only task-relevant indexed evidence into the model context, then reread referenced source files for exact details.";
const UNDERSTANDING_PROTOCOL: &[&str] = &[
    "Check workspace context status before non-trivial coding work.",
    "Use task-scoped context to find likely files, symbols, entry points, and verification commands.",
    "Reread current source files, docs, configs, and command output before implementation or review.",
    "Refresh stale indexes when hashes differ, then rerun task context as needed.",
    "Never treat cross-project memory, cached summaries, or auxiliary AI advice as completion evidence.",
];
const INDEX_IGNORED_DIRS: &[&str] = &[
    ".git",
    ".RaymanCodingSkill",
    "target",
    ".tmp",
    "node_modules",
    "dist",
    "build",
    "logs",
];
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "js", "jsx", "ts", "tsx", "py", "java", "go", "c", "h", "cpp", "hpp", "cs", "php", "rb",
    "swift", "kt", "kts", "scala", "sh", "ps1", "sql",
];
const TEST_NAME_MARKERS: &[&str] = &["test", "tests", "spec"];
const PROJECT_INPUT_CANDIDATES: &[&str] = &[
    "README.md",
    "AGENTS.md",
    "SKILL.md",
    "QUICKSTART.md",
    "docs/README.md",
    "docs/CLI.md",
    "docs/API.md",
    "docs/GOAL_WORKFLOWS.md",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "config/default_config.yaml",
    "config/models.yaml",
    "config/skills.yaml",
    "config/auxiliary_ai.yaml",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRecord {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub source: String,
    pub priority: Option<String>,
    pub path: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSummary {
    pub workspace_path: String,
    pub generated_at: String,
    pub status: String,
    pub counts: Value,
    pub records: Vec<ContextRecord>,
    pub guidance: Vec<String>,
    pub source_policy: String,
    pub understanding_protocol: Vec<String>,
    pub required_actions: Vec<String>,
    pub asset_retirement: AssetRetirementReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    pub path: String,
    pub absolute_path: String,
    pub kind: String,
    pub sha256: String,
    pub size: u64,
    pub line_start: usize,
    pub line_end: usize,
    pub modified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInventory {
    pub files: Vec<IndexedFile>,
    pub ignored_dirs: Vec<String>,
    pub source_count: usize,
    pub test_count: usize,
    pub script_count: usize,
    pub config_count: usize,
    pub docs_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub evidence_range: String,
    pub file_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyMap {
    pub manifests: Vec<IndexedFile>,
    pub entry_points: Vec<IndexedFile>,
    pub test_commands: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub task: String,
    pub relevant_files: Vec<IndexedFile>,
    pub relevant_symbols: Vec<SymbolEntry>,
    pub verification: Vec<String>,
    pub risks: Vec<String>,
    pub source_policy: String,
    #[serde(default)]
    pub required_actions: Vec<String>,
    #[serde(default)]
    pub asset_retirement: AssetRetirementReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextIndex {
    pub version: u32,
    pub workspace_path: String,
    pub generated_at: String,
    pub tool_version: String,
    pub source_policy: String,
    pub project_inputs: Vec<IndexedFile>,
    pub file_inventory: FileInventory,
    pub symbol_index: Vec<SymbolEntry>,
    pub dependency_map: DependencyMap,
    pub task_context: TaskContext,
    pub project_adapters: Vec<crate::project::AdapterReport>,
    pub language_indexes: Vec<LanguageIndex>,
    pub dependency_graph: Vec<DependencyEdge>,
    pub impact_cache: Option<ImpactReport>,
    pub test_selection: Vec<TestTarget>,
    pub confidence: String,
    pub stale_reasons: Vec<String>,
    #[serde(default)]
    pub asset_retirement: AssetRetirementReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextOsNode {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub priority: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextOsEdge {
    pub from: String,
    pub to: String,
    pub relation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextStateGraph {
    pub nodes: Vec<ContextOsNode>,
    pub edges: Vec<ContextOsEdge>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSynchronization {
    pub id: String,
    pub source: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextOsState {
    pub version: u32,
    pub name: String,
    pub aliases: Vec<String>,
    pub workspace_path: String,
    pub controller_scope: String,
    pub generated_at: String,
    pub tool_version: String,
    pub source_digest: String,
    pub source_policy: String,
    pub state_files: Vec<String>,
    pub ignored_roots: Vec<String>,
    pub state_graph: ContextStateGraph,
    pub synchronization: Vec<ContextSynchronization>,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextKernel {
    pub workspace: PathBuf,
}

impl ContextKernel {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        Ok(Self { workspace })
    }

    pub fn collect(&self) -> Result<ContextSummary> {
        let index = self.build_index(None)?;
        let mut records = self.base_records(&index)?;
        records.push(self.context_os_record(&index, &records)?);

        let pending_count = count_kind(&records, "pending_work");
        let blocker_count = count_kind(&records, "review_blocker");
        let audit_count = count_kind(&records, "audit_finding");
        let asset_blocker_count = index.asset_retirement.blockers.len();
        let customer_deploy_attention = records
            .iter()
            .filter(|record| {
                record.kind == "customer_deploy_config"
                    && ["missing", "attention"].contains(&record.status.as_str())
            })
            .count();
        let stale_context_count = records
            .iter()
            .filter(|record| {
                record.kind == "context_index"
                    && ["missing", "stale", "attention"].contains(&record.status.as_str())
            })
            .count();
        let stale_backup_count = records
            .iter()
            .filter(|record| {
                record.kind == "backup"
                    && record
                        .details
                        .get("stale")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
            })
            .count();
        let stale_context_os_count = records
            .iter()
            .filter(|record| {
                record.kind == "context_os"
                    && ["missing", "stale", "attention"].contains(&record.status.as_str())
            })
            .count();
        let status = if pending_count > 0
            || blocker_count > 0
            || audit_count > 0
            || asset_blocker_count > 0
            || customer_deploy_attention > 0
            || stale_context_count > 0
            || stale_context_os_count > 0
        {
            "attention"
        } else {
            "ready"
        };
        let context_index_record = records.iter().find(|record| record.kind == "context_index");
        let mut required_actions = required_actions_for_context_record(context_index_record);
        if let Some(context_os_record) = records.iter().find(|record| record.kind == "context_os")
            && ["missing", "stale", "attention"].contains(&context_os_record.status.as_str())
        {
            required_actions.extend(required_actions_for_context_os_record(context_os_record));
        }
        if asset_blocker_count > 0 {
            required_actions.extend(index.asset_retirement.required_actions.clone());
        }
        if customer_deploy_attention > 0 {
            required_actions.push(
                "Complete .RaymanCodingSkill/customer_deploy.yaml before release/deploy goals."
                    .into(),
            );
        }
        Ok(ContextSummary {
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            status: status.into(),
            counts: json!({
                "records": records.len(),
                "pending_work": pending_count,
                "review_blockers": blocker_count,
                "audit_findings": audit_count,
                "asset_retirement_blockers": asset_blocker_count,
                "customer_deploy_attention": customer_deploy_attention,
                "backups": count_kind(&records, "backup"),
                "stale_backups": stale_backup_count,
                "context_index_stale": stale_context_count,
                "file_inventory": index.file_inventory.files.len(),
                "symbol_index": index.symbol_index.len(),
                "dependency_entry_points": index.dependency_map.entry_points.len(),
                "project_adapters": index.project_adapters.len(),
                "language_indexes": index.language_indexes.len(),
                "dependency_graph": index.dependency_graph.len(),
                "test_selection": index.test_selection.len(),
                "project_inputs": count_kind(&records, "project_input"),
                "context_os_stale": stale_context_os_count,
            }),
            records,
            guidance: vec![
                "Current workspace files are the source of truth; cached summaries are navigation only.".into(),
                "Context Index and Context OS state stay workspace-local under .RaymanCodingSkill/context; no daemon, database, vector index, or cross-project memory is required.".into(),
                "Context OS state derives a state graph and append-only event log from current workspace evidence; rewrite it after source or workflow state changes.".into(),
                "When hashes differ, reread the current source files and refresh the index before relying on cached context.".into(),
            ],
            source_policy: PROJECT_SOURCE_POLICY.into(),
            understanding_protocol: understanding_protocol(),
            required_actions,
            asset_retirement: index.asset_retirement,
        })
    }

    pub fn status(&self) -> Result<Value> {
        let summary = self.collect()?;
        let context_os = summary
            .records
            .iter()
            .find(|record| record.kind == "context_os")
            .cloned();
        Ok(json!({
            "workspace_path": summary.workspace_path,
            "generated_at": summary.generated_at,
            "status": summary.status,
            "counts": summary.counts,
            "next_record": next_record(&summary.records),
            "guidance": summary.guidance,
            "source_policy": summary.source_policy,
            "understanding_protocol": summary.understanding_protocol,
            "required_actions": summary.required_actions,
            "context_os": context_os,
            "asset_retirement": summary.asset_retirement,
        }))
    }

    pub fn explain(&self) -> Result<Value> {
        let summary = self.collect()?;
        Ok(json!({
            "decision": "Use a workspace-local Context OS state graph built by the Context Kernel; do not introduce a daemon, database, vector index, or cross-project memory.",
            "workspace_path": summary.workspace_path,
            "status": summary.status,
            "counts": summary.counts,
            "inputs": summary.records.iter().filter(|record| record.kind == "project_input").collect::<Vec<_>>(),
            "context_index": summary.records.iter().find(|record| record.kind == "context_index"),
            "state_files": [
                ".RaymanCodingSkill/workspace_skill.yaml",
                ".RaymanCodingSkill/customer_deploy.yaml",
                ".RaymanCodingSkill/pending_work.json",
                ".RaymanCodingSkill/backups/*/index.json",
                CONTEXT_INDEX_RELATIVE_PATH,
                CONTEXT_OS_STATE_RELATIVE_PATH,
                CONTEXT_OS_EVENTS_RELATIVE_PATH
            ],
            "guidance": summary.guidance,
            "source_policy": summary.source_policy,
            "understanding_protocol": summary.understanding_protocol,
            "required_actions": summary.required_actions,
            "asset_retirement": summary.asset_retirement,
        }))
    }

    pub fn refresh_index(&self) -> Result<ContextIndex> {
        let index = self.build_index(None)?;
        let index_path = self.index_path()?;
        write_text(&index_path, &serde_json::to_string_pretty(&index)?)?;
        self.refresh_context_os("context_refresh")?;
        Ok(index)
    }

    pub fn context_os_status(&self) -> Result<Value> {
        let index = self.build_index(None)?;
        let records = self.base_records(&index)?;
        let state = self.context_os_state(&index, &records)?;
        let comparison = compare_cached_context_os(&self.context_os_state_path()?, &state)?;
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "generated_at": now_iso(),
            "state_path": CONTEXT_OS_STATE_RELATIVE_PATH,
            "event_log_path": CONTEXT_OS_EVENTS_RELATIVE_PATH,
            "status": comparison["status"].clone(),
            "stale": comparison["stale"].clone(),
            "comparison": comparison,
            "state": state,
            "required_actions": required_actions_for_context_os_status(&comparison),
        }))
    }

    pub fn refresh_context_os(&self, event_reason: &str) -> Result<ContextOsState> {
        let index = self.build_index(None)?;
        let records = self.base_records(&index)?;
        let state = self.context_os_state(&index, &records)?;
        let state_path = self.context_os_state_path()?;
        write_text(&state_path, &serde_json::to_string_pretty(&state)?)?;
        self.append_context_os_event(&state, event_reason)?;
        Ok(state)
    }

    pub fn task_context(&self, task: &str) -> Result<Value> {
        let index = self.build_index(Some(task))?;
        let index_status = self.index_status_record(&index)?;
        let customer_deploy = CustomerDeployManager::new(self.workspace.clone())?.status()?;
        Ok(json!({
            "workspace_path": index.workspace_path,
            "generated_at": index.generated_at,
            "context_index_status": index_status,
            "task_context": index.task_context,
            "customer_deploy_config": customer_deploy,
            "source_policy": index.source_policy,
            "understanding_protocol": understanding_protocol(),
            "required_actions": task_required_actions(task),
            "context_os": self.context_os_status()?,
            "asset_retirement": index.asset_retirement,
        }))
    }

    fn index_path(&self) -> Result<PathBuf> {
        ensure_within(
            &self.workspace.join(CONTEXT_INDEX_RELATIVE_PATH),
            &self.workspace,
            "上下文索引必须位于工作区内",
        )
    }

    fn context_os_state_path(&self) -> Result<PathBuf> {
        ensure_within(
            &self.workspace.join(CONTEXT_OS_STATE_RELATIVE_PATH),
            &self.workspace,
            "Context OS state 必须位于工作区内",
        )
    }

    fn context_os_events_path(&self) -> Result<PathBuf> {
        ensure_within(
            &self.workspace.join(CONTEXT_OS_EVENTS_RELATIVE_PATH),
            &self.workspace,
            "Context OS event log 必须位于工作区内",
        )
    }

    fn base_records(&self, index: &ContextIndex) -> Result<Vec<ContextRecord>> {
        let mut records = Vec::new();
        let index_status = self.index_status_record(index)?;
        records.push(self.workspace_record()?);
        records.extend(self.project_input_records()?);
        records.push(self.customer_deploy_record()?);
        records.push(index_status);
        records.extend(self.index_layer_records(index)?);
        records.extend(self.pending_records()?);
        records.extend(self.review_blocker_records()?);
        records.extend(self.backup_records()?);
        records.extend(self.audit_records()?);
        records.push(self.asset_retirement_record(index)?);
        Ok(records)
    }

    fn context_os_record(
        &self,
        index: &ContextIndex,
        records: &[ContextRecord],
    ) -> Result<ContextRecord> {
        let state = self.context_os_state(index, records)?;
        let comparison = compare_cached_context_os(&self.context_os_state_path()?, &state)?;
        let status = comparison
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("missing");
        Ok(ContextRecord {
            id: "context_os".into(),
            kind: "context_os".into(),
            title: "Workspace Context OS state graph".into(),
            status: if status == "ready" {
                "available".into()
            } else {
                status.into()
            },
            source: "context_kernel".into(),
            priority: Some("must".into()),
            path: Some(CONTEXT_OS_STATE_RELATIVE_PATH.into()),
            created_at: comparison
                .get("cached_generated_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            updated_at: Some(state.generated_at.clone()),
            details: json!({
                "comparison": comparison,
                "state_path": CONTEXT_OS_STATE_RELATIVE_PATH,
                "event_log_path": CONTEXT_OS_EVENTS_RELATIVE_PATH,
                "source_digest": state.source_digest,
                "controller_scope": state.controller_scope,
                "state_graph": state.state_graph,
                "synchronization": state.synchronization,
            }),
        })
    }

    fn context_os_state(
        &self,
        index: &ContextIndex,
        records: &[ContextRecord],
    ) -> Result<ContextOsState> {
        let required_actions = context_os_required_actions(records);
        let fingerprint =
            context_os_fingerprint(&self.workspace, index, records, &required_actions);
        let source_digest = digest_json(&fingerprint)?;
        Ok(ContextOsState {
            version: CONTEXT_OS_VERSION,
            name: "RaymanCodingSkill Context OS".into(),
            aliases: vec!["Content OS".into(), "Context Kernel state graph".into()],
            workspace_path: display_path(&self.workspace),
            controller_scope: index.asset_retirement.controller_scope.clone(),
            generated_at: now_iso(),
            tool_version: TOOL_VERSION.into(),
            source_digest,
            source_policy: PROJECT_SOURCE_POLICY.into(),
            state_files: vec![
                ".RaymanCodingSkill/workspace_skill.yaml".into(),
                ".RaymanCodingSkill/customer_deploy.yaml".into(),
                ".RaymanCodingSkill/pending_work.json".into(),
                ".RaymanCodingSkill/goals/*.json".into(),
                ".RaymanCodingSkill/backups/*/index.json".into(),
                ".RaymanCodingSkill/assets/retirement.json".into(),
                CONTEXT_INDEX_RELATIVE_PATH.into(),
                CONTEXT_OS_STATE_RELATIVE_PATH.into(),
                CONTEXT_OS_EVENTS_RELATIVE_PATH.into(),
            ],
            ignored_roots: INDEX_IGNORED_DIRS
                .iter()
                .map(|item| (*item).into())
                .collect(),
            state_graph: context_state_graph(records, &required_actions),
            synchronization: context_synchronization(index, records),
            required_actions,
        })
    }

    fn append_context_os_event(&self, state: &ContextOsState, event_reason: &str) -> Result<()> {
        let events_path = self.context_os_events_path()?;
        if let Some(parent) = events_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录: {}", parent.display()))?;
        }
        let event = json!({
            "version": CONTEXT_OS_VERSION,
            "event_type": "context_os_snapshot",
            "reason": event_reason,
            "generated_at": state.generated_at,
            "workspace_path": state.workspace_path,
            "controller_scope": state.controller_scope,
            "source_digest": state.source_digest,
            "state_path": CONTEXT_OS_STATE_RELATIVE_PATH,
            "event_log_path": CONTEXT_OS_EVENTS_RELATIVE_PATH,
            "required_action_count": state.required_actions.len(),
        });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&events_path)
            .with_context(|| format!("无法写入 Context OS 事件: {}", events_path.display()))?;
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        Ok(())
    }

    fn build_index(&self, task: Option<&str>) -> Result<ContextIndex> {
        let files = self.inventory_files()?;
        let project_inputs = self.project_input_files()?;
        let symbol_index = symbol_index(&self.workspace, &files)?;
        let dependency_map = dependency_map(&self.workspace, &files)?;
        let project_index = ProjectAnalyzer::new(self.workspace.clone())?.index()?;
        let asset_retirement = project_index.asset_retirement.clone();
        let non_current_paths = asset_retirement.non_current_paths();
        let task_inputs = project_inputs
            .iter()
            .filter(|file| !non_current_paths.contains(&file.path))
            .cloned()
            .collect::<Vec<_>>();
        let task_files = files
            .iter()
            .filter(|file| !non_current_paths.contains(&file.path))
            .cloned()
            .collect::<Vec<_>>();
        let task_symbols = symbol_index
            .iter()
            .filter(|symbol| !non_current_paths.contains(&symbol.path))
            .cloned()
            .collect::<Vec<_>>();
        let task_context = task_context(
            task.unwrap_or("general_project_understanding"),
            &task_inputs,
            &task_files,
            &task_symbols,
            &dependency_map,
            &asset_retirement,
        );
        Ok(ContextIndex {
            version: CONTEXT_INDEX_VERSION,
            workspace_path: display_path(&self.workspace),
            generated_at: now_iso(),
            tool_version: TOOL_VERSION.into(),
            source_policy: PROJECT_SOURCE_POLICY.into(),
            project_inputs,
            file_inventory: file_inventory(files),
            symbol_index,
            dependency_map,
            task_context,
            project_adapters: project_index.project_adapters,
            language_indexes: project_index.language_indexes,
            dependency_graph: project_index.dependency_graph,
            impact_cache: None,
            test_selection: project_index.test_selection,
            confidence: project_index.confidence,
            stale_reasons: project_index.stale_reasons,
            asset_retirement,
        })
    }

    fn index_status_record(&self, current: &ContextIndex) -> Result<ContextRecord> {
        let index_path = self.index_path()?;
        let details = compare_cached_index(&index_path, current)?;
        let status = details
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("missing")
            .to_string();
        Ok(ContextRecord {
            id: "context_index".into(),
            kind: "context_index".into(),
            title: "Workspace-local Context Index".into(),
            status,
            source: "context_kernel".into(),
            priority: Some("must".into()),
            path: Some(CONTEXT_INDEX_RELATIVE_PATH.into()),
            created_at: details
                .get("cached_generated_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            updated_at: Some(current.generated_at.clone()),
            details,
        })
    }

    fn index_layer_records(&self, index: &ContextIndex) -> Result<Vec<ContextRecord>> {
        Ok(vec![
            ContextRecord {
                id: "file_inventory".into(),
                kind: "file_inventory".into(),
                title: "Workspace file inventory".into(),
                status: "available".into(),
                source: "current_workspace_scan".into(),
                priority: None,
                path: Some("<workspace>".into()),
                created_at: None,
                updated_at: Some(index.generated_at.clone()),
                details: serde_json::to_value(&index.file_inventory)?,
            },
            ContextRecord {
                id: "symbol_index".into(),
                kind: "symbol_index".into(),
                title: "Project symbol index".into(),
                status: "available".into(),
                source: "current_workspace_scan".into(),
                priority: None,
                path: Some("<workspace>".into()),
                created_at: None,
                updated_at: Some(index.generated_at.clone()),
                details: json!({
                    "symbols": &index.symbol_index,
                    "source_policy": &index.source_policy,
                }),
            },
            ContextRecord {
                id: "dependency_map".into(),
                kind: "dependency_map".into(),
                title: "Manifest, entry point, and verification map".into(),
                status: "available".into(),
                source: "current_workspace_scan".into(),
                priority: None,
                path: Some("<workspace>".into()),
                created_at: None,
                updated_at: Some(index.generated_at.clone()),
                details: serde_json::to_value(&index.dependency_map)?,
            },
            ContextRecord {
                id: "task_context".into(),
                kind: "task_context".into(),
                title: "Default task context retrieval".into(),
                status: "available".into(),
                source: "current_workspace_scan".into(),
                priority: None,
                path: Some("<workspace>".into()),
                created_at: None,
                updated_at: Some(index.generated_at.clone()),
                details: serde_json::to_value(&index.task_context)?,
            },
            ContextRecord {
                id: "project_intelligence".into(),
                kind: "project_intelligence".into(),
                title: "Multi-language project intelligence".into(),
                status: if index.confidence == "none" {
                    "attention".into()
                } else {
                    "available".into()
                },
                source: "project_adapters".into(),
                priority: None,
                path: Some("<workspace>".into()),
                created_at: None,
                updated_at: Some(index.generated_at.clone()),
                details: json!({
                    "project_adapters": &index.project_adapters,
                    "language_indexes": &index.language_indexes,
                    "dependency_graph": &index.dependency_graph,
                    "test_selection": &index.test_selection,
                    "confidence": &index.confidence,
                    "stale_reasons": &index.stale_reasons,
                }),
            },
        ])
    }

    fn asset_retirement_record(&self, index: &ContextIndex) -> Result<ContextRecord> {
        Ok(ContextRecord {
            id: "asset_retirement".into(),
            kind: "asset_retirement".into(),
            title: "Obsolete asset retirement".into(),
            status: if index.asset_retirement.has_blockers() {
                "attention".into()
            } else {
                "available".into()
            },
            source: "asset_retirement".into(),
            priority: Some("must".into()),
            path: Some(".RaymanCodingSkill/assets/retirement.json".into()),
            created_at: None,
            updated_at: Some(index.generated_at.clone()),
            details: serde_json::to_value(&index.asset_retirement)?,
        })
    }

    fn project_input_files(&self) -> Result<Vec<IndexedFile>> {
        let mut inputs = Vec::new();
        for relative in PROJECT_INPUT_CANDIDATES {
            let path = self.workspace.join(relative);
            if path.exists() && path.is_file() {
                inputs.push(indexed_file(&self.workspace, &path, "project_input")?);
            }
        }
        Ok(inputs)
    }

    fn inventory_files(&self) -> Result<Vec<IndexedFile>> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&self.workspace)
            .into_iter()
            .filter_entry(|entry| !should_skip_index_entry(entry.path(), &self.workspace))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path();
            if should_skip_index_entry(path, &self.workspace) {
                continue;
            }
            files.push(indexed_file(
                &self.workspace,
                path,
                &classify_file(&self.workspace, path),
            )?);
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    fn workspace_record(&self) -> Result<ContextRecord> {
        let status = WorkspaceActivationManager::new(self.workspace.clone())?.status()?;
        let details = serde_json::to_value(status)?;
        let enabled = details
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let latest = details
            .get("uses_latest_skill_file")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(ContextRecord {
            id: "workspace_activation".into(),
            kind: "workspace_activation".into(),
            title: "RaymanCodingSkill workspace activation".into(),
            status: if enabled && latest {
                "ready".into()
            } else if enabled {
                "attention".into()
            } else {
                "disabled".into()
            },
            source: "workspace_skill".into(),
            priority: Some("must".into()),
            path: Some(".RaymanCodingSkill/workspace_skill.yaml".into()),
            created_at: details
                .get("enabled_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            updated_at: details
                .get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            details,
        })
    }

    fn customer_deploy_record(&self) -> Result<ContextRecord> {
        let manager = CustomerDeployManager::new(self.workspace.clone())?;
        let report = manager.validate()?;
        Ok(ContextRecord {
            id: "customer_deploy_config".into(),
            kind: "customer_deploy_config".into(),
            title: "Customer release/deploy configuration".into(),
            status: report.status,
            source: "customer_deploy".into(),
            priority: Some("should".into()),
            path: Some(".RaymanCodingSkill/customer_deploy.yaml".into()),
            created_at: None,
            updated_at: report
                .sanitized_config
                .get("updated_at")
                .and_then(Value::as_str)
                .map(str::to_string),
            details: json!({
                "present": report.present,
                "config_path": report.config_path,
                "missing_required": report.missing_required,
                "warnings": report.warnings,
                "config": report.sanitized_config,
                "source_policy": "Workspace-local customer deploy settings only; secret values are not stored, only env or credential references.",
            }),
        })
    }

    fn pending_records(&self) -> Result<Vec<ContextRecord>> {
        let manager = SessionManager::new(self.workspace.clone())?;
        Ok(manager
            .list_pending()?
            .into_iter()
            .map(|item| ContextRecord {
                id: item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("pending_work")
                    .into(),
                kind: "pending_work".into(),
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("pending work")
                    .into(),
                status: item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("pending")
                    .into(),
                source: item
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("session")
                    .into(),
                priority: item
                    .get("priority")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                path: Some(".RaymanCodingSkill/pending_work.json".into()),
                created_at: item
                    .get("created_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                updated_at: item
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                details: item,
            })
            .collect())
    }

    fn review_blocker_records(&self) -> Result<Vec<ContextRecord>> {
        let manager = SessionManager::new(self.workspace.clone())?;
        Ok(manager
            .scan_workspace_unfinished_code(None)
            .into_iter()
            .map(|blocker| {
                let path = blocker
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("<workspace>");
                let line = blocker.get("line").and_then(Value::as_u64).unwrap_or(0);
                ContextRecord {
                    id: format!("review_blocker:{path}:{line}"),
                    kind: "review_blocker".into(),
                    title: blocker
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("unfinished code marker")
                        .into(),
                    status: "blocked".into(),
                    source: "review_gate".into(),
                    priority: Some("must".into()),
                    path: Some(path.into()),
                    created_at: None,
                    updated_at: None,
                    details: blocker,
                }
            })
            .collect())
    }

    fn backup_records(&self) -> Result<Vec<ContextRecord>> {
        Ok(BackupManager::new(self.workspace.clone())?
            .list_backups()?
            .into_iter()
            .map(|backup| ContextRecord {
                id: backup
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("backup")
                    .into(),
                kind: "backup".into(),
                title: backup
                    .get("comment")
                    .and_then(Value::as_str)
                    .unwrap_or("backup")
                    .into(),
                status: if backup
                    .get("stale")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    "stale".into()
                } else {
                    "ready".into()
                },
                source: backup
                    .get("source")
                    .and_then(Value::as_str)
                    .unwrap_or("backup")
                    .into(),
                priority: None,
                path: Some(".RaymanCodingSkill/backups/*/index.json".into()),
                created_at: backup
                    .get("created_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                updated_at: None,
                details: backup,
            })
            .collect())
    }

    fn audit_records(&self) -> Result<Vec<ContextRecord>> {
        Ok(audit::audit_repository(&self.workspace)?
            .into_iter()
            .map(|finding| {
                let path = finding
                    .path
                    .strip_prefix(&self.workspace)
                    .unwrap_or(&finding.path)
                    .to_string_lossy()
                    .replace('\\', "/");
                ContextRecord {
                    id: format!("audit:{path}:{}", finding.line),
                    kind: "audit_finding".into(),
                    title: finding.message.clone(),
                    status: "blocked".into(),
                    source: "audit".into(),
                    priority: Some("must".into()),
                    path: Some(path),
                    created_at: None,
                    updated_at: None,
                    details: json!({
                        "line": finding.line,
                        "pattern": finding.pattern,
                        "message": finding.message,
                    }),
                }
            })
            .collect())
    }

    fn project_input_records(&self) -> Result<Vec<ContextRecord>> {
        let mut records = Vec::new();
        for relative in PROJECT_INPUT_CANDIDATES {
            let path = self.workspace.join(relative);
            if path.exists() {
                records.push(project_input_record(&self.workspace, &path, relative)?);
            }
        }
        Ok(records)
    }
}

fn project_input_record(workspace: &Path, path: &Path, relative: &str) -> Result<ContextRecord> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true));
    let title = match relative {
        "README.md" => "Project overview",
        "AGENTS.md" => "Agent instructions",
        "SKILL.md" => "Workspace skill rules",
        "QUICKSTART.md" => "Quickstart",
        "docs/README.md" => "Documentation index",
        "Cargo.toml" => "Rust package manifest",
        "package.json" => "Node package manifest",
        "pyproject.toml" => "Python package manifest",
        _ => "Project input",
    };
    Ok(ContextRecord {
        id: format!("project_input:{relative}"),
        kind: "project_input".into(),
        title: title.into(),
        status: "available".into(),
        source: "workspace_files".into(),
        priority: None,
        path: Some(relative.into()),
        created_at: None,
        updated_at: modified,
        details: json!({
            "path": relative,
            "absolute_path": display_path(&workspace.join(relative)),
            "size": metadata.len(),
        }),
    })
}

fn indexed_file(workspace: &Path, path: &Path, kind: &str) -> Result<IndexedFile> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .map(chrono::DateTime::<chrono::Utc>::from)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true));
    let line_end = fs::read_to_string(path)
        .ok()
        .map(|text| text.lines().count().max(1))
        .unwrap_or(1);
    let relative = path
        .strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(IndexedFile {
        path: relative,
        absolute_path: display_path(path),
        kind: kind.into(),
        sha256: sha256_file(path)?,
        size: metadata.len(),
        line_start: 1,
        line_end,
        modified_at: modified,
    })
}

fn file_inventory(files: Vec<IndexedFile>) -> FileInventory {
    FileInventory {
        source_count: files.iter().filter(|file| file.kind == "source").count(),
        test_count: files.iter().filter(|file| file.kind == "test").count(),
        script_count: files.iter().filter(|file| file.kind == "script").count(),
        config_count: files.iter().filter(|file| file.kind == "config").count(),
        docs_count: files.iter().filter(|file| file.kind == "docs").count(),
        ignored_dirs: INDEX_IGNORED_DIRS
            .iter()
            .map(|value| (*value).into())
            .collect(),
        files,
    }
}

fn classify_file(workspace: &Path, path: &Path) -> String {
    // 只在工作区相对路径上分类：否则工作区若位于含 "test" 的祖先目录下，整个文件清单会被误判为测试。
    let relative = path
        .strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // 目录判定按路径分量精确匹配（兼顾顶层 tests/ 与嵌套 .../test/）；名称标记只作用于文件名，不再吃整条路径。
    let in_test_dir = relative
        .split('/')
        .any(|component| component == "test" || component == "tests");
    let name_marks_test = SOURCE_EXTENSIONS.contains(&extension.as_str())
        && TEST_NAME_MARKERS
            .iter()
            .any(|marker| file_name.contains(marker));
    if in_test_dir || name_marks_test {
        "test".into()
    } else if ["md", "mdx", "rst", "txt"].contains(&extension.as_str()) {
        "docs".into()
    } else if ["yaml", "yml", "json", "toml", "ini", "env"].contains(&extension.as_str()) {
        "config".into()
    } else if ["sh", "ps1", "bat", "cmd"].contains(&extension.as_str()) {
        "script".into()
    } else if SOURCE_EXTENSIONS.contains(&extension.as_str()) {
        "source".into()
    } else {
        "asset".into()
    }
}

fn should_skip_index_entry(path: &Path, workspace: &Path) -> bool {
    path.strip_prefix(workspace)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                INDEX_IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref())
            })
        })
        .unwrap_or(true)
}

fn symbol_index(workspace: &Path, files: &[IndexedFile]) -> Result<Vec<SymbolEntry>> {
    let file_hashes = files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut symbols = Vec::new();
    for file in files
        .iter()
        .filter(|file| file.kind == "source" || file.kind == "test")
    {
        let path = workspace.join(&file.path);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if let Some((kind, name)) = extract_symbol(line) {
                symbols.push(SymbolEntry {
                    name,
                    kind,
                    path: file.path.clone(),
                    line: index + 1,
                    evidence_range: format!("{}:{}", file.path, index + 1),
                    file_sha256: file_hashes.get(&file.path).cloned().unwrap_or_default(),
                });
            }
        }
    }
    Ok(symbols)
}

fn extract_symbol(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('*') {
        return None;
    }
    if let Some(route) = extract_between(trimmed, ".route(\"", "\"") {
        return Some(("api_route".into(), route));
    }
    let mut normalized = trimmed;
    if let Some(rest) = normalized.strip_prefix("pub ") {
        normalized = rest;
    }
    if let Some(rest) = normalized.strip_prefix("async ") {
        normalized = rest;
    }
    for (prefix, kind) in [
        ("fn ", "function"),
        ("struct ", "type"),
        ("enum ", "type"),
        ("trait ", "type"),
        ("mod ", "module"),
        ("const ", "constant"),
    ] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            return Some((kind.into(), symbol_name(rest)));
        }
    }
    if let Some(rest) = trimmed.strip_prefix("class ") {
        return Some(("type".into(), symbol_name(rest)));
    }
    if let Some(rest) = trimmed.strip_prefix("def ") {
        return Some(("function".into(), symbol_name(rest)));
    }
    None
}

fn symbol_name(rest: &str) -> String {
    rest.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '/'))
        .next()
        .unwrap_or("")
        .to_string()
}

fn extract_between(text: &str, start: &str, end: &str) -> Option<String> {
    let start_index = text.find(start)? + start.len();
    let rest = &text[start_index..];
    let end_index = rest.find(end)?;
    Some(rest[..end_index].to_string())
}

fn dependency_map(workspace: &Path, files: &[IndexedFile]) -> Result<DependencyMap> {
    let manifests = files
        .iter()
        .filter(|file| {
            matches!(
                Path::new(&file.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or(""),
                "Cargo.toml" | "package.json" | "pyproject.toml"
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let entry_points = files
        .iter()
        .filter(|file| is_entry_point(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    let mut test_commands = BTreeSet::new();
    if manifests
        .iter()
        .any(|file| file.path.ends_with("Cargo.toml"))
    {
        test_commands.insert("cargo fmt --check".to_string());
        test_commands.insert("cargo clippy --all-targets -- -D warnings".to_string());
        test_commands.insert("cargo test --all".to_string());
    }
    if manifests
        .iter()
        .any(|file| file.path.ends_with("package.json"))
    {
        test_commands.insert("npm test".to_string());
    }
    if manifests
        .iter()
        .any(|file| file.path.ends_with("pyproject.toml"))
    {
        test_commands.insert("pytest".to_string());
    }
    Ok(DependencyMap {
        manifests,
        entry_points,
        test_commands: test_commands.into_iter().collect(),
        notes: vec![
            format!("entry points were inferred from current files under {}", display_path(workspace)),
            "manifest and command entries are navigation hints; run the current project commands for proof".into(),
        ],
    })
}

fn is_entry_point(path: &str) -> bool {
    matches!(
        path,
        "src/main.rs" | "main.py" | "app.py" | "server.js" | "src/index.js" | "src/index.ts"
    ) || path.ends_with("/src/main.rs")
        || path.ends_with("/src/lib.rs")
        || path.ends_with("/src/main.ts")
        || path.ends_with("/src/main.js")
        || path.ends_with("/src/App.tsx")
}

fn task_context(
    task: &str,
    project_inputs: &[IndexedFile],
    files: &[IndexedFile],
    symbols: &[SymbolEntry],
    dependency_map: &DependencyMap,
    asset_retirement: &AssetRetirementReport,
) -> TaskContext {
    let terms = task_terms(task);
    let mut relevant_files = project_inputs.iter().take(5).cloned().collect::<Vec<_>>();
    relevant_files.extend(
        files
            .iter()
            .filter(|file| {
                terms
                    .iter()
                    .any(|term| file.path.to_ascii_lowercase().contains(term))
            })
            .take(10)
            .cloned(),
    );
    relevant_files.extend(dependency_map.entry_points.iter().take(6).cloned());
    dedupe_files(&mut relevant_files);
    let relevant_symbols = symbols
        .iter()
        .filter(|symbol| {
            terms.iter().any(|term| {
                symbol.name.to_ascii_lowercase().contains(term)
                    || symbol.path.to_ascii_lowercase().contains(term)
                    || symbol.kind.to_ascii_lowercase().contains(term)
            })
        })
        .take(20)
        .cloned()
        .collect::<Vec<_>>();
    TaskContext {
        task: task.trim().to_string(),
        relevant_files,
        relevant_symbols,
        verification: dependency_map.test_commands.clone(),
        risks: vec![
            "Cached context may be stale; compare hashes and reread current file ranges before editing or review.".into(),
            "Summaries are navigation aids only and must not replace source files, tests, or current workspace state.".into(),
            "Recorded obsolete assets are not current-behavior evidence; use asset_retirement fields for cleanup work.".into(),
        ],
        source_policy: TASK_SOURCE_POLICY.into(),
        required_actions: task_required_actions(task),
        asset_retirement: asset_retirement.clone(),
    }
}

fn task_terms(task: &str) -> Vec<String> {
    let mut terms = task
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '/'))
        .filter(|term| term.len() >= 3)
        .map(|term| term.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        terms.push("context".into());
    }
    terms
}

fn dedupe_files(files: &mut Vec<IndexedFile>) {
    let mut seen = BTreeSet::new();
    files.retain(|file| seen.insert(file.path.clone()));
}

fn compare_cached_index(index_path: &Path, current: &ContextIndex) -> Result<Value> {
    if !index_path.exists() {
        return Ok(json!({
            "status": "missing",
            "index_path": display_path(index_path),
            "reason": "run rayman context refresh to create a workspace-local index",
            "source_policy": current.source_policy,
            "stale_files": {
                "changed": [],
                "missing": [],
                "new": [],
            },
            "required_actions": required_actions_for_index_details("missing", &json!({})),
        }));
    }
    // 缓存损坏/截断（例如写到一半崩溃）不应让整条命令报错：降级为 stale，提示刷新重建。
    let cached: ContextIndex = match fs::read_to_string(index_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        Some(cached) => cached,
        None => {
            return Ok(json!({
                "status": "stale",
                "index_path": display_path(index_path),
                "reason": "上下文索引缓存损坏或无法解析，运行 rayman context refresh 重建",
                "source_policy": current.source_policy,
                "stale_files": { "changed": [], "missing": [], "new": [] },
                "required_actions": required_actions_for_index_details("stale", &json!({})),
            }));
        }
    };
    let cached_files = indexed_file_map(&cached);
    let current_files = indexed_file_map(current);
    let mut changed_files = Vec::new();
    let mut missing_files = Vec::new();
    let mut new_files = Vec::new();
    for (path, cached_hash) in &cached_files {
        match current_files.get(path) {
            Some(current_hash) if current_hash != cached_hash => changed_files.push(path.clone()),
            Some(_) => {}
            None => missing_files.push(path.clone()),
        }
    }
    for path in current_files.keys() {
        if !cached_files.contains_key(path) {
            new_files.push(path.clone());
        }
    }
    let workspace_changed = cached.workspace_path != current.workspace_path;
    let stale = workspace_changed
        || !changed_files.is_empty()
        || !missing_files.is_empty()
        || !new_files.is_empty();
    let status = if stale { "stale" } else { "ready" };
    let details = json!({
        "changed_files": changed_files,
        "missing_files": missing_files,
        "new_files": new_files,
    });
    Ok(json!({
        "status": status,
        "index_path": display_path(index_path),
        "cached_workspace_path": cached.workspace_path,
        "current_workspace_path": current.workspace_path,
        "workspace_changed": workspace_changed,
        "cached_generated_at": cached.generated_at,
        "current_generated_at": current.generated_at,
        "cached_tool_version": cached.tool_version,
        "current_tool_version": current.tool_version,
        "changed_files": details["changed_files"],
        "missing_files": details["missing_files"],
        "new_files": details["new_files"],
        "stale_files": {
            "changed": details["changed_files"],
            "missing": details["missing_files"],
            "new": details["new_files"],
        },
        "cached_file_count": cached_files.len(),
        "current_file_count": current_files.len(),
        "source_policy": current.source_policy,
        "required_actions": required_actions_for_index_details(status, &details),
    }))
}

fn compare_cached_context_os(state_path: &Path, current: &ContextOsState) -> Result<Value> {
    if !state_path.exists() {
        return Ok(json!({
            "status": "missing",
            "stale": true,
            "state_path": display_path(state_path),
            "event_log_path": CONTEXT_OS_EVENTS_RELATIVE_PATH,
            "reason": "run rayman context os --write to create the workspace-local state graph and event log",
            "current_source_digest": current.source_digest,
        }));
    }
    // 状态图损坏/截断降级为 stale（需要 rayman context os --write 重建），不让整条命令报错。
    let cached: ContextOsState = match fs::read_to_string(state_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        Some(cached) => cached,
        None => {
            return Ok(json!({
                "status": "stale",
                "stale": true,
                "state_path": display_path(state_path),
                "event_log_path": CONTEXT_OS_EVENTS_RELATIVE_PATH,
                "reason": "Context OS state 损坏或无法解析，运行 rayman context os --write 重建",
                "current_source_digest": current.source_digest,
            }));
        }
    };
    let mut stale_reasons = Vec::new();
    if cached.version != current.version {
        stale_reasons.push("version_changed".to_string());
    }
    if cached.workspace_path != current.workspace_path {
        stale_reasons.push("workspace_changed".to_string());
    }
    if cached.controller_scope != current.controller_scope {
        stale_reasons.push("controller_scope_changed".to_string());
    }
    if cached.source_digest != current.source_digest {
        stale_reasons.push("source_digest_changed".to_string());
    }
    let stale = !stale_reasons.is_empty();
    Ok(json!({
        "status": if stale { "stale" } else { "ready" },
        "stale": stale,
        "state_path": display_path(state_path),
        "event_log_path": CONTEXT_OS_EVENTS_RELATIVE_PATH,
        "cached_generated_at": cached.generated_at,
        "current_generated_at": current.generated_at,
        "cached_tool_version": cached.tool_version,
        "current_tool_version": current.tool_version,
        "cached_workspace_path": cached.workspace_path,
        "current_workspace_path": current.workspace_path,
        "cached_controller_scope": cached.controller_scope,
        "current_controller_scope": current.controller_scope,
        "cached_source_digest": cached.source_digest,
        "current_source_digest": current.source_digest,
        "stale_reasons": stale_reasons,
    }))
}

fn context_os_fingerprint(
    workspace: &Path,
    index: &ContextIndex,
    records: &[ContextRecord],
    required_actions: &[String],
) -> Value {
    let record_fingerprints = records
        .iter()
        .map(|record| {
            json!({
                "id": &record.id,
                "kind": &record.kind,
                "status": &record.status,
                "priority": &record.priority,
                "path": &record.path,
                "details_digest": stable_record_details_digest(record),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "workspace_path": display_path(workspace),
        "controller_scope": &index.asset_retirement.controller_scope,
        "context_index_file_count": index.file_inventory.files.len(),
        "context_index_symbol_count": index.symbol_index.len(),
        "context_index_project_inputs": index.project_inputs.iter().map(|file| json!({
            "path": &file.path,
            "sha256": &file.sha256,
        })).collect::<Vec<_>>(),
        "context_index_stale_reasons": &index.stale_reasons,
        "asset_retirement_blockers": &index.asset_retirement.blockers,
        "asset_retirement_required_actions": &index.asset_retirement.required_actions,
        "records": record_fingerprints,
        "required_actions": required_actions,
    })
}

fn stable_record_details_digest(record: &ContextRecord) -> String {
    let stable = match record.kind.as_str() {
        "context_index" => json!({
            "status": record.details.get("status"),
            "workspace_changed": record.details.get("workspace_changed"),
            "stale_files": record.details.get("stale_files"),
            "cached_file_count": record.details.get("cached_file_count"),
            "current_file_count": record.details.get("current_file_count"),
        }),
        "asset_retirement" => json!({
            "controller_scope": record.details.get("controller_scope"),
            "blockers": record.details.get("blockers"),
            "required_actions": record.details.get("required_actions"),
            "records": record.details.get("records"),
        }),
        "customer_deploy_config" => json!({
            "status": &record.status,
            "required_fields_missing": record.details.get("required_fields_missing"),
            "environment": record.details.get("environment"),
        }),
        "pending_work" | "review_blocker" | "audit_finding" => json!({
            "id": &record.id,
            "status": &record.status,
            "title": &record.title,
            "path": &record.path,
            "priority": &record.priority,
        }),
        _ => json!({
            "status": &record.status,
            "path": &record.path,
        }),
    };
    digest_value(&stable)
}

fn digest_json(value: &Value) -> Result<String> {
    let text = serde_json::to_string(value)?;
    Ok(digest_text(&text))
}

fn digest_value(value: &Value) -> String {
    serde_json::to_string(value)
        .map(|text| digest_text(&text))
        .unwrap_or_else(|_| "unserializable".into())
}

fn digest_text(text: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(text.as_bytes());
    format!("{:x}", digest.finalize())
}

fn indexed_file_map(index: &ContextIndex) -> BTreeMap<String, String> {
    index
        .file_inventory
        .files
        .iter()
        .map(|file| (file.path.clone(), file.sha256.clone()))
        .collect()
}

fn count_kind(records: &[ContextRecord], kind: &str) -> usize {
    records.iter().filter(|record| record.kind == kind).count()
}

fn next_record(records: &[ContextRecord]) -> Value {
    let priority = [
        "pending_work",
        "review_blocker",
        "context_index",
        "context_os",
        "audit_finding",
    ];
    for kind in priority {
        if let Some(record) = records.iter().find(|record| {
            record.kind == kind && !["ready", "available"].contains(&record.status.as_str())
        }) {
            return serde_json::to_value(record).unwrap_or(Value::Null);
        }
    }
    Value::Null
}

fn context_state_graph(
    records: &[ContextRecord],
    required_actions: &[String],
) -> ContextStateGraph {
    let mut nodes = vec![ContextOsNode {
        id: "current_workspace_files".into(),
        kind: "source_of_truth".into(),
        title: "Current workspace files and command output".into(),
        status: "authoritative".into(),
        priority: Some("must".into()),
        path: Some("<workspace>".into()),
    }];
    nodes.extend(records.iter().map(|record| ContextOsNode {
        id: record.id.clone(),
        kind: record.kind.clone(),
        title: record.title.clone(),
        status: record.status.clone(),
        priority: record.priority.clone(),
        path: record.path.clone(),
    }));
    nodes.push(ContextOsNode {
        id: "context_os_state".into(),
        kind: "derived_state_graph".into(),
        title: "Context OS state snapshot and event log".into(),
        status: "derived".into(),
        priority: Some("must".into()),
        path: Some(CONTEXT_OS_STATE_RELATIVE_PATH.into()),
    });

    let edges = vec![
        ContextOsEdge {
            from: "current_workspace_files".into(),
            to: "context_index".into(),
            relation: "hashes_and_indexes".into(),
        },
        ContextOsEdge {
            from: "context_index".into(),
            to: "task_context".into(),
            relation: "narrows_model_context".into(),
        },
        ContextOsEdge {
            from: "asset_retirement".into(),
            to: "task_context".into(),
            relation: "excludes_non_current_assets".into(),
        },
        ContextOsEdge {
            from: "pending_work".into(),
            to: "context_os_state".into(),
            relation: "blocks_success_until_closed".into(),
        },
        ContextOsEdge {
            from: "review_blocker".into(),
            to: "context_os_state".into(),
            relation: "blocks_success_until_resolved".into(),
        },
        ContextOsEdge {
            from: "audit_finding".into(),
            to: "context_os_state".into(),
            relation: "blocks_success_until_triaged".into(),
        },
        ContextOsEdge {
            from: "context_index".into(),
            to: "context_os_state".into(),
            relation: "snapshot_digest_source".into(),
        },
        ContextOsEdge {
            from: "asset_retirement".into(),
            to: "context_os_state".into(),
            relation: "snapshot_digest_source".into(),
        },
    ];

    ContextStateGraph {
        nodes,
        edges,
        next_actions: required_actions.to_vec(),
    }
}

fn context_synchronization(
    index: &ContextIndex,
    records: &[ContextRecord],
) -> Vec<ContextSynchronization> {
    let mut items = Vec::new();
    if let Some(record) = records.iter().find(|record| record.id == "context_index") {
        items.push(ContextSynchronization {
            id: "context_index".into(),
            source: CONTEXT_INDEX_RELATIVE_PATH.into(),
            status: record.status.clone(),
            detail: if record.status == "ready" {
                "cached Context Index hashes match current files".into()
            } else {
                "cached Context Index is missing or stale".into()
            },
        });
    }
    items.push(ContextSynchronization {
        id: "asset_retirement".into(),
        source: ".RaymanCodingSkill/assets/retirement.json".into(),
        status: if index.asset_retirement.has_blockers() {
            "blocking".into()
        } else {
            "ready".into()
        },
        detail: format!(
            "controller_scope={}, blockers={}",
            index.asset_retirement.controller_scope,
            index.asset_retirement.blockers.len()
        ),
    });
    items.push(ContextSynchronization {
        id: "goal_session".into(),
        source: ".RaymanCodingSkill/goals/*.json and .RaymanCodingSkill/pending_work.json".into(),
        status: if count_kind(records, "pending_work") > 0 {
            "blocking".into()
        } else {
            "ready".into()
        },
        detail: format!("pending_work={}", count_kind(records, "pending_work")),
    });
    items.push(ContextSynchronization {
        id: "audit".into(),
        source: "rayman audit findings".into(),
        status: if count_kind(records, "audit_finding") > 0 {
            "blocking".into()
        } else {
            "ready".into()
        },
        detail: format!("audit_findings={}", count_kind(records, "audit_finding")),
    });
    items
}

fn context_os_required_actions(records: &[ContextRecord]) -> Vec<String> {
    let mut actions = Vec::new();
    if let Some(record) = records.iter().find(|record| record.kind == "context_index")
        && ["missing", "stale", "attention"].contains(&record.status.as_str())
    {
        actions.extend(required_actions_for_context_record(Some(record)));
    }
    if records
        .iter()
        .any(|record| record.kind == "asset_retirement" && record.status == "attention")
    {
        actions.push("Resolve asset_retirement blockers before using Context OS as current-behavior context.".into());
    }
    if count_kind(records, "pending_work") > 0 {
        actions.push("Close or carry forward pending work before goal/session success.".into());
    }
    actions.push("Run rayman context os --write after context refresh, asset cleanup, goal/session changes, or audit state changes.".into());
    dedupe_strings(&mut actions);
    actions
}

fn required_actions_for_context_os_record(record: &ContextRecord) -> Vec<String> {
    let comparison = record
        .details
        .get("comparison")
        .cloned()
        .unwrap_or_else(|| json!({"status": record.status}));
    required_actions_for_context_os_status(&comparison)
}

fn required_actions_for_context_os_status(comparison: &Value) -> Vec<String> {
    match comparison
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("missing")
    {
        "ready" => vec![
            "Use rayman context task \"<task>\" and reread current files before editing.".into(),
        ],
        "missing" => vec![
            "Run rayman context os --write to create .RaymanCodingSkill/context/state.json and events.jsonl.".into(),
            "Refresh the Context Index first when context_freshness is also stale.".into(),
        ],
        _ => vec![
            "Run rayman context os --write after reconciling current files and workflow state.".into(),
            "Rerun rayman context status --check before relying on task-scoped context.".into(),
        ],
    }
}

fn dedupe_strings(items: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

fn understanding_protocol() -> Vec<String> {
    UNDERSTANDING_PROTOCOL
        .iter()
        .map(|item| (*item).to_string())
        .collect()
}

fn task_required_actions(task: &str) -> Vec<String> {
    let query = task.replace('"', "\\\"");
    vec![
        format!("Run rayman context task \"{query}\" to narrow the working set."),
        "Reread every referenced current source, docs, and config file before editing or review.".into(),
        "Use rayman impact --path <file> and rayman regression plan --path <file> for touched paths.".into(),
        "Record requirement evidence from current files and validation output, not from cached summaries.".into(),
    ]
}

fn required_actions_for_context_record(record: Option<&ContextRecord>) -> Vec<String> {
    let Some(record) = record else {
        return vec![
            "Run rayman context task \"<task>\" to retrieve task-scoped navigation evidence."
                .into(),
            "Reread referenced current files before implementation or review.".into(),
        ];
    };
    required_actions_for_index_details(&record.status, &record.details)
}

fn required_actions_for_index_details(status: &str, details: &Value) -> Vec<String> {
    match status {
        "missing" => vec![
            "Run rayman context refresh to create the workspace-local Context Index.".into(),
            "Read current project inputs such as README, docs, AGENTS, SKILL, manifests, and build/test commands.".into(),
            "Use task-scoped context only after the index exists.".into(),
        ],
        "stale" | "attention" => {
            let mut actions = vec![
                "Reread current files whose hashes differ before implementation or review.".into(),
                "Run rayman context refresh after rereading or reconciling stale files.".into(),
                "Rerun rayman context task \"<task>\" before relying on indexed evidence.".into(),
            ];
            let stale = stale_file_summary(details);
            if !stale.is_empty() {
                actions.insert(0, format!("Stale file detail: {stale}"));
            }
            actions
        }
        _ => vec![
            "Use rayman context task \"<task>\" to narrow the model context for the current work.".into(),
            "Reread referenced source files from disk before editing or review.".into(),
            "Use impact and regression planning for touched paths before closing work.".into(),
        ],
    }
}

fn stale_file_summary(details: &Value) -> String {
    let groups = [
        ("changed", "changed_files"),
        ("missing", "missing_files"),
        ("new", "new_files"),
    ];
    groups
        .iter()
        .filter_map(|(label, key)| {
            let files = details
                .get(*key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .take(8)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (!files.is_empty()).then(|| format!("{label}={}", files.join(",")))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classify_file_uses_workspace_relative_path_not_ancestor_dirs() {
        // 工作区位于名字含 "test" 的祖先目录下时，源码文件不应被误判为测试。
        let workspace = Path::new("/home/user/my-test-projects/app");
        assert_eq!(
            classify_file(workspace, &workspace.join("src").join("main.rs")),
            "source"
        );
        // 真正位于 tests/ 目录或文件名带 test 标记的才算测试。
        assert_eq!(
            classify_file(workspace, &workspace.join("tests").join("ui.rs")),
            "test"
        );
        assert_eq!(
            classify_file(workspace, &workspace.join("src").join("user_test.rs")),
            "test"
        );
        // 顶层文档/配置分类不受祖先目录影响。
        assert_eq!(
            classify_file(workspace, &workspace.join("README.md")),
            "docs"
        );
    }

    #[test]
    fn context_collects_workspace_session_backup_and_audit_state() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        fs::write(temp.path().join("SKILL.md"), "# skill").unwrap();
        WorkspaceActivationManager::new(temp.path())
            .unwrap()
            .enable("test", "test")
            .unwrap();
        SessionManager::new(temp.path())
            .unwrap()
            .add_pending("finish gate", "details", "task", "test", "must", json!({}))
            .unwrap();
        BackupManager::new(temp.path())
            .unwrap()
            .create_backup(&["README.md".into()], "snapshot", "test")
            .unwrap();

        let summary = ContextKernel::new(temp.path()).unwrap().collect().unwrap();

        assert_eq!(summary.status, "attention");
        assert!(summary.source_policy.contains("Current workspace files"));
        assert!(
            summary
                .understanding_protocol
                .iter()
                .any(|item| item.contains("Reread current source files"))
        );
        assert!(
            summary
                .required_actions
                .iter()
                .any(|item| item.contains("context refresh"))
        );
        assert!(
            summary
                .records
                .iter()
                .any(|item| item.kind == "workspace_activation")
        );
        assert!(
            summary
                .records
                .iter()
                .any(|item| item.kind == "project_input")
        );
        assert!(
            summary
                .records
                .iter()
                .any(|item| item.kind == "pending_work")
        );
        assert!(summary.records.iter().any(|item| item.kind == "backup"));
        assert_eq!(summary.counts["pending_work"], 1);
    }

    #[test]
    fn context_reports_customer_deploy_config_attention_and_task_payload() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();

        let summary = ContextKernel::new(temp.path()).unwrap().collect().unwrap();
        let deploy = summary
            .records
            .iter()
            .find(|record| record.kind == "customer_deploy_config")
            .unwrap();

        assert_eq!(deploy.status, "missing");
        assert_eq!(summary.counts["customer_deploy_attention"], 1);
        assert!(
            summary
                .required_actions
                .iter()
                .any(|action| action.contains("customer_deploy.yaml"))
        );

        let task = ContextKernel::new(temp.path())
            .unwrap()
            .task_context("发布客户项目")
            .unwrap();
        assert_eq!(task["customer_deploy_config"]["status"], "missing");
    }

    #[test]
    fn context_reports_stale_workspace_skill_hash() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("SKILL.md");
        fs::write(&skill, "# skill").unwrap();
        WorkspaceActivationManager::new(temp.path())
            .unwrap()
            .enable("test", "test")
            .unwrap();
        fs::write(&skill, "# changed").unwrap();

        let summary = ContextKernel::new(temp.path()).unwrap().collect().unwrap();
        let workspace = summary
            .records
            .iter()
            .find(|record| record.kind == "workspace_activation")
            .unwrap();

        assert_eq!(workspace.status, "attention");
        assert_eq!(workspace.details["uses_latest_skill_file"], false);
    }

    #[test]
    fn context_refresh_writes_workspace_local_index() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::write(temp.path().join("src").join("main.rs"), "fn main() {}\n").unwrap();

        let index = ContextKernel::new(temp.path())
            .unwrap()
            .refresh_index()
            .unwrap();

        assert_eq!(index.workspace_path, display_path(temp.path()));
        assert!(temp.path().join(CONTEXT_INDEX_RELATIVE_PATH).exists());
        assert!(temp.path().join(CONTEXT_OS_STATE_RELATIVE_PATH).exists());
        assert!(temp.path().join(CONTEXT_OS_EVENTS_RELATIVE_PATH).exists());
        assert!(
            index
                .project_inputs
                .iter()
                .any(|file| file.path == "README.md")
        );
        assert!(
            index
                .dependency_map
                .entry_points
                .iter()
                .any(|file| file.path == "src/main.rs")
        );
        assert!(
            index
                .symbol_index
                .iter()
                .any(|symbol| symbol.name == "main" && symbol.kind == "function")
        );
    }

    #[test]
    fn context_os_status_and_write_manage_state_graph() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let kernel = ContextKernel::new(temp.path()).unwrap();

        let missing = kernel.context_os_status().unwrap();
        assert_eq!(missing["status"], "missing");
        assert_eq!(missing["stale"], true);

        let state = kernel.refresh_context_os("test_write").unwrap();
        assert_eq!(state.name, "RaymanCodingSkill Context OS");
        assert!(
            state
                .state_graph
                .nodes
                .iter()
                .any(|node| node.id == "context_index")
        );
        assert!(
            state
                .state_graph
                .edges
                .iter()
                .any(|edge| edge.relation == "snapshot_digest_source")
        );

        let ready = kernel.context_os_status().unwrap();
        assert_eq!(ready["status"], "ready");
        assert_eq!(ready["stale"], false);
    }

    #[test]
    fn context_status_reports_stale_cached_index_after_file_change() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let kernel = ContextKernel::new(temp.path()).unwrap();
        kernel.refresh_index().unwrap();

        fs::write(temp.path().join("README.md"), "# changed").unwrap();

        let status = kernel.status().unwrap();
        assert_eq!(status["counts"]["context_index_stale"], 1);
        assert_eq!(status["counts"]["context_os_stale"], 1);
        assert_eq!(status["next_record"]["kind"], "context_index");
        assert_eq!(status["next_record"]["status"], "stale");
        assert_eq!(
            status["next_record"]["details"]["changed_files"][0],
            "README.md"
        );
        assert_eq!(
            status["next_record"]["details"]["stale_files"]["changed"][0],
            "README.md"
        );
        assert!(
            status["required_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item
                    .as_str()
                    .unwrap()
                    .contains("Stale file detail: changed=README.md"))
        );
    }

    #[test]
    fn context_refresh_reindexes_deleted_entry_points() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        let main = temp.path().join("src").join("main.rs");
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::write(&main, "fn main() {}\n").unwrap();
        let kernel = ContextKernel::new(temp.path()).unwrap();
        let first = kernel.refresh_index().unwrap();
        assert_eq!(first.dependency_map.entry_points.len(), 1);

        fs::remove_file(main).unwrap();
        let second = kernel.refresh_index().unwrap();

        assert!(second.dependency_map.entry_points.is_empty());
    }

    #[test]
    fn task_context_returns_task_relevant_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        fs::write(
            temp.path().join("src").join("context.rs"),
            "pub struct ContextKernel;\n",
        )
        .unwrap();

        let task = ContextKernel::new(temp.path())
            .unwrap()
            .task_context("context kernel")
            .unwrap();

        let files = task["task_context"]["relevant_files"].as_array().unwrap();
        assert!(
            files
                .iter()
                .any(|file| file["path"].as_str() == Some("src/context.rs"))
        );
        let symbols = task["task_context"]["relevant_symbols"].as_array().unwrap();
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol["name"].as_str() == Some("ContextKernel"))
        );
        assert!(
            task["understanding_protocol"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("task-scoped context"))
        );
        assert!(
            task["required_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("Reread every referenced"))
        );
        assert!(
            task["task_context"]["required_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("impact"))
        );
    }

    #[test]
    fn task_context_excludes_obsolete_assets_from_current_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(
            temp.path().join("src").join("old_asset.rs"),
            "pub struct OldAsset;\n",
        )
        .unwrap();
        crate::assets::AssetRetirementManager::new(temp.path())
            .unwrap()
            .retire(crate::assets::AssetRetireRequest {
                path: PathBuf::from("src/old_asset.rs"),
                replacement_behavior: "src/current.rs".into(),
                deletion_reason: "replaced by current implementation".into(),
                validation_command: "cargo test".into(),
                apply_delete: false,
            })
            .unwrap();

        let task = ContextKernel::new(temp.path())
            .unwrap()
            .task_context("old asset")
            .unwrap();
        let files = task["task_context"]["relevant_files"].as_array().unwrap();
        let symbols = task["task_context"]["relevant_symbols"].as_array().unwrap();
        let summary = ContextKernel::new(temp.path()).unwrap().collect().unwrap();

        assert!(
            !files
                .iter()
                .any(|file| { file["path"].as_str() == Some("src/old_asset.rs") })
        );
        assert!(
            !symbols
                .iter()
                .any(|symbol| { symbol["name"].as_str() == Some("OldAsset") })
        );
        assert!(
            task["asset_retirement"]["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("retirement candidate"))
        );
        assert_eq!(summary.counts["asset_retirement_blockers"], 1);
    }
}
