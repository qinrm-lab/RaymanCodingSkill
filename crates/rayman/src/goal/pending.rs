use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{Goal, GoalStatus, acquire_state_lock, short_id};
use crate::file_io::{read_json, write_json};
use crate::hash::sha256_bytes;
use crate::state_paths;
use crate::timefmt::now_iso;

const PENDING_RELATIVE: &str = "pending.json";
const PENDING_CONTRACT_V2: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PendingList {
    pub items: Vec<PendingItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingItem {
    /// Version 0 is a readable legacy record. It can be listed, resolved, or
    /// explicitly migrated, but it can never authorize a frontier or native
    /// completion boundary.
    #[serde(default)]
    pub contract_version: u32,
    pub id: String,
    pub title: String,
    pub detail: String,
    pub created_at: String,
    #[serde(default)]
    pub goal_id: Option<String>,
    #[serde(default)]
    pub owner: PendingOwner,
    #[serde(default)]
    pub kind: PendingKind,
    #[serde(default)]
    pub attempts: Vec<String>,
    #[serde(default)]
    pub evidence_paths: Vec<String>,
    #[serde(default)]
    pub minimum_input: Option<String>,
    #[serde(default)]
    pub recommended_action: Option<String>,
    #[serde(default)]
    pub alternatives: Vec<String>,
    #[serde(default)]
    pub risk: Option<String>,
    #[serde(default)]
    pub resume_command: Option<String>,
    #[serde(default)]
    pub auto_resume_condition: Option<String>,
    #[serde(default)]
    pub consultation_timing: ConsultationTiming,
    #[serde(default)]
    pub background_mechanism: Option<String>,
    #[serde(default)]
    pub background_authority_evidence: Option<String>,
    #[serde(default)]
    pub background_isolation_evidence: Option<String>,
    /// Stable, task-scoped identity for one capability boundary. Legacy items
    /// deserialize without it; newly recorded non-agent boundaries require it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_sha256: Option<String>,
    /// Historical agent assertion retained for audit only.  `alias` reads the
    /// former `presentation` field, while every subsequent write makes the
    /// untrusted semantics explicit. It is never consulted by frontier or a
    /// native completion boundary.
    #[serde(
        default,
        alias = "presentation",
        skip_serializing_if = "Option::is_none"
    )]
    pub legacy_agent_assertion_untrusted: Option<LegacyAgentPresentationAssertion>,
    /// Atomic provenance for an explicit v0 -> v2 migration. The old digest is
    /// retained so an auditor can reconstruct which legacy bytes were bound to
    /// the new `(goal_id, capability_key)` contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_migration: Option<LegacyPendingMigrationProof>,
}

/// Historical self-assertion created by pre-v16 agents.  It proves neither
/// delivery nor visibility and is retained only so migration is lossless.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyAgentPresentationAssertion {
    pub presented_at: String,
    pub package_sha256: String,
    pub channel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LegacyPendingMigrationProof {
    pub migrated_at: String,
    pub from_contract_version: u32,
    pub legacy_package_sha256: String,
    pub goal_id: String,
    pub capability_key: String,
    pub boundary_class: String,
    pub new_package_sha256: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingOwner {
    #[default]
    Agent,
    Human,
    External,
}

impl PendingOwner {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "agent" => Ok(Self::Agent),
            "human" => Ok(Self::Human),
            "external" => Ok(Self::External),
            _ => bail!("未知 pending owner: {value}（可用: agent | human | external）"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Human => "human",
            Self::External => "external",
        }
    }
}

impl fmt::Display for PendingOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    #[default]
    MachineActionable,
    HumanInput,
    ExternalWait,
    DestructiveBoundary,
    HardGate,
    RepairExhausted,
    ExecutionContext,
}

impl PendingKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "machine_actionable" => Ok(Self::MachineActionable),
            "human_input" => Ok(Self::HumanInput),
            "external_wait" => Ok(Self::ExternalWait),
            "destructive_boundary" => Ok(Self::DestructiveBoundary),
            "hard_gate" => Ok(Self::HardGate),
            "repair_exhausted" => Ok(Self::RepairExhausted),
            "execution_context" => Ok(Self::ExecutionContext),
            _ => bail!(
                "未知 pending kind: {value}（可用: machine_actionable | human_input | external_wait | destructive_boundary | hard_gate | repair_exhausted | execution_context）"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MachineActionable => "machine_actionable",
            Self::HumanInput => "human_input",
            Self::ExternalWait => "external_wait",
            Self::DestructiveBoundary => "destructive_boundary",
            Self::HardGate => "hard_gate",
            Self::RepairExhausted => "repair_exhausted",
            Self::ExecutionContext => "execution_context",
        }
    }
}

