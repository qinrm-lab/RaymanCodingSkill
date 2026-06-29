use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use walkdir::WalkDir;

use crate::eval::{AgentEvalManager, AgentEvalProfile};
use crate::goal::GoalRecord;
use crate::regression_history::RegressionHistoryManager;
use crate::release::ReleaseEvidenceManager;
use crate::security::SecurityAuditManager;
use crate::{display_path, ensure_within, now_iso};

pub const PATTERN_REPEATED_VALUE_CENTRALIZATION: &str = "repeated_value_centralization";
pub const REPEATED_VALUE_CENTRALIZATION_RULE_TITLE: &str = "Repeated Value Centralization Rule";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityIncident {
    pub id: String,
    pub source: String,
    pub symptom: String,
    pub root_cause: String,
    pub fix: String,
    pub generalized_behavior: String,
    pub pattern_id: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityPattern {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    #[serde(default)]
    pub trigger_terms: Vec<String>,
    #[serde(default)]
    pub required_evidence: Vec<String>,
    #[serde(default)]
    pub incidents: Vec<String>,
    #[serde(default)]
    pub hit_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityGatePatternResult {
    pub pattern_id: String,
    pub name: String,
    pub source: String,
    pub incident_count: usize,
    pub hit_count: u64,
    pub required_evidence: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityGateReport {
    pub workspace_path: String,
    pub goal_id: String,
    pub status: String,
    pub hard_gate: bool,
    pub generated_at: String,
    pub matched_patterns: Vec<QualityGatePatternResult>,
    pub missing_evidence: Vec<String>,
    pub state_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityRegressionItem {
    pub pattern_id: String,
    pub name: String,
    pub source: String,
    pub incident_count: usize,
    pub required_evidence: Vec<String>,
    pub checklist: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct QualityIncidentDraft {
    pub source: String,
    pub symptom: String,
    pub root_cause: String,
    pub fix: String,
    pub generalized_behavior: String,
    pub pattern_id: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct QualityManager {
    workspace: PathBuf,
    quality_dir: PathBuf,
    incidents_dir: PathBuf,
    patterns_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PatternFile {
    version: u32,
    workspace_path: String,
    generated_at: String,
    updated_at: String,
    patterns: Vec<QualityPattern>,
}

impl QualityManager {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        let quality_dir = ensure_within(
            &workspace.join(".RaymanCodingSkill").join("quality"),
            &workspace,
            "质量模式目录必须位于工作区内",
        )?;
        let incidents_dir = ensure_within(
            &quality_dir.join("incidents"),
            &workspace,
            "质量 incident 目录必须位于工作区内",
        )?;
        let patterns_path = ensure_within(
            &quality_dir.join("patterns.json"),
            &workspace,
            "质量模式文件必须位于工作区内",
        )?;
        Ok(Self {
            workspace,
            quality_dir,
            incidents_dir,
            patterns_path,
        })
    }

    pub fn add_incident(&self, draft: QualityIncidentDraft) -> Result<QualityIncident> {
        validate_non_empty("source", &draft.source)?;
        validate_non_empty("symptom", &draft.symptom)?;
        validate_non_empty("root_cause", &draft.root_cause)?;
        let created_at = now_iso();
        let patterns = self.patterns()?;
        let pattern_id = draft
            .pattern_id
            .clone()
            .unwrap_or_else(|| classify_pattern(&patterns, &incident_corpus(&draft)));
        let incident = QualityIncident {
            id: incident_id(&draft.source, &draft.symptom, &created_at),
            source: draft.source.trim().into(),
            symptom: draft.symptom.trim().into(),
            root_cause: draft.root_cause.trim().into(),
            fix: draft.fix.trim().into(),
            generalized_behavior: draft.generalized_behavior.trim().into(),
            pattern_id,
            tags: draft.tags,
            created_at,
        };
        self.write_incident(&incident)?;
        self.upsert_pattern_for_incident(&incident)?;
        Ok(incident)
    }

    pub fn patterns(&self) -> Result<Vec<QualityPattern>> {
        let mut merged = default_patterns()
            .into_iter()
            .map(|pattern| (pattern.id.clone(), pattern))
            .collect::<BTreeMap<_, _>>();
        if self.patterns_path.exists() {
            let text = fs::read_to_string(&self.patterns_path).with_context(|| {
                format!("无法读取质量模式文件: {}", self.patterns_path.display())
            })?;
            if !text.trim().is_empty() {
                let file: PatternFile = serde_json::from_str(&text).with_context(|| {
                    format!("无法解析质量模式文件: {}", self.patterns_path.display())
                })?;
                for stored in file.patterns {
                    merged
                        .entry(stored.id.clone())
                        .and_modify(|pattern| merge_pattern(pattern, &stored))
                        .or_insert(stored);
                }
            }
        }
        Ok(merged.into_values().collect())
    }

    pub fn patterns_json(&self) -> Result<Value> {
        Ok(json!({
            "workspace_path": display_path(&self.workspace),
            "state_path": display_path(&self.patterns_path),
            "patterns": self.patterns()?,
        }))
    }

    pub fn gate_goal(
        &self,
        record: &GoalRecord,
        closing_evidence: Option<&str>,
    ) -> Result<QualityGateReport> {
        let corpus = goal_corpus(record, closing_evidence);
        let mut matched_patterns = Vec::new();
        let mut missing_evidence = Vec::new();
        for pattern in self.patterns()? {
            if !pattern_matches(&pattern, &corpus) {
                continue;
            }
            let missing = if pattern.id == "agent_eval_security_provenance" {
                self.agent_eval_security_provenance_missing(&corpus)
            } else {
                missing_evidence_for_pattern(&pattern, &corpus)
            };
            for item in &missing {
                missing_evidence.push(format!("{}: {item}", pattern.id));
            }
            matched_patterns.push(QualityGatePatternResult {
                pattern_id: pattern.id.clone(),
                name: pattern.name.clone(),
                source: pattern.source.clone(),
                incident_count: pattern.incidents.len(),
                hit_count: pattern.hit_count,
                required_evidence: pattern.required_evidence.clone(),
                missing_evidence: missing,
                rationale: format!(
                    "quality pattern `{}` matched current goal text, history, or completion evidence",
                    pattern.id
                ),
            });
        }
        let status = if missing_evidence.is_empty() {
            "passed"
        } else {
            "blocked"
        };
        Ok(QualityGateReport {
            workspace_path: display_path(&self.workspace),
            goal_id: record.id.clone(),
            status: status.into(),
            hard_gate: true,
            generated_at: now_iso(),
            matched_patterns,
            missing_evidence,
            state_path: display_path(&self.patterns_path),
        })
    }

    pub fn assert_goal_gate(
        &self,
        record: &GoalRecord,
        closing_evidence: Option<&str>,
    ) -> Result<QualityGateReport> {
        let report = self.gate_goal(record, closing_evidence)?;
        if report.status != "passed" {
            bail!(
                "质量模式硬门禁未通过: {}",
                report.missing_evidence.join("; ")
            );
        }
        Ok(report)
    }

    pub fn record_gate_hits(&self, report: &QualityGateReport) -> Result<()> {
        if report.matched_patterns.is_empty() {
            return Ok(());
        }
        let mut patterns = self.patterns()?;
        for result in &report.matched_patterns {
            if let Some(pattern) = patterns
                .iter_mut()
                .find(|pattern| pattern.id == result.pattern_id)
            {
                pattern.hit_count += 1;
                pattern.updated_at = now_iso();
            }
        }
        self.write_patterns(patterns)
    }

    pub fn regression_items_for_text(&self, corpus: &str) -> Result<Vec<QualityRegressionItem>> {
        let mut items = Vec::new();
        for pattern in self.patterns()? {
            if pattern.incidents.is_empty() && !pattern_matches(&pattern, corpus) {
                continue;
            }
            if !pattern.incidents.is_empty() || pattern_matches(&pattern, corpus) {
                items.push(QualityRegressionItem {
                    pattern_id: pattern.id.clone(),
                    name: pattern.name.clone(),
                    source: pattern.source.clone(),
                    incident_count: pattern.incidents.len(),
                    required_evidence: pattern.required_evidence.clone(),
                    checklist: regression_checklist_for_pattern(&pattern),
                });
            }
        }
        Ok(items)
    }

    pub fn stats(&self) -> Result<Value> {
        let patterns = self.patterns()?;
        let incident_count = self.incidents()?.len();
        let workspace_pattern_count = patterns
            .iter()
            .filter(|pattern| !pattern.incidents.is_empty() || pattern.source == "workspace")
            .count();
        Ok(json!({
            "incident_count": incident_count,
            "pattern_count": patterns.len(),
            "builtin_pattern_count": patterns.iter().filter(|pattern| pattern.source == "builtin").count(),
            "workspace_pattern_count": workspace_pattern_count,
            "patterns": patterns.iter().map(|pattern| json!({
                "id": pattern.id,
                "name": pattern.name,
                "source": pattern.source,
                "incident_count": pattern.incidents.len(),
                "hit_count": pattern.hit_count,
            })).collect::<Vec<_>>(),
            "state_path": display_path(&self.patterns_path),
            "incidents_dir": display_path(&self.incidents_dir),
        }))
    }

    fn incidents(&self) -> Result<Vec<QualityIncident>> {
        if !self.incidents_dir.exists() {
            return Ok(Vec::new());
        }
        WalkDir::new(&self.incidents_dir)
            .min_depth(1)
            .max_depth(1)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            })
            .map(|entry| {
                let text = fs::read_to_string(entry.path()).with_context(|| {
                    format!("无法读取质量 incident: {}", entry.path().display())
                })?;
                serde_json::from_str(&text)
                    .with_context(|| format!("无法解析质量 incident: {}", entry.path().display()))
            })
            .collect()
    }

    fn write_incident(&self, incident: &QualityIncident) -> Result<()> {
        fs::create_dir_all(&self.incidents_dir).with_context(|| {
            format!(
                "无法创建质量 incident 目录: {}",
                self.incidents_dir.display()
            )
        })?;
        let path = ensure_within(
            &self.incidents_dir.join(format!("{}.json", incident.id)),
            &self.workspace,
            "质量 incident 文件必须位于工作区内",
        )?;
        fs::write(&path, serde_json::to_string_pretty(incident)?)
            .with_context(|| format!("无法写入质量 incident: {}", path.display()))
    }

    fn upsert_pattern_for_incident(&self, incident: &QualityIncident) -> Result<()> {
        let mut patterns = self.patterns()?;
        if let Some(pattern) = patterns
            .iter_mut()
            .find(|pattern| pattern.id == incident.pattern_id)
        {
            if !pattern.incidents.contains(&incident.id) {
                pattern.incidents.push(incident.id.clone());
            }
            pattern.updated_at = now_iso();
        } else {
            patterns.push(workspace_pattern_from_incident(incident));
        }
        self.write_patterns(patterns)
    }

    fn write_patterns(&self, mut patterns: Vec<QualityPattern>) -> Result<()> {
        fs::create_dir_all(&self.quality_dir)
            .with_context(|| format!("无法创建质量模式目录: {}", self.quality_dir.display()))?;
        patterns.sort_by(|left, right| left.id.cmp(&right.id));
        let now = now_iso();
        let file = PatternFile {
            version: 1,
            workspace_path: display_path(&self.workspace),
            generated_at: now.clone(),
            updated_at: now,
            patterns,
        };
        fs::write(&self.patterns_path, serde_json::to_string_pretty(&file)?)
            .with_context(|| format!("无法写入质量模式文件: {}", self.patterns_path.display()))
    }

    fn agent_eval_security_provenance_missing(&self, corpus: &str) -> Vec<String> {
        let mut missing = agent_eval_security_provenance_missing(corpus);
        match AgentEvalManager::new(&self.workspace)
            .and_then(|manager| manager.assert_passed(AgentEvalProfile::Full))
        {
            Ok(_) => {}
            Err(error) => missing.push(format!("缺少实际 agent eval passed 状态: {error}")),
        }
        match SecurityAuditManager::new(&self.workspace).and_then(|manager| manager.assert_passed())
        {
            Ok(_) => {}
            Err(error) => missing.push(format!("缺少实际 security audit passed 状态: {error}")),
        }
        match RegressionHistoryManager::new(&self.workspace).and_then(|manager| {
            manager
                .latest_passed()?
                .context("latest regression history record is missing or not passed")
        }) {
            Ok(_) => {}
            Err(error) => missing.push(format!("缺少实际 regression history passed 状态: {error}")),
        }
        match ReleaseEvidenceManager::new(&self.workspace)
            .and_then(|manager| manager.generate("quality-gate", false))
        {
            Ok(report) if report.status == "ready" => {}
            Ok(report) => missing.push(format!(
                "缺少实际 release evidence ready 状态: {}",
                report.required_actions.join("; ")
            )),
            Err(error) => missing.push(format!("缺少实际 release evidence ready 状态: {error}")),
        }
        unique_strings(missing.iter())
    }
}

fn validate_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("quality incident {field} 不能为空");
    }
    Ok(())
}

fn default_patterns() -> Vec<QualityPattern> {
    let now = "builtin".to_string();
    vec![
        QualityPattern {
            id: "case_to_general_rule".into(),
            name: "Case Fix Must Generalize".into(),
            description: "Do not repair only one screenshot, phrase, or exact case; define the general trigger and regression examples.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["固定", "截图", "ocr", "泛化", "改写", "paraphrase", "screenshot", "case-specific", "exact phrase"]),
            required_evidence: vec![
                "generic trigger condition".into(),
                "at least 2 rewritten positive examples".into(),
                "at least 1 negative example".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "context_relevance".into(),
            name: "Relevant Context Discovery".into(),
            description: "Follow-up tasks must retrieve relevant historical/tool context, while independent questions must not inherit stale context.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["上下文", "历史", "同一对话", "附件", "context", "conversation", "history", "relevant context", "old context"]),
            required_evidence: vec![
                "relevant historical/tool context checked".into(),
                "independent-question no-pollution negative case".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "project_understanding_freshness".into(),
            name: "Fresh Project Understanding".into(),
            description: "Project understanding must come from fresh workspace-local, hash-backed context and reread current files instead of long-lived memory.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["项目理解", "完整项目", "项目上下文", "项目记忆", "过时记忆", "project understanding", "project context", "stale project", "context index", "long-lived memory", "cached summary"]),
            required_evidence: vec![
                "context status/task evidence".into(),
                "Context OS state graph freshness evidence".into(),
                "current source reread evidence".into(),
                "stale index handled evidence".into(),
                "impact/regression evidence for touched paths".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "managed_temp_freshness".into(),
            name: "Managed Runtime Temp".into(),
            description: "Runtime temp failures must use workspace-local managed temp diagnostics and cleanup evidence instead of ad hoc system temp assumptions.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["临时目录", "临时文件", "temp dir", "temp directory", "temporary file", "system temp", "stale temp", "locked temp", "rayman temp"]),
            required_evidence: vec![
                "rayman temp status or doctor evidence".into(),
                "managed cleanup evidence".into(),
                "no unmanaged system temp evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "obsolete_asset_retirement".into(),
            name: "Obsolete Asset Retirement".into(),
            description: "Review, refactor, and feature replacement work must retire stale code, docs, config, tests, and entrypoints with evidence before success.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["过时资产", "资产清理", "资产退役", "旧入口", "过时入口", "obsolete asset", "asset cleanup", "asset retirement", "stale entry", "old entrypoint", "feature replacement"]),
            required_evidence: vec![
                "obsolete asset inventory".into(),
                "replacement/current behavior evidence".into(),
                "docs/config/tests sync evidence".into(),
                "rayman audit evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "audit_failure_delivery_gate".into(),
            name: "Audit Failure Delivery Gate".into(),
            description: "Audit failures and validation gaps must be triaged, resolved, or recorded as partial/blocked work instead of being downgraded to notes.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["rayman audit failed", "audit failed", "审计失败", "仓库审计未通过", "审计未通过", "和本次无关", "不是本次", "旧文档", "old docs", "unrelated audit", "pre-existing blocker", "validation gap", "manual verification gap", "远端验证", "无法直接 invoke"]),
            required_evidence: vec![
                "audit output evidence".into(),
                "finding triage evidence".into(),
                "resolved audit or partial/blocked status evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: PATTERN_REPEATED_VALUE_CENTRALIZATION.into(),
            name: "Repeated Values Must Be Centralized".into(),
            description: "Repeated literals, thresholds, paths, prompt fragments, and policy values must use a shared constant, config key, helper, template variable, or referenced rule section across skill and program surfaces.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["大量出现", "重复值", "重复变量", "变量值", "用变量代替", "多处修改", "duplicate literal", "repeated literal", "repeated value", "magic value", "hard-coded value", "single source of truth", "constantization"]),
            required_evidence: vec![
                "duplicate value inventory".into(),
                "single source of truth or retained-duplication reason".into(),
                "skill/program scope checked".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "agent_eval_security_provenance".into(),
            name: "Agent Eval Security And Provenance".into(),
            description: "Agent workflow changes must include runnable behavior evals, LLM security audit evidence, release evidence, and regression history instead of relying on manual final answers.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["agent eval", "agent behavior", "LLM security", "prompt injection", "red-team", "red team", "supply chain", "provenance", "release evidence", "regression history", "observability", "代理评测", "安全审计", "发布证据", "供应链", "回归历史"]),
            required_evidence: vec![
                "rayman eval run evidence".into(),
                "rayman security audit evidence".into(),
                "rayman release evidence".into(),
                "regression history evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "research_agent_autonomy".into(),
            name: "Research Agent Autonomy Boundary".into(),
            description: "Multi-agent research and scientist-agent autonomy must use whitelist experiments, reflection evidence, conflict reconciliation, and hard no-edit/no-close controls.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["scientist agent", "research agent", "multi-agent research", "autonomous scientist", "experiment whitelist", "科研 agent", "科学家 agent", "多 agent 科研", "自主实验", "白名单实验"]),
            required_evidence: vec![
                "rayman research run evidence".into(),
                "whitelist command policy evidence".into(),
                "no file edit or goal close authority evidence".into(),
                "research conflict reconciliation evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "codex_host_subagent_ledger".into(),
            name: "Codex Host Subagent Ledger".into(),
            description: "Codex host subagent use must leave a workspace-local ledger with bounded task scope, result evidence, primary-agent review, and overlap/conflict disposition before success.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["Codex host subagent", "host subagent", "subagent ledger", "parallel subagent", "spawn subagent", "并发 subagent", "子代理并发", "主 agent 自动开"]),
            required_evidence: vec![
                "rayman subagent status evidence".into(),
                "primary-agent review evidence".into(),
                "write scope or read-only boundary evidence".into(),
                "overlap/conflict disposition evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "codex_harness_execution_contract".into(),
            name: "Codex Harness Execution Contract".into(),
            description: "Codex harness-style self-improvement must map undocumented harness language to documented Codex execution controls, including sandbox approvals, durable instruction surfaces, subagent inheritance, ledger review, and non-interactive approval failure behavior.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["Codex harness", "Codex execution envelope", "Codex execution contract", "codex_harness", "sandbox approval", "approval_policy", "sandbox_mode", "AGENTS.md skills hooks MCP", "non-interactive approval", "Codex 执行外壳", "Codex 执行契约"]),
            required_evidence: vec![
                "Codex manual or current-session capability mapping evidence".into(),
                "sandbox/approval boundary evidence".into(),
                "durable instruction-surface mapping evidence".into(),
                "subagent inheritance and Rayman ledger review evidence".into(),
                "non-interactive approval failure handling evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "tool_loop_recovery".into(),
            name: "Tool Loop Recovery".into(),
            description: "Empty model responses, diagnostic-only failures, or irrelevant tool results must trigger retry, supplemental lookup, or local synthesis.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["空响应", "工具", "搜索跑偏", "兜底", "empty response", "diagnostic", "irrelevant tool", "tool loop", "search drift", "fallback synthesis"]),
            required_evidence: vec![
                "empty/irrelevant result recovery path".into(),
                "retry, supplemental lookup, or local synthesis evidence".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "temporal_fact_evidence".into(),
            name: "Temporal Fact Evidence".into(),
            description: "Current facts and relative dates require absolute dates and fresh evidence instead of stale model memory.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["现在", "上个月", "最新", "总统", "ceo", "政策", "新闻", "current", "latest", "today", "president", "policy", "news"]),
            required_evidence: vec![
                "absolute date conversion".into(),
                "current evidence/source verification".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        QualityPattern {
            id: "debug_release_delivery".into(),
            name: "Debug And Release Delivery".into(),
            description: "Customer programs are incomplete until both debug and release builds compile; locked executables require a temp-target proof and later formal release verification.".into(),
            source: "builtin".into(),
            trigger_terms: terms(&["debug", "release", "客户程序", "编译通过", "locked exe", "target locked", "debug/release"]),
            required_evidence: vec![
                "debug build passed".into(),
                "release build passed".into(),
            ],
            incidents: Vec::new(),
            hit_count: 0,
            created_at: now,
            updated_at: "builtin".into(),
        },
    ]
}

fn terms(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn merge_pattern(base: &mut QualityPattern, stored: &QualityPattern) {
    base.incidents = unique_strings(base.incidents.iter().chain(stored.incidents.iter()));
    base.hit_count = base.hit_count.max(stored.hit_count);
    base.updated_at = stored.updated_at.clone();
    if base.trigger_terms.is_empty() {
        base.trigger_terms = stored.trigger_terms.clone();
    }
    if base.required_evidence.is_empty() {
        base.required_evidence = stored.required_evidence.clone();
    }
}

fn unique_strings<'a>(items: impl Iterator<Item = &'a String>) -> Vec<String> {
    items
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn classify_pattern(patterns: &[QualityPattern], corpus: &str) -> String {
    patterns
        .iter()
        .find(|pattern| pattern_matches(pattern, corpus))
        .map(|pattern| pattern.id.clone())
        .unwrap_or_else(|| "workspace_quality_followup".into())
}

fn pattern_matches(pattern: &QualityPattern, corpus: &str) -> bool {
    let normalized = normalize(corpus);
    pattern
        .trigger_terms
        .iter()
        .map(|term| normalize(term))
        .any(|term| !term.trim().is_empty() && normalized.contains(&term))
}

fn missing_evidence_for_pattern(pattern: &QualityPattern, corpus: &str) -> Vec<String> {
    match pattern.id.as_str() {
        "case_to_general_rule" => case_to_general_missing(corpus),
        "context_relevance" => context_relevance_missing(corpus),
        "project_understanding_freshness" => project_understanding_missing(corpus),
        "managed_temp_freshness" => managed_temp_missing(corpus),
        "obsolete_asset_retirement" => obsolete_asset_retirement_missing(corpus),
        "audit_failure_delivery_gate" => audit_failure_delivery_missing(corpus),
        PATTERN_REPEATED_VALUE_CENTRALIZATION => repeated_value_centralization_missing(corpus),
        "agent_eval_security_provenance" => agent_eval_security_provenance_missing(corpus),
        "research_agent_autonomy" => research_agent_autonomy_missing(corpus),
        "codex_host_subagent_ledger" => codex_host_subagent_ledger_missing(corpus),
        "codex_harness_execution_contract" => codex_harness_execution_contract_missing(corpus),
        "tool_loop_recovery" => tool_loop_missing(corpus),
        "temporal_fact_evidence" => temporal_fact_missing(corpus),
        "debug_release_delivery" => debug_release_missing(corpus),
        _ => pattern
            .required_evidence
            .iter()
            .filter(|marker| !contains_any(corpus, &[marker.as_str()]))
            .cloned()
            .collect(),
    }
}

fn case_to_general_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &["通用触发", "general trigger", "generic trigger", "泛化规则"],
    ) {
        missing.push("缺少通用触发条件说明".into());
    }
    let positive = count_markers(
        corpus,
        &["positive_", "正例", "positive example", "改写正例"],
    );
    if positive < 2 && !contains_any(corpus, &["2 positive", "two positive", "两个正例"]) {
        missing.push("缺少至少 2 个改写正例".into());
    }
    if !contains_any(
        corpus,
        &[
            "negative_",
            "负例",
            "negative example",
            "1 negative",
            "一个负例",
        ],
    ) {
        missing.push("缺少至少 1 个负例".into());
    }
    missing
}

fn context_relevance_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "相关历史",
            "历史上下文",
            "relevant context",
            "historical context",
            "tool context",
        ],
    ) {
        missing.push("缺少相关历史/工具上下文检索证据".into());
    }
    if !contains_any(
        corpus,
        &[
            "独立问题",
            "no pollution",
            "不污染",
            "negative case",
            "负例",
        ],
    ) {
        missing.push("缺少独立问题不受旧上下文污染的负例证据".into());
    }
    missing
}

fn project_understanding_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "rayman context status",
            "rayman context task",
            "context checked",
            "context status",
            "task context",
            "上下文已检查",
        ],
    ) {
        missing.push("缺少 context status/task evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "rayman context os --check",
            "rayman context os --write",
            "Context OS state graph",
            "context_os",
            "content os",
            "状态图已检查",
        ],
    ) {
        missing.push("缺少 Context OS state graph freshness evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "current source reread",
            "current files reread",
            "reread current source",
            "read current files",
            "source reread",
            "当前文件已重读",
            "重读当前源码",
        ],
    ) {
        missing.push("缺少 current source reread evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "rayman context refresh",
            "context refresh",
            "stale index handled",
            "hashes ready",
            "hash-backed",
            "索引刷新",
            "过期索引已处理",
        ],
    ) {
        missing.push("缺少 stale index handled evidence".into());
    }
    if !(contains_any(
        corpus,
        &["rayman impact", "impact evidence", "impact --path"],
    ) && contains_any(
        corpus,
        &[
            "rayman regression plan",
            "regression evidence",
            "regression plan",
            "regression --path",
        ],
    )) {
        missing.push("缺少 impact/regression evidence for touched paths".into());
    }
    missing
}

fn managed_temp_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "rayman temp status",
            "rayman temp doctor",
            "temp status",
            "temp doctor",
            "临时目录诊断",
        ],
    ) {
        missing.push("缺少 rayman temp status or doctor evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "rayman temp cleanup",
            "managed cleanup",
            "metadata.json",
            "只清理 managed",
            "managed temp",
        ],
    ) {
        missing.push("缺少 managed cleanup evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "no unmanaged system temp",
            "TempManager",
            ".RaymanCodingSkill/tmp",
            "workspace-local temp",
            "无系统 temp",
        ],
    ) {
        missing.push("缺少 no unmanaged system temp evidence".into());
    }
    missing
}

fn obsolete_asset_retirement_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "obsolete asset inventory",
            "asset inventory",
            "资产清单",
            "过时资产清单",
            "affected paths",
            "影响路径",
        ],
    ) {
        missing.push("缺少 obsolete asset inventory evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "replacement/current behavior",
            "replacement behavior",
            "current behavior",
            "替代行为",
            "当前行为",
        ],
    ) {
        missing.push("缺少 replacement/current behavior evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "docs/config/tests sync",
            "docs/config/tests synchronized",
            "documentation/config/test sync",
            "文档配置测试同步",
            "docs config tests synchronized",
        ],
    ) {
        missing.push("缺少 docs/config/tests sync evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "rayman audit",
            "audit passed",
            "audit evidence",
            "仓库审计",
            "审计通过",
        ],
    ) {
        missing.push("缺少 rayman audit evidence".into());
    }
    missing
}

fn audit_failure_delivery_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    let audit_output = contains_any(
        corpus,
        &[
            "rayman audit",
            "audit output",
            "audit failed",
            "审计输出",
            "仓库审计未通过",
            "审计失败",
        ],
    );
    if !audit_output {
        missing.push("缺少 audit output evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "triage",
            "current blocker",
            "pre-existing blocker",
            "historical/archive candidate",
            "finding triage",
            "处理方式",
            "逐项处理",
            "审计 finding",
        ],
    ) {
        missing.push("缺少 finding triage evidence".into());
    }
    let resolved = contains_any(
        corpus,
        &[
            "audit passed",
            "rayman audit passed",
            "仓库审计通过",
            "审计通过",
            "resolved audit",
            "resolved finding",
            "finding resolved",
            "已修复",
        ],
    );
    let non_success_or_pending = contains_any(
        corpus,
        &[
            "partial",
            "blocked",
            "failed",
            "pending",
            "待完成",
            "未完成",
            "session close --status partial",
            "goal close --status blocked",
        ],
    );
    if !resolved && !non_success_or_pending {
        missing.push("缺少 resolved audit or partial/blocked status evidence".into());
    }
    if contains_any(
        corpus,
        &[
            "和本次无关",
            "不是本次",
            "unrelated",
            "not caused by this change",
        ],
    ) && !contains_any(
        corpus,
        &[
            "pending",
            "partial",
            "blocked",
            "exempt",
            "retire",
            "resolved",
            "已修复",
            "待完成",
        ],
    ) {
        missing.push("不能仅以和本次无关降级审计失败".into());
    }
    missing
}

fn repeated_value_centralization_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "duplicate value inventory",
            "literal inventory",
            "repeated value inventory",
            "occurrence scan",
            "affected occurrences",
            "重复值清单",
            "重复文本清单",
            "出现次数",
            "多处出现",
        ],
    ) {
        missing.push("缺少 duplicate value inventory evidence".into());
    }
    let centralized = contains_any(
        corpus,
        &[
            "single source of truth",
            "named constant",
            "shared constant",
            "config key",
            "helper",
            "template variable",
            "rule section",
            "常量抽取",
            "提取常量",
            "集中化",
            "统一变量",
            "共享变量",
        ],
    );
    let retained_with_reason = contains_any(
        corpus,
        &[
            "retained duplication reason",
            "duplication retained because",
            "retained with reason",
            "保留重复原因",
            "重复保留原因",
        ],
    );
    if !centralized && !retained_with_reason {
        missing.push("缺少 single source of truth or retained-duplication reason evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "skill/program scope checked",
            "skill and program",
            "SKILL.md",
            "Rust code",
            "program surface checked",
            "skill surface checked",
            "skill 和程序",
            "技能和程序",
            "程序适用面",
        ],
    ) {
        missing.push("缺少 skill/program scope checked evidence".into());
    }
    missing
}

