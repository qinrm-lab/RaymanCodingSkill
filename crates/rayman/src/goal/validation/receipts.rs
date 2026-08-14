use super::*;

pub fn validation_scopes_for_impacts(impacts: &[ImpactEvidence]) -> Vec<ValidationImpactScope> {
    let mut scopes = impacts
        .iter()
        .map(|impact| ValidationImpactScope {
            changed_path: impact.changed_path.replace('\\', "/"),
            package: impact.package.clone(),
            manifest_path: impact
                .manifest_path
                .as_ref()
                .map(|path| path.replace('\\', "/")),
        })
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes
}

pub fn validation_invocation_sha256(command: &str, impact_paths: &[String]) -> String {
    let scopes = impact_paths
        .iter()
        .map(|path| ValidationImpactScope {
            changed_path: path.replace('\\', "/"),
            package: None,
            manifest_path: None,
        })
        .collect::<Vec<_>>();
    validation_invocation_sha256_scoped(command, &scopes, impact_paths.is_empty())
}

pub fn validation_invocation_sha256_scoped(
    command: &str,
    impact_scopes: &[ValidationImpactScope],
    non_code: bool,
) -> String {
    validation_invocation_sha256_scoped_mode(command, impact_scopes, non_code, false)
}

pub fn validation_invocation_sha256_scoped_mode(
    command: &str,
    impact_scopes: &[ValidationImpactScope],
    non_code: bool,
    workspace_snapshot: bool,
) -> String {
    let mut hasher = Sha256::new();
    if workspace_snapshot {
        hasher.update(b"rayman.workspace-snapshot-validation.v1");
        hasher.update([0]);
    }
    hasher.update(command.as_bytes());
    hasher.update([0]);
    hasher.update([u8::from(non_code)]);
    let mut scopes = impact_scopes.to_vec();
    scopes.sort();
    scopes.dedup();
    for scope in scopes {
        hasher.update(scope.changed_path.replace('\\', "/").as_bytes());
        hasher.update([0]);
        hasher.update(scope.package.as_deref().unwrap_or_default().as_bytes());
        hasher.update([0]);
        hasher.update(
            scope
                .manifest_path
                .as_deref()
                .unwrap_or_default()
                .replace('\\', "/")
                .as_bytes(),
        );
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub fn authority_invocation_sha256(
    command: &str,
    requirement_id: &str,
    repeat: u32,
    impact_scopes: &[ValidationImpactScope],
    non_code: bool,
) -> String {
    authority_invocation_sha256_mode(
        command,
        requirement_id,
        repeat,
        impact_scopes,
        non_code,
        false,
    )
}

pub fn authority_invocation_sha256_mode(
    command: &str,
    requirement_id: &str,
    repeat: u32,
    impact_scopes: &[ValidationImpactScope],
    non_code: bool,
    workspace_snapshot: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.authority-validation.v1");
    hasher.update(validation_invocation_sha256_scoped_mode(
        command,
        impact_scopes,
        non_code,
        workspace_snapshot,
    ));
    hasher.update([0]);
    hasher.update(requirement_id.as_bytes());
    hasher.update([0]);
    hasher.update(repeat.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn has_current_stable_authority_receipt(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> bool {
    has_direct_stable_authority_receipt(goal, root, current_fingerprint)
        || (goal.replacement_authority.is_some()
            && replacement_authority_error(goal, root, current_fingerprint).is_none())
}

pub fn has_current_stable_authority_receipt_with_baseline(
    goal: &Goal,
    all_goals: &[Goal],
    root: &Path,
    current: &WorkspaceBaseline,
) -> bool {
    has_direct_stable_authority_receipt_with_baseline(goal, root, current)
        || (goal.replacement_authority.is_some()
            && replacement_authority_error_with_baseline(goal, root, current, all_goals).is_none())
}

/// Capture-only authority readiness used by the readiness decision.  The
/// caller owns both the source bytes and the current baseline, so no authority
/// helper, gate path or Cargo manifest is reopened while deciding readiness.
pub fn has_current_stable_authority_receipt_with_context(
    goal: &Goal,
    all_goals: &[Goal],
    decision: &GoalDecisionContext<'_>,
) -> bool {
    has_direct_stable_authority_receipt_with_context(goal, decision)
        || (goal.replacement_authority.is_some()
            && replacement_authority_error_with_context(goal, decision, all_goals).is_none())
}

pub(in crate::goal) fn has_direct_stable_authority_receipt_with_baseline(
    goal: &Goal,
    root: &Path,
    current: &WorkspaceBaseline,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        direct_stable_authority_receipt_is_valid_with_baseline(goal, root, current, authority)
    })
}

pub(in crate::goal) fn has_direct_stable_authority_receipt_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        direct_stable_authority_receipt_is_valid_with_context(goal, decision, authority)
    })
}

