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

/// 直接写入"仅人工声明、无 receipt"的需求状态。
///
/// 生产侧的 `goal evidence` 旁路已删除（文档明言它不能支撑任何门禁），但门禁
/// 仍必须正确处理这种历史形态，所以测试改为直接构造它。
fn set_legacy_evidence(root: &Path, id: &str, req_id: &str, evidence: &str) -> Goal {
    set_legacy_evidence_with_commands(root, id, req_id, evidence, Vec::new())
}

fn set_legacy_evidence_with_commands(
    root: &Path,
    id: &str,
    req_id: &str,
    evidence: &str,
    validation_commands: Vec<String>,
) -> Goal {
    let path = root.join(GOALS_DIR).join(format!("{id}.json"));
    let _lock = acquire_state_lock(&path).unwrap();
    let mut goal = GoalStore::load_goal_file(&path).unwrap().unwrap();
    let now = now_iso();
    let req = goal
        .requirements
        .iter_mut()
        .find(|req| req.id == req_id)
        .expect("requirement exists");
    req.evidence = Some(evidence.into());
    req.status = RequirementStatus::Done;
    for command in validation_commands {
        req.validations.push(ValidationEvidence {
            command,
            recorded_at: now.clone(),
            impact_paths: Vec::new(),
            impact_scopes: Vec::new(),
            non_code: true,
            receipt: None,
        });
    }
    goal.updated_at = now;
    write_json(&path, &goal).unwrap();
    goal
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
    set_legacy_evidence(
        dir.path(),
        &goal.id,
        "req_1",
        "src/parser.rs + cargo test passed",
    );
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
        baseline: None,
        plan_receipts: Vec::new(),
        review_receipts: Vec::new(),
        authority_receipts: Vec::new(),
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
    set_legacy_evidence(dir.path(), &old.id, "req_1", "historical evidence");
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
    // 归档后不接受任何进一步的写入。
    assert!(
        store
            .record_validation_receipt(
                &old.id,
                "req_1",
                ValidationReceiptSubmission {
                    evidence: "late edit".into(),
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
fn legacy_v2_lifecycle_hash_projection_remains_byte_compatible() {
    let files = BTreeMap::from([("src/lib.rs".into(), "a".repeat(64))]);
    let goal = Goal {
        schema_version: GOAL_SCHEMA_VERSION,
        id: "goal_legacy_v2_snapshot".into(),
        title: "legacy v2 snapshot".into(),
        status: GoalStatus::Success,
        lifecycle: GoalLifecycle::Archived,
        lifecycle_reason: Some("completed".into()),
        superseded_by: None,
        lifecycle_proof: None,
        created_at: "2026-07-17T00:00:00Z".into(),
        updated_at: "2026-07-17T01:00:00Z".into(),
        baseline: Some(WorkspaceBaseline {
            recorded_at: "2026-07-17T00:00:01Z".into(),
            workspace_fingerprint: "b".repeat(64),
            files,
        }),
        plan_receipts: Vec::new(),
        review_receipts: Vec::new(),
        authority_receipts: Vec::new(),
        requirements: vec![Requirement {
            id: "req_1".into(),
            text: "preserve historical proof".into(),
            kind: RequirementKind::Must,
            status: RequirementStatus::Done,
            evidence: Some("validated".into()),
            validations: Vec::new(),
            impacts: Vec::new(),
        }],
        loaded_from_legacy: false,
    };

    assert_eq!(
        legacy_lifecycle_contract_sha256(&goal),
        "a51199e8ce76a5be87cfc045412efe72428a5f12201b3413493937dcc54b20f4"
    );
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
fn supersession_accepts_a_proven_archived_success_and_rejects_forgery() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let old = store
        .start("old", &[("preserve delivered invariant".into(), true)])
        .unwrap();
    let replacement = store
        .start(
            "replacement",
            &[("preserve delivered invariant".into(), true)],
        )
        .unwrap();
    let replacement = close_non_code_success(&store, dir.path(), &replacement);
    let superseded = store.supersede(&old.id, &replacement.id).unwrap();
    let replacement = store
        .archive(&replacement.id, "delivered replacement", false)
        .unwrap();
    let fingerprint = workspace_fingerprint(dir.path()).unwrap();
    assert_eq!(
        supersession_error(
            &superseded,
            std::slice::from_ref(&replacement),
            dir.path(),
            &fingerprint,
        ),
        None
    );

    let replacement_path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", replacement.id));
    let mut forged = replacement;
    forged.title.push_str(" forged");
    write_json(&replacement_path, &forged).unwrap();
    assert!(supersession_error(&superseded, &[forged], dir.path(), &fingerprint,).is_some());
}

/// `--migrate-unreceipted` 的文档承诺是"只适用于从来没有 receipt 的 pre-rollout
/// 记录"。缺了这条判定时，一个**有** receipt 但复核失败的目标也能被它洗成合法
/// 归档证明——即把"证明失效"降级成"从来没有证明"，而后者被无条件接受。
#[test]
fn pre_receipt_migration_refuses_a_record_that_actually_has_receipts() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a0").unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("has-receipt", &[("ship".into(), true)])
        .unwrap();
    let command = "echo validation-ok";
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "validated".into(),
                command: command.into(),
                receipt: successful_receipt(dir.path(), &goal, "req_1", command, &[], true),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
    store.close(&goal.id, "success").unwrap();

    // 回填到 rollout 之前，使 created_at 这一半资格成立。
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    let mut backdated = GoalStore::load_goal_file(&path).unwrap().unwrap();
    backdated.created_at = "2026-07-10T00:00:00Z".into();
    write_json(&path, &backdated).unwrap();

    let reloaded = GoalStore::load_goal_file(&path).unwrap().unwrap();
    assert!(
        !pre_receipt_migration_eligible(&reloaded),
        "带 receipt 的记录不能走 pre-receipt hatch"
    );

    // 对照：把 receipt 去掉后（真正的 pre-receipt 历史形态）资格才成立。
    let mut unreceipted = reloaded;
    for requirement in &mut unreceipted.requirements {
        for validation in &mut requirement.validations {
            validation.receipt = None;
        }
    }
    assert!(pre_receipt_migration_eligible(&unreceipted));
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
    set_legacy_evidence_with_commands(
        dir.path(),
        &weak.id,
        "req_1",
        "old typed evidence",
        vec!["cargo check".into()],
    );
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
    assert!(
        store
            .archive_with_receipt_policy(
                &migrated.id,
                "attempt repeated migration",
                false,
                Some(RECEIPT_POLICY_V1),
            )
            .is_err()
    );

    let mut handwritten = old_success;
    handwritten.lifecycle = GoalLifecycle::Archived;
    handwritten.lifecycle_reason = Some("handwritten".into());
    handwritten.lifecycle_proof = None;
    assert!(handwritten.current_schema_error().is_some());
}

#[test]
fn invalid_legacy_receipt_quarantine_preserves_history_without_minting_proof() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let started = store
        .start(
            "misclassified archive",
            &[("preserve history".into(), true)],
        )
        .unwrap();
    let path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", started.id));
    let mut historical = GoalStore::load_goal_file(&path).unwrap().unwrap();
    historical.created_at = "2026-07-11T00:00:00Z".into();
    write_json(&path, &historical).unwrap();

    let historical = GoalStore::load_goal_file(&path).unwrap().unwrap();
    store
        .record_validation_receipt(
            &historical.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "legacy receipt exists".into(),
                command: "echo validation-ok".into(),
                receipt: successful_receipt(
                    dir.path(),
                    &historical,
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
    store.close(&historical.id, "success").unwrap();
    let archived = store
        .archive(&historical.id, "original archive", false)
        .unwrap();

    // Reproduce the historical bookkeeping defect: a record containing real
    // receipts was labelled as the no-receipt migration.
    let mut misclassified = archived;
    {
        let proof = misclassified.lifecycle_proof.as_mut().unwrap();
        proof.migration = Some(PRE_RECEIPT_MIGRATION.into());
        proof.receipt_policy = None;
    }
    let legacy_contract = legacy_lifecycle_contract_sha256(&misclassified);
    misclassified
        .lifecycle_proof
        .as_mut()
        .unwrap()
        .contract_sha256 = legacy_contract;
    write_json(&path, &misclassified).unwrap();
    assert!(
        misclassified
            .lifecycle_proof_error(dir.path())
            .is_some_and(|error| error.contains("无效的历史迁移"))
    );

    let quarantined = store
        .quarantine_invalid_history(&historical.id, "retain as untrusted history")
        .unwrap();
    let proof = quarantined.lifecycle_proof.as_ref().unwrap();
    assert_eq!(
        proof.receipt_policy.as_deref(),
        Some(RECEIPT_POLICY_QUARANTINED)
    );
    assert_eq!(
        proof.migration.as_deref(),
        Some(QUARANTINED_HISTORY_MIGRATION)
    );
    assert_eq!(quarantined.lifecycle_proof_error(dir.path()), None);

    let mut superseded = store
        .start(
            "must not hide work",
            &[("finish current work".into(), true)],
        )
        .unwrap();
    superseded.lifecycle = GoalLifecycle::Superseded;
    superseded.superseded_by = Some(quarantined.id.clone());
    let current_fingerprint = workspace_fingerprint(dir.path()).unwrap();
    assert!(
        supersession_error(
            &superseded,
            std::slice::from_ref(&quarantined),
            dir.path(),
            &current_fingerprint,
        )
        .is_some_and(|error| error.contains("untrusted legacy quarantine"))
    );
    assert!(
        store
            .quarantine_invalid_history(&superseded.id, "cannot hide current work")
            .is_err()
    );
}

#[test]
fn historical_receipt_policy_is_versioned_and_only_real_v1_receipts_can_migrate() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("tests")).unwrap();
    fs::write(
        dir.path().join("src/api.py"),
        "def value():\n    return 1\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("tests/test_api.py"),
        "def test_value():\n    assert True\n",
    )
    .unwrap();

    let mut historical = store
        .start(
            "pre-policy Python delivery",
            &[("ship Python fix".into(), true)],
        )
        .unwrap();
    historical.created_at = "2026-07-17T00:00:00Z".into();
    historical.status = GoalStatus::Success;
    historical.requirements[0].status = RequirementStatus::Done;
    historical.requirements[0].evidence = Some("pytest passed under the v1 policy".into());
    let mut python_impact = impact("src/api.py");
    python_impact.candidate_tests = vec!["tests/test_api.py".into()];
    historical.requirements[0].impacts = vec![python_impact];
    let mut validation = current_validation(
        &historical,
        "req_1",
        dir.path(),
        "python -m pytest tests/test_api.py -q",
        &["src/api.py"],
    );
    let receipt = validation.receipt.as_mut().unwrap();
    receipt.passed_tests = None;
    receipt.listed_tests = None;
    receipt.ignored_tests = None;
    receipt.list_stdout_sha256 = None;
    receipt.list_stderr_sha256 = None;
    historical.requirements[0].validations = vec![validation];
    let path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", historical.id));
    write_json(&path, &historical).unwrap();

    assert!(
        store
            .archive(&historical.id, "implicit current-policy archive", false)
            .is_err(),
        "a current policy failure must not silently downgrade"
    );
    let migrated = store
        .archive_with_receipt_policy(
            &historical.id,
            "explicit policy migration",
            false,
            Some(RECEIPT_POLICY_V1),
        )
        .unwrap();
    let proof = migrated.lifecycle_proof.as_ref().unwrap();
    assert_eq!(proof.receipt_policy.as_deref(), Some(RECEIPT_POLICY_V1));
    assert_eq!(
        proof.migration.as_deref(),
        Some(RECEIPT_POLICY_V1_MIGRATION)
    );
    assert_eq!(migrated.lifecycle_proof_error(dir.path()), None);

    let mut downgraded = migrated.clone();
    downgraded.lifecycle_proof.as_mut().unwrap().receipt_policy = Some(RECEIPT_POLICY_V2.into());
    assert!(downgraded.lifecycle_proof_error(dir.path()).is_some());

    let mut original_v1_proof = migrated.clone();
    {
        let proof = original_v1_proof.lifecycle_proof.as_mut().unwrap();
        proof.receipt_policy = None;
        proof.migration = None;
    }
    let legacy_contract = legacy_lifecycle_contract_sha256(&original_v1_proof);
    original_v1_proof
        .lifecycle_proof
        .as_mut()
        .unwrap()
        .contract_sha256 = legacy_contract;
    assert_eq!(original_v1_proof.lifecycle_proof_error(dir.path()), None);

    let mut post_policy_unversioned = original_v1_proof.clone();
    post_policy_unversioned.created_at = "2026-07-19T00:00:00Z".into();
    let contract = validation_contract_sha256(&post_policy_unversioned, "req_1").unwrap();
    post_policy_unversioned.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap()
        .contract_sha256 = contract;
    post_policy_unversioned
        .lifecycle_proof
        .as_mut()
        .unwrap()
        .contract_sha256 = legacy_lifecycle_contract_sha256(&post_policy_unversioned);
    assert!(
        post_policy_unversioned
            .lifecycle_proof_error(dir.path())
            .is_some()
    );

    let mut missing_receipt = original_v1_proof.clone();
    missing_receipt.requirements[0].validations[0].receipt = None;
    missing_receipt
        .lifecycle_proof
        .as_mut()
        .unwrap()
        .contract_sha256 = legacy_lifecycle_contract_sha256(&missing_receipt);
    assert!(missing_receipt.lifecycle_proof_error(dir.path()).is_some());

    let mut too_new = historical;
    too_new.created_at = "2026-07-19T00:00:00Z".into();
    write_json(&path, &too_new).unwrap();
    assert!(
        store
            .archive_with_receipt_policy(
                &too_new.id,
                "attempt post-rollout downgrade",
                false,
                Some(RECEIPT_POLICY_V1),
            )
            .is_err()
    );
}

#[test]
fn plan_receipt_precedes_changes_and_high_review_is_source_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), "a0").unwrap();
    fs::write(root.join("b.txt"), "b0").unwrap();
    let store = GoalStore::new(root);
    let goal = store.start("planned", &[("ship".into(), true)]).unwrap();
    let planned = store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["b.txt".into(), "a.txt".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["a.txt".into(), "b.txt".into()],
                recommended_checks: vec!["focused".into()],
            },
        )
        .unwrap();
    assert_eq!(planned.plan_receipts[0].changed_paths, ["a.txt", "b.txt"]);

    fs::write(root.join("a.txt"), "a1").unwrap();
    fs::write(root.join("b.txt"), "b1").unwrap();
    let reviewed = store
        .record_review(
            &goal.id,
            "security-review",
            "reviewed final two-file change",
        )
        .unwrap();
    let impacts = vec![impact("a.txt"), impact("b.txt")];
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "validated planned change".into(),
                command: "git status".into(),
                receipt: successful_receipt(
                    root,
                    &reviewed,
                    "req_1",
                    "git status",
                    &impacts,
                    false,
                ),
                impacts,
                non_code: false,
            },
        )
        .unwrap();
    assert_eq!(
        store.close(&goal.id, "success").unwrap().status,
        GoalStatus::Success
    );
}

