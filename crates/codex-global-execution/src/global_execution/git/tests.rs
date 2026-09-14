use super::*;
#[cfg(windows)]
#[test]
fn worktree_metadata_rejects_git_execution_and_include_settings() {
    for value in [
        "[codex]\nlocalEnvironmentConfigPath = __none__\n",
        "[codex]\r\n\tlocalEnvironmentConfigPath = .codex/environments/environment.toml\r\n",
        "# comment\n",
    ] {
        validate_codex_worktree_metadata(value.as_bytes()).unwrap();
    }
    for value in [
        "[core]\nfsmonitor = !helper",
        "[include]\npath = elsewhere",
        "[codex]\nlocalEnvironmentConfigPath=x\n[core]\nworktree=y",
        "[codex]\nlocalEnvironmentConfigPath=x\\\n",
        "[codex]\nlocalEnvironmentConfigPath=x\nlocalEnvironmentConfigPath=y",
        "[codex]\nother=value",
    ] {
        assert!(
            validate_codex_worktree_metadata(value.as_bytes()).is_err(),
            "{value}"
        );
    }
}
#[cfg(windows)]
#[test]
fn detached_linked_worktree_commit_preserves_parent_and_recovers_interruption() {
    let base = tempfile::tempdir().unwrap();
    let repo = base.path().join("parent");
    let linked = base.path().join("linked");
    let trusted = base.path().join("trusted");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&trusted).unwrap();
    let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
    let run = |cwd: &Path, args: &[&str]| {
        let out = std::process::Command::new(&program)
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    run(&repo, &["init", "-b", "main"]);
    run(&repo, &["config", "user.name", "Fixture"]);
    run(&repo, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(repo.join(".gitattributes"), b"* text eol=lf\n").unwrap();
    std::fs::write(repo.join("selected.txt"), b"old\n").unwrap();
    std::fs::write(repo.join("preserved.txt"), b"old\n").unwrap();
    run(&repo, &["add", "."]);
    run(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-m", "baseline"],
    );
    let parent_head = run(&repo, &["rev-parse", "HEAD"]);
    let parent_index = std::fs::read(repo.join(".git/index")).unwrap();
    let parent_ref = std::fs::read(repo.join(".git/refs/heads/main")).unwrap();
    run(&repo, &["config", "extensions.worktreeConfig", "true"]);
    run(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            "HEAD",
        ],
    );
    let linked_gitdir = PathBuf::from(run(&linked, &["rev-parse", "--absolute-git-dir"]));
    std::fs::write(
        linked_gitdir.join("config.worktree"),
        b"[codex]\r\n\tlocalEnvironmentConfigPath = __none__\r\n",
    )
    .unwrap();
    std::fs::write(linked.join("selected.txt"), b"selected change\r\n").unwrap();
    std::fs::write(linked.join("preserved.txt"), b"unrelated change\r\n").unwrap();
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .unwrap();
    super::super::native::protect_fixture_directory(&trusted, &sid, "").unwrap();
    let isolation = ProtectedDirectory::open(&trusted, &sid).unwrap();
    std::fs::write(trusted.join("worker.exe"), b"fixture worker").unwrap();
    std::fs::write(trusted.join("client.exe"), b"fixture client").unwrap();
    let installation = Installation {
        schema_version: 1,
        installation_id: "a".repeat(32),
        owner_sid: sid,
        root_identity: isolation.identity().into(),
        worker_sha256: crate::hash::sha256_bytes(b"fixture worker"),
        client_sha256: crate::hash::sha256_bytes(b"fixture client"),
        source_project_id: "b".repeat(32),
    };
    crate::file_io::write_json(&trusted.join("installation.json"), &installation).unwrap();
    let enrolled = enroll(
        &trusted,
        &linked,
        Some((&program, "Fixture", "fixture@example.invalid")),
        true,
        true,
    )
    .expect("detached linked worktree enrollment must succeed without attaching a branch");
    let binding = enrolled.git.as_ref().unwrap();
    assert_eq!(binding.branch_ref, "HEAD");
    let inspector =
        GitInspector::open(&linked, binding, &enrolled.registration, &isolation).unwrap();
    let snapshot = inspector.capture(&enrolled.registration).unwrap();
    let mut request = super::super::tests::request(&enrolled.registration);
    request.source_sha256 = crate::source_fingerprint(&linked).unwrap();
    request.operation = Operation::Commit {
        expected_head: snapshot.head,
        expected_index_sha256: snapshot.index_sha256,
        message: "detached worktree fixture".into(),
        changes: snapshot
            .changes
            .into_iter()
            .filter(|c| c.path == "selected.txt")
            .collect(),
        hook_receipt: None,
    };
    let candidate = inspector
        .prepare_commit(
            &request,
            &enrolled.registration,
            1000,
            "Fixture",
            "fixture@example.invalid",
        )
        .unwrap();
    assert!(
        inspector
            .test_publish_cut(&request, &enrolled.registration, "ref_published")
            .is_err()
    );
    let published = inspector
        .publish_commit(&request, &enrolled.registration, 1000)
        .unwrap();
    assert_eq!(published.commit, candidate.commit);
    assert_eq!(run(&linked, &["rev-parse", "HEAD"]), candidate.commit);
    assert_eq!(run(&repo, &["rev-parse", "HEAD"]), parent_head);
    assert_eq!(
        std::fs::read(repo.join(".git/index")).unwrap(),
        parent_index
    );
    assert_eq!(
        std::fs::read(repo.join(".git/refs/heads/main")).unwrap(),
        parent_ref
    );
    assert_eq!(
        std::fs::read(linked.join("selected.txt")).unwrap(),
        b"selected change\r\n"
    );
    assert_eq!(
        std::fs::read(linked.join("preserved.txt")).unwrap(),
        b"unrelated change\r\n"
    );
    run(&linked, &["switch", "-c", "later-branch"]);
    assert!(inspector.capture(&enrolled.registration).is_err());
}
fn with_modify_only_commit_fixture(
    check: impl FnOnce(&GitInspector<'_>, &Request, &Registration, &CommitCandidate, &Path),
) {
    let repo = tempfile::tempdir().unwrap();
    let trusted = tempfile::tempdir().unwrap();
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .unwrap();
    super::super::native::protect_fixture_directory(trusted.path(), &sid, "").unwrap();
    let isolation = ProtectedDirectory::open(trusted.path(), &sid).unwrap();
    let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
    let run = |args: &[&str]| {
        let output = std::process::Command::new(&program)
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Fixture"]);
    run(&["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(repo.path().join(".gitattributes"), b"* text=auto eol=lf\n").unwrap();
    std::fs::write(repo.path().join("selected.txt"), b"old\n").unwrap();
    std::fs::write(repo.path().join("preserved.txt"), b"old\n").unwrap();
    run(&["add", "."]);
    run(&["-c", "commit.gpgsign=false", "commit", "-m", "fixture"]);
    std::fs::write(repo.path().join("selected.txt"), b"selected change\r\n").unwrap();
    std::fs::write(repo.path().join("preserved.txt"), b"unrelated change\r\n").unwrap();
    let gitdir = repo.path().join(".git");
    let ref_parent = gitdir.join("refs/heads");
    // Fresh seeds inherit Modify, while the original files carry an
    // explicit protected DACL. This forces metadata preservation instead
    // of the already-equal fast path, without impersonating another SID.
    for parent in [&gitdir, &ref_parent] {
        super::super::native::set_fixture_dacl(parent, &format!("D:P(A;OICI;0x1301bf;;;{sid})"))
            .unwrap();
    }
    for file in [gitdir.join("index"), ref_parent.join("main")] {
        super::super::native::set_fixture_dacl(&file, &format!("D:P(A;;0x1301bf;;;{sid})"))
            .unwrap();
    }
    let mut registration = super::super::tests::registration();
    registration.owner_sid = sid;
    registration.root_identity = super::super::native::SourceDirectory::open(repo.path())
        .unwrap()
        .identity()
        .into();
    registration.git_common_identity = super::super::native::SourceDirectory::open(&gitdir)
        .unwrap()
        .identity()
        .into();
    let binding = GitBinding {
        content_policy: GitContentPolicy::capture(&program, repo.path()).unwrap(),
        hook_policy: GitHookPolicy::capture(&program, repo.path()).unwrap(),
        executable_sha256: crate::hash::sha256_file(&program).unwrap(),
        executable: program,
        git_directory: gitdir.clone(),
        common_directory: gitdir.clone(),
        branch_ref: "refs/heads/main".into(),
        config_sha256: crate::hash::sha256_file(&gitdir.join("config")).unwrap(),
    };
    let git = GitInspector::open(repo.path(), &binding, &registration, &isolation).unwrap();
    let snapshot = git.capture(&registration).unwrap();
    let mut request = super::super::tests::request(&registration);
    request.source_sha256 = crate::source_fingerprint(repo.path()).unwrap();
    request.operation = Operation::Commit {
        expected_head: snapshot.head,
        expected_index_sha256: snapshot.index_sha256,
        message: "metadata policy fixture".into(),
        changes: snapshot
            .changes
            .into_iter()
            .filter(|x| x.path == "selected.txt")
            .collect(),
        hook_receipt: None,
    };
    let candidate = git
        .prepare_commit(
            &request,
            &registration,
            1000,
            "Fixture",
            "fixture@example.invalid",
        )
        .unwrap();
    check(&git, &request, &registration, &candidate, repo.path());
}

#[cfg(windows)]
#[test]
fn git_publication_preserves_access_under_modify_only_metadata() {
    with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
        let published = git.publish_commit(request, registration, 1000);
        assert!(
            published.is_ok(),
            "Modify-only metadata publication failed: {published:?}"
        );
        let after = git.capture(registration).unwrap();
        assert_eq!(after.head, candidate.commit);
        assert_eq!(after.index_sha256, candidate.candidate_index_sha256);
        assert_eq!(candidate.paths, vec!["selected.txt"]);
        assert_eq!(
            after
                .changes
                .iter()
                .map(|x| x.path.as_str())
                .collect::<Vec<_>>(),
            vec!["preserved.txt"]
        );
        assert_eq!(
            std::fs::read(repo.join("selected.txt")).unwrap(),
            b"selected change\r\n"
        );
        assert_eq!(
            std::fs::read(repo.join("preserved.txt")).unwrap(),
            b"unrelated change\r\n"
        );
    });
}

#[cfg(windows)]
#[test]
fn git_publication_recovers_seed_metadata_object_and_ref_interruptions() {
    for point in [
        "attempt_recorded",
        "index_seed_created",
        "ref_seed_created",
        "seeds_recorded",
        "index_metadata_prepared",
        "ref_metadata_prepared",
        "metadata_recorded",
        "index_lock_claimed",
        "ref_lock_claimed",
        "object_attempt_recorded",
        "object_seed_created",
        "object_seed_recorded",
        "object_published",
        "objects_published",
        "ref_published",
        "index_published",
        "complete",
    ] {
        with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
            let result = git.publish_commit_inner(request, registration, Some(point));
            assert!(result.is_err(), "fault point was not reached: {point}");
            let mut orphans = Vec::new();
            if matches!(
                point,
                "index_seed_created" | "ref_seed_created" | "object_seed_created"
            ) {
                let mut parents = vec![repo.join(".git"), repo.join(".git/refs/heads")];
                for entry in std::fs::read_dir(repo.join(".git/objects")).unwrap() {
                    let entry = entry.unwrap();
                    if entry.file_type().unwrap().is_dir()
                        && entry.file_name().to_string_lossy().len() == 2
                    {
                        parents.push(entry.path());
                    }
                }
                for parent in parents {
                    for entry in std::fs::read_dir(parent).unwrap() {
                        let entry = entry.unwrap();
                        if entry.file_name().to_string_lossy().starts_with(".rayman-") {
                            let path = entry.path();
                            let identity = super::super::publication::identity(
                                &std::fs::File::open(&path).unwrap(),
                                &path,
                            )
                            .unwrap();
                            orphans.push((path.clone(), identity, std::fs::read(path).unwrap()));
                        }
                    }
                }
                assert!(!orphans.is_empty(), "no unjournaled seed at {point}");
            }
            let recovered = git
                .recover_admitted_commit(request, registration)
                .unwrap_or_else(|e| panic!("recovery failed at {point}: {e:#}"));
            assert_eq!(recovered.commit, candidate.commit);
            let after = git.capture(registration).unwrap();
            assert_eq!(after.head, candidate.commit);
            assert_eq!(after.index_sha256, candidate.candidate_index_sha256);
            assert_eq!(
                after
                    .changes
                    .iter()
                    .map(|x| x.path.as_str())
                    .collect::<Vec<_>>(),
                vec!["preserved.txt"]
            );
            assert!(!repo.join(".git/index.lock").exists());
            assert!(!repo.join(".git/refs/heads/main.lock").exists());
            assert_eq!(
                std::fs::read(repo.join("selected.txt")).unwrap(),
                b"selected change\r\n"
            );
            assert_eq!(
                std::fs::read(repo.join("preserved.txt")).unwrap(),
                b"unrelated change\r\n"
            );
            for (path, identity, bytes) in orphans {
                assert_eq!(
                    std::fs::read(&path).unwrap(),
                    bytes,
                    "unknown seed changed at {point}"
                );
                assert_eq!(
                    super::super::publication::identity(
                        &std::fs::File::open(&path).unwrap(),
                        &path
                    )
                    .unwrap(),
                    identity,
                    "unknown seed adopted at {point}"
                );
            }
        });
    }
}

