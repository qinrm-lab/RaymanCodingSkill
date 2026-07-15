use super::*;

fn successful_receipt(
    root: &Path,
    goal: &Goal,
    requirement_id: &str,
    command: &str,
    impacts: &[ImpactEvidence],
    non_code: bool,
) -> ValidationReceipt {
    let fingerprint = workspace_fingerprint(root).unwrap();
    let parsed = parse_validation_command(command).unwrap();
    let is_cargo_test = validation_list_command(&parsed).unwrap().is_some();
    let impact_scopes = validation_scopes_for_impacts(impacts);
    ValidationReceipt {
        exit_code: 0,
        cwd: root.display().to_string(),
        workspace_fingerprint_before: fingerprint.clone(),
        workspace_fingerprint_after: fingerprint,
        stdout_sha256: "a".repeat(64),
        stderr_sha256: "b".repeat(64),
        invocation_sha256: validation_invocation_sha256_scoped(command, &impact_scopes, non_code),
        passed_tests: is_cargo_test.then_some(1),
        listed_tests: is_cargo_test.then_some(1),
        ignored_tests: is_cargo_test.then_some(0),
        list_stdout_sha256: is_cargo_test.then(|| "c".repeat(64)),
        list_stderr_sha256: is_cargo_test.then(|| "d".repeat(64)),
        contract_sha256: validation_contract_sha256(goal, requirement_id).unwrap(),
    }
}

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

fn current_validation(
    goal: &Goal,
    requirement_id: &str,
    root: &Path,
    command: &str,
    impact_paths: &[&str],
) -> ValidationEvidence {
    let impacts = impact_paths
        .iter()
        .map(|path| impact(path))
        .collect::<Vec<_>>();
    let impact_paths = impacts
        .iter()
        .map(|impact| impact.changed_path.clone())
        .collect::<Vec<_>>();
    let impact_scopes = validation_scopes_for_impacts(&impacts);
    let non_code = impacts.is_empty();
    ValidationEvidence {
        command: command.into(),
        recorded_at: now_iso(),
        impact_scopes,
        non_code,
        receipt: Some(successful_receipt(
            root,
            goal,
            requirement_id,
            command,
            &impacts,
            non_code,
        )),
        impact_paths,
    }
}

fn close_non_code_success(store: &GoalStore, root: &Path, goal: &Goal) -> Goal {
    let command = "echo validation-ok";
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "non-code validation passed".into(),
                command: command.into(),
                receipt: successful_receipt(root, goal, "req_1", command, &[], true),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
    store.close(&goal.id, "success").unwrap()
}

#[test]
fn close_success_requires_evidence_for_must_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start(
            "add parser",
            &[
                ("implement parser".into(), true),
                ("nice errors".into(), false),
            ],
        )
        .unwrap();

    // 缺 must 证据 → 拒绝 success。
    assert!(store.close(&goal.id, "success").is_err());
    // partial 允许。
    assert_eq!(
        store.close(&goal.id, "partial").unwrap().status,
        GoalStatus::Partial
    );

    // Typed evidence cannot close success without a current receipt.
    store
        .record_evidence(&goal.id, "req_1", "src/parser.rs + cargo test passed")
        .unwrap();
    assert!(store.close(&goal.id, "success").is_err());
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "cargo test passed".into(),
                command: "echo validation-ok".into(),
                receipt: successful_receipt(
                    dir.path(),
                    &goal,
                    "req_1",
                    "echo validation-ok",
                    &[],
                    true,
                ),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
    let closed = store.close(&goal.id, "success").unwrap();
    assert_eq!(closed.status, GoalStatus::Success);
}

#[test]
fn start_rejects_empty_must_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    assert!(store.start("empty", &[]).is_err());
    assert!(store.start("empty", &[(" ".into(), true)]).is_err());
}

#[test]
fn current_schema_contract_rejects_forged_or_empty_must_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("task", &[("must prove it".into(), true)])
        .unwrap();
    assert_eq!(goal.current_schema_error(), None);

    let mut zero_must = goal.clone();
    zero_must.requirements.clear();
    assert!(
        zero_must
            .current_schema_error()
            .is_some_and(|error| error.contains("must"))
    );

    let mut unknown_version = goal;
    unknown_version.schema_version = GOAL_SCHEMA_VERSION + 1;
    assert!(
        unknown_version
            .current_schema_error()
            .is_some_and(|error| error.contains("schema_version"))
    );
}