impl fmt::Display for PendingKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct PendingSubmission {
    pub title: String,
    pub detail: String,
    pub goal_id: Option<String>,
    pub owner: PendingOwner,
    pub kind: PendingKind,
    pub attempts: Vec<String>,
    pub evidence_paths: Vec<String>,
    pub minimum_input: Option<String>,
    pub recommended_action: Option<String>,
    pub alternatives: Vec<String>,
    pub risk: Option<String>,
    pub resume_command: Option<String>,
    pub auto_resume_condition: Option<String>,
    pub consultation_timing: ConsultationTiming,
    pub background_mechanism: Option<String>,
    pub background_authority_evidence: Option<String>,
    pub background_isolation_evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsultationTiming {
    #[default]
    Deferred,
    Immediate,
}

impl ConsultationTiming {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "deferred" => Ok(Self::Deferred),
            "immediate" => Ok(Self::Immediate),
            _ => bail!("未知 consultation timing: {value}（可用: deferred | immediate）"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontierDecision {
    Continue,
    AskUser,
    WaitExternal,
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontierExecution {
    ContinueForeground,
    ContinueBackground,
    PausedForUser,
    WaitExternal,
    Complete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontierConsultation {
    None,
    Deferred,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrontierReport {
    pub goal_id: String,
    pub decision: FrontierDecision,
    pub ask_user_allowed: bool,
    pub execution: FrontierExecution,
    pub consultation: FrontierConsultation,
    pub background_execution_allowed: bool,
    pub reason: String,
    pub blockers: Vec<PendingItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingRender {
    pub text: String,
    pub render_sha256: String,
    pub state_sha256: String,
    pub goal_ids: Vec<String>,
    pub pending_ids: Vec<String>,
    pub package_sha256s: Vec<String>,
}

pub struct PendingStore {
    root: PathBuf,
}

impl PendingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, create_parents: bool) -> Result<PathBuf> {
        state_paths::managed_state_file(&self.root, Path::new(PENDING_RELATIVE), create_parents)
    }

    /// 损坏或被手工降级的 pending.json 必须报错：静默当空列表会让 check
    /// 放行，下一次 add/resolve 还会覆盖销毁原有边界证据。
    fn load(&self) -> Result<PendingList> {
        let list: PendingList = read_json(&self.path(false)?)?.unwrap_or_default();
        let mut capability_keys = BTreeMap::<(&str, &str), (usize, &str)>::new();
        for (index, item) in list.items.iter().enumerate() {
            item.validate_contract().map_err(|error| {
                anyhow::anyhow!(
                    "pending.json 第 {} 项（id={}）合同无效: {error}",
                    index + 1,
                    item.id
                )
            })?;
            if item.contract_version == PENDING_CONTRACT_V2
                && let (Some(goal_id), Some(key)) =
                    (item.goal_id.as_deref(), item.capability_key.as_deref())
                && let Some((previous_index, previous_id)) =
                    capability_keys.insert((goal_id, key), (index, item.id.as_str()))
            {
                bail!(
                    "pending.json (goal_id, capability_key) 重复: 第 {} 项（id={}）与第 {} 项（id={}）都声明了 ({}, {})",
                    previous_index + 1,
                    previous_id,
                    index + 1,
                    item.id,
                    goal_id,
                    key
                );
            }
        }
        Ok(list)
    }

    pub fn list(&self) -> Result<Vec<PendingItem>> {
        Ok(self.load()?.items)
    }

    pub fn add(&self, title: &str, detail: &str) -> Result<PendingItem> {
        self.add_structured(PendingSubmission {
            title: title.into(),
            detail: detail.into(),
            goal_id: None,
            owner: PendingOwner::Agent,
            kind: PendingKind::MachineActionable,
            attempts: Vec::new(),
            evidence_paths: Vec::new(),
            minimum_input: None,
            recommended_action: None,
            alternatives: Vec::new(),
            risk: None,
            resume_command: None,
            auto_resume_condition: None,
            consultation_timing: ConsultationTiming::Deferred,
            background_mechanism: None,
            background_authority_evidence: None,
            background_isolation_evidence: None,
        })
    }

    pub fn add_structured(&self, submission: PendingSubmission) -> Result<PendingItem> {
        self.add_structured_inner(submission, None, None, false)
    }

    /// Add one capability-bound record. Unlike the legacy/internal structured
    /// entry point, non-agent records here must be goal-bound and carry a
    /// stable capability/boundary identity so repeated turns cannot mint a new
    /// question merely by changing its prose.
    pub fn add_capability_bound(
        &self,
        submission: PendingSubmission,
        capability_key: Option<String>,
        boundary_class: Option<String>,
    ) -> Result<PendingItem> {
        self.add_structured_inner(submission, capability_key, boundary_class, true)
    }

    fn add_structured_inner(
        &self,
        mut submission: PendingSubmission,
        capability_key: Option<String>,
        boundary_class: Option<String>,
        enforce_capability_contract: bool,
    ) -> Result<PendingItem> {
        submission.title = normalize_text(&submission.title);
        submission.detail = normalize_text(&submission.detail);
        if submission.title.is_empty() || submission.detail.is_empty() {
            bail!("pending title 与 detail 都不能为空");
        }
        for values in [
            &mut submission.attempts,
            &mut submission.evidence_paths,
            &mut submission.alternatives,
        ] {
            values
                .iter_mut()
                .for_each(|value| *value = normalize_text(value));
            values.retain(|value| !value.is_empty());
            values.sort();
            values.dedup();
        }
        submission.goal_id = normalize_optional_text(submission.goal_id);
        submission.minimum_input = normalize_optional_text(submission.minimum_input);
        submission.recommended_action = normalize_optional_text(submission.recommended_action);
        submission.risk = normalize_optional_text(submission.risk);
        submission.resume_command = normalize_optional_text(submission.resume_command);
        submission.auto_resume_condition =
            normalize_optional_text(submission.auto_resume_condition);
        submission.background_mechanism = normalize_optional_text(submission.background_mechanism);
        submission.background_authority_evidence =
            normalize_optional_text(submission.background_authority_evidence);
        submission.background_isolation_evidence =
            normalize_optional_text(submission.background_isolation_evidence);
        let capability_key = normalize_capability_identity(capability_key, "capability-key")?;
        let boundary_class = normalize_capability_identity(boundary_class, "boundary-class")?;
        if capability_key.is_some() && submission.goal_id.is_none() {
            bail!("capability-bound pending 必须绑定 --goal");
        }

        if enforce_capability_contract && submission.owner != PendingOwner::Agent {
            if submission.goal_id.is_none() {
                bail!("non-agent capability boundary 必须绑定 --goal");
            }
            validate_capability_identity(capability_key.as_deref(), "capability-key")?;
            validate_capability_identity(boundary_class.as_deref(), "boundary-class")?;
        } else {
            if capability_key.is_some() {
                validate_capability_identity(capability_key.as_deref(), "capability-key")?;
            }
            if boundary_class.is_some() {
                validate_capability_identity(boundary_class.as_deref(), "boundary-class")?;
            }
        }

        validate_pending_owner_kind(submission.owner, submission.kind)?;
        validate_background_contract(
            submission.owner,
            submission.consultation_timing,
            submission.background_mechanism.as_deref(),
            submission.background_authority_evidence.as_deref(),
            submission.background_isolation_evidence.as_deref(),
        )?;
        if submission.owner != PendingOwner::Agent {
            validate_solution_package(&submission)?;
        }
        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list = self.load()?;
        let package_sha256 = pending_package_sha256(
            &submission,
            capability_key.as_deref(),
            boundary_class.as_deref(),
        )?;
        if let (Some(goal_id), Some(key)) =
            (submission.goal_id.as_deref(), capability_key.as_deref())
            && let Some(existing) = list.items.iter().find(|item| {
                item.contract_version == PENDING_CONTRACT_V2
                    && item.goal_id.as_deref() == Some(goal_id)
                    && item.capability_key.as_deref() == Some(key)
            })
        {
            let existing_sha256 = existing.expected_package_sha256()?;
            if existing_sha256 == package_sha256 {
                return Ok(existing.clone());
            }
            bail!(
                "pending capability contract conflict: goal={} capability_key={} existing={} submitted={}; use the existing solution package or a genuinely different capability key",
                submission.goal_id.as_deref().unwrap_or("unbound"),
                key,
                existing_sha256,
                package_sha256
            );
        }
        let now = now_iso();
        let item = PendingItem {
            contract_version: PENDING_CONTRACT_V2,
            id: short_id(
                "pending",
                &format!("{}{now}{}", submission.title, list.items.len()),
            ),
            title: submission.title,
            detail: submission.detail,
            created_at: now,
            goal_id: submission.goal_id,
            owner: submission.owner,
            kind: submission.kind,
            attempts: submission.attempts,
            evidence_paths: submission.evidence_paths,
            minimum_input: submission.minimum_input,
            recommended_action: submission.recommended_action,
            alternatives: submission.alternatives,
            risk: submission.risk,
            resume_command: submission.resume_command,
            auto_resume_condition: submission.auto_resume_condition,
            consultation_timing: submission.consultation_timing,
            background_mechanism: submission.background_mechanism,
            background_authority_evidence: submission.background_authority_evidence,
            background_isolation_evidence: submission.background_isolation_evidence,
            capability_key,
            boundary_class,
            package_sha256: Some(package_sha256),
            legacy_agent_assertion_untrusted: None,
            legacy_migration: None,
        };
        item.validate_contract()?;
        list.items.push(item.clone());
        write_json(&path, &list)?;
        Ok(item)
    }

    /// Explicitly migrate one readable legacy non-agent package.  The caller
    /// must bind the exact old digest and stable `(goal, capability)` identity;
    /// no listing/frontier operation upgrades state implicitly.
    pub fn migrate_legacy(
        &self,
        id: &str,
        goal_id: &str,
        legacy_package_sha256: &str,
        capability_key: &str,
        boundary_class: &str,
    ) -> Result<PendingItem> {
        let id = normalize_required_text(id, "pending id")?;
        let goal_id = normalize_required_text(goal_id, "goal id")?;
        let legacy_package_sha256 =
            normalize_sha256(legacy_package_sha256, "legacy-package-sha256")?;
        let capability_key =
            normalize_capability_identity(Some(capability_key.to_string()), "capability-key")?
                .expect("required capability key");
        let boundary_class =
            normalize_capability_identity(Some(boundary_class.to_string()), "boundary-class")?
                .expect("required boundary class");

        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list = self.load()?;
        let Some(index) = list.items.iter().position(|item| item.id == id) else {
            bail!("pending 不存在: {id}");
        };
        {
            let item = &list.items[index];
            if item.contract_version != 0 {
                bail!("pending {id} 已是当前或未知版本，拒绝 legacy migration");
            }
            if item.owner == PendingOwner::Agent {
                bail!("agent pending 不需要 capability-bound legacy migration");
            }
            if item.goal_id.as_deref() != Some(goal_id.as_str()) {
                bail!("legacy migration goal 不匹配；拒绝改绑历史 package");
            }
            if !item.has_complete_solution_package() {
                bail!("legacy pending {id} 缺少完整 solution package");
            }
            let expected = item.expected_package_sha256()?;
            if expected != legacy_package_sha256 {
                bail!(
                    "legacy pending package hash 不匹配: expected={} supplied={}",
                    expected,
                    legacy_package_sha256
                );
            }
        }
        if let Some(conflict) = list.items.iter().enumerate().find(|(other_index, item)| {
            *other_index != index
                && item.contract_version == PENDING_CONTRACT_V2
                && item.goal_id.as_deref() == Some(goal_id.as_str())
                && item.capability_key.as_deref() == Some(capability_key.as_str())
        }) {
            bail!(
                "pending capability contract conflict: ({}, {}) 已由 {} 使用",
                goal_id,
                capability_key,
                conflict.1.id
            );
        }

        let item = &mut list.items[index];
        item.normalize_package_fields()?;
        item.contract_version = PENDING_CONTRACT_V2;
        item.capability_key = Some(capability_key);
        item.boundary_class = Some(boundary_class);
        let new_package_sha256 = item.expected_package_sha256()?;
        item.package_sha256 = Some(new_package_sha256.clone());
        item.legacy_migration = Some(LegacyPendingMigrationProof {
            migrated_at: now_iso(),
            from_contract_version: 0,
            legacy_package_sha256,
            goal_id,
            capability_key: item
                .capability_key
                .clone()
                .expect("migration assigned capability key"),
            boundary_class: item
                .boundary_class
                .clone()
                .expect("migration assigned boundary class"),
            new_package_sha256,
        });
        item.validate_contract()?;
        let migrated = item.clone();
        write_json(&path, &list)?;
        Ok(migrated)
    }

    /// Render every currently askable human package for the supplied goals as
    /// one deterministic, locale-independent human-boundary aggregate. Host
    /// adapters decide how the current response is observed; rendering never
    /// creates a delivery or completion receipt.
    pub fn render_for_goals(&self, goals: &[Goal]) -> Result<PendingRender> {
        if goals.is_empty() {
            bail!("pending render 至少需要一个 goal");
        }
        let mut sorted_goals = goals.to_vec();
        sorted_goals.sort_by(|left, right| left.id.cmp(&right.id));
        sorted_goals.dedup_by(|left, right| left.id == right.id);
        let list = self.load()?;
        let mut selected = Vec::<PendingItem>::new();
        let mut goal_ids = Vec::<String>::new();
        for goal in &sorted_goals {
            let frontier = frontier_report(goal, relevant_items(&list.items, &goal.id));
            if frontier.decision != FrontierDecision::AskUser
                || !frontier.ask_user_allowed
                || frontier.consultation != FrontierConsultation::Ready
            {
                bail!(
                    "goal {} 当前不能 render human-boundary aggregate: decision={:?} consultation={:?} reason={}",
                    goal.id,
                    frontier.decision,
                    frontier.consultation,
                    frontier.reason
                );
            }
            let has_agent = frontier
                .blockers
                .iter()
                .any(|item| item.owner == PendingOwner::Agent);
            selected.extend(frontier.blockers.into_iter().filter(|item| {
                item.owner == PendingOwner::Human
                    && item.contract_version == PENDING_CONTRACT_V2
                    && (!has_agent || item.consultation_timing == ConsultationTiming::Immediate)
            }));
            goal_ids.push(goal.id.clone());
        }
        selected.sort_by(|left, right| {
            (
                left.goal_id.as_deref().unwrap_or_default(),
                left.capability_key.as_deref().unwrap_or_default(),
                left.id.as_str(),
            )
                .cmp(&(
                    right.goal_id.as_deref().unwrap_or_default(),
                    right.capability_key.as_deref().unwrap_or_default(),
                    right.id.as_str(),
                ))
        });
        selected.dedup_by(|left, right| left.id == right.id);
        if selected.is_empty() {
            bail!("pending render 没有当前可咨询的 v2 human solution package");
        }
        goal_ids.sort();
        goal_ids.dedup();

        let mut packages = Vec::with_capacity(selected.len());
        for item in &selected {
            let package_sha256 = item.expected_package_sha256()?;
            if item.package_sha256.as_deref() != Some(package_sha256.as_str()) {
                bail!("pending {} stored package hash 已漂移", item.id);
            }
            packages.push(HumanBoundaryAggregatePackage {
                pending_id: item.id.clone(),
                package_sha256,
                solution: CanonicalPendingPackage::from_item(item)?,
            });
        }
        let aggregate = CanonicalHumanBoundaryAggregate {
            schema: "rayman.human-boundary-aggregate.v1",
            scope: "current_response_only",
            goal_ids: goal_ids.clone(),
            packages,
        };
        let aggregate_json = serde_json::to_string_pretty(&aggregate)?;
        let text =
            format!("Rayman human-boundary solution package\n\n```json\n{aggregate_json}\n```");
        let package_sha256s = aggregate
            .packages
            .iter()
            .map(|package| package.package_sha256.clone())
            .collect::<Vec<_>>();
        let pending_ids = aggregate
            .packages
            .iter()
            .map(|package| package.pending_id.clone())
            .collect::<Vec<_>>();
        let state_sha256 = sha256_bytes(&serde_json::to_vec(&(&sorted_goals, &selected))?);
        Ok(PendingRender {
            text: text.clone(),
            render_sha256: sha256_bytes(text.as_bytes()),
            state_sha256,
            goal_ids,
            pending_ids,
            package_sha256s,
        })
    }

    /// Remove one pending item.
    ///
    /// This is the only removal path, so it must not be gated on the same
    /// load-time contract validation it is the escape from: an item that fails
    /// `validate_contract` (a hand-edited or downgraded `pending.json`) made
    /// `load()` hard-fail, which blocked `check` *and* every attempt to delete
    /// the offending item, with no CLI way out. Read the list without enforcing
    /// the contract here; every other entry point still enforces it, so an
    /// invalid record can be removed but never used.
    pub fn resolve(&self, id: &str) -> Result<bool> {
        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list: PendingList = read_json(&self.path(false)?)?.unwrap_or_default();
        let before = list.items.len();
        list.items.retain(|item| item.id != id);
        let removed = list.items.len() != before;
        if removed {
            write_json(&path, &list)?;
        }
        // Deliberately no contract re-check on the not-found path: re-running
        // `load()` here would restore the very lockout this escape hatch exists
        // to break — `resolve` would fail again on an invalid file — for the
        // sake of a nicer message. `list`/`check` still report the invalid
        // contract, so the diagnosis is never lost, only not repeated here.
        Ok(removed)
    }

    pub fn frontier(&self, goal: &Goal) -> Result<FrontierReport> {
        let list = self.load()?;
        Ok(frontier_report(goal, relevant_items(&list.items, &goal.id)))
    }

    pub(super) fn proven_non_agent_boundary(&self, goal_id: &str) -> Result<bool> {
        let relevant = self
            .list()?
            .into_iter()
            .filter(|item| item.goal_id.as_deref().is_none_or(|id| id == goal_id))
            .collect::<Vec<_>>();
        Ok(!relevant.is_empty()
            && relevant.iter().all(|item| {
                item.contract_version == PENDING_CONTRACT_V2
                    && item.goal_id.as_deref() == Some(goal_id)
                    && item.capability_key.is_some()
                    && item.boundary_class.is_some()
                    && item.owner != PendingOwner::Agent
            })
            && relevant
                .iter()
                .all(PendingItem::has_complete_solution_package))
    }
}

fn relevant_items(items: &[PendingItem], goal_id: &str) -> Vec<PendingItem> {
    items
        .iter()
        .filter(|item| {
            item.goal_id
                .as_deref()
                .is_none_or(|pending_goal_id| pending_goal_id == goal_id)
        })
        .cloned()
        .collect()
}

fn frontier_report(goal: &Goal, blockers: Vec<PendingItem>) -> FrontierReport {
    let has_agent = blockers
        .iter()
        .any(|item| item.owner == PendingOwner::Agent);
    let has_legacy_boundary = blockers.iter().any(|item| {
        item.owner != PendingOwner::Agent && item.contract_version != PENDING_CONTRACT_V2
    });
    let human = blockers
        .iter()
        .filter(|item| {
            item.owner == PendingOwner::Human && item.contract_version == PENDING_CONTRACT_V2
        })
        .collect::<Vec<_>>();
    let ready_human = human;
    let ready_now = ready_human
        .iter()
        .copied()
        .filter(|item| !has_agent || item.consultation_timing == ConsultationTiming::Immediate)
        .collect::<Vec<_>>();

    let (decision, ask_user_allowed, execution, consultation, background_execution_allowed, reason) =
        if has_legacy_boundary {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::None,
                false,
                "a legacy human/external pending contract cannot authorize consultation or completion; explicitly migrate it with its legacy package hash and stable capability identity, or resolve and replace it"
                    .into(),
            )
        } else if !ready_now.is_empty() {
            let background_execution_allowed =
                has_agent && ready_now.iter().all(|item| item.has_background_authority());
            (
                FrontierDecision::AskUser,
                true,
                if background_execution_allowed {
                    FrontierExecution::ContinueBackground
                } else {
                    FrontierExecution::PausedForUser
                },
                FrontierConsultation::Ready,
                background_execution_allowed,
                if background_execution_allowed {
                    "a complete immediate human solution package is ready; emit the exact deterministic `goal pending render` output as the complete current response while the authorized isolated background mechanism continues"
                } else if has_agent {
                    "a complete immediate human solution package is ready; emit the exact deterministic `goal pending render` output as the complete current foreground response"
                } else {
                    "a complete human solution package is ready; emit the exact deterministic `goal pending render` output as the complete current response"
                }
                .into(),
            )
        } else if has_agent && !ready_human.is_empty() {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::Deferred,
                false,
                "finish bounded safe foreground work before presenting the recorded consultation; do not emit the question in transient progress output".into(),
            )
        } else if has_agent {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::None,
                false,
                "agent-owned work remains; continue safe foreground execution".into(),
            )
        } else if blockers
            .iter()
            .any(|item| item.owner == PendingOwner::External)
        {
            (
                FrontierDecision::WaitExternal,
                false,
                FrontierExecution::WaitExternal,
                FrontierConsultation::None,
                false,
                "waiting on an external condition with a recorded auto-resume strategy".into(),
            )
        } else if goal.status == GoalStatus::Success {
            (
                FrontierDecision::Complete,
                false,
                FrontierExecution::Complete,
                FrontierConsultation::None,
                false,
                "goal is success and no pending blocker remains".into(),
            )
        } else {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::None,
                false,
                "goal is not complete and no proven human boundary exists".into(),
            )
        };

