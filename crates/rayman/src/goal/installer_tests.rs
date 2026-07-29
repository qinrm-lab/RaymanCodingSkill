use std::fs;

use super::*;

fn impact(path: &str) -> ImpactEvidence {
    ImpactEvidence {
        changed_path: path.into(),
        package: None,
        manifest_path: None,
        direct_dependencies: Vec::new(),
        direct_dependents: Vec::new(),
        candidate_tests: Vec::new(),
        recommended_checks: Vec::new(),
        recommendation_basis: "test".into(),
        recorded_at: now_iso(),
    }
}

#[test]
fn installer_self_test_is_classified_as_test_without_weakening_installation() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("scripts")).unwrap();
    fs::write(
        root.path().join("scripts/install-rayman.ps1"),
        "# fixture\n",
    )
    .unwrap();

    let installer =
        parse_validation_command("pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes").unwrap();
    let self_test =
        parse_validation_command("pwsh -NoProfile -File scripts/install-rayman.ps1 -SelfTest")
            .unwrap();

    assert_eq!(
        validation_proof_kind("pwsh -NoProfile -File scripts/install-rayman.ps1 -SelfTest")
            .unwrap(),
        ProofKind::Test
    );
    assert_eq!(
        validation_proof_kind("pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes").unwrap(),
        ProofKind::Installation
    );
    assert!(command_is_workspace_wide(root.path(), &installer));
    assert!(!command_is_workspace_wide(root.path(), &self_test));
    assert!(
        validate_command_for_impacts(
            root.path(),
            "pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes",
            &[impact("crates/rayman/src/main.rs")],
            false,
        )
        .is_ok()
    );
    assert!(
        validate_command_for_impacts(
            root.path(),
            "pwsh -NoProfile -File scripts/install-rayman.ps1 -SelfTest",
            &[impact("crates/rayman/src/main.rs")],
            false,
        )
        .is_err()
    );
}