#[test]
fn plan_cannot_be_backfilled_and_unplanned_delta_blocks_validation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), "a0").unwrap();
    fs::write(root.join("b.txt"), "b0").unwrap();
    fs::write(root.join("c.txt"), "c0").unwrap();
    let store = GoalStore::new(root);

    let late = store.start("late", &[("ship".into(), true)]).unwrap();
    fs::write(root.join("a.txt"), "a1").unwrap();
    assert!(
        store
            .record_plan(
                &late.id,
                PlanReceiptSubmission {
                    changed_paths: vec!["a.txt".into()],
                    review_priority: "normal".into(),
                    impacted_paths: vec!["a.txt".into()],
                    recommended_checks: Vec::new(),
                },
            )
            .is_err()
    );

    fs::write(root.join("a.txt"), "a0").unwrap();
    let goal = store.start("scope", &[("ship".into(), true)]).unwrap();
    store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into(), "b.txt".into()],
                review_priority: "broad".into(),
                impacted_paths: vec!["a.txt".into(), "b.txt".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    fs::write(root.join("a.txt"), "a2").unwrap();
    fs::write(root.join("c.txt"), "c2").unwrap();
    let current = store.get(&goal.id).unwrap().unwrap();
    let impacts = vec![impact("a.txt"), impact("c.txt")];
    assert!(
        store
            .record_validation_receipt(
                &goal.id,
                "req_1",
                ValidationReceiptSubmission {
                    evidence: "attempted broad validation".into(),
                    command: "git status".into(),
                    receipt: successful_receipt(
                        root,
                        &current,
                        "req_1",
                        "git status",
                        &impacts,
                        false,
                    ),
                    impacts,
                    non_code: false,
                },
            )
            .is_err()
    );
}