pub(in crate::goal) fn has_direct_stable_authority_receipt(
    goal: &Goal,
    root: &Path,
    current_fingerprint: &str,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        direct_stable_authority_receipt_is_valid(goal, root, current_fingerprint, authority)
    })
}

pub(in crate::goal) fn has_direct_stable_authority_command(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    command: &str,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        authority.command == command
            && direct_stable_authority_receipt_is_valid(goal, root, fingerprint, authority)
    })
}

/// Validate an archived direct authority receipt from its immutable ledger
/// fields without reinterpreting a historical PowerShell dependency graph with
/// today's parser or today's helper bytes.  The replacement proof separately
/// binds the receipt-era gate closure.
pub(in crate::goal) fn has_archived_direct_stable_authority_command(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    command: &str,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        let Ok(contract_sha256) = validation_contract_sha256(goal, &authority.requirement_id)
        else {
            return false;
        };
        authority.command == command
            && authority.repeat >= 2
            && authority.runs.len() == authority.repeat as usize
            && authority.workspace_fingerprint == fingerprint
            && authority.contract_sha256 == contract_sha256
            && authority.invocation_sha256
                == authority_invocation_sha256_mode(
                    &authority.command,
                    &authority.requirement_id,
                    authority.repeat,
                    &authority.impact_scopes,
                    authority.non_code,
                    authority.workspace_snapshot,
                )
            && authority_scope_is_well_formed(authority)
            && validate_authority_command(root, &authority.command).is_ok()
            && authority.runs.iter().all(|run| {
                run.exit_code == 0
                    && run.workspace_fingerprint_before == fingerprint
                    && run.workspace_fingerprint_after == fingerprint
                    && is_sha256(&run.stdout_sha256)
                    && is_sha256(&run.stderr_sha256)
            })
    })
}

pub(in crate::goal) fn has_archived_direct_stable_authority_command_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    fingerprint: &str,
    command: &str,
) -> bool {
    goal.authority_receipts.iter().any(|authority| {
        let Ok(contract_sha256) = validation_contract_sha256(goal, &authority.requirement_id)
        else {
            return false;
        };
        authority.command == command
            && authority.repeat >= 2
            && authority.runs.len() == authority.repeat as usize
            && authority.workspace_fingerprint == fingerprint
            && authority.contract_sha256 == contract_sha256
            && authority.invocation_sha256
                == authority_invocation_sha256_mode(
                    &authority.command,
                    &authority.requirement_id,
                    authority.repeat,
                    &authority.impact_scopes,
                    authority.non_code,
                    authority.workspace_snapshot,
                )
            && authority_scope_is_well_formed(authority)
            && validate_authority_command_with_context(decision, &authority.command).is_ok()
            && authority.runs.iter().all(|run| {
                run.exit_code == 0
                    && run.workspace_fingerprint_before == fingerprint
                    && run.workspace_fingerprint_after == fingerprint
                    && is_sha256(&run.stdout_sha256)
                    && is_sha256(&run.stderr_sha256)
            })
    })
}