fn agent_eval_security_provenance_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "rayman eval run",
            "eval run passed",
            "agent eval evidence",
            "代理评测通过",
        ],
    ) {
        missing.push("缺少 rayman eval run evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "rayman security audit",
            "security audit passed",
            "LLM security audit",
            "安全审计通过",
        ],
    ) {
        missing.push("缺少 rayman security audit evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "rayman release evidence",
            "release evidence",
            "provenance",
            "发布证据",
        ],
    ) {
        missing.push("缺少 rayman release evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "regression history",
            ".RaymanCodingSkill/regression/history.jsonl",
            "回归历史",
            "history.jsonl",
        ],
    ) {
        missing.push("缺少 regression history evidence".into());
    }
    missing
}

fn research_agent_autonomy_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "rayman research run",
            "research run passed",
            "research session",
        ],
    ) {
        missing.push("缺少 rayman research run evidence".into());
    }
    if !contains_any(
        corpus,
        &["whitelist", "白名单", "allowed argv", "command policy"],
    ) {
        missing.push("缺少白名单命令策略证据".into());
    }
    if !contains_any(
        corpus,
        &[
            "can_edit_files=false",
            "can_edit_files: false",
            "no file edit",
            "不能编辑",
        ],
    ) || !contains_any(
        corpus,
        &[
            "can_close_goals=false",
            "can_close_goals: false",
            "no goal close",
            "不能关闭",
        ],
    ) {
        missing.push("缺少 scientist 无编辑/无关闭权限证据".into());
    }
    if !contains_any(
        corpus,
        &[
            "research reconcile",
            "conflict reconciled",
            "冲突归并",
            "policy violation gate",
        ],
    ) {
        missing.push("缺少 research 冲突归并证据".into());
    }
    missing
}