#[cfg(windows)]
#[test]
fn git_recovery_refuses_same_bytes_with_a_replaced_seed_identity() {
    with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
        assert!(
            git.publish_commit_inner(request, registration, Some("seeds_recorded"))
                .is_err()
        );
        let journal = git
            .isolation
            .join(format!("candidate-{}/publication.json", request.request_id));
        let record: PublicationRecord = crate::file_io::read_json(&journal).unwrap().unwrap();
        let seed = repo.join(".git").join(record.index_seed);
        let bytes = std::fs::read(&seed).unwrap();
        std::fs::remove_file(&seed).unwrap();
        std::fs::write(&seed, bytes).unwrap();
        let replacement =
            super::super::publication::identity(&std::fs::File::open(&seed).unwrap(), &seed)
                .unwrap();
        assert_ne!(replacement, record.index_identity);
        assert!(git.recover_admitted_commit(request, registration).is_err());
        let current = git.capture(registration).unwrap();
        assert_eq!(current.head, candidate.parent);
        assert_eq!(current.index_sha256, candidate.original_index_sha256);
    });
}

#[cfg(windows)]
#[test]
fn git_recovery_preserves_a_foreign_standard_lock() {
    with_modify_only_commit_fixture(|git, request, registration, candidate, repo| {
        let lock = repo.join(".git/index.lock");
        std::fs::write(&lock, b"another Git writer owns this lock").unwrap();
        assert!(git.publish_commit(request, registration, 1000).is_err());
        assert_eq!(
            std::fs::read(&lock).unwrap(),
            b"another Git writer owns this lock"
        );
        assert_eq!(git.capture(registration).unwrap().head, candidate.parent);
        // The fixture's other writer releases its own lock. The publisher
        // must never perform this deletion on its behalf.
        std::fs::remove_file(lock).unwrap();
        assert_eq!(
            git.recover_admitted_commit(request, registration)
                .unwrap()
                .commit,
            candidate.commit
        );
    });
}