    FrontierReport {
        goal_id: goal.id.clone(),
        decision,
        ask_user_allowed,
        execution,
        consultation,
        background_execution_allowed,
        reason,
        blockers,
    }
}

#[derive(Serialize)]
struct CanonicalHumanBoundaryAggregate {
    schema: &'static str,
    scope: &'static str,
    goal_ids: Vec<String>,
    packages: Vec<HumanBoundaryAggregatePackage>,
}

#[derive(Serialize)]
struct HumanBoundaryAggregatePackage {
    pending_id: String,
    package_sha256: String,
    solution: CanonicalPendingPackage,
}

#[derive(Clone, Serialize)]
struct CanonicalPendingPackage {
    schema: &'static str,
    title: String,
    detail: String,
    goal_id: Option<String>,
    owner: PendingOwner,
    kind: PendingKind,
    attempts: Vec<String>,
    evidence_paths: Vec<String>,
    minimum_input: Option<String>,
    recommended_action: Option<String>,
    alternatives: Vec<String>,
    risk: Option<String>,
    resume_command: Option<String>,
    auto_resume_condition: Option<String>,
    consultation_timing: ConsultationTiming,
    background_mechanism: Option<String>,
    background_authority_evidence: Option<String>,
    background_isolation_evidence: Option<String>,
    capability_key: Option<String>,
    boundary_class: Option<String>,
}

impl CanonicalPendingPackage {
    fn from_submission(
        submission: &PendingSubmission,
        capability_key: Option<&str>,
        boundary_class: Option<&str>,
    ) -> Result<Self> {
        Ok(Self {
            schema: "rayman.pending.solution-package.v2",
            title: normalize_text(&submission.title),
            detail: normalize_text(&submission.detail),
            goal_id: normalize_optional_text(submission.goal_id.clone()),
            owner: submission.owner,
            kind: submission.kind,
            attempts: normalize_values(&submission.attempts),
            evidence_paths: normalize_values(&submission.evidence_paths),
            minimum_input: normalize_optional_text(submission.minimum_input.clone()),
            recommended_action: normalize_optional_text(submission.recommended_action.clone()),
            alternatives: normalize_values(&submission.alternatives),
            risk: normalize_optional_text(submission.risk.clone()),
            resume_command: normalize_optional_text(submission.resume_command.clone()),
            auto_resume_condition: normalize_optional_text(
                submission.auto_resume_condition.clone(),
            ),
            consultation_timing: submission.consultation_timing,
            background_mechanism: normalize_optional_text(submission.background_mechanism.clone()),
            background_authority_evidence: normalize_optional_text(
                submission.background_authority_evidence.clone(),
            ),
            background_isolation_evidence: normalize_optional_text(
                submission.background_isolation_evidence.clone(),
            ),
            capability_key: normalize_capability_identity(
                capability_key.map(str::to_owned),
                "capability-key",
            )?,
            boundary_class: normalize_capability_identity(
                boundary_class.map(str::to_owned),
                "boundary-class",
            )?,
        })
    }