fn codex_host_subagent_ledger_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "rayman subagent status",
            "rayman host-subagent status",
            "subagent ledger status",
            "subagent_ledger",
        ],
    ) {
        missing.push("缺少 rayman subagent status evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "primary review",
            "primary-agent review",
            "primary reviewed",
            "主 agent 复核",
            "主代理复核",
        ],
    ) {
        missing.push("缺少 primary-agent review evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "write scope",
            "write-path",
            "read-only",
            "read only",
            "写入边界",
            "只读",
        ],
    ) {
        missing.push("缺少 write scope or read-only boundary evidence".into());
    }
    if !contains_any(
        corpus,
        &[
            "overlap-resolution",
            "overlap resolution",
            "no overlap",
            "conflict disposition",
            "冲突处理",
            "重叠写入",
        ],
    ) {
        missing.push("缺少 overlap/conflict disposition evidence".into());
    }
    missing
}

fn codex_harness_execution_contract_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "Codex manual",
            "codex-manual",
            "official Codex manual",
            "current-session capability mapping",
            "current session capability mapping",
            "documented Codex execution controls",
            "官方 Codex manual",
            "当前会话能力映射",
        ],
    ) {
        missing.push("缺少 Codex manual 或当前会话能力映射证据".into());
    }
    if !(contains_any(corpus, &["sandbox", "sandbox_mode", "沙箱"])
        && contains_any(corpus, &["approval", "approval_policy", "审批", "批准"]))
    {
        missing.push("缺少 sandbox/approval boundary evidence".into());
    }
    if !(contains_any(corpus, &["AGENTS.md", "agents guidance"])
        && contains_any(corpus, &["skills", "skill surface", "技能"])
        && contains_any(corpus, &["hooks", "hook surface", "钩子"])
        && contains_any(corpus, &["MCP", "rules", "规则"]))
    {
        missing.push(
            "缺少 AGENTS.md/skills/hooks/MCP-rules instruction-surface mapping evidence".into(),
        );
    }
    if !(contains_any(
        corpus,
        &[
            "subagent inheritance",
            "subagents inherit",
            "inherits sandbox",
            "继承 sandbox",
            "继承沙箱",
        ],
    ) && contains_any(
        corpus,
        &[
            "rayman subagent status",
            "Rayman subagent ledger",
            "subagent ledger review",
        ],
    ) && contains_any(
        corpus,
        &["primary-agent review", "primary review", "主 agent 复核"],
    )) {
        missing.push("缺少 subagent inheritance plus Rayman ledger review evidence".into());
    }
    if !(contains_any(corpus, &["non-interactive", "noninteractive", "非交互"])
        && contains_any(
            corpus,
            &[
                "approval failure",
                "approval fails",
                "escalation failure",
                "cannot surface approval",
                "审批失败",
                "无法请求审批",
            ],
        ))
    {
        missing.push("缺少 non-interactive approval/escalation failure handling evidence".into());
    }
    missing
}