#[test]
fn validation_command_is_direct_argv_and_exactly_classified() {
    let parsed = parse_validation_command(r#"cargo test --package "rayman core""#).unwrap();
    assert_eq!(parsed.program, "cargo");
    assert_eq!(parsed.args, ["test", "--package", "rayman core"]);

    let rust_impact = impact("src/lib.rs");
    let root = tempfile::tempdir().unwrap();
    assert!(
        validate_command_for_impacts(
            root.path(),
            "cargo test --all",
            std::slice::from_ref(&rust_impact),
            false,
        )
        .is_ok()
    );
    assert!(
        validate_command_for_impacts(
            root.path(),
            "echo cargo test",
            std::slice::from_ref(&rust_impact),
            false,
        )
        .is_err()
    );
    assert!(
        validate_command_for_impacts(
            root.path(),
            "cargo test || exit 0",
            std::slice::from_ref(&rust_impact),
            false,
        )
        .is_err()
    );
    assert!(
        validate_command_for_impacts(
            root.path(),
            "cmd /C cargo test",
            std::slice::from_ref(&rust_impact),
            false,
        )
        .is_err()
    );
    assert!(
        validate_command_for_impacts(root.path(), "sh -c 'cargo test'", &[rust_impact], false)
            .is_err()
    );
}

#[test]
fn powershell_validation_allows_only_workspace_file_mode() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("scripts/check-repo.ps1");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(&script, "exit 0\n").unwrap();
    let command = format!(
        "powershell -NoProfile -File \"{}\" -Quick",
        script.display()
    );
    let parsed = parse_validation_command(&command).unwrap();
    assert!(validate_command_security(root.path(), &parsed).is_ok());
    assert!(
        validate_command_for_impacts(root.path(), &command, &[impact("src/lib.rs")], false).is_ok()
    );

    assert!(parse_validation_command("powershell -NoProfile -Command 'cargo test'").is_err());
    assert!(parse_validation_command("pwsh -NoProfile -EncodedCommand ZQB4AGkAdAA=").is_err());
    assert!(parse_validation_command("scripts/check-repo.ps1").is_err());

    let outside = tempfile::tempdir().unwrap();
    let outside_script = outside.path().join("check-repo.ps1");
    fs::write(&outside_script, "exit 0\n").unwrap();
    let outside_command = format!(
        "powershell -NoProfile -File \"{}\"",
        outside_script.display()
    );
    let outside_parsed = parse_validation_command(&outside_command).unwrap();
    assert!(validate_command_security(root.path(), &outside_parsed).is_err());
}

#[test]
fn cargo_test_receipt_requires_actual_passed_tests() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        "cargo test --no-run",
        "cargo test -- --list",
        "cargo test --help",
        "cargo nextest run --version",
    ] {
        let parsed = parse_validation_command(command).unwrap();
        assert!(
            validate_command_security(root.path(), &parsed).is_err(),
            "{command} must not be receipt eligible"
        );
    }

    let parsed = parse_validation_command("cargo test nonexistent_filter").unwrap();
    assert!(
        validation_execution_proof(
            &parsed,
            b"test result: ok. 0 passed; 0 failed; 1 filtered out\n",
            b"",
            Some(1),
        )
        .is_err()
    );
    assert_eq!(
        validation_execution_proof(
            &parsed,
            b"test result: ok. 2 passed; 0 failed; 0 ignored\n",
            b"",
            Some(2),
        )
        .unwrap(),
        Some(TestExecutionProof {
            listed: 2,
            passed: 2,
            ignored: 0,
        })
    );
}