#[test]
fn high_priority_review_becomes_stale_after_source_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), "a0").unwrap();
    fs::write(root.join("b.txt"), "b0").unwrap();
    let store = GoalStore::new(root);
    let goal = store.start("review", &[("ship".into(), true)]).unwrap();
    store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into(), "b.txt".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["a.txt".into(), "b.txt".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    fs::write(root.join("a.txt"), "a1").unwrap();
    fs::write(root.join("b.txt"), "b1").unwrap();
    store
        .record_review(&goal.id, "reviewer", "reviewed snapshot")
        .unwrap();
    fs::write(root.join("a.txt"), "a2").unwrap();

    let current = store.get(&goal.id).unwrap().unwrap();
    let impacts = vec![impact("a.txt"), impact("b.txt")];
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "validated after late edit".into(),
                command: "git status".into(),
                receipt: successful_receipt(root, &current, "req_1", "git status", &impacts, false),
                impacts,
                non_code: false,
            },
        )
        .unwrap();
    assert!(store.close(&goal.id, "success").is_err());
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
fn structured_frontier_never_asks_while_agent_work_remains() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let goal = goals
        .start("owner task", &[("finish".into(), true)])
        .unwrap();
    let pending = PendingStore::new(dir.path());
    let agent = pending.add("keep working", "local repair remains").unwrap();

    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::Continue);
    assert!(!frontier.ask_user_allowed);
    assert_eq!(frontier.execution, FrontierExecution::ContinueForeground);
    assert_eq!(frontier.consultation, FrontierConsultation::None);
    assert!(goals.close(&goal.id, "blocked").is_err());
    pending.resolve(&agent.id).unwrap();

    assert!(
        pending
            .add_structured(PendingSubmission {
                title: "need decision".into(),
                detail: "two incompatible product choices".into(),
                goal_id: Some(goal.id.clone()),
                owner: PendingOwner::Human,
                kind: PendingKind::HumanInput,
                attempts: Vec::new(),
                evidence_paths: Vec::new(),
                minimum_input: None,
                recommended_action: None,
                alternatives: Vec::new(),
                risk: None,
                resume_command: None,
                auto_resume_condition: None,
                consultation_timing: ConsultationTiming::Deferred,
                background_mechanism: None,
                background_authority_evidence: None,
                background_isolation_evidence: None,
            })
            .is_err(),
        "a human boundary without a solution package must fail closed"
    );
    pending
        .add_structured(PendingSubmission {
            title: "need decision".into(),
            detail: "two incompatible product choices".into(),
            goal_id: Some(goal.id.clone()),
            owner: PendingOwner::Human,
            kind: PendingKind::HumanInput,
            attempts: vec!["tested both local variants".into()],
            evidence_paths: vec!["reports/options.md".into()],
            minimum_input: Some("choose A or B".into()),
            recommended_action: Some("choose A".into()),
            alternatives: vec!["choose B".into()],
            risk: Some("A favors safety; B favors speed".into()),
            resume_command: Some("rayman prepare --goal owner".into()),
            auto_resume_condition: Some("resume when the choice is recorded".into()),
            consultation_timing: ConsultationTiming::Deferred,
            background_mechanism: None,
            background_authority_evidence: None,
            background_isolation_evidence: None,
        })
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::AskUser);
    assert!(frontier.ask_user_allowed);
    assert_eq!(frontier.execution, FrontierExecution::PausedForUser);
    assert_eq!(frontier.consultation, FrontierConsultation::Presented);
    assert_eq!(
        goals.close(&goal.id, "blocked").unwrap().status,
        GoalStatus::Blocked
    );
}