    fn from_item(item: &PendingItem) -> Result<Self> {
        Ok(Self {
            schema: if item.contract_version == PENDING_CONTRACT_V2 {
                "rayman.pending.solution-package.v2"
            } else {
                "rayman.pending.solution-package.v1"
            },
            title: normalize_text(&item.title),
            detail: normalize_text(&item.detail),
            goal_id: normalize_optional_text(item.goal_id.clone()),
            owner: item.owner,
            kind: item.kind,
            attempts: normalize_values(&item.attempts),
            evidence_paths: normalize_values(&item.evidence_paths),
            minimum_input: normalize_optional_text(item.minimum_input.clone()),
            recommended_action: normalize_optional_text(item.recommended_action.clone()),
            alternatives: normalize_values(&item.alternatives),
            risk: normalize_optional_text(item.risk.clone()),
            resume_command: normalize_optional_text(item.resume_command.clone()),
            auto_resume_condition: normalize_optional_text(item.auto_resume_condition.clone()),
            consultation_timing: item.consultation_timing,
            background_mechanism: normalize_optional_text(item.background_mechanism.clone()),
            background_authority_evidence: normalize_optional_text(
                item.background_authority_evidence.clone(),
            ),
            background_isolation_evidence: normalize_optional_text(
                item.background_isolation_evidence.clone(),
            ),
            capability_key: normalize_capability_identity(
                item.capability_key.clone(),
                "capability-key",
            )?,
            boundary_class: normalize_capability_identity(
                item.boundary_class.clone(),
                "boundary-class",
            )?,
        })
    }