#[test]
fn relevance_requires_one_current_receipt_bound_to_command_and_impact() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn answer() -> i32 { 42 }\n",
    )
    .unwrap();
    let fingerprint = workspace_fingerprint(dir.path()).unwrap();
    let mut requirement = Requirement {
        id: "req_1".into(),
        text: "validate source".into(),
        kind: RequirementKind::Must,
        status: RequirementStatus::Done,
        evidence: Some("checked".into()),
        validations: vec![
            ValidationEvidence {
                command: "cargo test".into(),
                recorded_at: now_iso(),
                impact_paths: vec!["src/lib.rs".into()],
                impact_scopes: validation_scopes_for_impacts(&[impact("src/lib.rs")]),
                non_code: false,
                receipt: None,
            },
            // A receipt for an unrelated command cannot cover this source impact.
            ValidationEvidence {
                command: "rustc --version".into(),
                recorded_at: now_iso(),
                impact_paths: vec!["src/lib.rs".into()],
                impact_scopes: validation_scopes_for_impacts(&[impact("src/lib.rs")]),
                non_code: false,
                receipt: None,
            },
        ],
        impacts: vec![impact("src/lib.rs")],
    };
    let goal = Goal {
        schema_version: GOAL_SCHEMA_VERSION,
        id: "goal_test".into(),
        title: "validate source".into(),
        status: GoalStatus::Active,
        lifecycle: GoalLifecycle::Current,
        lifecycle_reason: None,
        superseded_by: None,
        lifecycle_proof: None,
        created_at: now_iso(),
        updated_at: now_iso(),
        requirements: vec![requirement.clone()],
        loaded_from_legacy: false,
    };

    assert_eq!(
        validation_relevance_gaps(&requirement, &goal, dir.path(), &fingerprint).len(),
        1,
        "typed cargo evidence and an unrelated receipt must not be combinable"
    );

    requirement.validations.push(current_validation(
        &goal,
        "req_1",
        dir.path(),
        "cargo test --all",
        &["src/lib.rs"],
    ));
    assert!(validation_relevance_gaps(&requirement, &goal, dir.path(), &fingerprint).is_empty());

    requirement.validations.last_mut().unwrap().command = "echo cargo test".into();
    let validated = requirement.validations.last().unwrap();
    assert!(
        !validation_has_current_receipt(validated, &goal, &requirement, dir.path(), &fingerprint),
        "editing the command must invalidate its invocation digest"
    );
}

#[test]
fn lifecycle_transitions_preserve_history_and_block_mutation_until_current() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let old = store.start("old", &[("old req".into(), true)]).unwrap();
    let replacement = store
        .start("replacement", &[("new req".into(), true)])
        .unwrap();
    let replacement = close_non_code_success(&store, dir.path(), &replacement);
    let old_path = dir.path().join(GOALS_DIR).join(format!("{}.json", old.id));

    assert!(
        store
            .archive(&old.id, "hide active blocker", false)
            .is_err()
    );
    store
        .record_evidence(&old.id, "req_1", "historical evidence")
        .unwrap();
    store
        .record_validation_receipt(
            &old.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "historical receipt".into(),
                command: "echo validation-ok".into(),
                receipt: successful_receipt(
                    dir.path(),
                    &old,
                    "req_1",
                    "echo validation-ok",
                    &[],
                    true,
                ),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
    store.close(&old.id, "success").unwrap();
    let archived = store.archive(&old.id, "historical task", false).unwrap();
    assert_eq!(archived.lifecycle, GoalLifecycle::Archived);
    assert!(old_path.is_file());
    assert!(
        store
            .record_evidence(&old.id, "req_1", "late edit")
            .is_err()
    );

    let current = store.mark_current(&old.id).unwrap();
    assert_eq!(current.lifecycle, GoalLifecycle::Current);
    assert!(current.lifecycle_reason.is_none());

    let superseded = store.supersede(&old.id, &replacement.id).unwrap();
    assert_eq!(superseded.lifecycle, GoalLifecycle::Superseded);
    assert_eq!(
        superseded.superseded_by.as_deref(),
        Some(replacement.id.as_str())
    );
    assert!(old_path.is_file());
}