pub(in crate::goal) fn direct_stable_authority_receipt_is_valid(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    authority: &AuthorityReceipt,
) -> bool {
    let Ok(contract_sha256) = validation_contract_sha256(goal, &authority.requirement_id) else {
        return false;
    };
    let snapshot_delta_is_empty = !authority.workspace_snapshot
        || workspace_baseline(root)
            .and_then(|current| goal_plan_delta(goal, &current))
            .is_ok_and(|delta| delta.actual_changed_paths.is_empty());
    authority.repeat >= 2
        && authority.runs.len() == authority.repeat as usize
        && authority.workspace_fingerprint == fingerprint
        && authority.contract_sha256 == contract_sha256
        && authority.invocation_sha256
            == authority_invocation_sha256_mode(
                &authority.command,
                &authority.requirement_id,
                authority.repeat,
                &authority.impact_scopes,
                authority.non_code,
                authority.workspace_snapshot,
            )
        && authority_scope_is_well_formed(authority)
        && snapshot_delta_is_empty
        && validate_authority_command_for_goal(root, goal, &authority.command).is_ok()
        && authority.runs.iter().all(|run| {
            run.exit_code == 0
                && run.workspace_fingerprint_before == fingerprint
                && run.workspace_fingerprint_after == fingerprint
                && is_sha256(&run.stdout_sha256)
                && is_sha256(&run.stderr_sha256)
        })
}

pub(in crate::goal) fn direct_stable_authority_receipt_is_valid_with_baseline(
    goal: &Goal,
    root: &Path,
    current: &WorkspaceBaseline,
    authority: &AuthorityReceipt,
) -> bool {
    let fingerprint = &current.workspace_fingerprint;
    let Ok(contract_sha256) = validation_contract_sha256(goal, &authority.requirement_id) else {
        return false;
    };
    let snapshot_delta_is_empty = !authority.workspace_snapshot
        || goal_plan_delta(goal, current).is_ok_and(|delta| delta.actual_changed_paths.is_empty());
    authority.repeat >= 2
        && authority.runs.len() == authority.repeat as usize
        && authority.workspace_fingerprint == *fingerprint
        && authority.contract_sha256 == contract_sha256
        && authority.invocation_sha256
            == authority_invocation_sha256_mode(
                &authority.command,
                &authority.requirement_id,
                authority.repeat,
                &authority.impact_scopes,
                authority.non_code,
                authority.workspace_snapshot,
            )
        && authority_scope_is_well_formed(authority)
        && snapshot_delta_is_empty
        && validate_authority_command_for_goal(root, goal, &authority.command).is_ok()
        && authority.runs.iter().all(|run| {
            run.exit_code == 0
                && run.workspace_fingerprint_before == *fingerprint
                && run.workspace_fingerprint_after == *fingerprint
                && is_sha256(&run.stdout_sha256)
                && is_sha256(&run.stderr_sha256)
        })
}

pub(in crate::goal) fn direct_stable_authority_receipt_is_valid_with_context(
    goal: &Goal,
    decision: &GoalDecisionContext<'_>,
    authority: &AuthorityReceipt,
) -> bool {
    let Some(current) = decision.current() else {
        return false;
    };
    let fingerprint = &current.workspace_fingerprint;
    let Ok(contract_sha256) = validation_contract_sha256(goal, &authority.requirement_id) else {
        return false;
    };
    let snapshot_delta_is_empty = !authority.workspace_snapshot
        || goal_plan_delta(goal, current).is_ok_and(|delta| delta.actual_changed_paths.is_empty());
    authority.repeat >= 2
        && authority.runs.len() == authority.repeat as usize
        && authority.workspace_fingerprint == *fingerprint
        && authority.contract_sha256 == contract_sha256
        && authority.invocation_sha256
            == authority_invocation_sha256_mode(
                &authority.command,
                &authority.requirement_id,
                authority.repeat,
                &authority.impact_scopes,
                authority.non_code,
                authority.workspace_snapshot,
            )
        && authority_scope_is_well_formed(authority)
        && snapshot_delta_is_empty
        && validate_authority_command_for_goal_with_context(decision, goal, &authority.command)
            .is_ok()
        && authority.runs.iter().all(|run| {
            run.exit_code == 0
                && run.workspace_fingerprint_before == *fingerprint
                && run.workspace_fingerprint_after == *fingerprint
                && is_sha256(&run.stdout_sha256)
                && is_sha256(&run.stderr_sha256)
        })
}

