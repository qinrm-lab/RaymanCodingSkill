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

/// A workspace holding the real gate-script paths plus same-named decoys in an
/// ordinary subdirectory.
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for directory in ["scripts", "tools"] {
        fs::create_dir_all(root.path().join(directory)).unwrap();
        for script in [
            "install-rayman.ps1",
            "check-repo.ps1",
            "audit-repository.ps1",
            "verify-release-contract.ps1",
        ] {
            fs::write(
                root.path().join(directory).join(script),
                "# fixture\nexit 0\n",
            )
            .unwrap();
        }
    }
    root
}

#[test]
fn installer_self_test_is_generic_evidence_not_a_typed_test_or_installation_proof() {
    let root = fixture();
    let installer =
        parse_validation_command("pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes").unwrap();
    let self_test =
        parse_validation_command("pwsh -NoProfile -File scripts/install-rayman.ps1 -SelfTest")
            .unwrap();

    assert_eq!(
        validation_proof_kind(
            root.path(),
            "pwsh -NoProfile -File scripts/install-rayman.ps1 -SelfTest"
        )
        .unwrap(),
        // NOT ProofKind::Test: the structured test proof (listed>0, passed>0,
        // passed+ignored==listed) is only ever demanded of cargo test / pytest
        // invocations, so a Test label here let a typed `--must-proof test::…`
        // obligation be discharged by a run that lists and executes zero tests.
        ProofKind::Generic
    );
    assert_eq!(
        validation_proof_kind(
            root.path(),
            "pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes"
        )
        .unwrap(),
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

/// PowerShell binds `-Self` to the `-SelfTest` switch, so an exact-string
/// comparison classified a self-test run — which installs and builds nothing —
/// as installation evidence and as full Rust build coverage.
#[test]
fn abbreviated_self_test_switches_never_mint_installation_evidence() {
    let root = fixture();
    for switch in ["-Self", "-SELF", "-selft", "-SelfTes", "-SelfTest:$false"] {
        let command = format!("pwsh -NoProfile -File scripts/install-rayman.ps1 {switch}");
        assert_eq!(
            validation_proof_kind(root.path(), &command).unwrap(),
            // Generic, not Test: a self-test proves neither an installation nor
            // a test execution, and a Test label made it satisfy a typed
            // `--must-proof test::…` obligation with zero tests run.
            ProofKind::Generic,
            "{switch} must not be installation evidence"
        );
        let parsed = parse_validation_command(&command).unwrap();
        assert!(
            !command_is_workspace_wide(root.path(), &parsed),
            "{switch} must not claim workspace-wide coverage"
        );
        assert!(
            validate_command_for_impacts(
                root.path(),
                &command,
                &[impact("crates/rayman/src/main.rs")],
                false,
            )
            .is_err(),
            "{switch} must not cover a Rust change"
        );
    }
    // A real install still is installation evidence; the guard is not a blanket
    // rejection of every argument that starts with `-S`.
    assert_eq!(
        validation_proof_kind(
            root.path(),
            "pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes -SkipPathUpdate"
        )
        .unwrap(),
        ProofKind::Installation
    );
}

/// The release handoff contract is built from the `installation`,
/// `repository_gate` and `source_fresh` proof kinds. Resolving those from the
/// script basename let a two-line `exit 0` decoy in any workspace directory
/// mint all three, so a handoff could close to success having built, audited
/// and installed nothing.
#[test]
fn decoy_gate_scripts_outside_the_reviewed_paths_mint_no_typed_proof() {
    let root = fixture();
    for (decoy, reviewed, kind) in [
        (
            "pwsh -NoProfile -File tools/install-rayman.ps1 -Yes",
            "pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes",
            ProofKind::Installation,
        ),
        (
            "pwsh -NoProfile -File tools/check-repo.ps1",
            "pwsh -NoProfile -File scripts/check-repo.ps1",
            ProofKind::RepositoryGate,
        ),
        (
            "pwsh -NoProfile -File tools/verify-release-contract.ps1 -CliPath cli -SkillPath skill -RequireSourceFresh",
            "pwsh -NoProfile -File scripts/verify-release-contract.ps1 -CliPath cli -SkillPath skill -RequireSourceFresh",
            ProofKind::SourceFresh,
        ),
    ] {
        assert_eq!(
            validation_proof_kind(root.path(), reviewed).unwrap(),
            kind,
            "the reviewed path must keep working: {reviewed}"
        );
        assert_eq!(
            validation_proof_kind(root.path(), decoy).unwrap(),
            ProofKind::Generic,
            "a decoy must not mint {kind:?}: {decoy}"
        );
        assert!(
            !proof_kind_matches(
                Some(kind),
                validation_proof_kind(root.path(), decoy).unwrap()
            ),
            "a decoy must not satisfy a typed {kind:?} requirement"
        );
        let parsed = parse_validation_command(decoy).unwrap();
        assert!(
            !command_is_workspace_wide(root.path(), &parsed),
            "a decoy must not claim workspace-wide coverage: {decoy}"
        );
        assert!(
            validate_command_for_impacts(root.path(), decoy, &[impact("src/lib.rs")], false)
                .is_err(),
            "a decoy must not cover a Rust change: {decoy}"
        );
    }
}

/// `--authority` always resolved the real path; the decoys must stay rejected
/// there too, and the reviewed scripts must stay accepted.
#[test]
fn authority_still_separates_reviewed_gate_scripts_from_decoys() {
    let root = fixture();
    assert!(
        validate_authority_command(root.path(), "pwsh -NoProfile -File scripts/check-repo.ps1")
            .is_ok()
    );
    assert!(
        validate_authority_command(root.path(), "pwsh -NoProfile -File tools/check-repo.ps1")
            .is_err()
    );
}

#[test]
fn repository_authority_requires_the_full_script_parameter_set() {
    let root = fixture();
    for full in [
        "pwsh -NoProfile -File scripts/check-repo.ps1",
        "pwsh -NoProfile -File scripts/audit-repository.ps1 -CliPath cli -SkillPath skill",
        "pwsh -NoProfile -File scripts/verify-release-contract.ps1 -CliPath cli -SkillPath skill",
    ] {
        assert!(
            validate_authority_command(root.path(), full).is_ok(),
            "full repository gate must remain authority: {full}"
        );
        assert_eq!(
            validation_proof_kind(root.path(), full).unwrap(),
            ProofKind::RepositoryGate,
            "full repository gate must retain its typed proof: {full}"
        );
    }

    let source_fresh = "pwsh -NoProfile -File scripts/verify-release-contract.ps1 -CliPath cli -SkillPath skill -RequireSourceFresh";
    assert!(validate_authority_command(root.path(), source_fresh).is_ok());
    assert_eq!(
        validation_proof_kind(root.path(), source_fresh).unwrap(),
        ProofKind::SourceFresh
    );

    for short_mode in [
        "pwsh -NoProfile -File scripts/check-repo.ps1 unexpected",
        "pwsh -NoProfile -File scripts/audit-repository.ps1",
        "pwsh -NoProfile -File scripts/audit-repository.ps1 -SelfTest",
        "pwsh -NoProfile -File scripts/audit-repository.ps1 -Self",
        "pwsh -NoProfile -File scripts/audit-repository.ps1 -DependencyPolicyOnly",
        "pwsh -NoProfile -File scripts/audit-repository.ps1 -PrepareAuditTools",
        "pwsh -NoProfile -File scripts/verify-release-contract.ps1 -RequireSourceFresh",
        "pwsh -NoProfile -File scripts/verify-release-contract.ps1 -SelfTest",
        "pwsh -NoProfile -File scripts/verify-release-contract.ps1 -InspectSourceFreshInputs",
    ] {
        assert!(
            validate_authority_command(root.path(), short_mode).is_err(),
            "short/focused mode must not be authority: {short_mode}"
        );
        assert_eq!(
            validation_proof_kind(root.path(), short_mode).unwrap(),
            ProofKind::Generic,
            "short/focused mode must not mint a typed proof: {short_mode}"
        );
        let parsed = parse_validation_command(short_mode).unwrap();
        assert!(
            !command_is_workspace_wide(root.path(), &parsed),
            "short/focused mode must not claim workspace coverage: {short_mode}"
        );
    }
}

#[test]
fn documentation_and_git_commit_proofs_require_canonical_semantics() {
    let root = fixture();
    for documentation in [
        "python quick_validate.py",
        "py -3 quick_validate.py --strict",
        "markdownlint README.md",
    ] {
        assert_eq!(
            validation_proof_kind(root.path(), documentation).unwrap(),
            ProofKind::Documentation,
            "canonical documentation command lost its typed proof: {documentation}"
        );
    }
    for decoy in [
        "python -c pass quick_validate.py",
        "python -m fake quick_validate.py",
        "markdownlint --version",
        "markdownlint --help README.md",
    ] {
        assert_eq!(
            validation_proof_kind(root.path(), decoy).unwrap(),
            ProofKind::Generic,
            "documentation decoy acquired a typed proof: {decoy}"
        );
    }

    let clean =
        parse_validation_command("git status --porcelain=v1 --untracked-files=all").unwrap();
    assert_eq!(
        validation_proof_kind(
            root.path(),
            "git status --porcelain=v1 --untracked-files=all"
        )
        .unwrap(),
        ProofKind::GitCommit
    );
    assert_eq!(
        validation_proof_kind(root.path(), "git rev-parse --verify HEAD").unwrap(),
        ProofKind::Generic
    );
    assert!(validation_execution_proof(&clean, b"", b"", None).is_ok());
    assert!(
        validation_execution_proof(&clean, b" M src/lib.rs\n", b"", None).is_err(),
        "dirty git status must not mint git_commit proof"
    );
    assert!(
        validation_execution_proof(&clean, b"", b"warning\n", None).is_err(),
        "ambiguous git diagnostics must fail closed"
    );
}

/// The contract calls the authority gate "selector-free". Matching only on the
/// presence of `--workspace` let a filtered run — which exits 0 while the rest
/// of the suite is red — stand in for the whole suite.
#[test]
fn narrowed_workspace_runs_are_not_whole_workspace_evidence() {
    let root = fixture();
    let wide = [
        "cargo test --locked --workspace --all-targets",
        "cargo test --workspace",
        "cargo test --all --no-fail-fast",
        "cargo test --workspace -- --nocapture",
        "cargo test --workspace -- --test-threads 1",
        "pytest",
        "pytest -q",
    ];
    let narrowed = [
        "cargo test --workspace trivial_ok",
        "cargo test --workspace --exclude app",
        "cargo build --workspace --exclude app",
        "cargo test --workspace -p app",
        "cargo test --workspace --package app",
        "cargo test --workspace --lib",
        "cargo test --workspace --bins",
        "cargo test --workspace --tests",
        "cargo test --workspace --doc",
        "cargo test --workspace --no-run",
        "cargo test --workspace --no-default-features",
        "cargo test --workspace --features narrow",
        "cargo test --workspace -Fnarrow",
        "cargo test --workspace -- --skip slow",
        "cargo test --workspace -- --ignored",
        "cargo test --workspace -- --exact my::test",
        "pytest -k trivial",
        "pytest -m smoke",
        "pytest --deselect tests/test_slow.py",
        "pytest --ignore=tests/slow",
        "pytest tests/unit",
    ];
    for command in wide {
        let parsed = parse_validation_command(command).unwrap();
        assert!(
            command_is_workspace_wide(root.path(), &parsed),
            "must stay whole-workspace: {command}"
        );
    }
    for command in narrowed {
        let parsed = parse_validation_command(command).unwrap();
        assert!(
            !command_is_workspace_wide(root.path(), &parsed),
            "must not claim whole-workspace coverage: {command}"
        );
    }

    // The same rule binds the strongest claim in the tool.
    assert!(validate_authority_command(root.path(), "cargo test --locked --workspace").is_ok());
    assert!(validate_authority_command(root.path(), "cargo test --workspace trivial_ok").is_err());
    assert!(
        validate_authority_command(root.path(), "cargo test --workspace --exclude app").is_err()
    );
    assert!(validate_authority_command(root.path(), "pytest").is_ok());
    assert!(validate_authority_command(root.path(), "pytest -k trivial").is_err());
}

/// `exclude = ["evals"]` means `cargo test --workspace` compiles nothing in
/// that directory, yet a workspace-wide receipt used to be accepted as
/// covering changes there.
#[test]
fn workspace_wide_cargo_does_not_cover_an_excluded_package() {
    let root = fixture();
    fs::write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/app\"]\nexclude = [\"evals\"]\nresolver = \"2\"\n",
    )
    .unwrap();

    for command in [
        "cargo test --locked --workspace --all-targets",
        "cargo build --workspace",
        "pwsh -NoProfile -File scripts/install-rayman.ps1 -Yes",
    ] {
        assert!(
            validate_command_for_impacts(
                root.path(),
                command,
                &[impact("evals/src/grade.rs")],
                false,
            )
            .is_err(),
            "must not cover an excluded package: {command}"
        );
        assert!(
            validate_command_for_impacts(
                root.path(),
                command,
                &[impact("crates/app/src/lib.rs")],
                false,
            )
            .is_ok(),
            "a workspace member must stay covered: {command}"
        );
    }

    // The repository gate scripts really do run the excluded package, so they
    // keep covering it.
    assert!(
        validate_command_for_impacts(
            root.path(),
            "pwsh -NoProfile -File scripts/check-repo.ps1",
            &[impact("evals/src/grade.rs")],
            false,
        )
        .is_ok()
    );
}