fn tool_loop_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "空响应",
            "empty response",
            "irrelevant tool",
            "搜索跑偏",
            "diagnostic",
        ],
    ) {
        missing.push("缺少空响应/无关工具结果触发证据".into());
    }
    if !contains_any(
        corpus,
        &[
            "重试",
            "retry",
            "补查",
            "supplemental lookup",
            "兜底",
            "fallback",
            "本地综合",
            "local synthesis",
        ],
    ) {
        missing.push("缺少重试/补查/本地综合恢复证据".into());
    }
    missing
}

fn temporal_fact_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(corpus, &["绝对日期", "absolute date"]) && !has_iso_like_date(corpus) {
        missing.push("缺少相对时间到绝对日期的转换证据".into());
    }
    if !contains_any(
        corpus,
        &[
            "当前证据",
            "官方",
            "source",
            "verified",
            "web",
            "browse",
            "核验",
            "证据",
        ],
    ) {
        missing.push("缺少当前来源核验证据".into());
    }
    missing
}

fn debug_release_missing(corpus: &str) -> Vec<String> {
    let mut missing = Vec::new();
    if !contains_any(
        corpus,
        &[
            "debug build",
            "cargo build",
            "debug 编译",
            "调试编译",
            "debug passed",
        ],
    ) {
        missing.push("缺少 debug 编译通过证据".into());
    }
    if !contains_any(
        corpus,
        &[
            "release build",
            "cargo build --release",
            "release 编译",
            "发布编译",
            "release passed",
        ],
    ) {
        missing.push("缺少 release 编译通过证据".into());
    }
    missing
}