#[test]
fn structured_frontier_keeps_presented_questions_out_of_foreground_progress() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let goal = goals
        .start("mixed frontier", &[("finish".into(), true)])
        .unwrap();
    let pending = PendingStore::new(dir.path());
    pending
        .add("safe repair", "independent local work")
        .unwrap();

    let human_submission =
        |timing, mechanism, authority_evidence, isolation_evidence| PendingSubmission {
            title: "need owner decision".into(),
            detail: "two incompatible product requirements".into(),
            goal_id: Some(goal.id.clone()),
            owner: PendingOwner::Human,
            kind: PendingKind::HumanInput,
            attempts: vec!["tested both variants".into()],
            evidence_paths: vec!["reports/options.md".into()],
            minimum_input: Some("choose A or B".into()),
            recommended_action: Some("choose A".into()),
            alternatives: vec!["choose B".into()],
            risk: Some("A is safer; B is faster".into()),
            resume_command: Some("rayman prepare --goal mixed".into()),
            auto_resume_condition: Some("choice recorded".into()),
            consultation_timing: timing,
            background_mechanism: mechanism,
            background_authority_evidence: authority_evidence,
            background_isolation_evidence: isolation_evidence,
        };

    let deferred = pending
        .add_structured(human_submission(
            ConsultationTiming::Deferred,
            None,
            None,
            None,
        ))
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::Continue);
    assert!(!frontier.ask_user_allowed);
    assert_eq!(frontier.execution, FrontierExecution::ContinueForeground);
    assert_eq!(frontier.consultation, FrontierConsultation::Deferred);
    assert!(!frontier.background_execution_allowed);

    assert!(
        pending
            .add_structured(human_submission(
                ConsultationTiming::Immediate,
                Some("worktree task".into()),
                Some("user instruction codex://threads/test".into()),
                None,
            ))
            .is_err(),
        "partial background proof must fail closed"
    );

    let immediate = pending
        .add_structured(human_submission(
            ConsultationTiming::Immediate,
            None,
            None,
            None,
        ))
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.execution, FrontierExecution::PausedForUser);
    assert_eq!(frontier.consultation, FrontierConsultation::Presented);
    assert!(frontier.ask_user_allowed);
    pending.resolve(&immediate.id).unwrap();

    pending
        .add_structured(human_submission(
            ConsultationTiming::Immediate,
            Some("isolated worktree task task_123".into()),
            Some("user instruction codex://threads/test".into()),
            Some("isolated worktree task task_123".into()),
        ))
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.execution, FrontierExecution::ContinueBackground);
    assert_eq!(frontier.consultation, FrontierConsultation::Presented);
    assert!(frontier.background_execution_allowed);
    assert!(pending.resolve(&deferred.id).unwrap());
}