    fn sha256(&self) -> Result<String> {
        Ok(sha256_bytes(&serde_json::to_vec(self)?))
    }
}

fn pending_package_sha256(
    submission: &PendingSubmission,
    capability_key: Option<&str>,
    boundary_class: Option<&str>,
) -> Result<String> {
    CanonicalPendingPackage::from_submission(submission, capability_key, boundary_class)?.sha256()
}

pub fn normalize_human_boundary_message(value: &str) -> String {
    let mut normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized
}

fn normalize_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim()
        .to_string()
}

fn normalize_required_text(value: &str, label: &str) -> Result<String> {
    let value = normalize_text(value);
    if value.is_empty() {
        bail!("{label} 不能为空");
    }
    Ok(value)
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let text = normalize_text(&text);
        (!text.is_empty()).then_some(text)
    })
}

fn normalize_values(values: &[String]) -> Vec<String> {
    let mut normalized = values
        .iter()
        .map(|value| normalize_text(value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

fn normalize_capability_identity(value: Option<String>, label: &str) -> Result<Option<String>> {
    let Some(value) = normalize_optional_text(value) else {
        return Ok(None);
    };
    let normalized = value.to_ascii_lowercase();
    validate_capability_identity(Some(&normalized), label)?;
    Ok(Some(normalized))
}

fn validate_capability_identity(value: Option<&str>, label: &str) -> Result<()> {
    let Some(value) = value else {
        bail!("{label} 不能为空");
    };
    if value.len() > 256
        || !value.is_ascii()
        || !value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | ':' | '/' | '-')
        })
        || !value
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        || !value
            .chars()
            .last()
            .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        bail!(
            "{label} 必须是 1..=256 字节的小写稳定标识，仅允许 a-z、0-9、.、_、:、/、-，且首尾必须是字母或数字"
        );
    }
    Ok(())
}

