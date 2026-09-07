use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

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
    assert!(!workflow.contains("    env:\n      CARGO_TARGET_DIR: ${{ runner.temp }}"));
    assert!(workflow.contains("workspace activate --skill-file"));
    assert!(!workflow.contains("Set-Content -Path \".RaymanCodingSkill/workspace_skill.yaml\""));

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
fn test_traceability_checker_executes_mutation_self_test_and_live_graph() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let script = repo_root.join("scripts/check-test-traceability.ps1");
    for (label, extra, marker) in [
        (
            "self-test",
            Some("-SelfTest"),
            "check-test-traceability self-test: PASS",
        ),
        (
            "live graph",
            None,
            "\"schema\": \"rayman.test-traceability.check.v2\"",
        ),
    ] {
        let mut command = Command::new("pwsh");
        command.args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            script
                .to_str()
                .expect("traceability checker path must be UTF-8"),
        ]);
        if let Some(extra) = extra {
            command.arg(extra);
        }
        let output = command
            .current_dir(&repo_root)
            .output()
            .expect("PowerShell 7 must run the traceability checker");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains(marker),
            "traceability {label} failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        if label == "live graph" {
            assert!(
                stdout.contains("repository_first_party_executable_tests")
                    && stdout.contains("inventory_sha256"),
                "live traceability output lost complete inventory binding\nstdout:\n{stdout}"
            );
        }
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
    fs::copy(
        repo_root.join("scripts/read-repository-quality.ps1"),
        fixture.path().join("read-repository-quality.ps1"),
    )
    .expect("shared quality reader fixture must be copied");
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

