use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn audit_self_test_exercises_only_the_audit_contract() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let fixture = tempfile::tempdir().expect("audit self-test fixture must be created");
    let fixture_bin = fixture.path().join("bin");
    let process_temp = fixture.path().join("process-temp");
    fs::create_dir_all(&fixture_bin).expect("fixture bin must be created");
    fs::create_dir_all(&process_temp).expect("fixture process temp must be created");

    #[cfg(windows)]
    let cargo_deny = fixture_bin.join("cargo-deny.cmd");
    #[cfg(not(windows))]
    let cargo_deny = fixture_bin.join("cargo-deny");
    #[cfg(windows)]
    fs::write(&cargo_deny, "@exit /b 0\r\n").expect("cargo-deny fixture must be written");
    #[cfg(not(windows))]
    {
        fs::write(&cargo_deny, "#!/bin/sh\nexit 0\n").expect("cargo-deny fixture must be written");
        let mut permissions = fs::metadata(&cargo_deny)
            .expect("cargo-deny fixture metadata must resolve")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cargo_deny, permissions)
            .expect("cargo-deny fixture must be executable");
    }

    let ambient_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(fixture_bin.clone()).chain(std::env::split_paths(&ambient_path)),
    )
    .expect("fixture PATH must be representable");
    let script = repo_root.join("scripts/audit-repository.ps1");
    let output = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script.to_str().expect("audit script path must be UTF-8"),
            "-SelfTest",
        ])
        .current_dir(&repo_root)
        .env("PATH", path)
        .env("TMP", &process_temp)
        .env("TEMP", &process_temp)
        .output()
        .expect("PowerShell 7 must run the audit self-test");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "audit self-test failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("exact isolated MSRV")
            && stdout.contains("audit-repository.ps1 self-test passed."),
        "audit self-test did not exercise the exact isolated MSRV contract\nstdout:\n{stdout}"
    );
    for sibling in [
        "release-closeout self-test",
        "Install self-test passed",
        "Release verifier self-test passed",
        "PowerShell profile repair self-test",
    ] {
        assert!(
            !stdout.contains(sibling),
            "audit self-test recursively launched sibling suite {sibling}\nstdout:\n{stdout}"
        );
    }
}

