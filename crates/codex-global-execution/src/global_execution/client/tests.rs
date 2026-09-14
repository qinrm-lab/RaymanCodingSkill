use super::*;
use std::cell::Cell;

struct CommitFixture {
    _backend: tempfile::TempDir,
    workspace: tempfile::TempDir,
    client: Client,
    registration: Registration,
}

impl CommitFixture {
    fn new(hook_body: Option<&str>) -> Self {
        let backend = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
        let git = |args: &[&str]| {
            let output = std::process::Command::new(&program)
                .args(args)
                .current_dir(workspace.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Expiry fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        let enrollment_path = backend
            .path()
            .join(format!("project-{}.json", "b".repeat(32)));
        if let Some(body) = hook_body {
            std::fs::create_dir(workspace.path().join(".githooks")).unwrap();
            let body = body
                .replace(
                    "@ENROLLMENT@",
                    &enrollment_path.to_string_lossy().replace('\\', "/"),
                )
                .replace(
                    "@WORKER@",
                    &backend
                        .path()
                        .join("worker.exe")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            std::fs::write(
                workspace.path().join(".githooks/pre-commit"),
                format!("#!/bin/sh\nset -e\n{body}\necho hook-completed >\"$GIT_DIR/hook-ran\"\n"),
            )
            .unwrap();
        }
        std::fs::write(workspace.path().join("tracked.txt"), b"before\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "initial"]);
        std::fs::write(workspace.path().join("history.txt"), b"history\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "commit.gpgsign=false", "commit", "-m", "second"]);
        if hook_body.is_some() {
            git(&["config", "core.hooksPath", ".githooks"]);
        }
        std::fs::write(workspace.path().join("tracked.txt"), b"after\n").unwrap();
        let sid = crate::execution_context::execution_context_probe()
            .principal_sid
            .unwrap();
        super::super::native::protect_fixture_directory(backend.path(), &sid, "").unwrap();
        let protected = ProtectedDirectory::open(backend.path(), &sid).unwrap();
        std::fs::write(backend.path().join("worker.exe"), b"fixture worker").unwrap();
        let installation = Installation {
            schema_version: 1,
            installation_id: "1".repeat(32),
            owner_sid: sid.clone(),
            root_identity: protected.identity().into(),
            worker_sha256: crate::hash::sha256_bytes(b"fixture worker"),
            client_sha256: "2".repeat(64),
            source_project_id: "9".repeat(32),
        };
        crate::file_io::write_json(&backend.path().join("installation.json"), &installation)
            .unwrap();
        let mut registration = super::super::tests::registration();
        registration.owner_sid = sid;
        registration.root_identity = super::super::native::SourceDirectory::open(workspace.path())
            .unwrap()
            .identity()
            .into();
        let git_dir = workspace.path().join(".git");
        registration.git_common_identity = super::super::native::SourceDirectory::open(&git_dir)
            .unwrap()
            .identity()
            .into();
        registration.capabilities.install_adapters.clear();
        let binding = GitBinding {
            executable: program.clone(),
            executable_sha256: crate::hash::sha256_file(&program).unwrap(),
            config_sha256: crate::hash::sha256_file(&git_dir.join("config")).unwrap(),
            git_directory: git_dir.clone(),
            common_directory: git_dir,
            branch_ref: "refs/heads/main".into(),
            content_policy: GitContentPolicy::capture(&program, workspace.path()).unwrap(),
            hook_policy: GitHookPolicy::capture(&program, workspace.path()).unwrap(),
        };
        let mut enrollment = Enrollment {
            schema_version: 1,
            installation_id: installation.installation_id.clone(),
            registration,
            workspace: workspace.path().into(),
            source_policy: SourcePolicy::WorkspaceFingerprintV1,
            git: Some(binding),
            commit_identity: Some(CommitIdentity {
                name: "Fixture".into(),
                email: "fixture@example.invalid".into(),
            }),
            install_policies: Default::default(),
        };
        enrollment.registration.policy_sha256 = enrollment.policy_digest().unwrap();
        crate::file_io::write_json(&enrollment_path, &enrollment).unwrap();
        let client = Client::open(backend.path()).unwrap();
        Self {
            _backend: backend,
            workspace,
            client,
            registration: enrollment.registration,
        }
    }
}

#[test]
fn commit_request_clock_is_sampled_after_hook_and_binding_revalidation() {
    let fixture = CommitFixture::new(Some(""));
    let calls = Cell::new(0);
    let before = 1000;
    let after_long_hook = before + 219;
    let request = fixture
        .client
        .prepare_commit_request_with_clock(
            fixture.workspace.path(),
            &["tracked.txt".into()],
            "reviewed candidate",
            || {
                assert!(fixture.workspace.path().join(".git/hook-ran").is_file());
                calls.set(calls.get() + 1);
                after_long_hook
            },
        )
        .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(request.created_at, after_long_hook);
    assert_eq!(request.expires_at - request.created_at, 120);
    validate_request(&request, &fixture.registration, after_long_hook).unwrap();
    assert!(validate_request(&request, &fixture.registration, after_long_hook + 120).is_err());
    let Operation::Commit {
        hook_receipt: Some(receipt),
        ..
    } = request.operation
    else {
        panic!("hook evidence missing");
    };
    assert_eq!(receipt.exit_code, 0);
    assert_eq!(receipt.source_sha256, request.source_sha256);
}

#[test]
fn commit_preflight_failure_or_binding_drift_never_reaches_request_issuance() {
    for body in [
        "exit 23",
        "printf drift > tracked.txt",
        "'C:/Program Files/Git/mingw64/bin/git.exe' update-ref refs/heads/main HEAD^",
        "printf drift >>\"$GIT_DIR/index\"",
        "printf '\\n# drift\\n' >>\"$GIT_DIR/config\"",
        "printf '# changed\\n' > .githooks/pre-commit",
        "printf invalid > '@ENROLLMENT@'",
        "printf invalid > '@WORKER@'",
    ] {
        let fixture = CommitFixture::new(Some(body));
        let called = Cell::new(false);
        let result = fixture.client.prepare_commit_request_with_clock(
            fixture.workspace.path(),
            &["tracked.txt".into()],
            "must refuse drift",
            || {
                called.set(true);
                1219
            },
        );
        assert!(result.is_err(), "drift was accepted: {body}");
        if body != "exit 23" {
            assert!(
                fixture.workspace.path().join(".git/hook-ran").is_file(),
                "the drift hook must succeed before the binding is rejected: {body}"
            );
        }
        assert!(
            !called.get(),
            "request issuance began despite failed preflight: {body}"
        );
        assert!(!fixture._backend.path().join("requests").exists());
    }
}

#[test]
fn invalid_commit_intent_does_not_run_hook_or_sample_issuance_clock() {
    let fixture = CommitFixture::new(Some(""));
    for (message, paths) in [
        ("", vec!["tracked.txt".into()]),
        ("invalid\nmessage", vec!["tracked.txt".into()]),
        ("valid", vec!["tracked.txt".into(), "tracked.txt".into()]),
        ("valid", vec!["../outside".into()]),
    ] {
        let called = Cell::new(false);
        assert!(
            fixture
                .client
                .prepare_commit_request_with_clock(
                    fixture.workspace.path(),
                    &paths,
                    message,
                    || {
                        called.set(true);
                        1219
                    },
                )
                .is_err()
        );
        assert!(!called.get());
        assert!(!fixture.workspace.path().join(".git/hook-ran").exists());
    }
}

#[test]
fn hookless_commit_retains_exact_snapshot_and_original_ttl() {
    let fixture = CommitFixture::new(None);
    let request = fixture
        .client
        .prepare_commit_request_with_clock(
            fixture.workspace.path(),
            &["tracked.txt".into()],
            "hookless candidate",
            || 2000,
        )
        .unwrap();
    assert_eq!(request.created_at, 2000);
    assert_eq!(request.expires_at, 2120);
    validate_request(&request, &fixture.registration, 2119).unwrap();
    assert!(validate_request(&request, &fixture.registration, 2120).is_err());
    assert!(matches!(
        request.operation,
        Operation::Commit {
            hook_receipt: None,
            ..
        }
    ));
}
