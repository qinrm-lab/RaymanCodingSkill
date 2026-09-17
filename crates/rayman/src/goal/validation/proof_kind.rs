//! Proof classification keeps complete audit and general authority distinct.
use super::*;

pub fn validation_proof_kind(root: &Path, command: &str) -> Result<ProofKind> {
    let parsed = parse_validation_command(command)?;
    let script = trusted_gate_script(root, &parsed).unwrap_or_default();

    if trusted_xtask_repository_gate(root, &parsed)? {
        return Ok(ProofKind::RepositoryGate);
    }
    if trusted_source_fresh_gate_script(root, &parsed) {
        return Ok(ProofKind::SourceFresh);
    }
    if trusted_workspace_gate_script(root, &parsed) {
        return Ok(if script == "audit-repository.ps1" {
            ProofKind::RepositoryAudit
        } else {
            ProofKind::RepositoryGate
        });
    }
    if release_installer_invocation(root, &parsed) {
        return Ok(ProofKind::Installation);
    }
    if documentation_invocation(script, &parsed) {
        return Ok(ProofKind::Documentation);
    }
    if git_clean_head_invocation(&parsed) {
        return Ok(ProofKind::GitCommit);
    }
    if test_invocation(&parsed) {
        return Ok(ProofKind::Test);
    }
    Ok(ProofKind::Generic)
}

/// Capture-only proof classification for readiness.  It deliberately avoids
/// reopening a PowerShell path after the decision capture.
pub(crate) fn validation_proof_kind_with_context(
    decision: &GoalDecisionContext<'_>,
    command: &str,
) -> Result<ProofKind> {
    let parsed = parse_validation_command(command)?;
    let script = trusted_gate_script_with_context(decision, &parsed)?.unwrap_or_default();
    if trusted_xtask_repository_gate_with_context(decision, &parsed)? {
        return Ok(ProofKind::RepositoryGate);
    }
    if trusted_source_fresh_gate_script_with_context(decision, &parsed)? {
        return Ok(ProofKind::SourceFresh);
    }
    if trusted_workspace_gate_script_with_context(decision, &parsed)? {
        return Ok(if script == "audit-repository.ps1" {
            ProofKind::RepositoryAudit
        } else {
            ProofKind::RepositoryGate
        });
    }
    if release_installer_invocation_with_context(decision, &parsed)? {
        return Ok(ProofKind::Installation);
    }
    if documentation_invocation(script, &parsed) {
        return Ok(ProofKind::Documentation);
    }
    if git_clean_head_invocation(&parsed) {
        return Ok(ProofKind::GitCommit);
    }
    if test_invocation(&parsed) {
        return Ok(ProofKind::Test);
    }
    Ok(ProofKind::Generic)
}

pub fn proof_kind_matches(required: Option<ProofKind>, actual: ProofKind) -> bool {
    matches!(required, None | Some(ProofKind::Generic))
        || required == Some(actual)
        || (required == Some(ProofKind::RepositoryGate) && actual == ProofKind::RepositoryAudit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_audit_proof_refuses_partial_lanes_and_general_authority() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("scripts")).unwrap();
        std::fs::write(
            root.path().join("scripts/audit-repository.ps1"),
            "# fixture\n",
        )
        .unwrap();
        let full = "pwsh -NoProfile -File scripts/audit-repository.ps1 -CliPath rayman -SkillPath SKILL.md";
        assert_eq!(
            validation_proof_kind(root.path(), full).unwrap(),
            ProofKind::RepositoryAudit
        );
        for suffix in [
            " -SelfTest",
            " -SelfTest:$false",
            " -DependencyPolicyOnly",
            " -PrepareAuditTools",
        ] {
            assert_ne!(
                validation_proof_kind(root.path(), &format!("{full}{suffix}")).unwrap(),
                ProofKind::RepositoryAudit
            );
        }
        assert!(!proof_kind_matches(
            Some(ProofKind::RepositoryAudit),
            ProofKind::RepositoryGate
        ));
        assert!(!proof_kind_matches(
            Some(ProofKind::RepositoryAudit),
            ProofKind::Test
        ));
        assert!(proof_kind_matches(
            Some(ProofKind::RepositoryGate),
            ProofKind::RepositoryAudit
        ));
        assert_eq!(
            "repository_audit".parse::<ProofKind>().unwrap(),
            ProofKind::RepositoryAudit
        );
    }
}