fn normalize_sha256(value: &str, label: &str) -> Result<String> {
    let value = normalize_required_text(value, label)?.to_ascii_lowercase();
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        bail!("{label} 必须是 64 位十六进制 SHA-256");
    }
    Ok(value)
}

fn validate_solution_package(submission: &PendingSubmission) -> Result<()> {
    if submission.attempts.is_empty()
        || submission.evidence_paths.is_empty()
        || submission.minimum_input.is_none()
        || submission.recommended_action.is_none()
        || submission.alternatives.is_empty()
        || submission.risk.is_none()
        || submission.resume_command.is_none()
        || submission.auto_resume_condition.is_none()
    {
        bail!(
            "human/external blocker 必须包含 attempts、evidence-path、minimum-input、recommended、alternative、risk、resume-command 与 auto-resume-condition"
        );
    }
    Ok(())
}

fn validate_background_contract(
    owner: PendingOwner,
    consultation_timing: ConsultationTiming,
    background_mechanism: Option<&str>,
    background_authority_evidence: Option<&str>,
    background_isolation_evidence: Option<&str>,
) -> Result<()> {
    let any_background_claim = background_mechanism.is_some()
        || background_authority_evidence.is_some()
        || background_isolation_evidence.is_some();
    if !any_background_claim {
        return Ok(());
    }
    if owner != PendingOwner::Human
        || consultation_timing != ConsultationTiming::Immediate
        || background_mechanism.is_none()
        || background_authority_evidence.is_none()
        || background_isolation_evidence.is_none()
    {
        bail!(
            "后台继续必须绑定 immediate human consultation，并同时记录非空 background-mechanism、background-authority-evidence 与 background-isolation-evidence"
        );
    }
    Ok(())
}

