use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn powershell_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(path) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{path}");
        }
        if let Some(path) = path.strip_prefix(r"\\?\") {
            return path.to_owned();
        }
    }
    path.into_owned()
}

fn powershell_function(source: &str, name: &str) -> String {
    let marker = format!("function {name}");
    let start = source
        .find(&marker)
        .unwrap_or_else(|| panic!("PowerShell source lost {name}"));
    let opening = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("PowerShell function {name} lost opening brace"));
    let bytes = source.as_bytes();
    let mut depth = 0_u32;
    let mut quote = None;
    let mut index = opening;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if byte == b'`' {
                index += 2;
                continue;
            }
            if byte == active {
                if index + 1 < bytes.len() && bytes[index + 1] == active {
                    index += 2;
                    continue;
                }
                quote = None;
            }
        } else {
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return source[start..=index].to_owned();
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    panic!("PowerShell function {name} lost closing brace");
}

#[test]
fn public_architecture_and_ci_coverage_contracts_avoid_drift_prone_counts() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let readme =
        fs::read_to_string(repo_root.join("README.md")).expect("README must be readable UTF-8");
    let quality = fs::read_to_string(repo_root.join(".RaymanCodingSkill/quality.json"))
        .expect("quality policy must be readable UTF-8");
    let workflow = fs::read_to_string(repo_root.join(".github/workflows/ci.yml"))
        .expect("CI workflow must be readable UTF-8");
    let cli_tests = fs::read_to_string(repo_root.join("crates/rayman/tests/cli.rs"))
        .expect("CLI tests must be readable UTF-8");

    assert!(readme.contains("`rayman` CLI"));
    assert!(readme.contains("`rayman-update-worker` binary is confined"));
    assert!(!readme.contains("One small Rust binary"));

    for forbidden in [
        "four activation-exempt CLI tests",
        "four cross-process CLI tests",
        "80 isolated behavioral cases",
    ] {
        assert!(
            !quality.contains(forbidden),
            "quality policy reintroduced drift-prone count: {forbidden}"
        );
    }
    for required in [
        "platform-gated activation-exempt CLI tests",
        "cross-process CLI tests cover offline status",
        "complete platform-specific behavioral suite",
    ] {
        assert!(
            quality.contains(required),
            "quality policy lost capability-based evidence wording: {required}"
        );
    }

    assert!(workflow.contains(
        "cargo check --locked -p rayman --target aarch64-unknown-linux-gnu --all-targets"
    ));

    let due_poll_start = cli_tests
        .find("fn non_windows_due_poll_stays_unsupported_even_with_install_consent()")
        .expect("non-Windows due-poll consent regression must exist");
    let due_poll_tail = &cli_tests[due_poll_start..];
    let due_poll_end = due_poll_tail
        .find("fn update_configure_requires_an_exact_selector_and_yes_without_workspace_writes()")
        .expect("following update test must delimit due-poll regression");
    let due_poll_test = &due_poll_tail[..due_poll_end];
    for required in [
        "--auto-install",
        "report[\"install_authorized\"], true",
        "report[\"install_ready\"], false",
        "report.get(\"worker_launch\").is_none()",
        "persisted[\"auto_install\"], true",
    ] {
        assert!(
            due_poll_test.contains(required),
            "non-Windows due-poll consent regression lost assertion: {required}"
        );
    }
}

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
fn repository_quality_provider_emits_the_exact_versioned_command_contract() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let script = repo_root.join("scripts/repository-quality.ps1");
    let expected = [
        (
            "Root",
            serde_json::json!([
                {"name": "fmt", "argv": ["fmt", "--all", "--check"]},
                {"name": "clippy", "argv": ["clippy", "--locked", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"]},
                {"name": "test", "argv": ["test", "--locked", "--workspace", "--all-targets"]}
            ]),
        ),
        (
            "Evals",
            serde_json::json!([
                {"name": "fmt", "argv": ["fmt", "--manifest-path", "evals/Cargo.toml", "--all", "--check"]},
                {"name": "clippy", "argv": ["clippy", "--manifest-path", "evals/Cargo.toml", "--locked", "--all-targets", "--all-features", "--", "-D", "warnings"]},
                {"name": "test", "argv": ["test", "--manifest-path", "evals/Cargo.toml", "--locked", "--all-targets"]}
            ]),
        ),
    ];
    let script = powershell_path(&script);

    for (suite, expected_commands) in expected {
        let output = Command::new("pwsh")
            .args([
                "-NoProfile",
                "-Command",
                "& $env:RAYMAN_TEST_SCRIPT -Suite $env:RAYMAN_TEST_SUITE",
            ])
            .current_dir(&repo_root)
            .env("RAYMAN_TEST_SCRIPT", &script)
            .env("RAYMAN_TEST_SUITE", suite)
            .output()
            .expect("PowerShell 7 must run the repository quality provider");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "repository quality provider failed for {suite}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let document: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid provider JSON for {suite}: {error}\n{stdout}"));
        assert_eq!(
            document,
            serde_json::json!({
                "schema": "rayman.repository-quality.commands.v1",
                "suite": suite,
                "commands": expected_commands
            })
        );
    }
}