#[test]
fn non_success_supersession_requires_all_must_text_in_the_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let old = store
        .start("old", &[("preserve security invariant".into(), true)])
        .unwrap();
    let unrelated = store
        .start("unrelated", &[("different work".into(), true)])
        .unwrap();
    let unrelated = close_non_code_success(&store, dir.path(), &unrelated);
    assert!(store.supersede(&old.id, &unrelated.id).is_err());
    assert_eq!(
        store.get(&old.id).unwrap().unwrap().lifecycle,
        GoalLifecycle::Current
    );

    let replacement = store
        .start(
            "replacement",
            &[(" preserve   security invariant ".into(), true)],
        )
        .unwrap();
    let replacement = close_non_code_success(&store, dir.path(), &replacement);
    let superseded = store.supersede(&old.id, &replacement.id).unwrap();
    let fingerprint = workspace_fingerprint(dir.path()).unwrap();
    assert_eq!(superseded.lifecycle, GoalLifecycle::Superseded);
    assert_eq!(
        supersession_error(
            &superseded,
            &[store.get(&replacement.id).unwrap().unwrap()],
            dir.path(),
            &fingerprint,
        ),
        None
    );
}

#[test]
fn historical_lifecycle_requires_a_bound_proof_and_explicit_old_schema_migration() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());

    let verified = store
        .start("verified", &[("ship verified".into(), true)])
        .unwrap();
    store
        .record_validation_receipt(
            &verified.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "current receipt".into(),
                command: "echo validation-ok".into(),
                receipt: successful_receipt(
                    dir.path(),
                    &verified,
                    "req_1",
                    "echo validation-ok",
                    &[],
                    true,
                ),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
    store.close(&verified.id, "success").unwrap();
    let archived = store
        .archive(&verified.id, "completed delivery", false)
        .unwrap();
    assert!(archived.lifecycle_proof.is_some());
    assert_eq!(archived.lifecycle_proof_error(dir.path()), None);

    fs::write(dir.path().join("later.txt"), "future change").unwrap();
    assert_eq!(
        archived.lifecycle_proof_error(dir.path()),
        None,
        "future workspace changes must not stale historical proof"
    );
    let mut tampered = archived.clone();
    tampered.title.push_str(" forged");
    assert!(tampered.lifecycle_proof_error(dir.path()).is_some());

    let weak = store.start("weak", &[("typed only".into(), true)]).unwrap();
    store
        .record_evidence_with_context(
            &weak.id,
            "req_1",
            "old typed evidence",
            vec!["cargo check".into()],
            Vec::new(),
        )
        .unwrap();
    assert!(store.close(&weak.id, "success").is_err());
    let weak_path = dir.path().join(GOALS_DIR).join(format!("{}.json", weak.id));
    let mut old_success = GoalStore::load_goal_file(&weak_path).unwrap().unwrap();
    old_success.status = GoalStatus::Success;
    old_success.created_at = "2026-07-11T00:00:00Z".into();
    write_json(&weak_path, &old_success).unwrap();
    assert!(store.archive(&weak.id, "implicit bypass", false).is_err());
    let migrated = store
        .archive(&weak.id, "explicit pre-receipt migration", true)
        .unwrap();
    assert_eq!(
        migrated
            .lifecycle_proof
            .as_ref()
            .and_then(|proof| proof.migration.as_deref()),
        Some(PRE_RECEIPT_MIGRATION)
    );
    assert_eq!(migrated.lifecycle_proof_error(dir.path()), None);

    let mut handwritten = old_success;
    handwritten.lifecycle = GoalLifecycle::Archived;
    handwritten.lifecycle_reason = Some("handwritten".into());
    handwritten.lifecycle_proof = None;
    assert!(handwritten.current_schema_error().is_some());
}

#[test]
fn pending_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let item = store.add("finish gate", "wire up CI").unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    assert!(store.resolve(&item.id).unwrap());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn state_lock_contention_includes_windows_delete_and_share_transients() {
    assert!(is_state_lock_contention(&std::io::Error::from(
        std::io::ErrorKind::AlreadyExists
    )));
    for code in [5, 32, 33] {
        assert!(is_state_lock_contention(
            &std::io::Error::from_raw_os_error(code)
        ));
    }
    assert!(!is_state_lock_contention(&std::io::Error::from(
        std::io::ErrorKind::NotFound
    )));
}