#[cfg(windows)]
#[test]
fn codex_powershell_broker_self_tests_execute_the_task_xml_contract() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let clean = tempfile::tempdir().expect("clean broker self-test root must be created");
    let process_temp = clean.path().join("process-temp");
    fs::create_dir_all(&process_temp).expect("clean process temp must be created");
    for source_path in [
        ".gitattributes",
        "AGENTS.md",
        "CLAUDE.md",
        "README.md",
        "crates/rayman/assets/repository-gate-inputs.json",
        "crates/rayman/src/goal/validation/cargo_isolation.rs",
        "crates/rayman/src/goal/validation/pytest_isolation.rs",
        "crates/rayman/tests/audit_script.rs",
        "docs/CODEX_POWERSHELL_BROKER.md",
        "governance/first-party-test-inventory.json",
        "governance/test-traceability.json",
        "scripts/check-agent-instructions.ps1",
        "scripts/codex-powershell-broker.ps1",
        "scripts/install-codex-powershell-broker.ps1",
    ] {
        let destination = clean.path().join(source_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!("clean broker self-test parent failed for {source_path}: {error}")
            });
        }
        fs::copy(repo_root.join(source_path), &destination).unwrap_or_else(|error| {
            panic!("clean broker self-test copy failed for {source_path}: {error}")
        });
    }
    let run_git = |arguments: &[&str]| {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(clean.path())
            .env("GIT_AUTHOR_NAME", "rayman-clean-selftest")
            .env("GIT_AUTHOR_EMAIL", "rayman-clean-selftest@example.invalid")
            .env("GIT_COMMITTER_NAME", "rayman-clean-selftest")
            .env(
                "GIT_COMMITTER_EMAIL",
                "rayman-clean-selftest@example.invalid",
            )
            .output()
            .unwrap_or_else(|error| panic!("clean broker self-test git spawn failed: {error}"));
        assert!(
            output.status.success(),
            "clean broker self-test git {arguments:?} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["add", "--all"]);
    run_git(&["commit", "-m", "clean broker self-test fixture"]);
    fs::copy(
        repo_root.join(".git/config"),
        clean.path().join(".git/config"),
    )
    .expect("reviewed clean Git config must be copied");
    fs::copy(
        repo_root.join(".git/info/exclude"),
        clean.path().join(".git/info/exclude"),
    )
    .expect("reviewed clean Git exclude file must be copied");
    assert!(
        !clean.path().join(".RaymanCodingSkill").exists(),
        "clean broker self-test fixture unexpectedly contains workspace state"
    );
    for (script_name, success_marker) in [
        (
            "install-codex-powershell-broker.ps1",
            "install-codex-powershell-broker.ps1 self-test passed.",
        ),
        (
            "codex-powershell-broker.ps1",
            "codex-powershell-broker.ps1 self-test passed.",
        ),
    ] {
        let script = clean.path().join("scripts").join(script_name);
        let output = Command::new("pwsh")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                script
                    .to_str()
                    .expect("broker self-test path must be UTF-8"),
                "-SelfTest",
            ])
            .current_dir(clean.path())
            .env("TEMP", &process_temp)
            .env("TMP", &process_temp)
            .env("TMPDIR", &process_temp)
            .output()
            .expect("PowerShell 7 must run the broker self-test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success() && stdout.contains(success_marker),
            "{script_name} self-test failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    let worker = clean.path().join("scripts/codex-powershell-broker.ps1");
    let installer = clean
        .path()
        .join("scripts/install-codex-powershell-broker.ps1");
    let output = Command::new("pwsh")
        .args([
            "-NoProfile",
            "-Command",
            "& $env:RAYMAN_WORKER_SELFTEST -SelfTest; & $env:RAYMAN_INSTALLER_SELFTEST -SelfTest",
        ])
        .env("RAYMAN_WORKER_SELFTEST", powershell_path(&worker))
        .env("RAYMAN_INSTALLER_SELFTEST", powershell_path(&installer))
        .current_dir(clean.path())
        .env("TEMP", &process_temp)
        .env("TMP", &process_temp)
        .env("TMPDIR", &process_temp)
        .output()
        .expect("PowerShell 7 must run the shared-runspace broker self-tests");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success()
            && stdout.contains("codex-powershell-broker.ps1 self-test passed.")
            && stdout.contains("install-codex-powershell-broker.ps1 self-test passed."),
        "shared-runspace broker self-tests failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let clean_status = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .current_dir(clean.path())
        .output()
        .expect("clean broker self-test status must run");
    assert!(
        clean_status.status.success() && clean_status.stdout.is_empty(),
        "clean broker self-test left repository state\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&clean_status.stdout),
        String::from_utf8_lossy(&clean_status.stderr)
    );
    assert!(
        !clean.path().join(".RaymanCodingSkill").exists(),
        "broker self-tests created workspace-local state in the clean fixture"
    );
}

#[cfg(windows)]
#[test]
fn codex_powershell_broker_production_rejects_explicit_false_confirmation() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root must resolve");
    let script = repo_root.join("scripts/install-codex-powershell-broker.ps1");
    let fingerprint = "a".repeat(64);
    let manifest_hash = "b".repeat(64);
    let cases = [
        vec!["-Install".to_owned()],
        vec!["-Uninstall".to_owned()],
        vec!["-RecoverPartialUninstall".to_owned()],
        vec![
            "-Upgrade".to_owned(),
            "-ExpectedGoalId".to_owned(),
            "goal_0123456789".to_owned(),
            "-ExpectedSourceFingerprint".to_owned(),
            fingerprint.clone(),
            "-UpgradeAuthorityManifestPath".to_owned(),
            "missing-authority.json".to_owned(),
            "-UpgradeAuthorityManifestSha256".to_owned(),
            manifest_hash.clone(),
        ],
        vec![
            "-PrepareUpgradeLauncher".to_owned(),
            "-ExpectedGoalId".to_owned(),
            "goal_0123456789".to_owned(),
            "-ExpectedSourceFingerprint".to_owned(),
            fingerprint.clone(),
        ],
        vec![
            "-PrepareInstallLauncher".to_owned(),
            "-ExpectedGoalId".to_owned(),
            "goal_0123456789".to_owned(),
            "-ExpectedSourceFingerprint".to_owned(),
            fingerprint,
        ],
    ];
    for mut case in cases {
        case.push("-Yes:$false".to_owned());
        let output = Command::new("pwsh")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .args(&case)
            .current_dir(&repo_root)
            .output()
            .expect("PowerShell 7 must run explicit-false confirmation probe");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success()
                && format!("{stdout}\n{stderr}")
                    .contains("requires explicit -Yes; -Yes:$false is not confirmation."),
            "production explicit-false confirmation was not rejected for {case:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
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
    let ci_checker = fs::read_to_string(repo_root.join("scripts/check-ci-workflow.ps1"))
        .expect("CI workflow checker must be readable UTF-8");
    let update_freshness = fs::read_to_string(repo_root.join("scripts/check-update-freshness.ps1"))
        .expect("update freshness script must be readable UTF-8");
    let codex_temp_config =
        fs::read_to_string(repo_root.join("scripts/configure-codex-validation-temp.ps1"))
            .expect("Codex validation temp configurator must be readable UTF-8");
    let codex_broker = fs::read_to_string(repo_root.join("scripts/codex-powershell-broker.ps1"))
        .expect("Codex PowerShell broker must be readable UTF-8");
    let codex_broker_installer =
        fs::read_to_string(repo_root.join("scripts/install-codex-powershell-broker.ps1"))
            .expect("Codex PowerShell broker installer must be readable UTF-8");
    let install_broker = powershell_function(&codex_broker_installer, "Install-Broker");
    let uninstall_broker = powershell_function(&codex_broker_installer, "Uninstall-Broker");
    let open_upgrade_authority =
        powershell_function(&codex_broker_installer, "Open-BrokerUpgradeAuthority");
    let codex_broker_contract =
        fs::read_to_string(repo_root.join("docs/CODEX_POWERSHELL_BROKER.md"))
            .expect("Codex PowerShell broker contract must be readable UTF-8");
    let repository_gate_inputs =
        fs::read_to_string(repo_root.join("crates/rayman/assets/repository-gate-inputs.json"))
            .expect("repository gate inputs must be readable UTF-8");
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
    assert!(check_repo.contains("'check-ci-workflow.ps1') -SelfTest"));
    assert!(check_repo.contains("'check-ci-workflow.ps1')"));
    assert!(ci_checker.contains("runner context in job-level env"));
    assert!(ci_checker.contains("must not hand-write workspace_skill.yaml"));
    assert!(check_repo.contains("'codex-powershell-broker.ps1') -SelfTest"));
    assert!(check_repo.contains("'install-codex-powershell-broker.ps1') -SelfTest"));
    assert!(source.contains("'codex-powershell-broker.ps1'"));
    assert!(source.contains("'install-codex-powershell-broker.ps1'"));
    assert!(codex_broker.contains("[ValidateSet('identity_probe', 'git_local_commit_v1')]"));
    assert!(codex_broker.contains("$script:SchemaVersion = 3"));
    assert!(codex_broker.contains("[string]$CommitMessage"));
    assert!(codex_broker.contains("[switch]$Yes"));
    assert!(codex_broker.contains("Arbitrary commands are never accepted."));
    assert!(codex_broker.contains("Open-ExclusiveRequest"));
    assert!(codex_broker.contains("Assert-InstalledAclContract"));
    assert!(codex_broker.contains("Assert-InstalledTaskContract"));
    assert!(!codex_broker.contains(".RaymanCodingSkill\\tmp"));
    assert!(codex_broker.contains("Broker self-test process temp root"));
    assert!(codex_broker.contains("Assert-TaskSecurityDescriptorContract"));
    assert!(codex_broker.contains("0x001F01FF"));
    assert!(codex_broker.contains("0x00120089"));
    assert!(codex_broker.contains("Get-BrokerTaskFileSnapshot"));
    assert!(codex_broker.contains("System32\\Tasks"));
    assert!(codex_broker.contains("GetSecurityDescriptorSddlForm"));
    assert!(codex_broker.contains("-AllowAutoInheritedControl -RequireProtectedDacl"));
    assert!(!codex_broker.contains("Schedule.Service"));
    assert!(!codex_broker.contains("GetSecurityDescriptor(7)"));
    assert!(codex_broker_installer.contains("GetSecurityDescriptor(7)"));
    assert!(codex_broker.contains("Assert-ResultCreateDenied"));
    assert!(codex_broker.contains("install_id"));
    assert!(!codex_broker.contains("Invoke-Expression"));
    assert!(!codex_broker.contains("--no-verify"));
    assert!(!codex_broker.contains("'push' {"));
    assert!(!codex_broker.contains("'fetch' {"));
    assert!(codex_broker.contains("JOB_OBJECT_LIMIT_ACTIVE_PROCESS"));
    assert!(codex_broker.contains("CREATE_SUSPENDED"));
    assert!(codex_broker.contains("RunSingleProcess"));
    assert!(codex_broker.contains("HeldRegularFile"));
    assert!(codex_broker.contains("ReadBoundedRegularFileTreeHeld"));
    assert!(codex_broker.contains("tracked-file junction ancestor"));
    assert!(codex_broker.contains("TerminateProcess(process.hProcess, 1)"));
    assert!(codex_broker.contains("GIT_CONFIG_NOSYSTEM"));
    assert!(codex_broker.contains("GIT_NO_LAZY_FETCH"));
    assert!(codex_broker.contains("core.hooksPath="));
    assert!(codex_broker.contains("git_local_commit_v1 rejects an active repository hook"));
    assert!(codex_broker.contains("'add-update' { @('add', '-u', '--', ':/') }"));
    assert!(
        codex_broker.contains("'ls-tree-head' { @('ls-tree', '-r', '-z', '--full-tree', 'HEAD') }")
    );
    let client_snapshot = codex_broker
        .split("function Get-GitTrackedChangeSnapshot")
        .nth(1)
        .and_then(|tail| tail.split("function Close-GitSnapshotHandles").next())
        .expect("broker client snapshot function must be bounded");
    assert!(client_snapshot.contains("-FixedCommand ls-tree-head"));
    assert!(!client_snapshot.contains("-FixedCommand write-tree"));
    assert!(
        codex_broker.contains("client snapshot created index.lock under a read-only Git directory")
    );
    assert!(codex_broker.contains("'hash-filtered-blob'"));
    assert!(codex_broker.contains("@('-C', [string]$Manifest.repository_root, 'hash-object',"));
    assert!(codex_broker.contains("('--path=' + $filteredPath), '--stdin')"));
    assert!(
        codex_broker.contains(
            "git_local_commit_v1 CRLF self-test did not bind the filtered index blob OID."
        )
    );
    assert!(
        codex_broker
            .contains("'hash-commit' { @('hash-object', '-t', 'commit', '-w', '--stdin') }")
    );
    assert!(codex_broker.contains("'allowed-ref-symbolic'"));
    assert!(codex_broker.contains("'update-ref' { @('update-ref', '--no-deref', '--stdin') }"));
    assert!(codex_broker.contains("$stage = Join-Path ([string]$manifest.git_dir) 'index.lock'"));
    assert!(codex_broker.contains("Stage-GitAlternateIndex"));
    assert!(
        codex_broker
            .contains("transaction recovery is indeterminate; preserved journal requires recovery")
    );
    assert!(codex_broker.contains("Publish-GitAlternateIndex"));
    assert!(codex_broker.contains("indeterminate_requires_recovery"));
    assert!(codex_broker.contains("persistent_install_grant"));
    assert!(codex_broker.contains("local_commit_only"));
    assert!(codex_broker.contains("push_allowed"));
    assert!(codex_broker.contains("$script:GitCapabilityReadyName"));
    assert!(codex_broker.contains("function Read-GitCapabilityReady"));
    assert!(codex_broker.contains("function Open-BrokerWorkerLock"));
    assert!(codex_broker.contains("Broker worker lock acquisition failed"));
    assert!(codex_broker.contains("function Remove-PublishedGitTransactionJournal"));
    assert!(
        codex_broker.contains("Terminal Git journal does not match the published success result.")
    );
    assert!(codex_broker.contains("terminal Git journal mismatched result"));
    assert!(codex_broker.contains("public static SafeFileHandle OpenDeleteHandle"));
    assert!(codex_broker.contains("cannot mark exact journal handle for deletion"));
    assert!(codex_broker.contains("Terminal Git journal exact-delete binding drifted."));
    assert!(!codex_broker.contains("catch [IO.IOException] { return }"));
    assert!(codex_broker.contains("requires info/attributes to remain absent"));
    assert!(codex_broker.contains("index backup cannot prove the replaced live index"));
    assert!(!codex_broker.contains("Local\\RaymanCodexPowerShellBroker"));
    assert!(codex_broker_installer.contains("<LogonType>InteractiveToken</LogonType>"));
    assert!(codex_broker_installer.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    assert!(codex_broker_installer.contains("'<RunLevel>HighestAvailable</RunLevel>'"));
    assert!(codex_broker_installer.contains("FileSystemAclExtensions]::CreateDirectory"));
    assert!(codex_broker_installer.contains("[AllowEmptyCollection()][byte[]]$Bytes"));
    assert!(
        codex_broker_installer.contains("Installer zero-byte lock publication self-test failed.")
    );
    assert!(codex_broker_installer.contains("Assert-BrokerTaskXmlBinding"));
    assert!(codex_broker_installer.contains("Set-And-AssertBrokerTaskSecurity"));
    assert!(codex_broker_installer.contains("Register-BrokerTaskWithContext"));
    assert!(codex_broker_installer.contains("Start-BrokerTaskWithContext"));
    assert!(codex_broker_installer.contains("RegisterTask("));
    assert!(codex_broker_installer.contains("TASK_CREATE_OR_UPDATE"));
    assert!(codex_broker_installer.contains("TASK_LOGON_INTERACTIVE_TOKEN"));
    assert!(!codex_broker_installer.contains("Invoke-Schtasks"));
    assert!(!codex_broker_installer.contains("$env:WINDIR"));
    assert!(!codex_broker_installer.contains("schtasks.exe"));
    assert!(codex_broker_installer.contains("Task Scheduler materialized self-test ACL"));
    assert!(codex_broker_installer.contains("descriptor drift"));
    assert!(codex_broker_installer.contains("Remove-BrokerTaskStrict"));
    assert!(codex_broker_installer.contains("function Get-BrokerTaskRuntimeDiagnostic"));
    assert!(codex_broker_installer.contains("last_task_result_hex"));
    assert!(codex_broker_installer.contains("last_heartbeat_error="));
    assert!(codex_broker_installer.contains("BrokerHeartbeatStartupTimeoutSeconds = 60"));
    assert!(!codex_broker_installer.contains("ConvertTo-CanonicalTaskSecurityDescriptor"));
    assert!(!codex_broker_installer.contains("Get-Command pwsh.exe"));
    assert!(codex_broker_installer.contains("function Upgrade-Broker"));
    assert!(codex_broker_installer.contains("function Invoke-BrokerUpgradeSwitchTransaction"));
    assert!(codex_broker_installer.contains("Broker upgrade new-heartbeat action returned"));
    assert!(codex_broker_installer.contains("Broker upgrade commit action returned"));
    assert!(codex_broker_installer.contains("Broker upgrade rollback heartbeat action returned"));
    assert!(codex_broker_installer.contains("function Read-BrokerUpgradeRecoveryJournal"));
    assert!(codex_broker_installer.contains("function Invoke-BrokerInterruptedUpgradeRecovery"));
    assert!(codex_broker_installer.contains("function Ensure-BrokerUpgradeResultsRoot"));
    assert!(codex_broker_installer.contains("OpenDirectoryGuard"));
    assert!(codex_broker_installer.contains("held upgrade result root removal"));
    assert!(codex_broker_installer.contains("function Publish-BrokerReceiptForward"));
    assert!(codex_broker_installer.contains("function Restore-BrokerReceiptFromJournal"));
    assert!(codex_broker_installer.contains("function Get-BrokerReceiptFileState"));
    assert!(codex_broker_installer.contains("function Resolve-BrokerUpgradeRecoveryDisposition"));
    assert!(codex_broker_installer.contains("RecoverySelfTestChild"));
    assert!(codex_broker_installer.contains("function Initialize-BrokerRecoverySelfTestFixture"));
    assert!(codex_broker_installer.contains("-UseRecoverySelfTestTaskStore"));
    assert!(codex_broker_installer.contains("Broker result root recovery guard"));
    assert!(codex_broker_installer.contains("Recovery child did not prove the result-root guard"));
    assert!(codex_broker_installer.contains("ready_published"));
    assert!(codex_broker_installer.contains("ProbeFileReplace"));
    assert!(codex_broker_installer.contains("held receipt replace probe"));
    assert!(codex_broker_installer.contains("forward failure with old receipt unchanged"));
    assert!(codex_broker_installer.contains("forward failure with unknown receipt bytes"));
    assert!(codex_broker_installer.contains("PrepareUpgradeLauncher"));
    assert!(codex_broker_installer.contains("PrepareInstallLauncher"));
    assert!(codex_broker_installer.contains("function Assert-BrokerExplicitConfirmation"));
    assert!(codex_broker_installer.contains("$Yes.IsPresent"));
    assert!(codex_broker_installer.contains("-Yes:`$false is not confirmation"));
    assert!(codex_broker_installer.contains("function Get-BrokerExistingInstallProof"));
    assert!(codex_broker_installer.contains("attestation_operation = 'identity_probe'"));
    assert!(codex_broker_installer.contains("function New-BrokerOwnedTreeSnapshot"));
    assert!(codex_broker_installer.contains("function Remove-BrokerOwnedTreeExact"));
    assert!(codex_broker_installer.contains("function Get-BrokerCommittedSourceBinding"));
    assert!(
        codex_broker_installer.contains("Committed source bytes, index, and HEAD blob disagree")
    );
    assert!(codex_broker_installer.contains("committed-source assume-unchanged flag"));
    assert!(codex_broker_installer.contains("committed-source skip-worktree flag"));
    assert!(codex_broker_installer.contains("git_blob_oid"));
    assert!(codex_broker_installer.contains("OpenOwnedDirectoryDeleteGuard"));
    assert!(codex_broker_installer.contains("function Remove-BrokerOwnedDirectoryHeld"));
    assert!(
        open_upgrade_authority
            .find("$currentGitInputPaths = @($script:UpgradeGitInputCandidates")
            .expect("administrator authority must open the complete Git input set")
            < open_upgrade_authority
                .find("Get-BrokerCommittedSourceBinding")
                .expect("administrator authority must verify committed source blobs"),
        "administrator authority queried source state before retaining Git inputs"
    );
    assert!(codex_broker_installer.contains(
        "Owned-tree retained descendant handle allowed an ancestor rename/replacement window"
    ));
    assert!(
        codex_broker_installer
            .contains("Owned-tree exact deletion crossed into the external sentinel")
    );
    assert!(!install_broker.contains("[IO.File]::ReadAllBytes($script:BrokerSource)"));
    assert!(!uninstall_broker.contains("Remove-Item -LiteralPath $root -Recurse"));
    assert!(codex_broker_installer.contains("function Get-BrokerSelfTestManagedRoot"));
    assert!(codex_broker_installer.contains("function Get-BrokerSelfTestCaseRoot"));
    assert!(codex_broker_installer.contains("'b-' + $Token"));
    assert!(
        codex_broker_installer
            .matches("Get-BrokerSelfTestCaseRoot")
            .count()
            >= 5,
        "main, crash parent and both child modes must share the compact-root constructor"
    );
    assert!(!codex_broker_installer.contains("broker-upgrade-crash-"));
    assert!(
        codex_broker_installer
            .contains("Broker self-test case path budget is unsafe for native held opens")
    );
    assert!(codex_broker_installer.contains("function Assert-BrokerUpgradeTransactionLockReady"));
    assert!(
        codex_broker_installer.contains("function New-BrokerUpgradeOwnedFileBindingFromHeldStream")
    );
    assert!(
        codex_broker_installer.contains("Legacy recovery lost its held transaction-lock binding")
    );
    assert!(codex_broker_installer.contains("transaction_lock_preflight"));
    assert!(codex_broker_installer.contains("function New-BrokerUpgradeUserCommandText"));
    assert!(codex_broker_installer.contains("function Open-BrokerUpgradeAuthority"));
    assert!(codex_broker_installer.contains("function Open-BrokerInstallationGuard"));
    assert!(codex_broker_installer.contains("function Remove-BrokerFileExact"));
    assert!(codex_broker_installer.contains("function Remove-BrokerDirectoryExact"));
    assert!(codex_broker_installer.contains("function Open-BrokerDirectoryGuardChain"));
    assert!(codex_broker_installer.contains("function Assert-BrokerUpgradeAuthorityLifetime"));
    assert!(codex_broker_installer.contains("function Assert-BrokerUpgradeFrontierReport"));
    assert!(codex_broker_installer.contains("function Assert-BrokerUpgradePendingBoundary"));
    assert!(codex_broker_installer.contains("function Invoke-BrokerUpgradeCrashSelfTestChild"));
    assert!(codex_broker_installer.contains("mutable_launcher_file_published = $false"));
    assert!(codex_broker_installer.contains("Cross-process installation guard exposed"));
    assert!(
        codex_broker_installer
            .contains("Trusted user command did not block a real cross-process replacement")
    );
    assert!(codex_broker_installer.contains("malformed_ready"));
    assert!(codex_broker_installer.contains("wrong_acl_ready"));
    assert!(codex_broker_installer.contains("directory_ready"));
    assert!(codex_broker_installer.contains("reparse_ready"));
    assert!(!codex_broker_installer.contains("function New-BrokerUpgradeUacOuterScriptText"));
    assert!(!codex_broker_installer.contains("function New-BrokerUpgradeElevatedScriptText"));
    assert!(!codex_broker_installer.contains("function New-BrokerUpgradeCmdText"));
    assert!(!codex_broker_installer.contains("$manifest.rayman"));
    assert!(!codex_broker_installer.contains("boundFiles['rayman']"));
    assert!(codex_broker_installer.contains("function Open-GitTransactionGuard"));
    assert!(codex_broker_installer.contains("function Get-GitTransactionOperationalState"));
    assert!(
        codex_broker_installer.contains("function Remove-TerminalGitTransactionJournalsForUpgrade")
    );
    assert!(codex_broker_installer.contains("function Test-BrokerExactInheritedFileSecurity"));
    assert!(codex_broker_installer.contains("function Assert-TerminalGitArtifactSecurity"));
    assert!(codex_broker_installer.contains(
        "Production-inherited terminal artifact simulation did not reproduce the worker ACL shape."
    ));
    assert!(codex_broker_installer.contains("terminal inherited journal with explicit ACL drift"));
    assert!(codex_broker_installer.contains("terminal inherited result with explicit ACL drift"));
    assert!(
        codex_broker_installer
            .contains("Terminal journal self-test did not retire exactly two verified inherited journals while preserving both results.")
    );
    assert!(
        codex_broker_installer.contains("Nonterminal journal self-test removed recovery evidence.")
    );
    assert!(
        codex_broker_installer
            .contains("function Assert-GitTransactionDirectorySafeForInstallerCleanup")
    );
    assert!(codex_broker_installer.contains("function Assert-GitCapabilityReadyInstalled"));
    assert!(
        codex_broker_installer
            .contains("Upgrade switch transaction rollback ordering self-test failed")
    );
    assert!(
        codex_broker_installer
            .contains("Upgrade switch transaction returned an invalid result shape")
    );
    assert!(
        codex_broker_installer
            .contains("Atomic-write failure self-test retained target or staging bytes")
    );
    assert!(
        codex_broker_installer
            .contains("Broker uninstall refuses a pending upgrade recovery journal.")
    );
    assert!(
        codex_broker_installer.contains("the exact schema-v2 task/receipt/heartbeat were restored")
    );
    assert!(codex_broker_installer.contains("C:\\Program Files\\Git\\cmd\\git.exe"));
    assert!(codex_broker_installer.contains("mingw64\\bin\\git.exe"));
    assert!(codex_broker_installer.contains("Get-AuthenticodeSignature"));
    assert!(codex_broker_installer.contains("git_capability_manifest_sha256"));
    assert!(codex_broker_contract.contains("BROKER-GIT-REGISTRATION"));
    assert!(codex_broker_contract.contains("BROKER-GIT-REQUEST"));
    assert!(codex_broker_contract.contains("BROKER-GIT-EXECUTION"));
    assert!(codex_broker_contract.contains("BROKER-GIT-TRANSACTION"));
    assert!(codex_broker_contract.contains("BROKER-GIT-RECOVERY"));
    assert!(codex_broker_contract.contains("BROKER-GIT-AUTHORIZATION"));
    assert!(codex_broker_contract.contains("BROKER-GIT-MUTATION-AUTHORITY"));
    assert!(codex_broker_contract.contains("BROKER-GIT-INSTALLATION-SERIALIZATION"));
    assert!(codex_broker_contract.contains("BROKER-GIT-DIAGNOSTIC-BOUNDARY"));
    assert!(codex_broker_contract.contains("required absence of `info/attributes`"));
    assert!(codex_broker_contract.contains("final ready marker"));
    assert!(codex_broker_contract.contains("upgrade-recovery-v1.json"));
    assert!(codex_broker_contract.contains("No mutable fixed diagnostic file"));
    assert!(codex_broker_contract.contains("C:\\Program Files\\PowerShell\\7\\pwsh.exe"));
    assert!(codex_broker_contract.contains("PrepareInstallLauncher"));
    assert!(codex_broker_contract.contains("already_current=true"));
    assert!(codex_broker_contract.contains("Task Scheduler COM"));
    assert!(codex_broker_contract.contains("Yes.IsPresent"));
    assert!(codex_broker_contract.contains("journaled v2/v3 receipt pair"));
    assert!(codex_broker_contract.contains("junction cannot"));
    assert!(codex_broker_contract.contains("must each return exactly one object"));
    assert!(codex_broker_contract.contains("sequentially publishes the new receipt/task"));
    assert!(codex_broker_contract.contains("one complete extra staged hash-version"));
    assert!(codex_broker_contract.contains("every ACE is inherited, no explicit ACE exists"));
    assert!(codex_broker_contract.contains("never rewrites, protects, normalizes or"));
    assert!(codex_broker_contract.contains("widens an ACL"));
    assert!(codex_broker_contract.contains("result identity and ACL"));
    assert!(codex_broker_contract.contains("must remain stable too"));
    assert!(!codex_broker_contract.contains("atomically switches receipt/task"));
    assert!(!codex_broker_contract.contains("Any failure restores"));
    assert!(repository_gate_inputs.contains("docs/CODEX_POWERSHELL_BROKER.md"));
    assert!(!codex_broker_installer.contains("E:\\codex-sandbox\\temp\\codex-powershell-broker"));
    assert!(check_repo.contains("'check-update-freshness.ps1') -SelfTest"));
    assert!(source.contains("'check-update-freshness.ps1'"));
    assert!(release_closeout.contains("'check-update-freshness.ps1'"));
    assert!(update_freshness.contains("manifest_verified"));
    assert!(update_freshness.contains("MinimumRemainingDays = 14"));
    assert!(update_freshness.contains("Do not replace the existing release assets"));
    assert!(update_freshness.contains("Get-VerifiedManifestAssetHashes"));
    assert!(update_freshness.contains("assets_verified = $assetHashes.Count"));
    assert!(workflow.contains("signed-release-freshness:"));
    assert!(workflow.contains("ci-${{ github.event_name }}-${{ github.ref }}"));
    assert!(workflow.contains("-MinimumRemainingDays 14"));
    assert!(workflow.contains("-MinimumRemainingDays 29"));
    assert!(workflow.contains("-ExpectedVersion $tag.Substring(1)"));
    assert!(workflow.contains("-AssetDirectory dist"));
    assert!(workflow.contains("-AssetDirectory $assetRoot"));
    for asset in [
        "rayman-windows-x86_64.exe",
        "rayman-update-worker-windows-x86_64.exe",
        "raymancodingskill-SKILL.md",
        "raymancodingskill-AGENTS.md",
        "raymancodingskill-workflow-contract.md",
        "install-rayman.ps1",
    ] {
        assert!(workflow.contains(&format!("--pattern {asset}")));
    }
    assert!(workflow.contains("Verify exact release binaries source-fresh before staging"));
    assert!(workflow.contains("Stage only the verified release bytes"));
    assert!(workflow.contains("-CliPath $finalCli"));
    assert!(workflow.contains("-WorkerPath $finalWorker"));
    assert!(workflow.contains("-SkillResourceMode Source"));
    assert!(workflow.contains("-RequireSourceFresh"));
    assert!(workflow.contains("-VerifyGitTag"));
    let final_asset_verification = workflow
        .find("Verify exact release binaries source-fresh before staging")
        .expect("signed release must verify the release binaries");
    let final_asset_copy = workflow
        .find("Copy-Item target/release/rayman.exe dist/rayman-windows-x86_64.exe")
        .expect("signed release must stage the verified CLI asset");
    let manifest_signing = workflow
        .find("Create and sign canonical update manifest")
        .expect("signed release must sign a canonical manifest");
    assert!(final_asset_verification < final_asset_copy);
    assert!(final_asset_copy < manifest_signing);
    assert!(workflow.contains("Staged CLI differs from the source-fresh verified binary."));
    assert!(workflow.contains("Staged worker differs from the source-fresh verified binary."));
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
    assert!(release_closeout.contains(
        "'--command', 'cargo run --locked --manifest-path xtask/Cargo.toml -- repository-gate'"
    ));
    assert!(
        !release_closeout.contains("'--command', 'pwsh -NoProfile -File scripts/check-repo.ps1'")
    );
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
    assert!(!source.contains("audit_self_test_exercises_only_the_audit_contract"));
    assert!(!check_repo.contains("audit_self_test_exercises_only_the_audit_contract"));
    for consumer in [&source, &check_repo] {
        assert!(consumer.contains("Join-Path $PSScriptRoot 'repository-quality.ps1'"));
        assert!(
            consumer.contains("47f405e725ad272b2d2c0d2b189855375962689f2b356eadc306305f957a0b77")
        );
        assert!(consumer.contains("$usingDefaultProvider"));
        assert!(consumer.contains("Join-Path $PSScriptRoot 'read-repository-quality.ps1'"));
        assert!(consumer.contains("Repository quality reader hash drifted"));
        assert!(
            !powershell_function(consumer, "Get-RepositoryQualityCommands")
                .contains("ConvertFrom-Json -Depth 8 -NoEnumerate")
        );
        assert!(!consumer.contains(". (Join-Path $PSScriptRoot 'repository-quality.ps1')"));
    }
    let reader = fs::read_to_string(repo_root.join("scripts/read-repository-quality.ps1"))
        .expect("shared quality reader must be readable");
    assert!(reader.contains("Repository quality command provider hash drifted"));
    assert!(reader.contains("ConvertFrom-Json -Depth 8 -NoEnumerate"));
    assert!(reader.contains("$document.commands -isnot [array]"));
    assert!(reader.contains("$command.argv -isnot [array]"));
    assert!(!reader.contains("if ($LASTEXITCODE -ne 0"));
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
        "schema = 'rayman.release.binding.v6'",
        "test_traceability = [ordered]@{",
        "'check-test-traceability.ps1'",
        "workspace_activation = $sourceFreshInputs.workspace_activation",
        "source_fresh_environment = $sourceFreshInputs.source_fresh_environment",
        "rayman.release.binding.v2",
        "rayman.release.binding.v3",
        "rayman.release.binding.v4",
        "rayman.release.binding.v5",
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