#[test]
fn plan_extension_is_monotonic_and_rejects_post_hoc_paths() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        fs::write(dir.path().join(name), "baseline").unwrap();
    }
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("expand safely", &[("done".into(), true)])
        .unwrap();
    store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["a.txt".into()],
                recommended_checks: vec!["check-a".into()],
            },
        )
        .unwrap();
    fs::write(dir.path().join("a.txt"), "changed as planned").unwrap();
    let extended = store
        .extend_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["b.txt".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["b.txt".into()],
                recommended_checks: vec!["check-b".into()],
            },
        )
        .unwrap();
    let receipt = &extended.plan_receipts[0];
    assert_eq!(receipt.effective_changed_paths(), ["a.txt", "b.txt"]);
    assert_eq!(receipt.effective_review_priority(), "high");
    assert!(plan_extensions_are_valid(receipt));

    fs::write(dir.path().join("c.txt"), "already changed").unwrap();
    assert!(
        store
            .extend_plan(
                &goal.id,
                PlanReceiptSubmission {
                    changed_paths: vec!["c.txt".into()],
                    review_priority: "normal".into(),
                    impacted_paths: vec!["c.txt".into()],
                    recommended_checks: Vec::new(),
                },
            )
            .unwrap_err()
            .to_string()
            .contains("事后补票")
    );
}

#[test]
fn stable_authority_receipt_requires_two_identical_workspace_passes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("lib.rs"), "pub fn value() -> i32 { 1 }").unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("stable finish", &[("prove".into(), true)])
        .unwrap();
    fs::write(dir.path().join("lib.rs"), "pub fn value() -> i32 { 2 }").unwrap();
    let command = "cargo test --workspace --all-targets";
    let impacts = vec![impact("lib.rs")];
    let impact_scopes = validation_scopes_for_impacts(&impacts);
    let fingerprint = workspace_fingerprint(dir.path()).unwrap();
    let contract_sha256 = validation_contract_sha256(&goal, "req_1").unwrap();
    let runs = (0..2)
        .map(|_| AuthorityRunReceipt {
            exit_code: 0,
            workspace_fingerprint_before: fingerprint.clone(),
            workspace_fingerprint_after: fingerprint.clone(),
            stdout_sha256: "a".repeat(64),
            stderr_sha256: "b".repeat(64),
        })
        .collect::<Vec<_>>();
    let authority = AuthorityReceipt {
        requirement_id: "req_1".into(),
        command: command.into(),
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint.clone(),
        repeat: 2,
        impact_scopes: impact_scopes.clone(),
        non_code: false,
        invocation_sha256: authority_invocation_sha256(command, "req_1", 2, &impact_scopes, false),
        contract_sha256,
        runs,
    };
    let completed = store
        .record_authority_validation_receipt(
            &goal.id,
            "req_1",
            AuthorityReceiptSubmission {
                validation: ValidationReceiptSubmission {
                    evidence: "stable twice".into(),
                    command: command.into(),
                    receipt: successful_receipt(
                        dir.path(),
                        &goal,
                        "req_1",
                        command,
                        &impacts,
                        false,
                    ),
                    impacts,
                    non_code: false,
                },
                authority,
            },
        )
        .unwrap();
    let completed = store.close(&completed.id, "success").unwrap();
    assert!(has_current_stable_authority_receipt(
        &completed,
        dir.path(),
        &fingerprint
    ));
}