fn regression_checklist_for_pattern(pattern: &QualityPattern) -> Vec<String> {
    match pattern.id.as_str() {
        "case_to_general_rule" => vec![
            "Define the generic trigger before implementation.".into(),
            "Add at least 2 paraphrased positive tests and 1 negative test.".into(),
        ],
        "context_relevance" => vec![
            "Retrieve relevant historical/tool context for follow-ups.".into(),
            "Add an independent-question negative check for stale-context pollution.".into(),
        ],
        "project_understanding_freshness" => vec![
            "Run context status and task-scoped context before implementation.".into(),
            "Check or rewrite the Context OS state graph with rayman context os --check/--write.".into(),
            "Reread current source/docs/config files referenced by task context.".into(),
            "Refresh or handle stale Context Index before relying on cached evidence.".into(),
            "Run impact and regression planning for touched paths.".into(),
        ],
        "managed_temp_freshness" => vec![
            "Run rayman temp status or rayman temp doctor before diagnosing temp failures.".into(),
            "Clean only Rayman-managed temp entries with metadata-backed cleanup.".into(),
            "Verify runtime code uses TempManager or same-directory atomic temp paths.".into(),
        ],
        "obsolete_asset_retirement" => vec![
            "Inventory obsolete code, docs, config, tests, entrypoints, and generated references before pruning.".into(),
            "Record replacement/current behavior evidence and deletion risk.".into(),
            "Synchronize docs/config/tests/examples/prompts after retirement.".into(),
            "Run focused validation and rayman audit before success.".into(),
        ],
        "audit_failure_delivery_gate" => vec![
            "Capture the exact rayman audit output.".into(),
            "Triage every finding as current blocker, pre-existing blocker, or historical/archive candidate.".into(),
            "Resolve findings or close partial/blocked with pending work before final handoff.".into(),
        ],
        PATTERN_REPEATED_VALUE_CENTRALIZATION => vec![
            "Inventory repeated literals, paths, thresholds, prompt fragments, or policy values before editing.".into(),
            "Extract a named constant, config key, helper, template variable, or referenced rule section when values change together.".into(),
            "Record any retained duplication reason and verify both skill and program surfaces were checked.".into(),
        ],
        "agent_eval_security_provenance" => vec![
            "Run rayman eval run for deterministic agent behavior contracts.".into(),
            "Run rayman security audit for LLM-specific security blockers.".into(),
            "Generate rayman release evidence for release/provenance handoff.".into(),
            "Verify regression history was written for the approving run.".into(),
        ],
        "research_agent_autonomy" => vec![
            "Run rayman research run and capture the session evidence.".into(),
            "Verify scientist experiments use allowed argv whitelist commands only.".into(),
            "Verify scientist can_edit_files=false and can_close_goals=false.".into(),
            "Reconcile or block all research conflicts before success.".into(),
        ],
        "codex_host_subagent_ledger" => vec![
            "Record every Codex host subagent with rayman subagent record.".into(),
            "Record result evidence and changed paths with rayman subagent result.".into(),
            "Record primary-agent review before success with rayman subagent review.".into(),
            "Resolve overlapping write scopes or conflicts before final handoff.".into(),
        ],
        "codex_harness_execution_contract" => vec![
            "Map Codex harness language to documented Codex execution controls or current-session capabilities.".into(),
            "Verify sandbox and approval boundaries, including escalation behavior.".into(),
            "Map durable instruction surfaces: AGENTS.md, skills, hooks, and MCP/rules.".into(),
            "Verify subagent inheritance and Rayman subagent ledger primary review.".into(),
            "Document non-interactive approval failure handling before success.".into(),
        ],
        "tool_loop_recovery" => vec![
            "Exercise empty response, diagnostic failure, or irrelevant tool-result recovery.".into(),
            "Verify retry, supplemental lookup, or local synthesis before success.".into(),
        ],
        "temporal_fact_evidence" => vec![
            "Convert relative dates to explicit absolute dates.".into(),
            "Verify current facts with fresh evidence before answering.".into(),
        ],
        "debug_release_delivery" => vec![
            "Run debug build.".into(),
            "Run release build, using temp target only as a lock-file workaround until formal release output is verified.".into(),
        ],
        _ => pattern.required_evidence.clone(),
    }
}