#[derive(Serialize)]
struct ImmutableRequirementContract<'a> {
    id: &'a str,
    text: &'a str,
    kind: RequirementKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_kind: Option<ProofKind>,
}

#[derive(Serialize)]
struct ImmutableGoalContract<'a> {
    goal_id: &'a str,
    title: &'a str,
    created_at: &'a str,
    target_requirement_id: &'a str,
    requirements: Vec<ImmutableRequirementContract<'a>>,
}

#[derive(Serialize)]
struct ImmutableGoalIdentityContract<'a> {
    goal_id: &'a str,
    title: &'a str,
    created_at: &'a str,
    requirements: Vec<ImmutableRequirementContract<'a>>,
}

fn immutable_requirements(goal: &Goal) -> Vec<ImmutableRequirementContract<'_>> {
    goal.requirements
        .iter()
        .map(|requirement| ImmutableRequirementContract {
            id: &requirement.id,
            text: &requirement.text,
            kind: requirement.kind,
            proof_kind: requirement.proof_kind,
        })
        .collect()
}

pub fn goal_contract_sha256(goal: &Goal) -> Result<String> {
    let contract = ImmutableGoalIdentityContract {
        goal_id: &goal.id,
        title: &goal.title,
        created_at: &goal.created_at,
        requirements: immutable_requirements(goal),
    };
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&contract)?);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn authority_receipt_sha256(receipt: &AuthorityReceipt) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.authority-receipt.v1");
    hasher.update(serde_json::to_vec(receipt)?);
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn validation_contract_sha256(goal: &Goal, requirement_id: &str) -> Result<String> {
    if !goal
        .requirements
        .iter()
        .any(|requirement| requirement.id == requirement_id)
    {
        bail!("需求不存在: {requirement_id}");
    }
    let contract = ImmutableGoalContract {
        goal_id: &goal.id,
        title: &goal.title,
        created_at: &goal.created_at,
        target_requirement_id: requirement_id,
        requirements: immutable_requirements(goal),
    };
    let bytes = serde_json::to_vec(&contract)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

pub(in crate::goal) fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn receipt_identity_matches_live(
    receipt: &ValidationReceipt,
    root: &Path,
    policy: ReceiptValidationPolicy,
) -> bool {
    match policy {
        ReceiptValidationPolicy::CurrentV3 => {
            receipt.workspace_identity == workspace_identity(root)
                && is_sha256(&receipt.workspace_identity)
        }
        // v1/v2 are preserved only as historical audit records. Their old
        // path-based predicate deliberately retains its original semantics.
        ReceiptValidationPolicy::LegacyV1 | ReceiptValidationPolicy::CurrentV2 => {
            match (Path::new(&receipt.cwd).canonicalize(), root.canonicalize()) {
                (Ok(left), Ok(right)) => left == right,
                _ => false,
            }
        }
    }
}

fn receipt_identity_matches_context(
    receipt: &ValidationReceipt,
    decision: &GoalDecisionContext<'_>,
    policy: ReceiptValidationPolicy,
) -> bool {
    match policy {
        ReceiptValidationPolicy::CurrentV3 => match decision.captured_workspace_identity() {
            Some(identity) => {
                receipt.workspace_identity == identity && is_sha256(&receipt.workspace_identity)
            }
            None => receipt_identity_matches_live(receipt, decision.root(), policy),
        },
        // Capture does not reopen arbitrary receipt paths. Legacy records are
        // only readable audit history; their previous literal comparison is
        // intentionally retained rather than manufacturing an identity.
        ReceiptValidationPolicy::LegacyV1 | ReceiptValidationPolicy::CurrentV2 => {
            receipt.cwd == decision.root().display().to_string()
        }
    }
}

fn test_receipt_has_structured_proof(receipt: &ValidationReceipt) -> bool {
    matches!(
        (receipt.listed_tests, receipt.passed_tests, receipt.ignored_tests),
        (Some(listed), Some(passed), Some(ignored))
            if listed > 0 && passed > 0 && passed.saturating_add(ignored) == listed
    ) && receipt.list_stdout_sha256.as_deref().is_some_and(is_sha256)
        && receipt.list_stderr_sha256.as_deref().is_some_and(is_sha256)
}

fn validation_scope_is_well_formed(validation: &ValidationEvidence) -> bool {
    let mut paths = validation
        .impact_paths
        .iter()
        .map(|path| path.replace('\\', "/"))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let scope_paths = validation
        .impact_scopes
        .iter()
        .map(|scope| scope.changed_path.replace('\\', "/"))
        .collect::<Vec<_>>();
    paths == scope_paths
        && ((validation.workspace_snapshot
            && !validation.non_code
            && validation.impact_scopes.is_empty())
            || (!validation.workspace_snapshot
                && validation.non_code
                && validation.impact_scopes.is_empty())
            || (!validation.workspace_snapshot
                && !validation.non_code
                && !validation.impact_scopes.is_empty()))
}

pub(in crate::goal) fn authority_scope_is_well_formed(authority: &AuthorityReceipt) -> bool {
    (authority.workspace_snapshot && !authority.non_code && authority.impact_scopes.is_empty())
        || (!authority.workspace_snapshot
            && authority.non_code
            && authority.impact_scopes.is_empty())
        || (!authority.workspace_snapshot
            && !authority.non_code
            && !authority.impact_scopes.is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::goal) enum ReceiptValidationPolicy {
    /// Policy used before Python/pytest receipts gained mandatory collect proof
    /// and Python impact relevance checks.
    LegacyV1,
    /// Current receipt integrity, test-proof, and relevance policy.
    CurrentV2,
    /// Identity-bound receipts. This is the only policy eligible to carry
    /// archived success authority back into a current lifecycle decision.
    CurrentV3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GoalPlanningValidationPolicy {
    Skip,
    Current,
    RetiringLegacySuccess,
    HistoricalLegacySuccess,
}

pub fn validation_has_current_receipt(
    validation: &ValidationEvidence,
    goal: &Goal,
    requirement: &Requirement,
    root: &Path,
    current_fingerprint: &str,
) -> bool {
    if validation.workspace_snapshot
        && !workspace_baseline(root)
            .and_then(|current| goal_plan_delta(goal, &current))
            .is_ok_and(|delta| delta.actual_changed_paths.is_empty())
    {
        return false;
    }
    if !proof_kind_matches(
        requirement.proof_kind,
        validation_proof_kind(root, &validation.command)
            .ok()
            .unwrap_or_default(),
    ) {
        return false;
    }
    let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
        return false;
    };
    validation_has_receipt_for_fingerprint(
        validation,
        root,
        current_fingerprint,
        &contract_sha256,
        true,
        ReceiptValidationPolicy::CurrentV3,
    )
}

pub fn validation_has_current_receipt_with_baseline(
    validation: &ValidationEvidence,
    goal: &Goal,
    requirement: &Requirement,
    root: &Path,
    current: &WorkspaceBaseline,
) -> bool {
    if validation.workspace_snapshot
        && !goal_plan_delta(goal, current).is_ok_and(|delta| delta.actual_changed_paths.is_empty())
    {
        return false;
    }
    if !proof_kind_matches(
        requirement.proof_kind,
        validation_proof_kind(root, &validation.command)
            .ok()
            .unwrap_or_default(),
    ) {
        return false;
    }
    let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
        return false;
    };
    validation_has_receipt_for_fingerprint(
        validation,
        root,
        &current.workspace_fingerprint,
        &contract_sha256,
        true,
        ReceiptValidationPolicy::CurrentV3,
    )
}

pub(crate) fn validation_has_current_receipt_with_context(
    validation: &ValidationEvidence,
    goal: &Goal,
    requirement: &Requirement,
    decision: &GoalDecisionContext<'_>,
) -> bool {
    let Some(current) = decision.current() else {
        return false;
    };
    if validation.workspace_snapshot
        && !goal_plan_delta(goal, current).is_ok_and(|delta| delta.actual_changed_paths.is_empty())
    {
        return false;
    }
    if !proof_kind_matches(
        requirement.proof_kind,
        validation_proof_kind_with_context(decision, &validation.command)
            .ok()
            .unwrap_or_default(),
    ) {
        return false;
    }
    let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
        return false;
    };
    validation_has_receipt_for_fingerprint_with_context(
        validation,
        decision,
        &current.workspace_fingerprint,
        &contract_sha256,
        true,
        ReceiptValidationPolicy::CurrentV3,
    )
}