#[test]
fn authority_classification_rejects_a_focused_command_promoted_by_flag() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("lib.rs"), "pub fn value() -> i32 { 1 }").unwrap();

    let rejected =
        validate_authority_command(dir.path(), "rustc --crate-type lib lib.rs --out-dir target")
            .unwrap_err();
    assert!(rejected.to_string().contains("authority gate"));
    assert!(validate_authority_command(dir.path(), "cargo test --workspace --all-targets").is_ok());
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
                // 走 acquire_state_lock 的读-改-写路径，与生产写入同一把锁。
                set_legacy_evidence(
                    &root,
                    &id,
                    &format!("req_{}", index + 1),
                    &format!("parallel evidence {index}"),
                );
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
fn pending_store_rejects_hand_tampered_owner_kind_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    store.add("local repair", "agent can execute it").unwrap();
    let path = dir.path().join(PENDING_PATH);
    let mut tampered: PendingList = read_json(&path).unwrap().unwrap();
    tampered.items[0].owner = PendingOwner::Human;
    // A human-owned machine_actionable item would let hand-edited state turn
    // executable agent work into a fake consultation boundary.
    write_json(&path, &tampered).unwrap();
    let original = fs::read(&path).unwrap();

    assert!(store.list().is_err());
    assert!(store.add("new", "must not overwrite").is_err());
    assert!(store.resolve("pending_x").is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn pending_store_rejects_hand_tampered_incomplete_solution_package() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let item = store
        .add_structured(PendingSubmission {
            title: "owner choice".into(),
            detail: "two incompatible requirements".into(),
            goal_id: None,
            owner: PendingOwner::Human,
            kind: PendingKind::HumanInput,
            attempts: vec!["tested both variants".into()],
            evidence_paths: vec!["reports/options.md".into()],
            minimum_input: Some("choose A or B".into()),
            recommended_action: Some("choose A".into()),
            alternatives: vec!["choose B".into()],
            risk: Some("B weakens safety".into()),
            resume_command: Some("rayman prepare --goal goal_x".into()),
            auto_resume_condition: Some("choice recorded".into()),
            consultation_timing: ConsultationTiming::Deferred,
            background_mechanism: None,
            background_authority_evidence: None,
            background_isolation_evidence: None,
        })
        .unwrap();
    let path = dir.path().join(PENDING_PATH);
    let mut tampered: PendingList = read_json(&path).unwrap().unwrap();
    tampered.items[0].recommended_action = None;
    write_json(&path, &tampered).unwrap();

    assert!(store.list().is_err());
    assert!(store.resolve(&item.id).is_err());
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
    set_legacy_evidence(dir.path(), &goal.id, "req_1", "did the work");

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

#[test]
fn pytest_receipt_requires_collect_proof_and_matches_python_impact() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("src")).unwrap();
    fs::create_dir_all(root.path().join("tests")).unwrap();
    fs::write(
        root.path().join("src/api.py"),
        "def value():\n    return 1\n",
    )
    .unwrap();
    fs::write(
        root.path().join("tests/test_api.py"),
        "from src.api import value\ndef test_value():\n    assert value() == 1\n",
    )
    .unwrap();

    let parsed = parse_validation_command("python -m pytest tests/test_api.py -q").unwrap();
    let list = validation_list_command(&parsed).unwrap().unwrap();
    assert!(
        list.args
            .iter()
            .any(|argument| argument == "--collect-only")
    );
    assert_eq!(
        listed_test_count(
            &list,
            b"tests/test_api.py::test_value\n1 test collected in 0.01s\n",
            b"",
        )
        .unwrap(),
        1
    );
    assert_eq!(
        validation_execution_proof(&parsed, b"1 passed in 0.02s\n", b"", Some(1)).unwrap(),
        Some(TestExecutionProof {
            listed: 1,
            passed: 1,
            ignored: 0,
        })
    );

    let mut python_impact = impact("src/api.py");
    python_impact.candidate_tests = vec!["tests/test_api.py".into()];
    assert!(
        validate_command_for_impacts(
            root.path(),
            "python -m pytest tests/test_api.py -q",
            std::slice::from_ref(&python_impact),
            false,
        )
        .is_ok()
    );
    assert!(
        validate_command_for_impacts(
            root.path(),
            "python -m pytest tests/test_other.py -q",
            &[python_impact],
            false,
        )
        .is_err()
    );
    let collect_only = parse_validation_command("pytest --collect-only").unwrap();
    assert!(validate_command_security(root.path(), &collect_only).is_err());
}

#[test]
fn completed_success_can_be_historicized_after_later_source_changes() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());

    let archived_source = store
        .start(
            "archive stale success",
            &[("validated before drift".into(), true)],
        )
        .unwrap();
    let archived_source = close_non_code_success(&store, dir.path(), &archived_source);
    let archived_fingerprint = archived_source.requirements[0].validations[0]
        .receipt
        .as_ref()
        .unwrap()
        .workspace_fingerprint_after
        .clone();
    fs::write(dir.path().join("later.txt"), "later source").unwrap();
    let archived = store
        .archive(
            &archived_source.id,
            "completed before later maintenance",
            false,
        )
        .unwrap();
    assert_eq!(
        archived
            .lifecycle_proof
            .as_ref()
            .unwrap()
            .workspace_fingerprint,
        archived_fingerprint
    );
    assert_eq!(archived.lifecycle_proof_error(dir.path()), None);

    let superseded_source = store
        .start(
            "supersede stale success",
            &[("validated before replacement".into(), true)],
        )
        .unwrap();
    let superseded_source = close_non_code_success(&store, dir.path(), &superseded_source);
    let superseded_fingerprint = superseded_source.requirements[0].validations[0]
        .receipt
        .as_ref()
        .unwrap()
        .workspace_fingerprint_after
        .clone();
    fs::write(dir.path().join("newer.txt"), "newer source").unwrap();
    let replacement = store
        .start(
            "current replacement",
            &[("replacement is current".into(), true)],
        )
        .unwrap();
    let replacement = close_non_code_success(&store, dir.path(), &replacement);
    let superseded = store
        .supersede(&superseded_source.id, &replacement.id)
        .unwrap();
    assert_eq!(
        superseded
            .lifecycle_proof
            .as_ref()
            .unwrap()
            .workspace_fingerprint,
        superseded_fingerprint
    );
    assert_eq!(superseded.lifecycle_proof_error(dir.path()), None);
    let current = workspace_fingerprint(dir.path()).unwrap();
    assert_eq!(
        supersession_error(&superseded, &[replacement], dir.path(), &current),
        None
    );
}
#[test]
fn baseline_less_current_goal_is_not_gate_ready_but_can_be_historicized() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = store
        .start("pre-planning", &[("ship".into(), true)])
        .unwrap();
    goal.baseline = None;
    let current = workspace_fingerprint(dir.path()).unwrap();
    let gaps = goal_planning_gaps(&goal, dir.path(), &current);
    assert!(gaps.iter().any(|gap| gap.contains("缺少开工 baseline")));

    goal.lifecycle = GoalLifecycle::Archived;
    assert!(goal_planning_gaps(&goal, dir.path(), &current).is_empty());
}

