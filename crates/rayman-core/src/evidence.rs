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
    "ai thinks",
    "ai says",
    "ai judged",
    "llm says",
    "llm judged",
    "model says",
    "model thinks",
    "model judgment",
    "primary ai",
    "research",
    "confidence",
    "cached",
    "cache",
    "context index",
    "memory",
    "remembered",
    "summary only",
    "ai认为",
    "ai 判断",
    "ai判断",
    "模型判断",
    "模型认为",
    "辅助",
    "辅助ai",
    "主力ai",
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
pub struct SearchEffort {
    pub source: String,
    pub query: String,
    pub status: EvidenceStatus,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CounterexampleChallenge {
    pub challenge: String,
    pub status: EvidenceStatus,
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub text: String,
    pub status: EvidenceStatus,
    pub evidence_refs: Vec<EvidenceRef>,
    #[serde(default)]
    pub search_effort: Vec<SearchEffort>,
    #[serde(default)]
    pub counterexample_challenges: Vec<CounterexampleChallenge>,
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
        let search_effort = search_effort_from_evidence(evidence);
        let counterexample_challenges = counterexample_challenges_from_evidence(evidence);
        if evidence.is_empty() {
            blockers.push("missing current evidence; report unknown instead of success".into());
            return Claim {
                id,
                text,
                status: EvidenceStatus::Unknown,
                evidence_refs: refs,
                search_effort,
                counterexample_challenges,
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

        let mut status = if refs
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

        if status == EvidenceStatus::Verified && looks_like_success_claim(&text) {
            let challenge_blockers =
                counterexample_blockers_for_success_evidence_with_resolver(evidence, self);
            if !challenge_blockers.is_empty() {
                status = EvidenceStatus::Blocked;
                blockers.extend(challenge_blockers.into_iter().map(|blocker| {
                    format!("verified_success_claim_requires_counterexample: {blocker}")
                }));
            }
        }

        Claim {
            id,
            text,
            status,
            evidence_refs: refs,
            search_effort,
            counterexample_challenges,
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
        let mut claim = self.claim(id, text, evidence.unwrap_or_default());
        if claim.status == EvidenceStatus::Verified && looks_like_success_status(status_text) {
            let challenge_blockers = counterexample_blockers_for_success_evidence_with_resolver(
                evidence.unwrap_or_default(),
                self,
            );
            if !challenge_blockers.is_empty() {
                claim.status = EvidenceStatus::Blocked;
                claim.blockers.extend(
                    challenge_blockers.into_iter().map(|blocker| {
                        format!("success_status_requires_counterexample: {blocker}")
                    }),
                );
            }
        }
        claim
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

pub fn counterexample_blockers_for_success_evidence(evidence: &str) -> Vec<String> {
    let Ok(cwd) = std::env::current_dir() else {
        return vec!["missing_evidence_resolver: cannot prove counterexample evidence".into()];
    };
    let Ok(resolver) = EvidenceResolver::new(cwd) else {
        return vec!["missing_evidence_resolver: cannot prove counterexample evidence".into()];
    };
    counterexample_blockers_for_success_evidence_with_resolver(evidence, &resolver)
}

pub fn counterexample_blockers_for_success_evidence_with_resolver(
    evidence: &str,
    resolver: &EvidenceResolver,
) -> Vec<String> {
    let mut blockers = Vec::new();
    let evidence = evidence.trim();
    if evidence.is_empty() {
        return vec![
            "missing_success_evidence: success requires current evidence plus counterexample challenge"
                .into(),
        ];
    }
    let search_effort = search_effort_from_evidence(evidence);
    if search_effort.is_empty() {
        blockers.push(
            "missing_search_effort: success evidence must state what was searched or checked before claiming success"
                .into(),
        );
    }
    for effort in &search_effort {
        if effort.status != EvidenceStatus::Verified {
            blockers.push(format!(
                "search_effort_not_cleared: {} status={}",
                effort.query,
                effort.status.as_str()
            ));
        }
        let query_claim = resolver.claim("search_effort", "search effort", &effort.query);
        let result_claim = resolver.claim(
            "search_effort_result",
            "search effort result",
            &effort.result,
        );
        if query_claim.status != EvidenceStatus::Verified
            && result_claim.status != EvidenceStatus::Verified
        {
            blockers.push(format!(
                "search_effort_missing_current_evidence: {}",
                effort.query
            ));
        }
    }
    let challenges = counterexample_challenges_from_evidence(evidence);
    if challenges.is_empty() {
        blockers.push(
            "missing_counterexample_challenge: success evidence must include a counterexample/adversarial challenge and its result"
                .into(),
        );
    }
    for challenge in challenges {
        if challenge.status != EvidenceStatus::Verified {
            blockers.push(format!(
                "counterexample_challenge_not_cleared: {} status={}",
                challenge.challenge,
                challenge.status.as_str()
            ));
        }
        if challenge.evidence_refs.is_empty() {
            blockers.push(format!(
                "counterexample_challenge_missing_evidence: {}",
                challenge.challenge
            ));
            continue;
        }
        for evidence_ref in &challenge.evidence_refs {
            let ref_claim = resolver.claim(
                "counterexample_challenge_evidence",
                "counterexample challenge evidence",
                evidence_ref,
            );
            if ref_claim.status != EvidenceStatus::Verified {
                blockers.push(format!(
                    "counterexample_challenge_unresolved_evidence: {} ref={} status={}",
                    challenge.challenge,
                    evidence_ref,
                    ref_claim.status.as_str()
                ));
            }
        }
    }
    blockers
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
                    search_effort: Vec::new(),
                    counterexample_challenges: Vec::new(),
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
                    search_effort: Vec::new(),
                    counterexample_challenges: Vec::new(),
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
                    search_effort: Vec::new(),
                    counterexample_challenges: Vec::new(),
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
                    search_effort: Vec::new(),
                    counterexample_challenges: Vec::new(),
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
                search_effort: Vec::new(),
                counterexample_challenges: Vec::new(),
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
    let resolver = EvidenceResolver::new(&workspace)?;
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
                if status == "verified" && looks_like_success_claim(text) {
                    for blocker in claim_counterexample_blockers(claim, &resolver) {
                        blockers.push(format!(
                            "unchallenged_success_claim {}: {} {}",
                            display_path(entry.path()),
                            text,
                            blocker
                        ));
                    }
                }
            }
        }
        for claim in
            find_structured_success_claims(&value, is_goal_state_file(entry.path(), &workspace))
        {
            for blocker in claim_counterexample_blockers(claim.value, &resolver) {
                blockers.push(format!(
                    "unchallenged_success_summary {}: {} status={} {}",
                    display_path(entry.path()),
                    claim.text,
                    claim.status,
                    blocker
                ));
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

#[derive(Debug)]
struct StructuredSuccessClaim<'a> {
    text: String,
    status: String,
    value: &'a Value,
}

fn find_structured_success_claims(
    value: &Value,
    allow_legacy_goal_requirement_records: bool,
) -> Vec<StructuredSuccessClaim<'_>> {
    let mut out = Vec::new();
    let mut path = Vec::new();
    collect_structured_success_claims(
        value,
        allow_legacy_goal_requirement_records,
        &mut path,
        &mut out,
    );
    out
}

fn collect_structured_success_claims<'a>(
    value: &'a Value,
    allow_legacy_goal_requirement_records: bool,
    path: &mut Vec<&'a str>,
    out: &mut Vec<StructuredSuccessClaim<'a>>,
) {
    match value {
        Value::Object(map) => {
            if let Some(claim) =
                structured_success_claim(value, map, allow_legacy_goal_requirement_records, path)
            {
                out.push(claim);
            }
            for (key, value) in map {
                path.push(key.as_str());
                collect_structured_success_claims(
                    value,
                    allow_legacy_goal_requirement_records,
                    path,
                    out,
                );
                path.pop();
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_structured_success_claims(
                    value,
                    allow_legacy_goal_requirement_records,
                    path,
                    out,
                );
            }
        }
        _ => {}
    }
}

fn structured_success_claim<'a>(
    value: &'a Value,
    map: &serde_json::Map<String, Value>,
    allow_legacy_goal_requirement_records: bool,
    path: &[&str],
) -> Option<StructuredSuccessClaim<'a>> {
    if map.contains_key("claim_ledger") || looks_like_claim_ledger_entry(map) {
        return None;
    }
    if allow_legacy_goal_requirement_records && looks_like_legacy_goal_requirement_record(map, path)
    {
        return None;
    }
    let status = first_string_field(map, &["status", "result", "outcome", "state"]);
    let evidence_status = first_string_field(map, &["evidence_status"]);
    if looks_like_research_agent_finding(map) && evidence_status != Some("verified") {
        return None;
    }
    let text = first_string_field(
        map,
        &[
            "summary",
            "validation_summary",
            "message",
            "result_summary",
            "claim",
            "text",
        ],
    )
    .unwrap_or_default();
    let status_success = status.is_some_and(looks_like_success_status);
    let evidence_verified = evidence_status == Some("verified");
    if !(status_success || evidence_verified && looks_like_success_claim(text)) {
        return None;
    }
    if text.trim().is_empty() {
        return None;
    }
    Some(StructuredSuccessClaim {
        text: text.trim().to_string(),
        status: status.or(evidence_status).unwrap_or("verified").to_string(),
        value,
    })
}

fn first_string_field<'a>(
    map: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .filter_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

fn looks_like_claim_ledger_entry(map: &serde_json::Map<String, Value>) -> bool {
    map.contains_key("text")
        && map.contains_key("status")
        && map.contains_key("evidence_refs")
        && (map.contains_key("checked_at")
            || map.contains_key("blockers")
            || map.contains_key("counterexample_challenges"))
}

fn looks_like_legacy_goal_requirement_record(
    map: &serde_json::Map<String, Value>,
    path: &[&str],
) -> bool {
    path.ends_with(&["contract", "requirements"])
        && map.get("id").and_then(Value::as_str).is_some()
        && map.get("priority").and_then(Value::as_str).is_some()
        && map.get("text").and_then(Value::as_str).is_some()
        && map.get("status").and_then(Value::as_str).is_some()
        && !map.contains_key("evidence_refs")
        && !map.contains_key("search_effort")
        && !map.contains_key("counterexample_challenges")
        && (map
            .get("evidence")
            .and_then(Value::as_str)
            .is_some_and(|evidence| !evidence.trim().is_empty())
            || map
                .get("validation_commands")
                .and_then(Value::as_array)
                .is_some_and(|commands| !commands.is_empty()))
}

fn looks_like_research_agent_finding(map: &serde_json::Map<String, Value>) -> bool {
    map.contains_key("role")
        && map.contains_key("prompt_hash")
        && map.contains_key("response_hash")
        && map.contains_key("summary")
        && map.contains_key("risk_level")
}

fn looks_like_success_claim(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "ready",
        "fixed",
        "clean",
        "success",
        "satisfied",
        "verified",
        "complete",
        "completed",
        "passed",
        "resolved",
        "成功",
        "修复",
        "干净",
        "就绪",
        "完成",
        "已验证",
        "通过",
        "可下单",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn looks_like_success_status(status: &str) -> bool {
    let lower = status.trim().to_ascii_lowercase();
    [
        "success",
        "succeeded",
        "satisfied",
        "pass",
        "passed",
        "verified",
        "complete",
        "completed",
        "ready",
        "clean",
        "fixed",
        "resolved",
        "ok",
        "成功",
        "通过",
        "完成",
        "已验证",
        "就绪",
        "可下单",
    ]
    .iter()
    .any(|marker| lower == *marker || lower.contains(marker))
}

fn claim_counterexample_blockers(claim: &Value, resolver: &EvidenceResolver) -> Vec<String> {
    let mut blockers = Vec::new();
    blockers.extend(success_claim_conflict_blockers(claim));
    let evidence_refs = claim
        .get("evidence_refs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if evidence_refs.is_empty() {
        blockers.push("missing_evidence_refs".into());
    }
    for evidence_ref in &evidence_refs {
        if let Some(status) = evidence_ref_declared_status(evidence_ref)
            && status != "verified"
        {
            blockers.push(format!("evidence_ref_not_verified status={status}"));
        }
        let Some(evidence_text) = evidence_ref_text(evidence_ref) else {
            blockers.push("evidence_ref_missing_value".into());
            continue;
        };
        let ref_claim = resolver.claim(
            "success_claim_evidence_ref",
            "claim evidence ref",
            &evidence_text,
        );
        if ref_claim.status != EvidenceStatus::Verified {
            blockers.push(format!(
                "evidence_ref_unresolved ref={} status={}",
                evidence_text,
                ref_claim.status.as_str()
            ));
        }
    }
    let searches = claim
        .get("search_effort")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if searches.is_empty() {
        blockers.push("missing_search_effort".into());
    }
    for search in &searches {
        let status = search
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if status != "verified" {
            blockers.push(format!("search_effort_not_cleared status={status}"));
        }
        let query = search.get("query").and_then(Value::as_str).unwrap_or("");
        let result = search.get("result").and_then(Value::as_str).unwrap_or("");
        if query.trim().is_empty() && result.trim().is_empty() {
            blockers.push("search_effort_missing_query_or_result".into());
            continue;
        }
        let query_claim = resolver.claim("search_effort", "search effort", query);
        let result_claim = resolver.claim("search_effort_result", "search effort result", result);
        if query_claim.status != EvidenceStatus::Verified
            && result_claim.status != EvidenceStatus::Verified
        {
            blockers.push(format!(
                "search_effort_missing_current_evidence status={} query={}",
                query_claim.status.as_str(),
                query
            ));
        }
    }
    let challenges = claim
        .get("counterexample_challenges")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if challenges.is_empty() {
        blockers.push("missing_counterexample_challenge".into());
        return blockers;
    }
    for challenge in challenges {
        let challenge_text = challenge
            .get("challenge")
            .and_then(Value::as_str)
            .unwrap_or("<unnamed>");
        let status = challenge
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if status != "verified" {
            blockers.push(format!(
                "counterexample_challenge_not_cleared challenge={challenge_text} status={status}"
            ));
        }
        let evidence_refs = challenge
            .get("evidence_refs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if evidence_refs.is_empty() {
            blockers.push(format!(
                "counterexample_challenge_missing_evidence challenge={challenge_text}"
            ));
            continue;
        }
        for evidence_ref in evidence_refs {
            let evidence_ref = evidence_ref.as_str().unwrap_or_default();
            let ref_claim = resolver.claim(
                "counterexample_challenge_evidence",
                "counterexample challenge evidence",
                evidence_ref,
            );
            if ref_claim.status != EvidenceStatus::Verified {
                blockers.push(format!(
                    "counterexample_challenge_unresolved_evidence challenge={challenge_text} ref={evidence_ref} status={}",
                    ref_claim.status.as_str()
                ));
            }
        }
    }
    blockers
}

fn success_claim_conflict_blockers(claim: &Value) -> Vec<String> {
    let mut blockers = Vec::new();
    for (field, blocker) in [
        ("blockers", "success_claim_has_blockers"),
        ("unknowns", "success_claim_has_unknowns"),
        ("assumptions", "success_claim_has_assumptions"),
    ] {
        if json_field_has_unresolved_items(claim.get(field)) {
            blockers.push(blocker.into());
        }
    }
    blockers
}

fn json_field_has_unresolved_items(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Array(values)) => values.iter().any(|value| match value {
            Value::Null => false,
            Value::String(text) => !text.trim().is_empty(),
            Value::Array(inner) => !inner.is_empty(),
            Value::Object(map) => !map.is_empty(),
            _ => true,
        }),
        Some(Value::Object(map)) => !map.is_empty(),
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(_)) => true,
        Some(Value::Null) | None => false,
    }
}