#[test]
fn audit_orchestration_has_no_environment_bypass_or_implicit_provisioning() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let source = fs::read_to_string(repo_root.join("scripts/audit-repository.ps1"))
        .expect("audit script must be readable UTF-8");
    let check_repo = fs::read_to_string(repo_root.join("scripts/check-repo.ps1"))
        .expect("check-repo script must be readable UTF-8");
    let release_closeout = fs::read_to_string(repo_root.join("scripts/release-closeout.ps1"))
        .expect("release closeout script must be readable UTF-8");
    let release_verifier =
        fs::read_to_string(repo_root.join("scripts/verify-release-contract.ps1"))
            .expect("release verifier must be readable UTF-8");

    assert!(!source.contains("RAYMAN_AUDIT_SELF_TEST"));
    assert!(source.contains("switch ($PSCmdlet.ParameterSetName)"));
    assert!(source.contains("-PrepareAuditTools:$false grants no provisioning authority"));
    assert!(source.contains("-IncludeCompleteAuditTools ($PSCmdlet.ParameterSetName -eq 'Audit')"));
    assert!(source.contains("if ($PSCmdlet.ParameterSetName -eq 'SelfTest')"));
    assert!(source.contains("if ($PSCmdlet.ParameterSetName -eq 'DependencyPolicy')"));
    let preparation_start = source
        .find("if ($PSCmdlet.ParameterSetName -eq 'PrepareAuditTools') {")
        .expect("explicit preparation entrypoint must exist");
    let repository_helper = source
        .find(". (Join-Path $PSScriptRoot 'repository-quality.ps1')")
        .expect("normal audit must load the repository quality helper");
    assert!(preparation_start < repository_helper);
    let preparation = &source[preparation_start..repository_helper];
    for forbidden in [
        "$CliPath",
        "$SkillPath",
        "Invoke-AuditBootstrap",
        "Invoke-AuditScriptSelfTest",
        "Invoke-IsolatedCargoDenyChecks",
        "Get-RepositoryQualityCommands",
        "New-ManagedAuditDirectory",
    ] {
        assert!(
            !preparation.contains(forbidden),
            "tool preparation unexpectedly owns audit concern {forbidden}"
        );
    }
    for required in [
        "Resolve-PersistentCargoInstallRoot",
        "Get-MsrvLlvmPreparationArguments",
        "Get-CoverageToolPreparationArguments",
        "schema = 'rayman.audit.tool-preparation.v1'",
        "Write-AuditPhase -Name 'prepare_audit_tools' -Status 'pass'",
        "return",
    ] {
        assert!(preparation.contains(required));
    }
    let normal_audit = &source[repository_helper..];
    for forbidden in [
        "Get-MsrvLlvmPreparationArguments",
        "Get-CoverageToolPreparationArguments",
        "[bool]$PrepareAuditTools",
    ] {
        assert!(
            !normal_audit.contains(forbidden),
            "normal audit retained implicit provisioning path {forbidden}"
        );
    }
    assert!(source.contains("--skip', $SkippedIntegrationTest"));
    assert!(check_repo.contains("--skip', $auditIntegrationTestName"));
    assert!(source.contains("Invoke-SourceFreshInputInspection"));
    assert!(source.contains("-InspectSourceFreshInputs"));
    for duplicated_policy in [
        "'RUSTFLAGS'",
        "'CARGO_ENCODED_RUSTFLAGS'",
        "'^CARGO_PROFILE_'",
        "CARGO_TARGET_.+_",
    ] {
        assert!(
            !source.contains(duplicated_policy),
            "audit duplicated release-verifier environment policy {duplicated_policy}"
        );
        assert!(
            !release_closeout.contains(duplicated_policy),
            "closeout duplicated release-verifier environment policy {duplicated_policy}"
        );
    }
    for policy_input in [
        "'RUSTFLAGS'",
        "'CARGO_ENCODED_RUSTFLAGS'",
        "'RUSTC_BOOTSTRAP'",
        "'RUSTC_WRAPPER'",
        "'RUSTC_WORKSPACE_WRAPPER'",
        "'CARGO_BUILD_INCREMENTAL'",
        "'^CARGO_PROFILE_'",
        "^CARGO_TARGET_.+_",
    ] {
        assert!(
            release_verifier.contains(policy_input),
            "release verifier lost build-shaping environment policy input {policy_input}"
        );
    }

    for forbidden in [
        "[switch]$PrepareAuditTools",
        "$arguments.PrepareAuditTools",
        "-PrepareAuditTools:$PrepareAuditTools",
    ] {
        assert!(
            !release_closeout.contains(forbidden),
            "release closeout retained provisioning authority surface {forbidden}"
        );
    }
    for required in [
        "schema = 'rayman.release.binding.v3'",
        "workspace_activation = $sourceFreshInputs.workspace_activation",
        "source_fresh_environment = $sourceFreshInputs.source_fresh_environment",
        "rayman.release.binding.v2",
        "cargo-deny",
        "cargo-llvm-cov",
        "llvm-cov",
        "llvm-profdata",
        "advisory_database",
        "'pwsh-host' = Get-CurrentPowerShellHostIdentity",
        "$candidate.cargo_net_offline.effective -ne $true",
        "Release binding drifted while revalidating reusable evidence",
        "Release binding drifted before closeout completion",
        "if ($PSCmdlet.ParameterSetName -eq 'SelfTest')",
        "if (-not $SelfTest.IsPresent)",
    ] {
        assert!(
            release_closeout.contains(required),
            "release binding lost required audit input {required}"
        );
    }
}