pub(in crate::goal) fn validation_has_receipt_for_fingerprint(
    validation: &ValidationEvidence,
    root: &Path,
    fingerprint: &str,
    contract_sha256: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
) -> bool {
    let Ok(parsed) = parse_validation_command(&validation.command) else {
        return false;
    };
    if enforce_current_security && validate_command_security(root, &parsed).is_err() {
        return false;
    }
    let Some(receipt) = validation.receipt.as_ref() else {
        return false;
    };
    receipt.exit_code == 0
        && receipt.workspace_fingerprint_before == fingerprint
        && receipt.workspace_fingerprint_after == fingerprint
        && receipt.workspace_fingerprint_before == receipt.workspace_fingerprint_after
        && receipt_identity_matches_live(receipt, root, policy)
        && is_sha256(&receipt.stdout_sha256)
        && is_sha256(&receipt.stderr_sha256)
        && is_sha256(&receipt.invocation_sha256)
        && receipt.contract_sha256 == contract_sha256
        && is_sha256(&receipt.contract_sha256)
        && receipt.invocation_sha256
            == validation_invocation_sha256_scoped_mode(
                &validation.command,
                &validation.impact_scopes,
                validation.non_code,
                validation.workspace_snapshot,
            )
        && validation_scope_is_well_formed(validation)
        && (!(match policy {
            ReceiptValidationPolicy::LegacyV1 => cargo_test_invocation(&parsed),
            ReceiptValidationPolicy::CurrentV2 | ReceiptValidationPolicy::CurrentV3 => {
                test_invocation(&parsed)
            }
        }) || test_receipt_has_structured_proof(receipt))
}