fn evidence_ref_declared_status(reference: &Value) -> Option<&str> {
    reference
        .as_object()
        .and_then(|map| map.get("status"))
        .and_then(Value::as_str)
}

fn evidence_ref_text(reference: &Value) -> Option<String> {
    if let Some(text) = reference
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Some(text.to_string());
    }
    let map = reference.as_object()?;
    ["value", "path", "file", "artifact", "command", "detail"]
        .iter()
        .filter_map(|key| map.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn search_effort_from_evidence(evidence: &str) -> Vec<SearchEffort> {
    evidence
        .lines()
        .filter(|line| contains_search_effort_marker(line))
        .map(|line| SearchEffort {
            source: "completion_evidence".into(),
            query: line.trim().to_string(),
            status: if contains_blocked_marker(line) {
                EvidenceStatus::Blocked
            } else {
                EvidenceStatus::Verified
            },
            result: line.trim().to_string(),
        })
        .collect()
}

fn counterexample_challenges_from_evidence(evidence: &str) -> Vec<CounterexampleChallenge> {
    evidence
        .lines()
        .filter(|line| contains_counterexample_marker(line))
        .map(|line| {
            let line = line.trim();
            CounterexampleChallenge {
                challenge: line.to_string(),
                status: if contains_blocked_marker(line)
                    || !contains_counterexample_clear_marker(line)
                {
                    EvidenceStatus::Blocked
                } else {
                    EvidenceStatus::Verified
                },
                evidence_refs: counterexample_evidence_refs(line),
            }
        })
        .collect()
}

fn counterexample_evidence_refs(line: &str) -> Vec<String> {
    let lower = line.to_ascii_lowercase();
    for marker in [
        "evidence_refs:",
        "evidence refs:",
        "evidence:",
        "refs:",
        "ref:",
        "proof:",
        "证据:",
        "引用:",
    ] {
        if let Some(index) = lower.find(marker) {
            let evidence = line[index + marker.len()..].trim();
            if evidence.is_empty() {
                return Vec::new();
            }
            if evidence_path_candidates(evidence).is_empty()
                && validation_command_markers(&evidence.to_ascii_lowercase()).is_empty()
            {
                return Vec::new();
            }
            return vec![evidence.to_string()];
        }
    }
    Vec::new()
}

fn contains_search_effort_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "search:",
        "searched",
        "checked:",
        "evidence search",
        "rg ",
        "git status",
        "git diff",
        "cargo test",
        "cargo fmt",
        "cargo clippy",
        "rayman ",
        "搜索",
        "查找",
        "检查",
        "已查",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn contains_counterexample_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "counterexample",
        "challenge:",
        "adversarial",
        "negative check",
        "falsification",
        "反例",
        "质证",
        "反查",
        "负例",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn contains_counterexample_clear_marker(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "cleared",
        "not found",
        "no hit",
        "passed",
        "verified",
        "未发现",
        "无命中",
        "无阻断",
        "通过",
        "已验证",
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

fn is_goal_state_file(path: &Path, workspace: &Path) -> bool {
    let relative = path.strip_prefix(workspace).unwrap_or(path);
    let components = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    components.len() == 3
        && components[0] == ".RaymanCodingSkill"
        && components[1] == "goals"
        && path.extension().and_then(|ext| ext.to_str()) == Some("json")
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

        let claim = resolver.claim("req_1", "test command evidence", "cargo test passed");

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
    fn resolver_downgrades_ai_correctness_without_current_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim(
            "req_1",
            "implementation correctness",
            "AI认为正确; model says implementation is correct",
        );

        assert_eq!(claim.status, EvidenceStatus::Advisory);
        assert!(
            claim
                .evidence_refs
                .iter()
                .any(|reference| reference.kind == EvidenceRefKind::Advisory)
        );
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

    #[test]
    fn failed_validation_overrides_ai_correctness_and_path_evidence() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::with_validation_records(
            temp.path(),
            vec!["cargo test passed".into()],
        )
        .unwrap();

        let claim = resolver.claim(
            "req_1",
            "implementation correctness",
            "README.md updated; cargo test failed; AI thinks correct",
        );

        assert_eq!(claim.status, EvidenceStatus::Blocked);
        assert!(
            claim
                .blockers
                .iter()
                .any(|blocker| blocker.contains("blocked/failed/unverified"))
        );
    }

    #[test]
    fn resolver_blocks_success_claim_without_counterexample_challenge() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim("req_1", "feature completed", "README.md updated");

        assert_eq!(claim.status, EvidenceStatus::Blocked);
        assert!(
            claim.blockers.iter().any(|blocker| {
                blocker.contains("verified_success_claim_requires_counterexample")
            })
        );
    }

    #[test]
    fn claim_from_status_blocks_satisfied_status_without_counterexample() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let claim = resolver.claim_from_status(
            "req_1",
            "neutral requirement text",
            "satisfied",
            Some("README.md"),
        );

        assert_eq!(claim.status, EvidenceStatus::Blocked);
        assert!(
            claim
                .blockers
                .iter()
                .any(|blocker| blocker.contains("success_status_requires_counterexample"))
        );
        assert!(
            claim
                .blockers
                .iter()
                .any(|blocker| blocker.contains("missing_counterexample_challenge"))
        );
    }

    #[test]
    fn marker_only_counterexample_evidence_stays_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let blockers = counterexample_blockers_for_success_evidence_with_resolver(
            "checked: not run\ncounterexample challenge: not found docs/MISSING.md",
            &resolver,
        )
        .join("\n");

        assert!(blockers.contains("search_effort_not_cleared"));
        assert!(blockers.contains("counterexample_challenge_missing_evidence"));
    }

    #[test]
    fn free_form_counterexample_requires_explicit_evidence_ref() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let blockers = counterexample_blockers_for_success_evidence_with_resolver(
            "checked: README.md\ncounterexample challenge: stale success evidence cleared by README.md",
            &resolver,
        )
        .join("\n");

        assert!(blockers.contains("counterexample_challenge_missing_evidence"));
    }

    #[test]
    fn free_form_counterexample_accepts_explicit_current_evidence_ref() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        let resolver = EvidenceResolver::new(temp.path()).unwrap();

        let blockers = counterexample_blockers_for_success_evidence_with_resolver(
            "checked: README.md\nnegative check: stale success evidence not found; evidence: README.md",
            &resolver,
        );

        assert!(blockers.is_empty());
    }

    #[test]
    fn scan_success_claims_blocks_fabricated_structured_challenge_metadata() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("claim-report.json"),
            serde_json::json!({
                "claim_ledger": {
                    "claims": [{
                        "id": "claim_1",
                        "text": "feature completed successfully",
                        "status": "verified",
                        "evidence_refs": [],
                        "search_effort": [{
                            "source": "test",
                            "query": "docs/MISSING.md",
                            "status": "blocked",
                            "result": "not run"
                        }],
                        "counterexample_challenges": [{
                            "challenge": "missing docs counterexample",
                            "status": "verified",
                            "evidence_refs": ["docs/MISSING.md"]
                        }],
                        "blockers": [],
                        "checked_at": "2026-07-03T00:00:00Z"
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let blockers = scan_success_claims(temp.path()).unwrap().join("\n");

        assert!(blockers.contains("unchallenged_success_claim"));
        assert!(blockers.contains("search_effort_not_cleared"));
        assert!(blockers.contains("counterexample_challenge_unresolved_evidence"));
    }

    #[test]
    fn scan_success_claims_blocks_success_summary_with_conflicting_blockers() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        fs::write(
            temp.path().join("summary-report.json"),
            serde_json::json!({
                "status": "success",
                "summary": "release ready",
                "evidence_refs": ["README.md"],
                "search_effort": [{
                    "source": "test",
                    "query": "README.md",
                    "status": "verified",
                    "result": "README.md"
                }],
                "counterexample_challenges": [{
                    "challenge": "missing readiness evidence counterexample cleared",
                    "status": "verified",
                    "evidence_refs": ["README.md"]
                }],
                "blockers": ["manual validation missing"]
            })
            .to_string(),
        )
        .unwrap();

        let blockers = scan_success_claims(temp.path()).unwrap().join("\n");

        assert!(blockers.contains("unchallenged_success_summary"));
        assert!(blockers.contains("success_claim_has_blockers"));
    }

    #[test]
    fn scan_success_claims_accepts_cleared_challenge_with_current_path() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        fs::write(
            temp.path().join("claim-report.json"),
            serde_json::json!({
                "claim_ledger": {
                    "claims": [{
                        "id": "claim_1",
                        "text": "feature completed successfully",
                        "status": "verified",
                        "evidence_refs": ["README.md"],
                        "search_effort": [{
                            "source": "test",
                            "query": "README.md",
                            "status": "verified",
                            "result": "README.md"
                        }],
                        "counterexample_challenges": [{
                            "challenge": "stale evidence counterexample cleared",
                            "status": "verified",
                            "evidence_refs": ["README.md"]
                        }],
                        "blockers": [],
                        "checked_at": "2026-07-03T00:00:00Z"
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        assert!(scan_success_claims(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn scan_success_claims_blocks_summary_status_success_without_challenge() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("summary-report.json"),
            serde_json::json!({
                "status": "success",
                "summary": "release ready"
            })
            .to_string(),
        )
        .unwrap();

        let blockers = scan_success_claims(temp.path()).unwrap().join("\n");

        assert!(blockers.contains("unchallenged_success_summary"));
        assert!(blockers.contains("missing_evidence_refs"));
        assert!(blockers.contains("missing_search_effort"));
        assert!(blockers.contains("missing_counterexample_challenge"));
    }

    #[test]
    fn scan_success_claims_ignores_legacy_goal_requirement_records_in_goal_state() {
        let temp = tempfile::tempdir().unwrap();
        let goals_dir = temp.path().join(".RaymanCodingSkill").join("goals");
        fs::create_dir_all(&goals_dir).unwrap();
        fs::write(
            goals_dir.join("goal_old.json"),
            serde_json::json!({
                "id": "goal_old",
                "contract": {
                    "goal": "old goal",
                    "workflow_name": "feature_update",
                    "requirements": [{
                        "id": "req_1",
                        "priority": "must",
                        "text": "neutral persisted requirement",
                        "status": "satisfied",
                        "evidence": "req_1: README.md updated; cargo test passed",
                        "validation_commands": ["cargo test"]
                    }]
                },
                "status": "success",
                "current_stage": "complete",
                "next_action": "goal complete",
                "steps": []
            })
            .to_string(),
        )
        .unwrap();

        let blockers = scan_success_claims(temp.path()).unwrap().join("\n");

        assert!(!blockers.contains("neutral persisted requirement"));
        assert!(!blockers.contains("unchallenged_success_summary"));
    }

    #[test]
    fn scan_success_claims_blocks_goal_requirement_shape_outside_goal_state() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("public-goal-report.json"),
            serde_json::json!({
                "contract": {
                    "requirements": [{
                        "id": "req_1",
                        "priority": "must",
                        "text": "release ready",
                        "status": "satisfied",
                        "evidence": "req_1: README.md updated; cargo test passed",
                        "validation_commands": ["cargo test"]
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        let blockers = scan_success_claims(temp.path()).unwrap().join("\n");

        assert!(blockers.contains("unchallenged_success_summary"));
        assert!(blockers.contains("release ready"));
        assert!(blockers.contains("missing_counterexample_challenge"));
    }

    #[test]
    fn scan_success_claims_ignores_advisory_research_execution_success() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("research-session.json"),
            serde_json::json!({
                "findings": [{
                    "id": "finding_scientist_1",
                    "role": "scientist",
                    "status": "succeeded",
                    "model_ref": "ai_ubuntu_8888/auto",
                    "prompt_hash": "abc",
                    "response_hash": "def",
                    "summary": "{\"hypotheses\":[\"The platform successfully blocks direct file edits.\"],\"experiments\":[\"Try a bypass.\"],\"risks\":[\"stale state\"],\"next_action\":\"audit logs\"}",
                    "evidence_status": "advisory",
                    "evidence_refs": [],
                    "confidence": 0.85,
                    "risk_level": "medium",
                    "created_at": "2026-07-03T00:00:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        assert!(scan_success_claims(temp.path()).unwrap().is_empty());
    }

    #[test]
    fn scan_success_claims_blocks_verified_research_success_without_challenge() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("research-session.json"),
            serde_json::json!({
                "findings": [{
                    "id": "finding_scientist_1",
                    "role": "scientist",
                    "status": "succeeded",
                    "model_ref": "ai_ubuntu_8888/auto",
                    "prompt_hash": "abc",
                    "response_hash": "def",
                    "summary": "release ready",
                    "evidence_status": "verified",
                    "evidence_refs": [],
                    "confidence": 0.85,
                    "risk_level": "medium",
                    "created_at": "2026-07-03T00:00:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let blockers = scan_success_claims(temp.path()).unwrap().join("\n");

        assert!(blockers.contains("unchallenged_success_summary"));
        assert!(blockers.contains("missing_counterexample_challenge"));
    }

    #[test]
    fn scan_success_claims_accepts_summary_status_with_current_challenge() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("README.md"), "# project").unwrap();
        fs::write(
            temp.path().join("summary-report.json"),
            serde_json::json!({
                "status": "success",
                "summary": "release ready",
                "evidence_refs": ["README.md"],
                "search_effort": [{
                    "source": "test",
                    "query": "README.md",
                    "status": "verified",
                    "result": "README.md"
                }],
                "counterexample_challenges": [{
                    "challenge": "missing readiness evidence counterexample cleared",
                    "status": "verified",
                    "evidence_refs": ["README.md"]
                }]
            })
            .to_string(),
        )
        .unwrap();

        assert!(scan_success_claims(temp.path()).unwrap().is_empty());
    }
}