#[cfg(windows)]
#[test]
fn git_recovery_reads_legacy_post_metadata_journals() {
    with_modify_only_commit_fixture(|git, request, registration, candidate, _| {
        assert!(
            git.publish_commit_inner(request, registration, Some("metadata_recorded"))
                .is_err()
        );
        let journal = git
            .isolation
            .join(format!("candidate-{}/publication.json", request.request_id));
        let mut legacy: serde_json::Value = crate::file_io::read_json(&journal).unwrap().unwrap();
        for key in [
            "version",
            "metadata_ready",
            "object_seeds",
            "object_attempts",
        ] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        crate::file_io::write_json(&journal, &legacy).unwrap();
        assert_eq!(
            git.recover_admitted_commit(request, registration)
                .unwrap()
                .commit,
            candidate.commit
        );
    });
}

#[cfg(windows)]
#[test]
fn git_recovery_bounds_unjournaled_staging_attempts() {
    with_modify_only_commit_fixture(|git, request, registration, candidate, _| {
        for _ in 0..8 {
            assert!(
                git.publish_commit_inner(request, registration, Some("index_seed_created"))
                    .is_err()
            );
        }
        let error = git
            .recover_admitted_commit(request, registration)
            .unwrap_err();
        assert!(error.to_string().contains("retry limit reached"));
        let current = git.capture(registration).unwrap();
        assert_eq!(current.head, candidate.parent);
        assert_eq!(current.index_sha256, candidate.original_index_sha256);
    });
}

