use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{Goal, GoalStatus, acquire_state_lock, short_id};
use crate::file_io::{read_json, write_json};
use crate::state_paths;
use crate::timefmt::now_iso;

const PENDING_RELATIVE: &str = "pending.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PendingList {
    pub items: Vec<PendingItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingItem {
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
    pub background_authorized: bool,
    #[serde(default)]
    pub background_isolated: bool,
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
            _ => bail!(
                "未知 pending kind: {value}（可用: machine_actionable | human_input | external_wait | destructive_boundary | hard_gate | repair_exhausted）"
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
    pub background_authorized: bool,
    pub background_isolated: bool,
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
    Presented,
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
        for (index, item) in list.items.iter().enumerate() {
            item.validate_contract().map_err(|error| {
                anyhow::anyhow!(
                    "pending.json 第 {} 项（id={}）合同无效: {error}",
                    index + 1,
                    item.id
                )
            })?;
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
            background_authorized: false,
            background_isolated: false,
        })
    }

    pub fn add_structured(&self, mut submission: PendingSubmission) -> Result<PendingItem> {
        submission.title = submission.title.trim().to_string();
        submission.detail = submission.detail.trim().to_string();
        if submission.title.is_empty() || submission.detail.is_empty() {
            bail!("pending title 与 detail 都不能为空");
        }
        for values in [
            &mut submission.attempts,
            &mut submission.evidence_paths,
            &mut submission.alternatives,
        ] {
            values.retain(|value| !value.trim().is_empty());
            values
                .iter_mut()
                .for_each(|value| *value = value.trim().into());
            values.sort();
            values.dedup();
        }
        let trim_optional = |value: Option<String>| {
            value.and_then(|text| {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
        };
        submission.goal_id = trim_optional(submission.goal_id);
        submission.minimum_input = trim_optional(submission.minimum_input);
        submission.recommended_action = trim_optional(submission.recommended_action);
        submission.risk = trim_optional(submission.risk);
        submission.resume_command = trim_optional(submission.resume_command);
        submission.auto_resume_condition = trim_optional(submission.auto_resume_condition);
        submission.background_mechanism = trim_optional(submission.background_mechanism);

        validate_pending_owner_kind(submission.owner, submission.kind)?;
        validate_background_contract(
            submission.owner,
            submission.consultation_timing,
            submission.background_mechanism.as_deref(),
            submission.background_authorized,
            submission.background_isolated,
        )?;
        if submission.owner != PendingOwner::Agent {
            validate_solution_package(&submission)?;
        }
        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list = self.load()?;
        let now = now_iso();
        let item = PendingItem {
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
            background_authorized: submission.background_authorized,
            background_isolated: submission.background_isolated,
        };
        list.items.push(item.clone());
        write_json(&path, &list)?;
        Ok(item)
    }

    pub fn resolve(&self, id: &str) -> Result<bool> {
        let path = self.path(true)?;
        let _lock = acquire_state_lock(&path)?;
        let mut list = self.load()?;
        let before = list.items.len();
        list.items.retain(|item| item.id != id);
        let removed = list.items.len() != before;
        if removed {
            write_json(&path, &list)?;
        }
        Ok(removed)
    }

    pub fn frontier(&self, goal: &Goal) -> Result<FrontierReport> {
        let blockers = self
            .list()?
            .into_iter()
            .filter(|item| {
                item.goal_id
                    .as_deref()
                    .is_none_or(|goal_id| goal_id == goal.id)
            })
            .collect::<Vec<_>>();
        let has_agent = blockers
            .iter()
            .any(|item| item.owner == PendingOwner::Agent);
        let human = blockers
            .iter()
            .filter(|item| item.owner == PendingOwner::Human)
            .collect::<Vec<_>>();
        let immediate_human = human
            .iter()
            .copied()
            .filter(|item| item.consultation_timing == ConsultationTiming::Immediate)
            .collect::<Vec<_>>();
        let present_human = !human.is_empty() && (!has_agent || !immediate_human.is_empty());
        let background_execution_allowed = has_agent
            && present_human
            && immediate_human
                .iter()
                .all(|item| item.has_background_authority());
        let (decision, ask_user_allowed, execution, consultation, reason) = if has_agent
            && present_human
            && background_execution_allowed
        {
            (
                FrontierDecision::AskUser,
                true,
                FrontierExecution::ContinueBackground,
                FrontierConsultation::Presented,
                "present the immediate consultation as a stable user-facing handoff; only the recorded authorized isolated background mechanism may continue".into(),
            )
        } else if has_agent && present_human {
            (
                FrontierDecision::AskUser,
                true,
                FrontierExecution::PausedForUser,
                FrontierConsultation::Presented,
                "present the immediate consultation as the final foreground handoff and pause; no authorized isolated background mechanism is recorded".into(),
            )
        } else if has_agent && !human.is_empty() {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::Deferred,
                "finish bounded safe foreground work before presenting the recorded consultation; do not emit the question in transient progress output".into(),
            )
        } else if has_agent {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::None,
                "agent-owned work remains; continue safe foreground execution".into(),
            )
        } else if !human.is_empty() {
            (
                FrontierDecision::AskUser,
                true,
                FrontierExecution::PausedForUser,
                FrontierConsultation::Presented,
                "present the complete solution package as the final foreground handoff and pause"
                    .into(),
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
                "waiting on an external condition with a recorded auto-resume strategy".into(),
            )
        } else if goal.status == GoalStatus::Success {
            (
                FrontierDecision::Complete,
                false,
                FrontierExecution::Complete,
                FrontierConsultation::None,
                "goal is success and no pending blocker remains".into(),
            )
        } else {
            (
                FrontierDecision::Continue,
                false,
                FrontierExecution::ContinueForeground,
                FrontierConsultation::None,
                "goal is not complete and no proven human boundary exists".into(),
            )
        };
        Ok(FrontierReport {
            goal_id: goal.id.clone(),
            decision,
            ask_user_allowed,
            execution,
            consultation,
            background_execution_allowed,
            reason,
            blockers,
        })
    }

    pub(super) fn proven_non_agent_boundary(&self, goal_id: &str) -> Result<bool> {
        let relevant = self
            .list()?
            .into_iter()
            .filter(|item| item.goal_id.as_deref().is_none_or(|id| id == goal_id))
            .collect::<Vec<_>>();
        Ok(!relevant.is_empty()
            && relevant
                .iter()
                .all(|item| item.owner != PendingOwner::Agent)
            && relevant
                .iter()
                .all(PendingItem::has_complete_solution_package))
    }
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
    background_authorized: bool,
    background_isolated: bool,
) -> Result<()> {
    let any_background_claim =
        background_mechanism.is_some() || background_authorized || background_isolated;
    if !any_background_claim {
        return Ok(());
    }
    if owner != PendingOwner::Human
        || consultation_timing != ConsultationTiming::Immediate
        || background_mechanism.is_none()
        || !background_authorized
        || !background_isolated
    {
        bail!(
            "后台继续必须绑定 immediate human consultation，并同时记录非空 background-mechanism、--background-authorized 与 --background-isolated"
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
            ) =>
        {
            bail!("human owner 只允许 human_input/destructive_boundary/repair_exhausted")
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
    fn validate_contract(&self) -> Result<()> {
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
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                bail!("{label} 不能是空字符串");
            }
        }
        validate_pending_owner_kind(self.owner, self.kind)?;
        validate_background_contract(
            self.owner,
            self.consultation_timing,
            self.background_mechanism.as_deref(),
            self.background_authorized,
            self.background_isolated,
        )?;
        if self.owner != PendingOwner::Agent && !self.has_complete_solution_package() {
            bail!("human/external blocker 缺少完整 solution package，不能作为咨询或等待边界");
        }
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
            && self.background_authorized
            && self.background_isolated
    }
}
