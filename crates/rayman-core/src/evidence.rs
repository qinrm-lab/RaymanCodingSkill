use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::{display_path, ensure_within, now_iso};

const ADVISORY_MARKERS: &[&str] = &[
    "auxiliary",
    "advisory",
    "research",
    "confidence",
    "cached",
    "cache",
    "context index",
    "memory",
    "remembered",
    "summary only",
    "辅助",
    "建议",
    "研究",
    "置信",
    "缓存",
    "记忆",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Verified,
    Unknown,
    Assumption,
    Blocked,
    Advisory,
}

impl EvidenceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unknown => "unknown",
            Self::Assumption => "assumption",
            Self::Blocked => "blocked",
            Self::Advisory => "advisory",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Verified => 0,
            Self::Assumption => 1,
            Self::Advisory => 2,
            Self::Unknown => 3,
            Self::Blocked => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRefKind {
    WorkspacePath,
    ValidationCommand,
    Artifact,
    GoalState,
    SessionState,
    ContextState,
    Advisory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRef {
    pub kind: EvidenceRefKind,
    pub status: EvidenceStatus,
    pub value: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub text: String,
    pub status: EvidenceStatus,
    pub evidence_refs: Vec<EvidenceRef>,
    pub blockers: Vec<String>,
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClaimLedger {
    pub claims: Vec<Claim>,
}

impl ClaimLedger {
    pub fn new(claims: Vec<Claim>) -> Self {
        Self { claims }
    }

    pub fn status(&self) -> EvidenceStatus {
        aggregate_status(self.claims.iter().map(|claim| claim.status))
    }

    pub fn unknowns(&self) -> Vec<String> {
        self.claims
            .iter()
            .filter(|claim| claim.status == EvidenceStatus::Unknown)
            .map(|claim| claim.text.clone())
            .collect()
    }

    pub fn assumptions(&self) -> Vec<String> {
        self.claims
            .iter()
            .filter(|claim| claim.status == EvidenceStatus::Assumption)
            .map(|claim| claim.text.clone())
            .collect()
    }

    pub fn blockers(&self) -> Vec<String> {
        self.claims
            .iter()
            .filter(|claim| claim.status == EvidenceStatus::Blocked)
            .flat_map(|claim| {
                if claim.blockers.is_empty() {
                    vec![claim.text.clone()]
                } else {
                    claim.blockers.clone()
                }
            })
            .collect()
    }

    pub fn verified_count(&self) -> usize {
        self.claims
            .iter()
            .filter(|claim| claim.status == EvidenceStatus::Verified)
            .count()
    }

    pub fn unverified_claims(&self) -> Vec<&Claim> {
        self.claims
            .iter()
            .filter(|claim| claim.status != EvidenceStatus::Verified)
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceReport {
    pub workspace_path: String,
    pub generated_at: String,
    pub scope: String,
    pub status: EvidenceStatus,
    pub evidence_status: EvidenceStatus,
    pub claim_count: usize,
    pub verified_count: usize,
    pub claim_ledger: ClaimLedger,
    pub unknowns: Vec<String>,
    pub assumptions: Vec<String>,
    pub blockers: Vec<String>,
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EvidenceCheckOptions {
    pub scope: String,
    pub goal_id: Option<String>,
    pub include_advisory: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EvidenceResolver {
    workspace: PathBuf,
    validation_records: Vec<String>,
}

impl EvidenceResolver {
    pub fn new(workspace: impl Into<PathBuf>) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        Ok(Self {
            workspace,
            validation_records: Vec::new(),
        })
    }

    pub fn with_validation_records(
        workspace: impl Into<PathBuf>,
        validation_records: Vec<String>,
    ) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("无法解析工作区路径")?;
        Ok(Self {
            workspace,
            validation_records,
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn claim(&self, id: impl Into<String>, text: impl Into<String>, evidence: &str) -> Claim {
        let id = id.into();
        let text = text.into();
        let evidence = evidence.trim();
        let mut refs = Vec::new();
        let mut blockers = Vec::new();
        if evidence.is_empty() {
            blockers.push("missing current evidence; report unknown instead of success".into());
            return Claim {
                id,
                text,
                status: EvidenceStatus::Unknown,
                evidence_refs: refs,
                blockers,
                checked_at: now_iso(),
            };
        }

        if contains_blocked_marker(evidence) {
            refs.push(EvidenceRef {
                kind: EvidenceRefKind::ContextState,
                status: EvidenceStatus::Blocked,
                value: evidence.to_string(),
                detail: "evidence text contains an unresolved blocker or failed validation marker"
                    .into(),
            });
            blockers.push("evidence contains blocked/failed/unverified marker".into());
        }

        for path in evidence_path_candidates(evidence) {
            if let Ok(path) =
                ensure_within(&path, &self.workspace, "evidence path escaped workspace")
                && path.exists()
            {
                refs.push(EvidenceRef {
                    kind: EvidenceRefKind::WorkspacePath,
                    status: EvidenceStatus::Verified,
                    value: display_path(&path),
                    detail: "current workspace path exists".into(),
                });
            }
        }

        if let Some(command) = self.successful_validation_command(evidence) {
            refs.push(EvidenceRef {
                kind: EvidenceRefKind::ValidationCommand,
                status: EvidenceStatus::Verified,
                value: command,
                detail: "matches a recorded successful validation step".into(),
            });
        }

        if refs
            .iter()
            .any(|reference| reference.kind == EvidenceRefKind::WorkspacePath)
            && contains_artifact_language(evidence)
        {
            refs.push(EvidenceRef {
                kind: EvidenceRefKind::Artifact,
                status: EvidenceStatus::Verified,
                value: evidence.to_string(),
                detail: "artifact language is backed by an existing workspace path".into(),
            });
        }

        if contains_advisory_marker(evidence) {
            refs.push(EvidenceRef {
                kind: EvidenceRefKind::Advisory,
                status: EvidenceStatus::Advisory,
                value: evidence.to_string(),
                detail:
                    "advisory, cached, memory, research, or confidence-only evidence is not proof"
                        .into(),
            });
        }

        let status = if refs
            .iter()
            .any(|reference| reference.status == EvidenceStatus::Blocked)
        {
            EvidenceStatus::Blocked
        } else if refs
            .iter()
            .any(|reference| reference.status == EvidenceStatus::Verified)
        {
            EvidenceStatus::Verified
        } else if refs
            .iter()
            .any(|reference| reference.status == EvidenceStatus::Advisory)
        {
            EvidenceStatus::Advisory
        } else if contains_assumption_marker(evidence) {
            EvidenceStatus::Assumption
        } else {
            EvidenceStatus::Unknown
        };

        if matches!(status, EvidenceStatus::Unknown | EvidenceStatus::Advisory) {
            blockers.push(
                "no current workspace path, successful validation command, or evidence artifact backs this claim"
                    .into(),
            );
        }

        Claim {
            id,
            text,
            status,
            evidence_refs: refs,
            blockers,
            checked_at: now_iso(),
        }
    }

    pub fn claim_from_status(
        &self,
        id: impl Into<String>,
        text: impl Into<String>,
        status_text: &str,
        evidence: Option<&str>,
    ) -> Claim {
        let status_lower = status_text.to_ascii_lowercase();
        if status_lower == "blocked" || status_lower == "failed" {
            let mut claim = self.claim(id, text, evidence.unwrap_or(status_text));
            claim.status = EvidenceStatus::Blocked;
            if claim.blockers.is_empty() {
                claim.blockers.push(status_text.to_string());
            }
            return claim;
        }
        if status_lower == "assumption" {
            let mut claim = self.claim(id, text, evidence.unwrap_or(status_text));
            claim.status = EvidenceStatus::Assumption;
            return claim;
        }
        self.claim(id, text, evidence.unwrap_or_default())
    }

    fn successful_validation_command(&self, evidence: &str) -> Option<String> {
        let lower = evidence.to_ascii_lowercase();
        if contains_validation_negation(&lower) || !contains_validation_success(&lower) {
            return None;
        }
        let markers = validation_command_markers(&lower);
        if markers.is_empty() {
            return None;
        }
        self.validation_records.iter().find_map(|record| {
            let record_lower = record.to_ascii_lowercase();
            (!contains_validation_negation(&record_lower)
                && markers.iter().any(|marker| record_lower.contains(marker)))
            .then(|| record.clone())
        })
    }
}

pub fn aggregate_status(statuses: impl IntoIterator<Item = EvidenceStatus>) -> EvidenceStatus {
    statuses
        .into_iter()
        .max_by_key(|status| status.rank())
        .unwrap_or(EvidenceStatus::Unknown)
}

pub fn report_from_ledger(
    workspace: &Path,
    scope: impl Into<String>,
    ledger: ClaimLedger,
) -> EvidenceReport {
    let status = ledger.status();
    let blockers = ledger.blockers();
    let unknowns = ledger.unknowns();
    let assumptions = ledger.assumptions();
    let required_actions = if matches!(status, EvidenceStatus::Verified) {
        Vec::new()
    } else {
        vec![
            "Attach current workspace path evidence, recorded successful validation command, or existing evidence artifact before claiming success.".into(),
            "Downgrade unsupported claims to unknown, assumption, blocked, or advisory.".into(),
        ]
    };
    EvidenceReport {
        workspace_path: display_path(workspace),
        generated_at: now_iso(),
        scope: scope.into(),
        status,
        evidence_status: status,
        claim_count: ledger.claims.len(),
        verified_count: ledger.verified_count(),
        claim_ledger: ledger,
        unknowns,
        assumptions,
        blockers,
        required_actions,
    }
}

pub fn evidence_status_json(ledger: &ClaimLedger) -> Value {
    json!({
        "evidence_status": ledger.status().as_str(),
        "claim_ledger": ledger,
        "unknowns": ledger.unknowns(),
        "assumptions": ledger.assumptions(),
        "blockers": ledger.blockers(),
    })
}

pub fn check_workspace_evidence(
    workspace: impl Into<PathBuf>,
    options: EvidenceCheckOptions,
) -> Result<EvidenceReport> {
    let workspace = workspace.into().canonicalize()?;
    let resolver = EvidenceResolver::new(&workspace)?;
    let mut claims = Vec::new();
    match options.scope.as_str() {
        "workspace" => {
            for path in workspace_claim_paths(&workspace) {
                let evidence = path.to_string_lossy().to_string();
                claims.push(resolver.claim(
                    format!("workspace_{}", claims.len() + 1),
                    format!("workspace evidence file {}", display_path(&path)),
                    &evidence,
                ));
            }
            if claims.is_empty() {
                claims.push(Claim {
                    id: "workspace_1".into(),
                    text: "no workspace evidence files found".into(),
                    status: EvidenceStatus::Unknown,
                    evidence_refs: Vec::new(),
                    blockers: vec!["workspace has no checked evidence files".into()],
                    checked_at: now_iso(),
                });
            }
        }
        "goal" => {
            let goals_dir = workspace.join(".RaymanCodingSkill").join("goals");
            if goals_dir.exists() {
                for entry in fs::read_dir(&goals_dir)
                    .with_context(|| format!("无法读取目标目录: {}", goals_dir.display()))?
                {
                    let path = entry?.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let text = fs::read_to_string(&path)?;
                    let value: Value = serde_json::from_str(&text)?;
                    if let Some(goal_id) = &options.goal_id
                        && value.get("id").and_then(Value::as_str) != Some(goal_id.as_str())
                    {
                        continue;
                    }
                    claims.extend(goal_value_claims(&resolver, &value));
                }
            }
            if claims.is_empty() {
                claims.push(Claim {
                    id: "goal_1".into(),
                    text: "no matching goal evidence found".into(),
                    status: EvidenceStatus::Unknown,
                    evidence_refs: Vec::new(),
                    blockers: vec!["start or select a goal before claiming goal evidence".into()],
                    checked_at: now_iso(),
                });
            }
        }
        "session" => {
            let state = workspace
                .join(".RaymanCodingSkill")
                .join("pending_work.json");
            if state.exists() {
                claims.push(resolver.claim(
                    "session_1",
                    "session pending-work state exists",
                    &state.to_string_lossy(),
                ));
            } else {
                claims.push(Claim {
                    id: "session_1".into(),
                    text: "session pending-work state is absent".into(),
                    status: EvidenceStatus::Unknown,
                    evidence_refs: Vec::new(),
                    blockers: vec!["no session state artifact exists".into()],
                    checked_at: now_iso(),
                });
            }
        }
        "research" => {
            let research_dir = workspace.join(".RaymanCodingSkill").join("research");
            if research_dir.exists() {
                for entry in fs::read_dir(&research_dir).with_context(|| {
                    format!("无法读取 research 目录: {}", research_dir.display())
                })? {
                    let path = entry?.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let text = fs::read_to_string(&path)?;
                    let value: Value = serde_json::from_str(&text)?;
                    claims.extend(research_value_claims(
                        &resolver,
                        &value,
                        options.include_advisory,
                    ));
                }
            }
            if claims.is_empty() {
                claims.push(Claim {
                    id: "research_1".into(),
                    text: "no research evidence found".into(),
                    status: EvidenceStatus::Unknown,
                    evidence_refs: Vec::new(),
                    blockers: vec!["research findings are absent or advisory-only".into()],
                    checked_at: now_iso(),
                });
            }
        }
        other => {
            claims.push(Claim {
                id: "scope_1".into(),
                text: format!("unsupported evidence scope {other}"),
                status: EvidenceStatus::Blocked,
                evidence_refs: Vec::new(),
                blockers: vec!["scope must be workspace, goal, session, or research".into()],
                checked_at: now_iso(),
            });
        }
    }
    Ok(report_from_ledger(
        &workspace,
        options.scope,
        ClaimLedger::new(claims),
    ))
}

pub fn validation_records_from_steps<'a>(
    steps: impl IntoIterator<Item = (&'a str, &'a str, Option<i32>, Option<&'a str>)>,
) -> Vec<String> {
    steps
        .into_iter()
        .filter_map(|(stage, status, exit_code, evidence)| {
            (stage == "validate" && status == "succeeded" && exit_code == Some(0))
                .then(|| evidence.unwrap_or_default().trim().to_string())
                .filter(|text| !text.is_empty())
        })
        .collect()
}

pub fn evidence_path_candidates(evidence: &str) -> Vec<PathBuf> {
    let normalized = evidence.replace('\\', "/");
    normalized
        .split(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'
                )
        })
        .filter_map(|raw| {
            let token = raw
                .trim()
                .trim_matches(|ch: char| matches!(ch, ':' | '.' | '!' | '?' | '<' | '>'));
            if token.is_empty() || !looks_like_evidence_path(token) {
                return None;
            }
            Some(PathBuf::from(token))
        })
        .collect()
}

pub fn validation_command_markers(lower: &str) -> Vec<&'static str> {
    [
        "cargo test",
        "cargo fmt",
        "cargo clippy",
        "cargo build",
        "rayman ",
        "dotnet test",
        "npm test",
        "npm run",
        "pytest",
    ]
    .iter()
    .copied()
    .filter(|marker| lower.contains(marker))
    .collect()
}

pub fn contains_validation_success(lower: &str) -> bool {
    [
        " passed",
        " pass",
        "success",
        "succeeded",
        "exit 0",
        "exit:0",
        "通过",
        "成功",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

pub fn contains_validation_negation(lower: &str) -> bool {
    [
        "not passed",
        "not pass",
        "not run",
        "did not run",
        "without running",
        "failed",
        "failure",
        "skipped",
        "未运行",
        "未通过",
        "失败",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn workspace_claim_paths(workspace: &Path) -> Vec<PathBuf> {
    [
        "SKILL.md",
        "README.md",
        "Cargo.toml",
        "config/feature_coverage.yaml",
    ]
    .iter()
    .map(|path| workspace.join(path))
    .filter(|path| path.exists())
    .collect()
}

fn goal_value_claims(resolver: &EvidenceResolver, value: &Value) -> Vec<Claim> {
    let mut validation_records = Vec::new();
    if let Some(steps) = value.get("steps").and_then(Value::as_array) {
        validation_records = validation_records_from_steps(steps.iter().map(|step| {
            (
                step.get("stage")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                step.get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                step.get("exit_code")
                    .and_then(Value::as_i64)
                    .map(|code| code as i32),
                step.get("evidence").and_then(Value::as_str),
            )
        }));
    }
    let resolver =
        EvidenceResolver::with_validation_records(resolver.workspace(), validation_records)
            .unwrap_or_else(|_| resolver.clone());
    let mut claims = Vec::new();
    let goal_id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("goal_unknown");
    if let Some(requirements) = value
        .get("contract")
        .and_then(|contract| contract.get("requirements"))
        .and_then(Value::as_array)
    {
        for requirement in requirements {
            let id = requirement
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("req_unknown");
            let text = requirement
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("goal requirement");
            let status = requirement
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("pending");
            let evidence = requirement.get("evidence").and_then(Value::as_str);
            claims.push(resolver.claim_from_status(
                format!("{goal_id}_{id}"),
                format!("{id}: {text}"),
                status,
                evidence,
            ));
        }
    }
    claims
}

fn research_value_claims(
    resolver: &EvidenceResolver,
    value: &Value,
    include_advisory: bool,
) -> Vec<Claim> {
    let mut claims = Vec::new();
    let session_id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("research_unknown");
    if let Some(findings) = value.get("findings").and_then(Value::as_array) {
        for finding in findings {
            let finding_id = finding
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("finding_unknown");
            let summary = finding
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("research finding");
            let evidence = finding
                .get("evidence_refs")
                .and_then(Value::as_array)
                .map(|refs| {
                    refs.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            let mut claim =
                resolver.claim(format!("{session_id}_{finding_id}"), summary, &evidence);
            if claim.status == EvidenceStatus::Unknown && include_advisory {
                claim.status = EvidenceStatus::Advisory;
                claim.evidence_refs.push(EvidenceRef {
                    kind: EvidenceRefKind::Advisory,
                    status: EvidenceStatus::Advisory,
                    value: summary.to_string(),
                    detail: "research finding is advisory unless backed by current evidence".into(),
                });
            }
            claims.push(claim);
        }
    }
    claims
}

fn looks_like_evidence_path(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    [
        "crates/",
        "docs/",
        "config/",
        "references/",
        "src/",
        "tests/",
        "skill.md",
        "readme.md",
        "quickstart.md",
        "cargo.toml",
        "cargo.lock",
        ".raymancodingskill/",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || [
            ".rs", ".md", ".yaml", ".yml", ".json", ".toml", ".html", ".ps1", ".sh", ".py", ".ts",
            ".tsx", ".js", ".jsx", ".cs", ".go",
        ]
        .iter()
        .any(|extension| lower.contains(extension))
}

fn contains_artifact_language(evidence: &str) -> bool {
    let lower = evidence.replace('\\', "/").to_ascii_lowercase();
    [
        "evidence artifact",
        "artifact:",
        "release evidence",
        "audit report",
        "security audit report",
        "regression history",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn contains_advisory_marker(evidence: &str) -> bool {
    let lower = evidence.to_ascii_lowercase();
    ADVISORY_MARKERS.iter().any(|marker| lower.contains(marker))
}

fn contains_assumption_marker(evidence: &str) -> bool {
    let lower = evidence.to_ascii_lowercase();
    ["assumption", "assume", "假设"]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn contains_blocked_marker(evidence: &str) -> bool {
    let lower = evidence.to_ascii_lowercase();
    contains_validation_negation(&lower)
        || [
            "blocked",
            "blocker",
            "conflict",
            "unresolved",
            "unknown",
            "not verified",
            "blocked",
            "阻断",
            "冲突",
            "未解决",
            "不知道",
            "未知",
            "未验证",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
}

pub fn scan_success_claims(workspace: impl Into<PathBuf>) -> Result<Vec<String>> {
    let workspace = workspace.into().canonicalize()?;
    let mut blockers = Vec::new();
    for entry in WalkDir::new(&workspace)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if !entry.file_type().is_file() || should_skip_scan(entry.path(), &workspace) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(claims) = find_claim_ledgers(&value) {
            for claim in claims {
                let status = claim.get("status").and_then(Value::as_str).unwrap_or("");
                let text = claim.get("text").and_then(Value::as_str).unwrap_or("claim");
                if status != "verified" && looks_like_success_claim(text) {
                    blockers.push(format!(
                        "unverified_success_claim {}: {} status={}",
                        display_path(entry.path()),
                        text,
                        status
                    ));
                }
            }
        }
    }
    Ok(blockers)
}

fn find_claim_ledgers(value: &Value) -> Option<Vec<&Value>> {
    let mut out = Vec::new();
    collect_claims(value, &mut out);
    (!out.is_empty()).then_some(out)
}

fn collect_claims<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            if let Some(claims) = map
                .get("claim_ledger")
                .and_then(|ledger| ledger.get("claims"))
                .and_then(Value::as_array)
            {
                out.extend(claims);
            }
            for value in map.values() {
                collect_claims(value, out);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_claims(value, out);
            }
        }
        _ => {}
    }
}

fn looks_like_success_claim(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "success",
        "satisfied",
        "verified",
        "complete",
        "completed",
        "passed",
        "成功",
        "完成",
        "已验证",
        "通过",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn should_skip_scan(path: &Path, workspace: &Path) -> bool {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    relative.components().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        matches!(
            text.as_ref(),
            ".git" | "target" | ".tmp" | "node_modules" | "dist" | "build" | "logs"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_verifies_existing_workspace_path() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim("req_1", "README updated", "README.md updated");

        assert_eq!(claim.status, EvidenceStatus::Verified);
        assert!(
            claim
                .evidence_refs
                .iter()
                .any(|reference| reference.kind == EvidenceRefKind::WorkspacePath)
        );
    }

    #[test]
    fn resolver_rejects_missing_path_as_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim("req_1", "docs updated", "docs/MISSING.md updated");

        assert_eq!(claim.status, EvidenceStatus::Unknown);
        assert!(!claim.blockers.is_empty());
    }

    #[test]
    fn resolver_verifies_recorded_validation_command() {
        let temp = tempfile::tempdir().unwrap();
        let resolver = EvidenceResolver::with_validation_records(
            temp.path(),
            vec!["cargo test passed".into()],
        )
        .unwrap();

        let claim = resolver.claim("req_1", "tests passed", "cargo test passed");

        assert_eq!(claim.status, EvidenceStatus::Verified);
        assert!(
            claim
                .evidence_refs
                .iter()
                .any(|reference| reference.kind == EvidenceRefKind::ValidationCommand)
        );
    }

    #[test]
    fn resolver_downgrades_advisory_without_current_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim(
            "research_1",
            "research conclusion",
            "confidence 0.9 research finding",
        );

        assert_eq!(claim.status, EvidenceStatus::Advisory);
    }

    #[test]
    fn blocked_marker_overrides_path_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim(
            "req_1",
            "README verified",
            "README.md updated but not verified",
        );

        assert_eq!(claim.status, EvidenceStatus::Blocked);
    }
}