#[test]
fn baseline_less_goal_cannot_absorb_receipts_at_all() {
    // 此前整个 plan/差量门禁被包在 `if let Some(baseline)` 里且没有 else 分支，
    // 于是缺 baseline 的目标（旧版本写下的 v2 记录即为此形态）可以吸收任意
    // 未声明变更并照常写出 receipt。写入侧必须与"永不 gate-ready"一致地 fail-closed。
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a0").unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("no-baseline", &[("ship".into(), true)])
        .unwrap();

    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    let mut stripped = GoalStore::load_goal_file(&path).unwrap().unwrap();
    stripped.baseline = None;
    write_json(&path, &stripped).unwrap();

    fs::write(dir.path().join("undeclared.txt"), "sneaked in").unwrap();
    let command = "echo validation-ok";
    let error = store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "non-code validation passed".into(),
                command: command.into(),
                receipt: successful_receipt(dir.path(), &goal, "req_1", command, &[], true),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("缺少开工 baseline"), "error={error}");
}

#[test]
fn goal_plan_is_one_immutable_aggregate_receipt() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a0").unwrap();
    fs::write(dir.path().join("b.txt"), "b0").unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store.start("aggregate", &[("ship".into(), true)]).unwrap();
    let first = PlanReceiptSubmission {
        changed_paths: vec!["a.txt".into()],
        review_priority: "normal".into(),
        impacted_paths: vec!["a.txt".into()],
        recommended_checks: Vec::new(),
    };
    assert_eq!(
        store
            .record_plan(
                &goal.id,
                PlanReceiptSubmission {
                    changed_paths: first.changed_paths.clone(),
                    review_priority: first.review_priority.clone(),
                    impacted_paths: first.impacted_paths.clone(),
                    recommended_checks: first.recommended_checks.clone(),
                }
            )
            .unwrap()
            .plan_receipts
            .len(),
        1
    );
    assert_eq!(
        store
            .record_plan(&goal.id, first)
            .unwrap()
            .plan_receipts
            .len(),
        1
    );
    assert!(
        store
            .record_plan(
                &goal.id,
                PlanReceiptSubmission {
                    changed_paths: vec!["b.txt".into()],
                    review_priority: "normal".into(),
                    impacted_paths: vec!["b.txt".into()],
                    recommended_checks: Vec::new(),
                },
            )
            .is_err()
    );
}

#[test]
fn pytest_selectors_are_scoped_and_terminal_summary_is_not_double_counted() {
    let root = tempfile::tempdir().unwrap();
    for directory in ["src", "tests", "other_tests"] {
        fs::create_dir_all(root.path().join(directory)).unwrap();
    }
    fs::write(
        root.path().join("src/api.py"),
        "def value():\n    return 1\n",
    )
    .unwrap();
    fs::write(
        root.path().join("src/other.py"),
        "def other():\n    return 2\n",
    )
    .unwrap();
    fs::write(
        root.path().join("tests/test_api.py"),
        "from src.api import value\ndef test_value():\n    assert value() == 1\n",
    )
    .unwrap();
    fs::write(
        root.path().join("other_tests/test_other.py"),
        "from src.other import other\ndef test_other():\n    assert other() == 2\n",
    )
    .unwrap();

    let directory = parse_validation_command("python -m pytest tests -q").unwrap();
    assert_eq!(pytest_path_arguments(&directory), ["tests"]);
    let mut api = impact("src/api.py");
    api.candidate_tests = vec!["tests/test_api.py".into()];
    assert!(
        validate_command_for_impacts(
            root.path(),
            "python -m pytest tests -q",
            std::slice::from_ref(&api),
            false,
        )
        .is_ok()
    );

    let mut other = impact("src/other.py");
    other.candidate_tests = vec!["other_tests/test_other.py".into()];
    assert!(
        validate_command_for_impacts(root.path(), "python -m pytest tests -q", &[other], false,)
            .is_err()
    );
    assert!(
        validate_command_for_impacts(
            root.path(),
            "pytest tests/test_api.py::test_value -q",
            &[api],
            false,
        )
        .is_ok()
    );

    let report_option = parse_validation_command("pytest --junitxml reports/out.xml -q").unwrap();
    assert!(pytest_path_arguments(&report_option).is_empty());
    assert!(command_is_workspace_wide(root.path(), &report_option));

    let parallel = parse_validation_command("python -m pytest -n 4 --dist loadscope -q").unwrap();
    assert!(pytest_path_arguments(&parallel).is_empty());
    assert!(command_is_workspace_wide(root.path(), &parallel));
    let parallel_scoped =
        parse_validation_command("python -m pytest -n 4 --dist loadscope tests -q").unwrap();
    assert_eq!(pytest_path_arguments(&parallel_scoped), ["tests"]);

    let proof = validation_execution_proof(
        &parse_validation_command("pytest tests/test_api.py -q").unwrap(),
        b"debug text: 99 passed\n1 passed in 0.02s\n",
        b"",
        Some(1),
    )
    .unwrap();
    assert_eq!(
        proof,
        Some(TestExecutionProof {
            listed: 1,
            passed: 1,
            ignored: 0,
        })
    );
}