#[test]
fn concurrent_pending_and_goal_writes_do_not_lose_records() {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Barrier};

    const WORKERS: usize = 8;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let pending_barrier = Arc::new(Barrier::new(WORKERS + 1));
    let pending_handles: Vec<_> = (0..WORKERS)
        .map(|index| {
            let root = root.clone();
            let barrier = Arc::clone(&pending_barrier);
            std::thread::spawn(move || {
                barrier.wait();
                PendingStore::new(root)
                    .add(&format!("pending {index}"), "parallel regression")
                    .unwrap();
            })
        })
        .collect();
    pending_barrier.wait();
    for handle in pending_handles {
        handle.join().unwrap();
    }
    let pending = PendingStore::new(&root).list().unwrap();
    assert_eq!(pending.len(), WORKERS);
    assert_eq!(
        pending
            .iter()
            .map(|item| &item.id)
            .collect::<BTreeSet<_>>()
            .len(),
        WORKERS
    );

    let requirements: Vec<_> = (0..WORKERS)
        .map(|index| (format!("must {index}"), true))
        .collect();
    let goal = GoalStore::new(&root)
        .start("parallel goal", &requirements)
        .unwrap();
    let goal_barrier = Arc::new(Barrier::new(WORKERS + 1));
    let goal_handles: Vec<_> = (0..WORKERS)
        .map(|index| {
            let root = root.clone();
            let id = goal.id.clone();
            let barrier = Arc::clone(&goal_barrier);
            std::thread::spawn(move || {
                barrier.wait();
                GoalStore::new(root)
                    .record_evidence(
                        &id,
                        &format!("req_{}", index + 1),
                        &format!("parallel evidence {index}"),
                    )
                    .unwrap();
            })
        })
        .collect();
    goal_barrier.wait();
    for handle in goal_handles {
        handle.join().unwrap();
    }
    let persisted = GoalStore::new(&root).get(&goal.id).unwrap().unwrap();
    assert!(
        persisted
            .requirements
            .iter()
            .all(|requirement| requirement.status == RequirementStatus::Done)
    );
}

#[test]
fn corrupt_pending_store_errors_instead_of_wiping() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    store.add("keep me", "important").unwrap();
    let path = dir.path().join(PENDING_PATH);
    std::fs::write(&path, "{ not json").unwrap();

    // 损坏文件必须报错，且 add/resolve 不得覆盖原文件。
    assert!(store.list().is_err());
    assert!(store.add("new", "item").is_err());
    assert!(store.resolve("pending_x").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn close_rejects_unknown_status_and_traversal_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store.start("task", &[("req".into(), true)]).unwrap();

    // 未知状态（含大小写/拼写错误）不得绕过证据门禁。
    assert!(store.close(&goal.id, "done").is_err());
    assert!(store.close(&goal.id, "Success").is_err());

    // id 含路径分隔符/.. 时拒绝，防止越出 goals 目录。
    assert!(store.get("../../x").is_err());
    assert!(store.close("..\\evil", "partial").is_err());
}

#[test]
fn close_success_rejects_a_hand_tampered_goal_with_duplicate_requirement_ids() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store.start("task", &[("req".into(), true)]).unwrap();
    store
        .record_evidence(&goal.id, "req_1", "did the work")
        .unwrap();

    // Simulate a hand-edited state file: clone req_1's evidence onto a
    // second requirement sharing the same id. The naive "every must has
    // evidence" scan alone can't detect this kind of tampering; only the
    // schema re-validation catches the duplicate id.
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    let mut tampered = GoalStore::load_goal_file(&path).unwrap().unwrap();
    let cloned = tampered.requirements[0].clone();
    tampered.requirements.push(cloned);
    write_json(&path, &tampered).unwrap();

    assert!(store.close(&goal.id, "success").is_err());
}

#[cfg(unix)]
#[test]
fn list_with_issues_rejects_a_linked_goal_file() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let goals = workspace.path().join(GOALS_DIR);
    fs::create_dir_all(&goals).unwrap();
    let external_goal = outside.path().join("external.json");
    fs::write(&external_goal, r#"{"id":"external","title":"outside"}"#).unwrap();
    symlink(&external_goal, goals.join("external.json")).unwrap();

    let (goals, issues) = GoalStore::new(workspace.path()).list_with_issues().unwrap();
    assert!(goals.is_empty());
    assert_eq!(issues.len(), 1);
    assert!(issues[0].error.contains("链接/reparse"));
}