fn goal_corpus(record: &GoalRecord, closing_evidence: Option<&str>) -> String {
    let mut parts = vec![
        record.id.clone(),
        record.contract.goal.clone(),
        record.contract.workflow_name.clone(),
    ];
    parts.extend(
        record
            .contract
            .requirements
            .iter()
            .map(|item| item.text.clone()),
    );
    parts.extend(record.contract.acceptance_criteria.clone());
    parts.extend(record.contract.verification.clone());
    parts.extend(record.contract.assumptions.clone());
    for step in &record.steps {
        parts.push(step.stage.clone());
        parts.push(step.status.clone());
        if let Some(value) = &step.evidence {
            parts.push(value.clone());
        }
        if let Some(value) = &step.error {
            parts.push(value.clone());
        }
        if let Some(value) = &step.command {
            parts.push(value.clone());
        }
        if let Some(value) = &step.stderr_summary {
            parts.push(value.clone());
        }
    }
    if let Some(evidence) = closing_evidence {
        parts.push(evidence.into());
    }
    parts.join("\n")
}

fn incident_corpus(draft: &QualityIncidentDraft) -> String {
    [
        draft.source.as_str(),
        draft.symptom.as_str(),
        draft.root_cause.as_str(),
        draft.fix.as_str(),
        draft.generalized_behavior.as_str(),
    ]
    .join("\n")
}