#[test]
fn pytest_collect_proof_counts_selected_tests_not_the_deselected_total() {
    // `-k` / `-m <marker>` / `--deselect` 时 pytest 报 `M/N tests collected (K deselected)`，
    // 而运行期 summary 报的是 M。取 N 会让 passed+ignored==listed 恒不成立，
    // 于是这些命令永远写不出 receipt——而 `-k` 是文档明确建模的用法。
    assert_eq!(
        pytest_collected_count("2/5 tests collected (3 deselected) in 0.01s"),
        Some(2)
    );
    assert_eq!(
        pytest_collected_count("3/9 tests collected (6 deselected) in 0.02s"),
        Some(3)
    );
    // 未取消选择时的常规形式不受影响。
    assert_eq!(
        pytest_collected_count("5 tests collected in 0.01s"),
        Some(5)
    );
    assert_eq!(pytest_collected_count("1 test collected in 0.01s"), Some(1));

    // 端到端：选中 2 个、跑过 2 个，一致性检查必须放行。
    let proof = validation_execution_proof(
        &parse_validation_command("python -m pytest -k alpha tests").unwrap(),
        b"2 passed, 3 deselected in 0.05s\n",
        b"",
        Some(2),
    )
    .unwrap();
    assert_eq!(
        proof,
        Some(TestExecutionProof {
            listed: 2,
            passed: 2,
            ignored: 0,
        })
    );
}

#[test]
fn python_arbitrary_code_hosts_are_not_accepted_as_a_pytest_proof() {
    // `python -c CODE -m pytest`：Python 吃掉 `-c CODE`，`-m pytest` 退化成惰性的
    // sys.argv 内容，pytest 从不运行。若把它当 pytest 调用，攻击者代码就同时
    // 产出 collect proof 与终局摘要，且空参数尾部让它"覆盖"全部 .py 路径。
    for forged in [
        "python -c print('3 passed in 0.02s') -m pytest",
        "python -cprint(1) -m pytest",
        "python script.py -m pytest",
        "python - -m pytest",
        "python -Ec print(1) -m pytest",
    ] {
        let command = parse_validation_command(forged).unwrap();
        assert!(
            !pytest_invocation(&command),
            "must not be classified as pytest: {forged}"
        );
    }

    // 真正的解释器选项位仍然照常识别，包括可组合的无值标志与 py 启动器版本选择符。
    for genuine in [
        "python -m pytest",
        "python -q -m pytest tests",
        "python -Es -m pytest",
        "python -W ignore -m pytest",
        "python -Wignore::DeprecationWarning -m pytest",
        "py -3.12 -m pytest",
    ] {
        let command = parse_validation_command(genuine).unwrap();
        assert!(
            pytest_invocation(&command),
            "must stay a pytest invocation: {genuine}"
        );
    }

    // 参数尾部必须从模块名之后开始，否则选择器作用域会被算错。
    let scoped = parse_validation_command("python -q -m pytest tests -q").unwrap();
    assert_eq!(pytest_path_arguments(&scoped), ["tests"]);
}

#[test]
fn current_success_can_refresh_review_after_source_drift() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a0").unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("review-refresh", &[("ship".into(), true)])
        .unwrap();
    store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["a.txt".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    fs::write(dir.path().join("a.txt"), "a1").unwrap();
    let reviewed = store
        .record_review(&goal.id, "reviewer", "reviewed first snapshot")
        .unwrap();
    let impacts = vec![impact("a.txt")];
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "validated".into(),
                command: "git status".into(),
                receipt: successful_receipt(
                    dir.path(),
                    &reviewed,
                    "req_1",
                    "git status",
                    &impacts,
                    false,
                ),
                impacts,
                non_code: false,
            },
        )
        .unwrap();
    assert_eq!(
        store.close(&goal.id, "success").unwrap().status,
        GoalStatus::Success
    );

    fs::write(dir.path().join("a.txt"), "a2").unwrap();
    let refreshed = store
        .record_review(&goal.id, "reviewer", "reviewed refreshed snapshot")
        .unwrap();
    assert_eq!(refreshed.status, GoalStatus::Success);
    assert_eq!(refreshed.review_receipts.len(), 2);
}

#[test]
fn with_locked_goal_holds_the_goal_lock_for_the_entire_operation() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("locked operation", &[("stay current".into(), true)])
        .unwrap();
    let root = dir.path().to_path_buf();
    let id = goal.id.clone();
    let (acquired_tx, acquired_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        GoalStore::new(root)
            .with_locked_goal(&id, |_| {
                acquired_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(200));
                Ok(())
            })
            .unwrap();
    });

    acquired_rx.recv().unwrap();
    let started = std::time::Instant::now();
    store
        .with_locked_goal(&goal.id, |locked| {
            assert_eq!(locked.id, goal.id);
            Ok(())
        })
        .unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_millis(100));
    worker.join().unwrap();
}