pub(in crate::goal) fn validation_has_receipt_for_fingerprint_with_context(
    validation: &ValidationEvidence,
    decision: &GoalDecisionContext<'_>,
    fingerprint: &str,
    contract_sha256: &str,
    enforce_current_security: bool,
    policy: ReceiptValidationPolicy,
) -> bool {
    let Ok(parsed) = parse_validation_command(&validation.command) else {
        return false;
    };
    if enforce_current_security
        && validate_command_security_with_context(decision, &parsed).is_err()
    {
        return false;
    }
    let Some(receipt) = validation.receipt.as_ref() else {
        return false;
    };
    receipt.exit_code == 0
        && receipt.workspace_fingerprint_before == fingerprint
        && receipt.workspace_fingerprint_after == fingerprint
        && receipt.workspace_fingerprint_before == receipt.workspace_fingerprint_after
        && receipt_identity_matches_context(receipt, decision, policy)
        && is_sha256(&receipt.stdout_sha256)
        && is_sha256(&receipt.stderr_sha256)
        && is_sha256(&receipt.invocation_sha256)
        && receipt.contract_sha256 == contract_sha256
        && is_sha256(&receipt.contract_sha256)
        && receipt.invocation_sha256
            == validation_invocation_sha256_scoped_mode(
                &validation.command,
                &validation.impact_scopes,
                validation.non_code,
                validation.workspace_snapshot,
            )
        && validation_scope_is_well_formed(validation)
        && (!(match policy {
            ReceiptValidationPolicy::LegacyV1 => cargo_test_invocation(&parsed),
            ReceiptValidationPolicy::CurrentV2 | ReceiptValidationPolicy::CurrentV3 => {
                test_invocation(&parsed)
            }
        }) || test_receipt_has_structured_proof(receipt))
}