#[cfg(windows)]
#[test]
fn git_tree_and_index_parsers_reject_conflicts_aliases_and_nonfiles() {
    let oid = "a".repeat(40);
    let index = format!("100644 {oid} 0\tsrc/模块.rs\0");
    let tree = format!("100644 blob {oid}\tsrc/模块.rs\0");
    assert_eq!(
        parse_entries(index.as_bytes(), false, 40).unwrap(),
        parse_entries(tree.as_bytes(), true, 40).unwrap()
    );
    for text in [
        format!("100644 {oid} 1\ta\0"),
        format!("120000 {oid} 0\ta\0"),
        format!("100644 {oid} 0\t../a\0"),
        format!("100644 {oid} 0\ta\0").repeat(2),
        format!("100644 {oid} 0\tA\0") + &format!("100644 {oid} 0\ta\0"),
    ] {
        assert!(parse_entries(text.as_bytes(), false, 40).is_err());
    }
}

#[cfg(windows)]
#[test]
fn real_git_snapshot_preserves_source_and_index_with_added_crlf_and_deleted_files() {
    for (key, value) in [
        ("=C:", "C:\\Users\\fixture"),
        ("=E:", "E:\\work"),
        ("", "value"),
        ("BAD=NAME", "value"),
        ("BAD\0NAME", "value"),
        ("NAME", "bad\0value"),
        ("git_dir", "untrusted"),
        ("API_TOKEN", "private"),
        ("MY_SECRET", "private"),
        ("AUTH_KEY", "private"),
    ] {
        assert!(!hook_environment_entry_allowed(key, value));
    }
    assert!(hook_environment_entry_allowed("PATH", "C:\\tools"));
    assert!(hook_environment_entry_allowed("TEMP", "E:\\临时目录"));
    assert!(hook_environment_entry_allowed("EMPTY", ""));
    let repo = tempfile::tempdir().unwrap();
    let trusted = tempfile::tempdir().unwrap();
    let sid = crate::execution_context::execution_context_probe()
        .principal_sid
        .unwrap();
    super::super::native::protect_fixture_directory(trusted.path(), &sid, "").unwrap();
    let isolation = ProtectedDirectory::open(trusted.path(), &sid).unwrap();
    let program = PathBuf::from("C:/Program Files/Git/mingw64/bin/git.exe");
    let run = |args: &[&str]| {
        let out = std::process::Command::new(&program)
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Fixture"]);
    run(&["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(repo.path().join(".gitattributes"), b"* text=auto eol=lf\n").unwrap();
    std::fs::write(repo.path().join("old.txt"), b"before\n").unwrap();
    std::fs::write(repo.path().join("deleted.txt"), b"delete\n").unwrap();
    std::fs::create_dir(repo.path().join(".RaymanCodingSkill")).unwrap();
    std::fs::write(
        repo.path().join(".RaymanCodingSkill/workspace_skill.yaml"),
        b"original state\n",
    )
    .unwrap();
    std::fs::create_dir(repo.path().join(".agent-checkpoints")).unwrap();
    std::fs::write(
        repo.path().join(".agent-checkpoints/tracked-state"),
        b"checkpoint state\n",
    )
    .unwrap();
    run(&["add", "."]);
    run(&["-c", "commit.gpgsign=false", "commit", "-m", "fixture"]);
    std::fs::create_dir(repo.path().join(".githooks")).unwrap();
    std::fs::write(
            repo.path().join(".githooks/pre-commit"),
            b"#!/bin/sh\nset -e\ngit diff --cached --name-only | grep -Fx added.txt >/dev/null\ngit diff --cached --name-only | grep -Fx deleted.txt >/dev/null\nif git diff --cached --name-only | grep -Fx old.txt >/dev/null; then exit 9; fi\n",
        )
        .unwrap();
    run(&["add", ".githooks/pre-commit"]);
    run(&["-c", "commit.gpgsign=false", "commit", "-m", "tracked hook"]);
    run(&["config", "core.hooksPath", ".githooks"]);
    std::fs::write(repo.path().join("old.txt"), b"after\r\n").unwrap();
    std::fs::write(repo.path().join("added.txt"), b"added\r\n").unwrap();
    std::fs::remove_file(repo.path().join("deleted.txt")).unwrap();
    std::fs::write(
        repo.path().join(".RaymanCodingSkill/workspace_skill.yaml"),
        b"preserved dirty state\n",
    )
    .unwrap();
    std::fs::write(
        repo.path().join(".agent-checkpoints/untracked-lock"),
        b"preserved lock\n",
    )
    .unwrap();
    let gitdir = repo.path().join(".git");
    let old_index = std::fs::read(gitdir.join("index")).unwrap();
    let mut r = super::super::tests::registration();
    r.root_identity = super::super::native::SourceDirectory::open(repo.path())
        .unwrap()
        .identity()
        .into();
    r.git_common_identity = super::super::native::SourceDirectory::open(&gitdir)
        .unwrap()
        .identity()
        .into();
    let binding = GitBinding {
        content_policy: GitContentPolicy::capture(&program, repo.path()).unwrap(),
        hook_policy: GitHookPolicy::capture(&program, repo.path()).unwrap(),
        executable: program.clone(),
        executable_sha256: crate::hash::sha256_file(&program).unwrap(),
        git_directory: gitdir.clone(),
        common_directory: gitdir.clone(),
        branch_ref: "refs/heads/main".into(),
        config_sha256: crate::hash::sha256_file(&gitdir.join("config")).unwrap(),
    };
    let reader = GitInspector::open(repo.path(), &binding, &r, &isolation).unwrap();
    let snapshot = reader.capture(&r).unwrap();
    assert_eq!(
        snapshot
            .changes
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        vec!["added.txt", "deleted.txt", "old.txt"]
    );
    assert_eq!(
        snapshot.changes[0].after.as_ref().unwrap().raw_sha256,
        crate::hash::sha256_bytes(b"added\r\n")
    );
    assert_eq!(
        std::fs::read(repo.path().join("added.txt")).unwrap(),
        b"added\r\n"
    );
    assert_eq!(std::fs::read(gitdir.join("index")).unwrap(), old_index);
    let mut q = super::super::tests::request(&r);
    q.source_sha256 = crate::source_fingerprint(repo.path()).unwrap();
    q.operation = Operation::Commit {
        expected_head: snapshot.head.clone(),
        expected_index_sha256: snapshot.index_sha256.clone(),
        message: "exact candidate".into(),
        changes: snapshot
            .changes
            .into_iter()
            .filter(|c| c.path != "old.txt")
            .collect(),
        hook_receipt: None,
    };
    let receipt = reader
        .run_hook_preflight(&q, &r, binding.hook_policy.as_ref().unwrap())
        .unwrap();
    let Operation::Commit { hook_receipt, .. } = &mut q.operation else {
        unreachable!()
    };
    *hook_receipt = Some(receipt);
    let candidate = reader
        .prepare_commit(&q, &r, 1000, "Fixture", "fixture@example.invalid")
        .unwrap();
    assert_eq!(candidate.paths, vec!["added.txt", "deleted.txt"]);
    let mut without_hook = q.clone();
    without_hook.request_id = "8".repeat(32);
    let Operation::Commit { hook_receipt, .. } = &mut without_hook.operation else {
        unreachable!()
    };
    *hook_receipt = None;
    assert!(
        reader
            .prepare_commit(
                &without_hook,
                &r,
                1000,
                "Fixture",
                "fixture@example.invalid"
            )
            .is_err()
    );
    assert_eq!(candidate.preserved_paths, vec!["old.txt"]);
    assert_eq!(std::fs::read(gitdir.join("index")).unwrap(), old_index);
    assert_eq!(reader.capture(&r).unwrap().head, candidate.parent);
    assert!(
        reader
            .prepare_commit(&q, &r, 1000, "Fixture", "fixture@example.invalid")
            .is_err()
    );
    std::fs::write(repo.path().join("added.txt"), b"later edit\r\n").unwrap();
    assert!(reader.publish_commit(&q, &r, 1001).is_err());
    assert_eq!(reader.capture(&r).unwrap().head, candidate.parent);
    assert!(!gitdir.join("index.lock").exists());
    std::fs::write(repo.path().join("added.txt"), b"added\r\n").unwrap();
    let fault = reader
        .publish_commit_inner(&q, &r, Some("ref_published"))
        .err()
        .unwrap();
    assert!(
        fault.to_string().contains("simulated interruption"),
        "{fault:#}"
    );
    assert!(gitdir.join("index.lock").exists());
    drop(reader);
    let reader = GitInspector::open(repo.path(), &binding, &r, &isolation).unwrap();
    let published = reader.publish_commit(&q, &r, 1002).unwrap();
    assert_eq!(
        std::fs::read(repo.path().join(".RaymanCodingSkill/workspace_skill.yaml")).unwrap(),
        b"preserved dirty state\n"
    );
    assert_eq!(
        std::fs::read(repo.path().join(".agent-checkpoints/untracked-lock")).unwrap(),
        b"preserved lock\n"
    );
    assert!(validate_relative_path(".RaymanCodingSkill/workspace_skill.yaml").is_err());
    assert!(validate_relative_path(".agent-checkpoints/tracked-state").is_err());

    assert!(!gitdir.join("index.lock").exists());
    assert_eq!(reader.capture(&r).unwrap().head, published.commit);
    assert_eq!(
        reader
            .capture(&r)
            .unwrap()
            .changes
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        vec!["old.txt"]
    );
    assert_eq!(
        reader.publish_commit(&q, &r, 1002).unwrap().commit,
        published.commit
    );
    drop(reader);
    run(&["add", "old.txt"]);
    let reader = GitInspector::open(repo.path(), &binding, &r, &isolation).unwrap();
    assert!(reader.capture(&r).is_err());
}
