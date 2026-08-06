use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{
    HandoffContract, LaneRecord, ProgressReceipt, ReplacementAuthorityCommandRebind, WorkPackage,
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequirementKind {
    #[default]
    Must,
    Should,
}

impl RequirementKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Must => "must",
            Self::Should => "should",
        }
    }
}

impl fmt::Display for RequirementKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProofKind {
    #[default]
    Generic,
    Test,
    RepositoryGate,
    SourceFresh,
    Installation,
    Documentation,
    GitCommit,
}

impl ProofKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Test => "test",
            Self::RepositoryGate => "repository_gate",
            Self::SourceFresh => "source_fresh",
            Self::Installation => "installation",
            Self::Documentation => "documentation",
            Self::GitCommit => "git_commit",
        }
    }
}

impl std::str::FromStr for ProofKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim() {
            "generic" => Ok(Self::Generic),
            "test" => Ok(Self::Test),
            "repository_gate" => Ok(Self::RepositoryGate),
            "source_fresh" => Ok(Self::SourceFresh),
            "installation" => Ok(Self::Installation),
            "documentation" => Ok(Self::Documentation),
            "git_commit" => Ok(Self::GitCommit),
            other => bail!("未知 proof kind: {other}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequirementStatus {
    #[default]
    Open,
    Done,
}

impl RequirementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Done => "done",
        }
    }
}

impl fmt::Display for RequirementStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    Active,
    Success,
    Partial,
    Blocked,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GoalLifecycle {
    #[default]
    Current,
    Archived,
    Superseded,
}

impl GoalLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Archived => "archived",
            Self::Superseded => "superseded",
        }
    }
}

impl fmt::Display for GoalLifecycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Success => "success",
            Self::Partial => "partial",
            Self::Blocked => "blocked",
        }
    }
}