fn validate_pending_owner_kind(owner: PendingOwner, kind: PendingKind) -> Result<()> {
    match owner {
        PendingOwner::Agent
            if matches!(
                kind,
                PendingKind::HumanInput
                    | PendingKind::ExternalWait
                    | PendingKind::DestructiveBoundary
            ) =>
        {
            bail!("agent-owned pending 不能伪装成人工/外部边界")
        }
        PendingOwner::Human
            if !matches!(
                kind,
                PendingKind::HumanInput
                    | PendingKind::DestructiveBoundary
                    | PendingKind::RepairExhausted
                    | PendingKind::ExecutionContext
            ) =>
        {
            bail!(
                "human owner 只允许 human_input/destructive_boundary/repair_exhausted/execution_context"
            )
        }
        PendingOwner::External
            if !matches!(
                kind,
                PendingKind::ExternalWait | PendingKind::RepairExhausted
            ) =>
        {
            bail!("external owner 只允许 external_wait/repair_exhausted")
        }
        _ => Ok(()),
    }
}

impl PendingItem {
    pub fn is_current_contract(&self) -> bool {
        self.contract_version == PENDING_CONTRACT_V2
    }

    fn validate_contract(&self) -> Result<()> {
        if !matches!(self.contract_version, 0 | PENDING_CONTRACT_V2) {
            bail!(
                "不支持的 pending contract_version={}（当前只接受 legacy 0 或 v{}）",
                self.contract_version,
                PENDING_CONTRACT_V2
            );
        }
        if self.id.trim().is_empty()
            || self.title.trim().is_empty()
            || self.detail.trim().is_empty()
            || self.created_at.trim().is_empty()
        {
            bail!("id、title、detail 与 created_at 都不能为空");
        }
        if self
            .goal_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            bail!("goal_id 不能是空字符串");
        }
        for (label, values) in [
            ("attempts", &self.attempts),
            ("evidence_paths", &self.evidence_paths),
            ("alternatives", &self.alternatives),
        ] {
            if values.iter().any(|value| value.trim().is_empty()) {
                bail!("{label} 不能包含空字符串");
            }
        }
        for (label, value) in [
            ("minimum_input", self.minimum_input.as_deref()),
            ("recommended_action", self.recommended_action.as_deref()),
            ("risk", self.risk.as_deref()),
            ("resume_command", self.resume_command.as_deref()),
            (
                "auto_resume_condition",
                self.auto_resume_condition.as_deref(),
            ),
            ("background_mechanism", self.background_mechanism.as_deref()),
            (
                "background_authority_evidence",
                self.background_authority_evidence.as_deref(),
            ),
            (
                "background_isolation_evidence",
                self.background_isolation_evidence.as_deref(),
            ),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                bail!("{label} 不能是空字符串");
            }
        }

        match (
            self.capability_key.as_deref(),
            self.boundary_class.as_deref(),
        ) {
            (Some(capability_key), Some(boundary_class)) => {
                validate_capability_identity(Some(capability_key), "capability_key")?;
                validate_capability_identity(Some(boundary_class), "boundary_class")?;
                if normalize_capability_identity(
                    Some(capability_key.to_string()),
                    "capability_key",
                )?
                .as_deref()
                    != Some(capability_key)
                    || normalize_capability_identity(
                        Some(boundary_class.to_string()),
                        "boundary_class",
                    )?
                    .as_deref()
                        != Some(boundary_class)
                {
                    bail!("capability_key 与 boundary_class 必须使用稳定规范化形式");
                }
                if self.goal_id.is_none() {
                    bail!("capability-bound pending 必须绑定 goal_id");
                }
            }
            (None, None) => {}
            _ => bail!("capability_key 与 boundary_class 必须同时存在或同时缺省"),
        }

        validate_pending_owner_kind(self.owner, self.kind)?;
        if self.contract_version == PENDING_CONTRACT_V2
            && self.owner != PendingOwner::Agent
            && (self.goal_id.is_none()
                || self.capability_key.is_none()
                || self.boundary_class.is_none())
        {
            bail!("v2 human/external pending 必须绑定 goal_id、capability_key 与 boundary_class");
        }
        validate_background_contract(
            self.owner,
            self.consultation_timing,
            self.background_mechanism.as_deref(),
            self.background_authority_evidence.as_deref(),
            self.background_isolation_evidence.as_deref(),
        )?;
        if self.owner != PendingOwner::Agent && !self.has_complete_solution_package() {
            bail!("human/external blocker 缺少完整 solution package，不能作为咨询或等待边界");
        }

        let expected_package_sha256 = self.expected_package_sha256()?;
        if self.contract_version == PENDING_CONTRACT_V2 && self.package_sha256.is_none() {
            bail!("v2 pending 必须携带 canonical package_sha256");
        }
        if let Some(stored_sha256) = self.package_sha256.as_deref() {
            let normalized = normalize_sha256(stored_sha256, "package_sha256")?;
            if normalized != stored_sha256 {
                bail!("package_sha256 必须使用小写规范化形式");
            }
            if stored_sha256 != expected_package_sha256 {
                bail!(
                    "stored package_sha256 与 solution package 不匹配: stored={} expected={}",
                    stored_sha256,
                    expected_package_sha256
                );
            }
            let mut normalized_item = self.clone();
            normalized_item.normalize_package_fields()?;
            if normalized_item != *self {
                bail!("带 package_sha256 的 pending solution package 必须使用稳定规范化形式");
            }
        }

        if let Some(migration) = self.legacy_migration.as_ref() {
            if self.contract_version != PENDING_CONTRACT_V2 {
                bail!("legacy migration proof 只能附着在 v2 pending");
            }
            if migration.from_contract_version != 0 {
                bail!("legacy migration proof 必须声明 from_contract_version=0");
            }
            let migrated_at = normalize_required_text(&migration.migrated_at, "migrated_at")?;
            let legacy_sha256 = normalize_sha256(
                &migration.legacy_package_sha256,
                "legacy migration package sha256",
            )?;
            let new_sha256 = normalize_sha256(
                &migration.new_package_sha256,
                "legacy migration new package sha256",
            )?;
            let goal_id = normalize_required_text(&migration.goal_id, "migration goal_id")?;
            let capability_key = normalize_capability_identity(
                Some(migration.capability_key.clone()),
                "migration capability_key",
            )?
            .expect("required migration capability key");
            let boundary_class = normalize_capability_identity(
                Some(migration.boundary_class.clone()),
                "migration boundary_class",
            )?
            .expect("required migration boundary class");
            if migrated_at != migration.migrated_at
                || legacy_sha256 != migration.legacy_package_sha256
                || new_sha256 != migration.new_package_sha256
                || goal_id != migration.goal_id
                || capability_key != migration.capability_key
                || boundary_class != migration.boundary_class
            {
                bail!("legacy migration proof 必须使用稳定规范化形式");
            }
            if self.goal_id.as_deref() != Some(migration.goal_id.as_str())
                || self.capability_key.as_deref() != Some(migration.capability_key.as_str())
                || self.boundary_class.as_deref() != Some(migration.boundary_class.as_str())
                || self.package_sha256.as_deref() != Some(migration.new_package_sha256.as_str())
                || expected_package_sha256 != migration.new_package_sha256
            {
                bail!("legacy migration proof 与当前 v2 package identity 不匹配");
            }
        }

        if let Some(receipt) = self.legacy_agent_assertion_untrusted.as_ref() {
            if self.owner != PendingOwner::Human {
                bail!("legacy agent presentation assertion 只允许绑定 owner=human");
            }
            if self.goal_id.is_none() {
                bail!("legacy agent presentation assertion 必须绑定明确 goal_id");
            }
            let presented_at = normalize_required_text(&receipt.presented_at, "presented_at")?;
            let receipt_sha256 =
                normalize_sha256(&receipt.package_sha256, "legacy assertion package_sha256")?;
            let channel = normalize_required_text(&receipt.channel, "legacy assertion channel")?;
            let reference = normalize_optional_text(receipt.reference.clone());
            if presented_at != receipt.presented_at
                || receipt_sha256 != receipt.package_sha256
                || channel != receipt.channel
                || reference != receipt.reference
            {
                bail!("legacy agent presentation assertion 必须使用稳定规范化形式");
            }
            if channel.chars().any(char::is_control) || channel.len() > 128 {
                bail!("legacy assertion channel 必须是长度不超过 128 的单行非控制字符文本");
            }
            if reference.as_deref().is_some_and(|reference| {
                reference.chars().any(char::is_control) || reference.len() > 2048
            }) {
                bail!(
                    "legacy assertion reference 必须缺省或为长度不超过 2048 的单行非控制字符文本"
                );
            }
            if self.contract_version == 0
                && (self.package_sha256.as_deref() != Some(receipt.package_sha256.as_str())
                    || receipt.package_sha256 != expected_package_sha256)
            {
                bail!("legacy assertion 未绑定其 legacy stored solution package hash");
            }
            if self.contract_version == PENDING_CONTRACT_V2
                && self.legacy_migration.as_ref().is_some_and(|migration| {
                    receipt.package_sha256 != migration.legacy_package_sha256
                })
            {
                bail!("legacy assertion 未绑定 migration proof 中的旧 package hash");
            }
        }
        Ok(())
    }

    /// Recompute the stable digest of the complete solution package. The
    /// identifier, creation timestamp, stored digest, and legacy agent
    /// assertion are deliberately excluded: they are metadata about the
    /// package, not part of the package itself.
    pub fn expected_package_sha256(&self) -> Result<String> {
        CanonicalPendingPackage::from_item(self)?.sha256()
    }

    fn normalize_package_fields(&mut self) -> Result<()> {
        self.title = normalize_text(&self.title);
        self.detail = normalize_text(&self.detail);
        self.goal_id = normalize_optional_text(self.goal_id.take());
        self.attempts = normalize_values(&self.attempts);
        self.evidence_paths = normalize_values(&self.evidence_paths);
        self.minimum_input = normalize_optional_text(self.minimum_input.take());
        self.recommended_action = normalize_optional_text(self.recommended_action.take());
        self.alternatives = normalize_values(&self.alternatives);
        self.risk = normalize_optional_text(self.risk.take());
        self.resume_command = normalize_optional_text(self.resume_command.take());
        self.auto_resume_condition = normalize_optional_text(self.auto_resume_condition.take());
        self.background_mechanism = normalize_optional_text(self.background_mechanism.take());
        self.background_authority_evidence =
            normalize_optional_text(self.background_authority_evidence.take());
        self.background_isolation_evidence =
            normalize_optional_text(self.background_isolation_evidence.take());
        self.capability_key =
            normalize_capability_identity(self.capability_key.take(), "capability_key")?;
        self.boundary_class =
            normalize_capability_identity(self.boundary_class.take(), "boundary_class")?;
        Ok(())
    }

    pub fn has_complete_solution_package(&self) -> bool {
        self.owner == PendingOwner::Agent
            || (!self.attempts.is_empty()
                && !self.evidence_paths.is_empty()
                && self
                    .minimum_input
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                && self
                    .recommended_action
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                && !self.alternatives.is_empty()
                && self
                    .risk
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                && self
                    .resume_command
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
                && self
                    .auto_resume_condition
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()))
    }

    fn has_background_authority(&self) -> bool {
        self.consultation_timing == ConsultationTiming::Immediate
            && self
                .background_mechanism
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .background_authority_evidence
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .background_isolation_evidence
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }
}