#[test]
fn repository_quality_consumers_reject_malformed_types_and_ignore_stale_native_exit_codes() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let fixture = tempfile::tempdir().expect("quality contract fixture must be created");
    let provider = fixture.path().join("provider.ps1");
    let process_temp = fixture.path().join("process-temp");
    fs::create_dir_all(&process_temp).expect("process temp must be created");
    let consumer_sources = [
        fs::read_to_string(repo_root.join("scripts/check-repo.ps1"))
            .expect("check-repo consumer must be readable"),
        fs::read_to_string(repo_root.join("scripts/audit-repository.ps1"))
            .expect("audit consumer must be readable"),
    ];
    let cases = [
        (
            "valid after stale native exit code",
            "$global:LASTEXITCODE = 7\nWrite-Output '{\"schema\":\"rayman.repository-quality.commands.v1\",\"suite\":\"Root\",\"commands\":[{\"name\":\"fmt\",\"argv\":[\"fmt\"]},{\"name\":\"clippy\",\"argv\":[\"clippy\"]},{\"name\":\"test\",\"argv\":[\"test\"]}]}'\n",
            true,
        ),
        (
            "top-level array",
            "Write-Output '[{\"schema\":\"rayman.repository-quality.commands.v1\",\"suite\":\"Root\",\"commands\":[{\"name\":\"fmt\",\"argv\":[\"fmt\"]},{\"name\":\"clippy\",\"argv\":[\"clippy\"]},{\"name\":\"test\",\"argv\":[\"test\"]}]}]'\n",
            false,
        ),
        (
            "scalar argv",
            "Write-Output '{\"schema\":\"rayman.repository-quality.commands.v1\",\"suite\":\"Root\",\"commands\":[{\"name\":\"fmt\",\"argv\":\"fmt\"},{\"name\":\"clippy\",\"argv\":[\"clippy\"]},{\"name\":\"test\",\"argv\":[\"test\"]}]}'\n",
            false,
        ),
        (
            "array schema",
            "Write-Output '{\"schema\":[\"rayman.repository-quality.commands.v1\"],\"suite\":\"Root\",\"commands\":[{\"name\":\"fmt\",\"argv\":[\"fmt\"]},{\"name\":\"clippy\",\"argv\":[\"clippy\"]},{\"name\":\"test\",\"argv\":[\"test\"]}]}'\n",
            false,
        ),
        (
            "array command name",
            "Write-Output '{\"schema\":\"rayman.repository-quality.commands.v1\",\"suite\":\"Root\",\"commands\":[{\"name\":[\"fmt\"],\"argv\":[\"fmt\"]},{\"name\":\"clippy\",\"argv\":[\"clippy\"]},{\"name\":\"test\",\"argv\":[\"test\"]}]}'\n",
            false,
        ),
        (
            "blank argv entry",
            "Write-Output '{\"schema\":\"rayman.repository-quality.commands.v1\",\"suite\":\"Root\",\"commands\":[{\"name\":\"fmt\",\"argv\":[\"   \"]},{\"name\":\"clippy\",\"argv\":[\"clippy\"]},{\"name\":\"test\",\"argv\":[\"test\"]}]}'\n",
            false,
        ),
    ];

    for (label, provider_source, should_succeed) in cases {
        fs::write(&provider, provider_source).expect("provider fixture must be written");
        for (consumer_index, consumer_source) in consumer_sources.iter().enumerate() {
            let consumer = fixture
                .path()
                .join(format!("consumer-{consumer_index}.ps1"));
            let function = powershell_function(consumer_source, "Get-RepositoryQualityCommands");
            fs::write(
                &consumer,
                format!(
                    "Set-StrictMode -Version Latest\n$ErrorActionPreference = 'Stop'\n{function}\nif ($ExecutionContext.SessionState.LanguageMode -cne 'ConstrainedLanguage') {{ throw 'consumer self-test did not enter ConstrainedLanguage' }}\n$null = Get-RepositoryQualityCommands -Suite Root -ProviderPath $env:RAYMAN_TEST_PROVIDER\nWrite-Output 'repository-quality consumer self-test: PASS'\n"
                ),
            )
            .expect("consumer fixture must be written");
            let consumer = powershell_path(&consumer);
            let provider_path = powershell_path(&provider);
            let output = Command::new("pwsh")
                .args([
                    "-NoProfile",
                    "-Command",
                    "$ExecutionContext.SessionState.LanguageMode = 'ConstrainedLanguage'; & $env:RAYMAN_TEST_CONSUMER",
                ])
                .current_dir(&repo_root)
                .env("RAYMAN_TEST_CONSUMER", &consumer)
                .env("RAYMAN_TEST_PROVIDER", &provider_path)
                .env("TMP", &process_temp)
                .env("TEMP", &process_temp)
                .output()
                .expect("PowerShell 7 must run the quality consumer self-test");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.success(),
                should_succeed,
                "quality consumer result mismatch for {label}: {}\nstdout:\n{stdout}\nstderr:\n{stderr}",
                consumer
            );
            if should_succeed {
                assert!(stdout.contains("repository-quality consumer self-test: PASS"));
            }
        }
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
    let update_freshness = fs::read_to_string(repo_root.join("scripts/check-update-freshness.ps1"))
        .expect("update freshness script must be readable UTF-8");
    let codex_temp_config =
        fs::read_to_string(repo_root.join("scripts/configure-codex-validation-temp.ps1"))
            .expect("Codex validation temp configurator must be readable UTF-8");
    let repository_quality = fs::read_to_string(repo_root.join("scripts/repository-quality.ps1"))
        .expect("repository quality provider must be readable UTF-8");
    let release_closeout = fs::read_to_string(repo_root.join("scripts/release-closeout.ps1"))
        .expect("release closeout script must be readable UTF-8");
    let release_verifier =
        fs::read_to_string(repo_root.join("scripts/verify-release-contract.ps1"))
            .expect("release verifier must be readable UTF-8");
    let workflow = fs::read_to_string(repo_root.join(".github/workflows/ci.yml"))
        .expect("CI workflow must be readable UTF-8");

    assert!(!source.contains("RAYMAN_AUDIT_SELF_TEST"));
    assert!(codex_temp_config.contains("RAYMAN_VALIDATION_TEMP_ROOT"));
    assert!(codex_temp_config.contains("TEMP"));
    assert!(codex_temp_config.contains("TMPDIR"));
    assert!(codex_temp_config.contains(r"E:\codex-sandbox\temp"));
    assert!(codex_temp_config.contains(r"E:\codex-sandbox\rayman-validation"));
    assert!(!codex_temp_config.contains(r"E:\codex-cache\cargo\codex-sandbox"));
    assert!(codex_temp_config.contains("does not cover managed root"));
    assert!(check_repo.contains("'configure-codex-validation-temp.ps1') -SelfTest"));
    assert!(check_repo.contains("'check-update-freshness.ps1') -SelfTest"));
    assert!(source.contains("'check-update-freshness.ps1'"));
    assert!(release_closeout.contains("'check-update-freshness.ps1'"));
    assert!(update_freshness.contains("manifest_verified"));
    assert!(update_freshness.contains("MinimumRemainingDays = 14"));
    assert!(update_freshness.contains("Do not replace the existing release assets"));
    assert!(workflow.contains("signed-release-freshness:"));
    assert!(workflow.contains("ci-${{ github.event_name }}-${{ github.ref }}"));
    assert!(workflow.contains("-MinimumRemainingDays 14"));
    assert!(workflow.contains("-MinimumRemainingDays 29"));
    assert!(workflow.contains("-ExpectedVersion $tag.Substring(1)"));
    assert!(!workflow.contains("rayman-release-source-$nonce"));
    assert!(source.contains("switch ($PSCmdlet.ParameterSetName)"));
    assert!(source.contains("-PrepareAuditTools:$false grants no provisioning authority"));
    assert!(source.contains("-IncludeCompleteAuditTools ($PSCmdlet.ParameterSetName -eq 'Audit')"));
    assert!(source.contains("if ($PSCmdlet.ParameterSetName -eq 'SelfTest')"));
    assert!(source.contains("if ($PSCmdlet.ParameterSetName -eq 'DependencyPolicy')"));
    let preparation_start = source
        .find("if ($PSCmdlet.ParameterSetName -eq 'PrepareAuditTools') {")
        .expect("explicit preparation entrypoint must exist");
    let repository_helper = source
        .find("function Get-RepositoryQualityCommands")
        .expect("normal audit must define the repository quality provider consumer");
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
    for consumer in [&source, &check_repo] {
        assert!(consumer.contains("Join-Path $PSScriptRoot 'repository-quality.ps1'"));
        assert!(consumer.contains("rayman.repository-quality.commands.v1"));
        assert!(consumer.contains("ConvertFrom-Json -Depth 8 -NoEnumerate"));
        assert!(consumer.contains("$document -is [array]"));
        assert!(consumer.contains("$document -isnot [pscustomobject]"));
        assert!(consumer.contains("$document.commands -isnot [array]"));
        assert!(consumer.contains("$command.argv -isnot [array]"));
        assert!(consumer.contains("$command -is [array]"));
        assert!(consumer.contains("[string]::IsNullOrWhiteSpace($_)"));
        assert!(consumer.contains("if (-not $? -or [string]::IsNullOrWhiteSpace($json))"));
        assert!(
            !consumer.contains("if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($json))")
        );
        assert!(!consumer.contains(". (Join-Path $PSScriptRoot 'repository-quality.ps1')"));
    }
    for expected in [
        "schema = 'rayman.repository-quality.commands.v1'",
        "suite = $Suite",
        "name = 'fmt'",
        "name = 'clippy'",
        "name = 'test'",
    ] {
        assert!(repository_quality.contains(expected));
    }
    assert!(source.contains("& (Join-Path $PSScriptRoot 'verify-release-contract.ps1')"));
    assert!(!source.contains("& './scripts/verify-release-contract.ps1'"));
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
        "schema = 'rayman.release.binding.v4'",
        "workspace_activation = $sourceFreshInputs.workspace_activation",
        "source_fresh_environment = $sourceFreshInputs.source_fresh_environment",
        "rayman.release.binding.v2",
        "rayman.release.binding.v3",
        "worker = [ordered]@{ path = $resolvedWorker; sha256 = Get-Sha256 $resolvedWorker }",
        "-WorkerPath $WorkerPath",
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