/// Historical lifecycle verification needs one additional identity mode: a v3
/// receipt is bound to the archived lifecycle proof's immutable workspace
/// identity, not to the path where that history happens to be inspected now.
/// This stays private to historical proof validation; all current/replacement
/// paths continue to require the live or captured identity.
pub(in crate::goal) fn validation_has_historical_receipt_for_fingerprint_with_identity(
    validation: &ValidationEvidence,
    root: &Path,
    fingerprint: &str,
    contract_sha256: &str,
    policy: ReceiptValidationPolicy,
    expected_workspace_identity: Option<&str>,
) -> bool {
    let Ok(parsed) = parse_validation_command(&validation.command) else {
        return false;
    };
    let Some(receipt) = validation.receipt.as_ref() else {
        return false;
    };
    let identity_matches = match policy {
        ReceiptValidationPolicy::CurrentV3 => expected_workspace_identity
            .is_some_and(|identity| receipt.workspace_identity == identity && is_sha256(identity)),
        ReceiptValidationPolicy::LegacyV1 | ReceiptValidationPolicy::CurrentV2 => {
            receipt_identity_matches_live(receipt, root, policy)
        }
    };
    receipt.exit_code == 0
        && receipt.workspace_fingerprint_before == fingerprint
        && receipt.workspace_fingerprint_after == fingerprint
        && receipt.workspace_fingerprint_before == receipt.workspace_fingerprint_after
        && identity_matches
        && is_sha256(&receipt.stdout_sha256)
        && is_sha256(&receipt.stderr_sha256)
        && is_sha256(&receipt.invocation_sha256)
        && receipt.contract_sha256 == contract_sha256
        && is_sha256(&receipt.contract_sha256)
        && receipt.invocation_sha256
            == validation_invocation_sha256_scoped_mode(
                &validation.command,
                &validation.impact_scopes,
                validation.non_code,
                validation.workspace_snapshot,
            )
        && validation_scope_is_well_formed(validation)
        && (!(match policy {
            ReceiptValidationPolicy::LegacyV1 => cargo_test_invocation(&parsed),
            ReceiptValidationPolicy::CurrentV2 | ReceiptValidationPolicy::CurrentV3 => {
                test_invocation(&parsed)
            }
        }) || test_receipt_has_structured_proof(receipt))
}

pub(in crate::goal) fn goal_success_historical_receipt_gaps_with_identity(
    goal: &Goal,
    root: &Path,
    fingerprint: &str,
    policy: ReceiptValidationPolicy,
    expected_workspace_identity: Option<&str>,
) -> Vec<String> {
    let mut gaps = Vec::new();
    if goal.status != GoalStatus::Success {
        gaps.push(format!("goal 状态为 {}，不是 success", goal.status));
    }
    if goal.replacement_authority.is_some() {
        return gaps;
    }
    for requirement in &goal.requirements {
        if requirement.kind != RequirementKind::Must {
            continue;
        }
        if requirement.status != RequirementStatus::Done
            || requirement
                .evidence
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .is_empty()
        {
            gaps.push(format!("must {} 未完成或缺少 evidence", requirement.id));
            continue;
        }
        let Ok(contract_sha256) = validation_contract_sha256(goal, &requirement.id) else {
            gaps.push(format!(
                "must {} immutable contract 无法计算",
                requirement.id
            ));
            continue;
        };
        if !requirement.validations.iter().any(|validation| {
            proof_kind_matches(
                requirement.proof_kind,
                validation_proof_kind(root, &validation.command)
                    .ok()
                    .unwrap_or_default(),
            ) && validation_has_historical_receipt_for_fingerprint_with_identity(
                validation,
                root,
                fingerprint,
                &contract_sha256,
                policy,
                expected_workspace_identity,
            )
        }) {
            gaps.push(format!(
                "must {} 缺少当前成功 validation receipt",
                requirement.id
            ));
        }
    }
    gaps
}