fn workspace_pattern_from_incident(incident: &QualityIncident) -> QualityPattern {
    let now = now_iso();
    QualityPattern {
        id: incident.pattern_id.clone(),
        name: "Workspace Quality Follow-up".into(),
        description: format!(
            "Workspace-local repeated quality issue from {}",
            incident.source
        ),
        source: "workspace".into(),
        trigger_terms: keyword_terms(&format!(
            "{}\n{}\n{}",
            incident.symptom, incident.root_cause, incident.generalized_behavior
        )),
        required_evidence: vec![incident.generalized_behavior.clone()]
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect(),
        incidents: vec![incident.id.clone()],
        hit_count: 0,
        created_at: now.clone(),
        updated_at: now,
    }
}

fn keyword_terms(text: &str) -> Vec<String> {
    normalize(text)
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|word| word.chars().count() >= 5)
        .take(12)
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn contains_any(corpus: &str, markers: &[&str]) -> bool {
    let normalized = normalize(corpus);
    markers
        .iter()
        .map(|marker| normalize(marker))
        .any(|marker| normalized.contains(&marker))
}

fn count_markers(corpus: &str, markers: &[&str]) -> usize {
    let normalized = normalize(corpus);
    markers
        .iter()
        .map(|marker| normalize(marker))
        .map(|marker| normalized.matches(&marker).count())
        .sum()
}

fn has_iso_like_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(10).any(|window| {
        window[0].is_ascii_digit()
            && window[1].is_ascii_digit()
            && window[2].is_ascii_digit()
            && window[3].is_ascii_digit()
            && matches!(window[4], b'-' | b'/')
            && window[5].is_ascii_digit()
            && window[6].is_ascii_digit()
            && matches!(window[7], b'-' | b'/')
            && window[8].is_ascii_digit()
            && window[9].is_ascii_digit()
    })
}

fn normalize(text: &str) -> String {
    text.to_lowercase()
}