impl fmt::Display for GoalStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Requirement {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub kind: RequirementKind,
    /// None preserves the legacy generic contract; Some(kind) is an atomic typed proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_kind: Option<ProofKind>,
    #[serde(default)]
    pub status: RequirementStatus,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub validations: Vec<ValidationEvidence>,
    #[serde(default)]
    pub impacts: Vec<ImpactEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementSpec {
    pub text: String,
    pub kind: RequirementKind,
    pub proof_kind: Option<ProofKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationEvidence {
    pub command: String,
    pub recorded_at: String,
    /// The exact impact paths supplied to the same `goal validate` invocation.
    /// Requirement-level impact history is retained separately, but cannot be
    /// combined with an unrelated receipt to satisfy validation relevance.
    #[serde(default)]
    pub impact_paths: Vec<String>,
    #[serde(default)]
    pub impact_scopes: Vec<ValidationImpactScope>,
    #[serde(default)]
    pub non_code: bool,
    /// 旧的人工声明没有实际退出码/工作区绑定，只能作为迁移信息保留。
    #[serde(default)]
    pub receipt: Option<ValidationReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationReceipt {
    pub exit_code: i32,
    pub cwd: String,
    pub workspace_fingerprint_before: String,
    pub workspace_fingerprint_after: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    /// Binds the executed command and its declared impact paths to this receipt.
    /// Old receipts without this field are retained as history but cannot make
    /// a current standard/release claim.
    #[serde(default)]
    pub invocation_sha256: String,
    /// Present for executed test commands — `cargo test` and pytest alike.
    /// (`nextest` never reaches this field: it has no independent list proof
    /// and `validate_test_execution_mode` rejects it outright.) A zero-test or
    /// compile-only invocation cannot satisfy a test receipt.
    #[serde(default)]
    pub passed_tests: Option<u64>,
    #[serde(default)]
    pub listed_tests: Option<u64>,
    #[serde(default)]
    pub ignored_tests: Option<u64>,
    #[serde(default)]
    pub list_stdout_sha256: Option<String>,
    #[serde(default)]
    pub list_stderr_sha256: Option<String>,
    /// Binds this receipt to the immutable goal and requirement contract.
    #[serde(default)]
    pub contract_sha256: String,
}

pub struct ValidationReceiptSubmission {
    pub evidence: String,
    pub command: String,
    pub receipt: ValidationReceipt,
    pub impacts: Vec<ImpactEvidence>,
    pub non_code: bool,
}

pub struct AuthorityReceiptSubmission {
    pub validation: ValidationReceiptSubmission,
    pub authority: AuthorityReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceBaseline {
    pub recorded_at: String,
    pub workspace_fingerprint: String,
    pub files: BTreeMap<String, String>,
}

/// A source-of-truth comparison between a goal's opening baseline, its
/// effective aggregate plan, and one freshly hashed workspace snapshot.
///
/// This deliberately does not use Git status: Git reports a HEAD-relative
/// delta and is unavailable in supported non-Git workspaces, while the goal
/// contract is bound to its own content baseline.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalPlanDelta {
    pub baseline_fingerprint: String,
    pub current_fingerprint: String,
    pub actual_changed_paths: Vec<String>,
    pub planned_changed_paths: Vec<String>,
    pub unplanned_changed_paths: Vec<String>,
    pub plan_recorded: bool,
    pub plan_required: bool,
    pub covered: bool,
}

pub struct PlanReceiptSubmission {
    pub changed_paths: Vec<String>,
    pub review_priority: String,
    pub impacted_paths: Vec<String>,
    pub recommended_checks: Vec<String>,
}

/// Write-ahead marker for the only interval in which a plan could otherwise
/// become a post-hoc receipt.  The intent is published before the final
/// workspace compare-and-swap.  A crash or source drift leaves it in place so
/// every normal goal gate fails closed instead of silently promoting it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanPublishIntent {
    /// The enclosing goal identity. Empty is accepted only when retiring an
    /// unbound pre-v16 development record as non-success history.
    #[serde(default)]
    pub goal_id: String,
    pub prepared_at: String,
    pub kind: PlanPublishIntentKind,
    pub baseline_fingerprint: String,
    pub precheck_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_plan_sha256: Option<String>,
    pub changed_paths: Vec<String>,
    pub review_priority: String,
    pub impacted_paths: Vec<String>,
    pub recommended_checks: Vec<String>,
    pub intent_sha256: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanPublishIntentKind {
    Initial,
    Extension,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanPublicationState {
    Pending,
    Committed,
}

/// Durable proof that the plan was first published as a fail-closed intent,
/// then promoted only after the workspace still matched its precheck.  The
/// v2 plan hash includes this proof, so an older writer that drops unknown
/// fields leaves a hash it cannot validate as a legacy v1 receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanPublicationProof {
    /// The enclosing goal identity. The v3 publication hash binds it so a
    /// receipt cannot be transplanted between otherwise identical goals.
    #[serde(default)]
    pub goal_id: String,
    pub state: PlanPublicationState,
    pub intent_sha256: String,
    pub precheck_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_fingerprint: Option<String>,
    pub published_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<String>,
    pub publication_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanReceipt {
    pub recorded_at: String,
    pub baseline_fingerprint: String,
    pub changed_paths: Vec<String>,
    pub review_priority: String,
    pub impacted_paths: Vec<String>,
    pub recommended_checks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<PlanPublicationProof>,
    pub plan_sha256: String,
    /// Monotonic cumulative snapshots. The base receipt above never changes;
    /// every extension binds the previous effective hash and can only widen
    /// paths/checks or increase review priority.
    #[serde(default)]
    pub extensions: Vec<PlanExtensionReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanExtensionReceipt {
    pub recorded_at: String,
    pub previous_plan_sha256: String,
    pub changed_paths: Vec<String>,
    pub review_priority: String,
    pub impacted_paths: Vec<String>,
    pub recommended_checks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication: Option<PlanPublicationProof>,
    pub extension_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReviewReceipt {
    pub recorded_at: String,
    pub source_fingerprint: String,
    pub reviewer: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorityRunReceipt {
    pub exit_code: i32,
    pub workspace_fingerprint_before: String,
    pub workspace_fingerprint_after: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorityReceipt {
    pub requirement_id: String,
    pub command: String,
    pub recorded_at: String,
    pub workspace_fingerprint: String,
    pub repeat: u32,
    pub impact_scopes: Vec<ValidationImpactScope>,
    pub non_code: bool,
    pub invocation_sha256: String,
    pub contract_sha256: String,
    pub runs: Vec<AuthorityRunReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidationImpactScope {
    pub changed_path: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub manifest_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactEvidence {
    pub changed_path: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub manifest_path: Option<String>,
    pub direct_dependencies: Vec<String>,
    pub direct_dependents: Vec<String>,
    pub candidate_tests: Vec<String>,
    pub recommended_checks: Vec<String>,
    pub recommendation_basis: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleProof {
    pub recorded_at: String,
    pub workspace_fingerprint: String,
    pub contract_sha256: String,
    #[serde(default)]
    pub migration: Option<String>,
    /// Receipt classifier used when this historical proof was issued. Older
    /// proofs omit the field and are verified with the exact v1 policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_policy: Option<String>,
}

/// Explicit proof for a source-honest, lifecycle-only replacement.  This is
/// intentionally separate from validation receipts: it can only transfer the
/// exact mandatory contract of named unfinished goals, and it is anchored to
/// a direct, current-policy authority receipt from an archived success at the
/// same workspace identity and source fingerprint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplacementAuthorityProof {
    pub recorded_at: String,
    pub workspace_identity: String,
    pub workspace_fingerprint: String,
    pub authority_goal_id: String,
    pub authority_lifecycle_contract_sha256: String,
    pub replacement_contract_sha256: String,
    pub predecessor_contracts: BTreeMap<String, String>,
    #[serde(default)]
    pub source_delta_paths: Vec<String>,
    #[serde(default)]
    pub live_authority: ReplacementAuthorityReceipt,
    pub proof_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ReplacementAuthorityReceipt {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_rebind: Option<ReplacementAuthorityCommandRebind>,
    pub recorded_at: String,
    pub workspace_fingerprint: String,
    pub repeat: u32,
    pub invocation_sha256: String,
    pub runs: Vec<AuthorityRunReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    #[serde(default)]
    pub schema_version: u32,
    pub id: String,
    pub title: String,
    pub status: GoalStatus,
    #[serde(default)]
    pub lifecycle: GoalLifecycle,
    #[serde(default)]
    pub lifecycle_reason: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub lifecycle_proof: Option<LifecycleProof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement_authority: Option<ReplacementAuthorityProof>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub baseline: Option<WorkspaceBaseline>,
    #[serde(default)]
    pub plan_receipts: Vec<PlanReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_publish_intent: Option<PlanPublishIntent>,
    /// Explicit epoch for plan publication semantics.  Goals created before
    /// write-ahead publication shipped have no marker and are readable only
    /// through the bounded legacy policy in `plan_chain_error`; v16 never
    /// appends a new plan node to such a chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_publication_policy: Option<String>,
    #[serde(default)]
    pub review_receipts: Vec<ReviewReceipt>,
    #[serde(default)]
    pub authority_receipts: Vec<AuthorityReceipt>,
    #[serde(default)]
    pub work_packages: Vec<WorkPackage>,
    #[serde(default)]
    pub progress_receipts: Vec<ProgressReceipt>,
    #[serde(default)]
    pub lanes: Vec<LaneRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffContract>,
    pub requirements: Vec<Requirement>,
    #[serde(default, skip)]
    pub loaded_from_legacy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedValidationCommand {
    pub program: String,
    pub args: Vec<String>,
}