fn incident_id(source: &str, symptom: &str, created_at: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(source.as_bytes());
    hasher.update(symptom.as_bytes());
    hasher.update(created_at.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("incident_{}", &digest[..12])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::GoalManager;
    use crate::rayman_exe_name;
    use crate::regression_history::{RegressionRunRecord, RegressionStepRecord};
    use std::fs;
    use std::path::Path;

    #[test]
    fn incident_json_persists_and_aggregates_builtin_pattern() {
        let temp = tempfile::tempdir().unwrap();
        let manager = QualityManager::new(temp.path()).unwrap();

        let incident = manager
            .add_incident(QualityIncidentDraft {
                source: "codex://threads/example".into(),
                symptom: "empty response then search drift".into(),
                root_cause: "tool loop did not recover from empty response".into(),
                fix: "added retry and local synthesis".into(),
                generalized_behavior: "empty response must trigger retry".into(),
                pattern_id: None,
                tags: vec!["tool".into()],
            })
            .unwrap();

        assert_eq!(incident.pattern_id, "tool_loop_recovery");
        assert!(
            manager
                .incidents_dir
                .join(format!("{}.json", incident.id))
                .exists()
        );
        let patterns = manager.patterns().unwrap();
        let pattern = patterns
            .iter()
            .find(|pattern| pattern.id == "tool_loop_recovery")
            .unwrap();
        assert_eq!(pattern.incidents, vec![incident.id]);
    }

    #[test]
    fn gate_blocks_case_specific_fix_without_positive_and_negative_examples() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "fix OCR screenshot case",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let report = quality
            .gate_goal(&goal, Some("req_1: fixed exact screenshot"))
            .unwrap();

        assert_eq!(report.status, "blocked");
        assert!(
            report
                .missing_evidence
                .iter()
                .any(|item| item.contains("改写正例"))
        );
    }

    #[test]
    fn gate_passes_case_specific_fix_with_generalized_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "generalize screenshot OCR handling",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let report = quality
            .gate_goal(
                &goal,
                Some("req_1: 通用触发 OCR fallback; positive_1 ok; positive_2 ok; negative_1 ok"),
            )
            .unwrap();

        assert_eq!(report.status, "passed");
    }

    #[test]
    fn gate_blocks_repeated_value_changes_without_centralization_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "大量出现的变量值用变量代替",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let report = quality.gate_goal(&goal, Some("req_1: fixed")).unwrap();

        assert_eq!(report.status, "blocked");
        assert!(
            report
                .matched_patterns
                .iter()
                .any(|pattern| { pattern.pattern_id == PATTERN_REPEATED_VALUE_CENTRALIZATION })
        );
        assert!(
            report
                .missing_evidence
                .iter()
                .any(|item| item.contains("duplicate value inventory"))
        );
    }

    #[test]
    fn gate_passes_repeated_value_changes_with_centralization_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "deduplicate repeated literal values into one variable",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let report = quality
            .gate_goal(
                &goal,
                Some("req_1: duplicate value inventory covered affected occurrences in SKILL.md and Rust code; extracted single source of truth with a named constant and template variable; skill/program scope checked"),
            )
            .unwrap();

        assert_eq!(report.status, "passed", "{:?}", report.missing_evidence);
    }

    #[test]
    fn temporal_fact_gate_requires_absolute_date_and_current_source() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "answer latest president policy news",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let report = quality.gate_goal(&goal, Some("req_1: answered")).unwrap();
        assert_eq!(report.status, "blocked");
        assert!(
            report
                .missing_evidence
                .iter()
                .any(|item| item.contains("绝对日期"))
        );

        let report = quality
            .gate_goal(
                &goal,
                Some("req_1: absolute date 2026-06-01; current evidence verified from official source"),
            )
            .unwrap();
        assert_eq!(report.status, "passed");
    }

    #[test]
    fn tool_loop_gate_requires_recovery_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "handle empty response and irrelevant tool result",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: detected empty response"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: empty response path verified; retry and local synthesis passed"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed", "{:?}", passed.missing_evidence);
    }

    #[test]
    fn project_understanding_gate_requires_fresh_context_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "strengthen project understanding from context index instead of long-lived memory",
                "feature_update",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: implemented"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");
        assert!(
            blocked
                .missing_evidence
                .iter()
                .any(|item| item.contains("context status/task"))
        );

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: rayman context status and rayman context task checked; rayman context os --check verified Context OS state graph freshness; relevant context checked; independent-question negative case prevents old context pollution; current source reread from current files; rayman context refresh handled stale index with hash-backed evidence; rayman impact --path crates/rayman-core/src/context.rs impact evidence and rayman regression plan --path crates/rayman-core/src/context.rs regression evidence recorded; absolute date 2026-06-02; current evidence verified from source files"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed", "{:?}", passed.missing_evidence);
    }

    #[test]
    fn debug_release_gate_requires_both_build_modes() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "customer program must pass debug/release",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: debug build passed"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: debug build passed; release build passed"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed");
    }

    #[test]
    fn agent_eval_security_provenance_gate_requires_all_expert_gate_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "implement agent eval LLM security release evidence provenance and regression history",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: implemented"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");
        assert!(
            blocked
                .missing_evidence
                .iter()
                .any(|item| item.contains("rayman eval run"))
        );

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: rayman eval run passed; rayman security audit passed; local rayman release evidence ready; regression history written to .RaymanCodingSkill/regression/history.jsonl; debug build passed; release build passed; relevant context checked; independent-question negative case covered"),
            )
            .unwrap();
        assert_eq!(passed.status, "blocked");
        assert!(passed.missing_evidence.iter().any(|item| {
            item.contains("缺少实际 regression history passed 状态")
                || item.contains("缺少实际 release evidence ready 状态")
        }));

        let _dependency_policy = crate::dependency_policy::force_test_dependency_policy_passed();
        prepare_actual_agent_gate_evidence(temp.path());
        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: rayman eval run passed; rayman security audit passed; local rayman release evidence ready; regression history written to .RaymanCodingSkill/regression/history.jsonl; debug build passed; release build passed; relevant context checked; independent-question negative case covered"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed", "{:?}", passed.missing_evidence);
    }

    #[test]
    fn research_agent_autonomy_gate_requires_whitelist_and_reconcile_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "implement autonomous scientist agent with multi-agent research experiments",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: scientist agent implemented"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");
        assert!(blocked.missing_evidence.iter().any(|item| {
            item.contains("research_agent_autonomy") && item.contains("rayman research run")
        }));

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: rayman research run passed; whitelist command policy evidence captured with allowed argv; can_edit_files=false; can_close_goals=false; research reconcile completed; conflict reconciled; rayman eval run passed; rayman security audit passed; local rayman release evidence ready; regression history written; relevant context checked; independent-question negative case covered; debug build passed; release build passed; absolute date conversion completed for 2026-06-08; current evidence/source verification complete"),
            )
            .unwrap();
        assert_eq!(passed.status, "blocked");

        let _dependency_policy = crate::dependency_policy::force_test_dependency_policy_passed();
        prepare_actual_agent_gate_evidence(temp.path());
        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: rayman research run passed; whitelist command policy evidence captured with allowed argv; can_edit_files=false; can_close_goals=false; research reconcile completed; conflict reconciled; rayman eval run passed; rayman security audit passed; local rayman release evidence ready; regression history written; relevant context checked; independent-question negative case covered; debug build passed; release build passed; absolute date conversion completed for 2026-06-08; current evidence/source verification complete"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed", "{:?}", passed.missing_evidence);
    }

    #[test]
    fn codex_harness_execution_contract_blocks_keyword_only_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "implement Codex harness execution contract self-improvement",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: Codex harness improvement implemented"))
            .unwrap();

        assert_eq!(blocked.status, "blocked");
        assert!(blocked.missing_evidence.iter().any(|item| {
            item.contains("codex_harness_execution_contract") && item.contains("Codex manual")
        }));
    }

    #[test]
    fn codex_harness_execution_contract_accepts_complete_boundary_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "implement Codex harness execution contract self-improvement",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: official Codex manual source mapping to documented Codex execution controls verified on 2026-06-11; sandbox approval boundary and approval_policy/sandbox_mode handling verified; AGENTS.md skills hooks MCP/rules durable instruction-surface mapping recorded; subagent inheritance verified with rayman subagent status, primary-agent review, read-only boundary, and no overlap conflict disposition; non-interactive approval failure and escalation failure handling documented"),
            )
            .unwrap();

        assert_eq!(passed.status, "passed", "{:?}", passed.missing_evidence);
    }

    #[test]
    fn managed_temp_gate_requires_diagnostic_and_cleanup_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "fix recurring temp directory failures",
                "feature_update",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: fixed stale temp directory issue"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");
        assert!(
            blocked
                .missing_evidence
                .iter()
                .any(|item| item.contains("rayman temp status"))
        );

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: rayman temp status and rayman temp doctor passed; rayman temp cleanup --stale removed only managed temp metadata.json entries; TempManager uses workspace-local temp .RaymanCodingSkill/tmp with no unmanaged system temp"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed");
    }

    #[test]
    fn obsolete_asset_gate_requires_inventory_sync_and_audit_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "cleanup obsolete assets after feature replacement",
                "feature_update",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(&goal, Some("req_1: removed stale entrypoint"))
            .unwrap();
        assert_eq!(blocked.status, "blocked");
        assert!(
            blocked
                .missing_evidence
                .iter()
                .any(|item| item.contains("obsolete asset inventory"))
        );

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: obsolete asset inventory covered affected paths; replacement behavior verified; docs/config/tests sync completed; rayman audit passed"),
            )
            .unwrap();
        assert_eq!(passed.status, "passed");
    }

    #[test]
    fn audit_failure_gate_blocks_unrelated_audit_downgrade() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "finish feature despite rayman audit failed on old docs",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let blocked = quality
            .gate_goal(
                &goal,
                Some("req_1: tests passed; rayman audit failed but old docs are unrelated"),
            )
            .unwrap();

        assert_eq!(blocked.status, "blocked");
        assert!(
            blocked
                .missing_evidence
                .iter()
                .any(|item| item.contains("finding triage"))
        );
        assert!(
            blocked
                .missing_evidence
                .iter()
                .any(|item| item.contains("不能仅以和本次无关"))
        );
    }

    #[test]
    fn audit_failure_gate_passes_after_triage_and_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let goals = GoalManager::new(temp.path()).unwrap();
        let goal = goals
            .start(
                "repair rayman audit failed on old docs",
                "standard_development",
                &[],
                &[],
                &[],
                &[],
            )
            .unwrap();
        let quality = QualityManager::new(temp.path()).unwrap();

        let passed = quality
            .gate_goal(
                &goal,
                Some("req_1: audit output captured from rayman audit; finding triage historical/archive candidate docs/old.md; resolved finding by syncing docs/config/tests; rayman audit passed"),
            )
            .unwrap();

        assert_eq!(passed.status, "passed");
    }

    fn prepare_actual_agent_gate_evidence(root: &Path) {
        fs::write(root.join("Cargo.lock"), "# lock").unwrap();
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        fs::write(root.join("deny.toml"), "[licenses]\n").unwrap();
        fs::write(root.join("SKILL.md"), "# skill").unwrap();
        fs::create_dir_all(root.join("target").join("release")).unwrap();
        fs::write(
            root.join("target").join("release").join(rayman_exe_name()),
            "binary",
        )
        .unwrap();
        let finished_at = now_iso();
        RegressionHistoryManager::new(root)
            .unwrap()
            .append(&RegressionRunRecord {
                id: "regression_parallel_full_1".into(),
                profile: "parallel-full".into(),
                status: "passed".into(),
                started_at: finished_at.clone(),
                finished_at,
                duration_ms: 60000,
                steps: vec![
                    RegressionStepRecord {
                        name: "agent eval".into(),
                        command: "rayman eval run --profile full".into(),
                        success: true,
                        exit_code: Some(0),
                        duration_ms: 1000,
                        stdout_tail: "passed".into(),
                        stderr_tail: String::new(),
                    },
                    RegressionStepRecord {
                        name: "security audit".into(),
                        command: "rayman security audit".into(),
                        success: true,
                        exit_code: Some(0),
                        duration_ms: 1000,
                        stdout_tail: "passed".into(),
                        stderr_tail: String::new(),
                    },
                ],
            })
            .unwrap();
    }
}
