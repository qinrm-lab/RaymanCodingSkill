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
/// `goal evidence` 仍然是在产的命令（main.rs 的 GoalAction::Evidence →
/// record_evidence_with_context），它记录的是尚未被机器验证的进展，按定义
/// 不能支撑任何门禁。这个 helper 直接构造同一种形态，好让门禁测试不依赖
/// 那条命令的具体参数形状——但**别再**据此以为生产入口不存在：它存在，
/// 而写路径必须假定这种记录随时会出现（见
/// close_success_requires_receipts_for_done_should_requirements）。
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

fn archived_direct_authority_success(store: &GoalStore, root: &Path) -> Goal {
    archived_direct_authority_success_for_command(
        store,
        root,
        "cargo test --workspace --all-targets",
    )
}

fn archived_direct_authority_success_for_command(
    store: &GoalStore,
    root: &Path,
    command: &str,
) -> Goal {
    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 1 }").unwrap();
    let goal = store
        .start("direct authority", &[("prove repository".into(), true)])
        .unwrap();
    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 2 }").unwrap();
    let impacts = vec![impact("lib.rs")];
    let impact_scopes = validation_scopes_for_impacts(&impacts);
    let fingerprint = workspace_fingerprint(root).unwrap();
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
        workspace_fingerprint: fingerprint,
        repeat: 2,
        impact_scopes: impact_scopes.clone(),
        non_code: false,
        invocation_sha256: authority_invocation_sha256(command, "req_1", 2, &impact_scopes, false),
        contract_sha256,
        runs,
    };
    store
        .record_authority_validation_receipt(
            &goal.id,
            "req_1",
            AuthorityReceiptSubmission {
                validation: ValidationReceiptSubmission {
                    evidence: "stable direct authority".into(),
                    command: command.into(),
                    receipt: successful_receipt(root, &goal, "req_1", command, &impacts, false),
                    impacts,
                    non_code: false,
                },
                authority,
            },
        )
        .unwrap();
    store.close(&goal.id, "success").unwrap();
    store
        .archive(&goal.id, "direct authority proof", false)
        .unwrap()
}

fn live_replacement_authority(
    root: &Path,
    replacement_id: &str,
    predecessor_ids: &[String],
    authority_goal_id: &str,
) -> ReplacementAuthorityReceipt {
    let command = "cargo test --workspace --all-targets";
    let fingerprint = workspace_fingerprint(root).unwrap();
    ReplacementAuthorityReceipt {
        command: command.into(),
        command_rebind: None,
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint.clone(),
        repeat: 2,
        invocation_sha256: replacement_authority_invocation_sha256(
            command,
            replacement_id,
            authority_goal_id,
            predecessor_ids,
            2,
        ),
        runs: (0..2)
            .map(|_| AuthorityRunReceipt {
                exit_code: 0,
                workspace_fingerprint_before: fingerprint.clone(),
                workspace_fingerprint_after: fingerprint.clone(),
                stdout_sha256: "a".repeat(64),
                stderr_sha256: "b".repeat(64),
            })
            .collect(),
    }
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

/// 写路径（close/archive 的 success 谓词）对非 Must 需求必须执行与读门禁
/// （goal_gate_verdict）相同的 Done-无-receipt 检查；否则 `goal evidence`
/// 标 Done 的 should 让 close 以 exit 0 报出一个 `check --goal` 立刻拒绝、
/// Stop hook 拒绝收尾、且 success 终态无法降级的"成功"。
#[test]
fn close_success_requires_receipts_for_done_should_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start(
            "ship with docs",
            &[
                ("implement parser".into(), true),
                ("docs updated".into(), false),
            ],
        )
        .unwrap();
    record_non_code_must_receipt(&store, dir.path(), &goal);

    // `goal evidence`：should 标 Done，evidence 有文本，validations 为空。
    store
        .record_evidence_with_context(&goal.id, "req_2", "docs reviewed", Vec::new(), Vec::new())
        .unwrap();

    let error = store.close(&goal.id, "success").unwrap_err().to_string();
    assert!(
        error.contains("req_2") && error.contains("缺少验证 receipt"),
        "{error}"
    );

    store
        .record_validation_receipt(
            &goal.id,
            "req_2",
            ValidationReceiptSubmission {
                evidence: "docs verified".into(),
                command: "echo validation-ok".into(),
                receipt: successful_receipt(
                    dir.path(),
                    &goal,
                    "req_2",
                    "echo validation-ok",
                    &[],
                    true,
                ),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
    assert_eq!(
        store.close(&goal.id, "success").unwrap().status,
        GoalStatus::Success
    );
}

/// close --status success 必须校验 handoff 契约：此前只有读门禁
/// （goal_gate_verdict）查 handoff_contract_error，漂移/损坏契约的
/// release-handoff 目标能以 exit 0 关成 success，再被 check 永久拦下。
#[test]
fn close_success_rejects_an_invalid_handoff_contract() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start("release handoff", &[("ship".into(), true)])
        .unwrap();
    record_non_code_must_receipt(&store, dir.path(), &goal);

    let path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", goal.id));
    let mut tampered = GoalStore::load_goal_file(&path).unwrap().unwrap();
    tampered.handoff = Some(HandoffContract {
        source_goal_id: "goal_missing".into(),
        source_goal_contract_sha256: "a".repeat(64),
        source_authority_sha256: "b".repeat(64),
        git_commit: "c".repeat(40),
        workspace_identity: lifecycle::workspace_identity(dir.path()),
        workspace_fingerprint: workspace_fingerprint(dir.path()).unwrap(),
        created_at: now_iso(),
        stages: Vec::new(),
        contract_sha256: "d".repeat(64),
    });
    write_json(&path, &tampered).unwrap();

    let error = store.close(&goal.id, "success").unwrap_err().to_string();
    assert!(error.contains("handoff contract"), "{error}");
}

fn record_non_code_must_receipt(store: &GoalStore, root: &Path, goal: &Goal) {
    store
        .record_validation_receipt(
            &goal.id,
            "req_1",
            ValidationReceiptSubmission {
                evidence: "non-code validation passed".into(),
                command: "echo validation-ok".into(),
                receipt: successful_receipt(root, goal, "req_1", "echo validation-ok", &[], true),
                impacts: Vec::new(),
                non_code: true,
            },
        )
        .unwrap();
}

#[test]
fn close_success_rejects_open_lane() {
    // 回归：close 曾在 status 仍为 Active 时跑 current_schema_error，其中 lane-closed
    // 不变量以 status==Success 为前提，故永不触发——开着 lane 也能 close 成 success。
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store.start("task", &[("do it".into(), true)]).unwrap();
    record_non_code_must_receipt(&store, dir.path(), &goal);
    store
        .open_lane(&goal.id, "lane1", LaneMode::AdvisoryReadOnly, Vec::new())
        .unwrap();

    let error = store.close(&goal.id, "success").unwrap_err().to_string();
    assert!(error.contains("lane"), "unexpected error: {error}");
    assert_eq!(
        store.get(&goal.id).unwrap().unwrap().status,
        GoalStatus::Active,
        "拒绝 close 后目标必须仍为 active"
    );

    // 关闭 lane 后同一目标应能正常 close 成 success。
    store.close_lane(&goal.id, "lane1").unwrap();
    assert_eq!(
        store.close(&goal.id, "success").unwrap().status,
        GoalStatus::Success
    );
}

#[test]
fn close_success_rejects_incomplete_required_work_package() {
    // 回归：required work package 未完成不变量同样以 status==Success 为前提。
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store.start("task", &[("do it".into(), true)]).unwrap();
    record_non_code_must_receipt(&store, dir.path(), &goal);
    store
        .add_work_package(&goal.id, "wp1", "stage one", None, Vec::new(), true)
        .unwrap();

    let error = store.close(&goal.id, "success").unwrap_err().to_string();
    assert!(error.contains("work package"), "unexpected error: {error}");
    assert_eq!(
        store.get(&goal.id).unwrap().unwrap().status,
        GoalStatus::Active
    );
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
        proof_kind: None,
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
        replacement_authority: None,
        created_at: now_iso(),
        updated_at: now_iso(),
        baseline: None,
        plan_receipts: Vec::new(),
        review_receipts: Vec::new(),
        authority_receipts: Vec::new(),
        work_packages: Vec::new(),
        progress_receipts: Vec::new(),
        lanes: Vec::new(),
        handoff: None,
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
        replacement_authority: None,
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
        work_packages: Vec::new(),
        progress_receipts: Vec::new(),
        lanes: Vec::new(),
        handoff: None,
        requirements: vec![Requirement {
            id: "req_1".into(),
            text: "preserve historical proof".into(),
            kind: RequirementKind::Must,
            proof_kind: None,
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
fn invalid_success_requires_an_exact_gate_ready_replacement_before_supersession() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let old = store
        .start(
            "old invalid success",
            &[("preserve proven behavior".into(), true)],
        )
        .unwrap();
    let mut old = close_non_code_success(&store, dir.path(), &old);
    old.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap()
        .contract_sha256 = "0".repeat(64);
    let old_path = dir.path().join(GOALS_DIR).join(format!("{}.json", old.id));
    write_json(&old_path, &old).unwrap();

    let unrelated = store
        .start("unrelated replacement", &[("different work".into(), true)])
        .unwrap();
    let unrelated = close_non_code_success(&store, dir.path(), &unrelated);
    assert!(store.supersede(&old.id, &unrelated.id).is_err());
    assert_eq!(
        store.get(&old.id).unwrap().unwrap().lifecycle,
        GoalLifecycle::Current
    );

    let replacement = store
        .start(
            "exact proven replacement",
            &[(" preserve   proven behavior ".into(), true)],
        )
        .unwrap();
    let replacement = close_non_code_success(&store, dir.path(), &replacement);
    let superseded = store.supersede(&old.id, &replacement.id).unwrap();
    let current = workspace_fingerprint(dir.path()).unwrap();

    assert_eq!(superseded.lifecycle, GoalLifecycle::Superseded);
    assert_eq!(superseded.lifecycle_proof_error(dir.path()), None);
    assert_eq!(
        supersession_error(
            &superseded,
            std::slice::from_ref(&replacement),
            dir.path(),
            &current,
        ),
        None
    );

    let mut forged_archive = superseded.clone();
    forged_archive.lifecycle = GoalLifecycle::Archived;
    forged_archive.lifecycle_reason = Some("forged archive".into());
    forged_archive.superseded_by = None;
    forged_archive.lifecycle_proof = Some(issue_lifecycle_proof(
        &forged_archive,
        current,
        None,
        Some(VERIFIED_REPLACEMENT_TRANSFER_POLICY.into()),
    ));
    assert!(
        forged_archive
            .lifecycle_proof_error(dir.path())
            .is_some_and(|error| error.contains("只允许"))
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

#[test]
fn lifecycle_only_replacement_transfers_exact_musts_from_direct_archived_authority() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let authority = archived_direct_authority_success(&store, dir.path());
    let first = store
        .start("first unfinished", &[("preserve alpha".into(), true)])
        .unwrap();
    let second = store
        .start("second unfinished", &[("preserve beta".into(), true)])
        .unwrap();
    let replacement = store
        .start(
            "exact replacement",
            &[
                ("preserve alpha".into(), true),
                (" preserve   beta ".into(), true),
            ],
        )
        .unwrap();

    let authorized = store
        .authorize_replacement(
            &replacement.id,
            &[first.id.clone(), second.id.clone()],
            &authority.id,
            live_replacement_authority(
                dir.path(),
                &replacement.id,
                &[first.id.clone(), second.id.clone()],
                &authority.id,
            ),
        )
        .unwrap();
    let fingerprint = workspace_fingerprint(dir.path()).unwrap();
    assert_eq!(authorized.status, GoalStatus::Success);
    assert!(authorized.replacement_authority.is_some());
    assert!(has_current_stable_authority_receipt(
        &authorized,
        dir.path(),
        &fingerprint
    ));
    assert!(goal_success_receipt_gaps(&authorized, dir.path(), &fingerprint).is_empty());

    let first = store.supersede(&first.id, &authorized.id).unwrap();
    let second = store.supersede(&second.id, &authorized.id).unwrap();
    let authorized = store.get(&authorized.id).unwrap().unwrap();
    assert_eq!(first.lifecycle, GoalLifecycle::Superseded);
    assert_eq!(second.lifecycle, GoalLifecycle::Superseded);
    assert_eq!(
        replacement_authority_error(&authorized, dir.path(), &fingerprint),
        None
    );
    let archived = store
        .archive(&authorized.id, "lifecycle transfer complete", false)
        .unwrap();
    assert_eq!(archived.lifecycle_proof_error(dir.path()), None);
}

#[test]
fn lifecycle_only_replacement_stays_standard_ready_after_superseding_predecessors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    let authority = archived_direct_authority_success(&store, root);
    let predecessor = store
        .start(
            "planned predecessor",
            &[("preserve planned delta".into(), true)],
        )
        .unwrap();
    let predecessor = store
        .record_plan(
            &predecessor.id,
            PlanReceiptSubmission {
                changed_paths: vec!["first.rs".into(), "second.rs".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["first.rs".into(), "second.rs".into()],
                recommended_checks: vec!["cargo test --workspace --all-targets".into()],
            },
        )
        .unwrap();
    let replacement = store
        .start(
            "lifecycle-only replacement",
            &[("preserve planned delta".into(), true)],
        )
        .unwrap();
    fs::write(root.join("first.rs"), "pub fn first() {}\n").unwrap();
    fs::write(root.join("second.rs"), "pub fn second() {}\n").unwrap();

    let authorized = store
        .authorize_replacement(
            &replacement.id,
            std::slice::from_ref(&predecessor.id),
            &authority.id,
            live_replacement_authority(
                root,
                &replacement.id,
                std::slice::from_ref(&predecessor.id),
                &authority.id,
            ),
        )
        .unwrap();
    let predecessor = store.supersede(&predecessor.id, &authorized.id).unwrap();
    let authorized = store.get(&authorized.id).unwrap().unwrap();
    let fingerprint = workspace_fingerprint(root).unwrap();
    let goals = store.list().unwrap();
    let verdict = goal_gate_verdict(&authorized, &goals, root, Some(&fingerprint));

    assert!(
        verdict.blockers.is_empty(),
        "valid lifecycle-only replacement must bypass ordinary planning gaps: {:?}",
        verdict.blockers
    );

    fs::write(root.join("first.rs"), "pub fn first() -> i32 { 1 }\n").unwrap();
    let later_fingerprint = workspace_fingerprint(root).unwrap();
    assert_ne!(later_fingerprint, fingerprint);
    let archived = store
        .archive(&authorized.id, "delivered lifecycle replacement", false)
        .unwrap();
    assert_eq!(
        archived
            .lifecycle_proof
            .as_ref()
            .map(|proof| proof.workspace_fingerprint.as_str()),
        Some(fingerprint.as_str())
    );
    assert_eq!(archived.lifecycle_proof_error(root), None);
    assert_eq!(
        supersession_error(
            &predecessor,
            std::slice::from_ref(&archived),
            root,
            &later_fingerprint,
        ),
        None
    );
}

#[test]
fn lifecycle_only_replacement_rebinds_only_a_verified_maintenance_cycle_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join(".check-repo-output")).unwrap();
    fs::write(root.join("scripts/check-repo.ps1"), "exit 0\n").unwrap();
    let archived_cycle = ".check-repo-output/archived-maintenance-review-cycle.json";
    let current_cycle = ".check-repo-output/current-maintenance-review-cycle.json";
    fs::write(root.join(archived_cycle), "{\"snapshot\":\"old\"}\n").unwrap();
    fs::write(root.join(current_cycle), "{\"snapshot\":\"current\"}\n").unwrap();
    let command = format!(
        "pwsh -NoProfile -File scripts/check-repo.ps1 -QuickParallel -MaintenanceOrchestrationCycle {archived_cycle}"
    );
    let authority = archived_direct_authority_success_for_command(&store, root, command.as_str());
    let old = store
        .start("old", &[("preserve exact contract".into(), true)])
        .unwrap();
    let replacement = store
        .start("replacement", &[("preserve exact contract".into(), true)])
        .unwrap();
    let (effective, rebind) =
        prepare_maintenance_cycle_rebind(root, &command, current_cycle).unwrap();
    assert_eq!(
        effective.args.last().map(String::as_str),
        Some(current_cycle)
    );
    assert_eq!(rebind.archived_value, archived_cycle);
    assert_eq!(
        rebind.current_sha256,
        crate::hash::sha256_file(&root.join(current_cycle)).unwrap()
    );

    let fingerprint = workspace_fingerprint(root).unwrap();
    let predecessors = vec![old.id.clone()];
    let live = ReplacementAuthorityReceipt {
        command: command.clone(),
        command_rebind: Some(rebind.clone()),
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint.clone(),
        repeat: 2,
        invocation_sha256: replacement_authority_invocation_sha256_with_rebind(
            &command,
            &replacement.id,
            &authority.id,
            &predecessors,
            2,
            Some(&rebind),
        ),
        runs: (0..2)
            .map(|_| AuthorityRunReceipt {
                exit_code: 0,
                workspace_fingerprint_before: fingerprint.clone(),
                workspace_fingerprint_after: fingerprint.clone(),
                stdout_sha256: "a".repeat(64),
                stderr_sha256: "b".repeat(64),
            })
            .collect(),
    };
    let authorized = store
        .authorize_replacement(&replacement.id, &predecessors, &authority.id, live)
        .unwrap();
    assert_eq!(authorized.status, GoalStatus::Success);
    assert_eq!(
        authorized
            .replacement_authority
            .as_ref()
            .unwrap()
            .live_authority
            .command,
        command
    );
    assert_eq!(
        replacement_authority_error(&authorized, root, &fingerprint),
        None
    );

    fs::write(root.join(current_cycle), "{\"snapshot\":\"drifted\"}\n").unwrap();
    assert!(verify_maintenance_cycle_rebind_artifact(root, &rebind).is_err());
    // 读侧复验器必须与写侧一致地把 rebind 工件哈希当 fatal：工件可位于不进
    // workspace fingerprint 的路径（gitignored），授权后被改写只有这里能翻红。
    assert!(replacement_authority_error(&authorized, root, &fingerprint).is_some());
}

#[test]
fn maintenance_cycle_rebind_rejects_substitution_traversal_and_ambiguous_flags() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".check-repo-output")).unwrap();
    let current_cycle = ".check-repo-output/current-maintenance-review-cycle.json";
    fs::write(root.join(current_cycle), "{}\n").unwrap();
    let exact = "pwsh -NoProfile -File scripts/check-repo.ps1 -MaintenanceOrchestrationCycle .check-repo-output/old-maintenance-review-cycle.json";
    assert!(prepare_maintenance_cycle_rebind(root, exact, current_cycle).is_ok());
    for invalid_command in [
        "pwsh -NoProfile -File scripts/check-repo.ps1 -OtherCycle .check-repo-output/old-maintenance-review-cycle.json",
        "pwsh -NoProfile -File scripts/check-repo.ps1 -MaintenanceOrchestrationCycle .check-repo-output/a-maintenance-review-cycle.json -MaintenanceOrchestrationCycle .check-repo-output/b-maintenance-review-cycle.json",
    ] {
        assert!(prepare_maintenance_cycle_rebind(root, invalid_command, current_cycle).is_err());
    }
    for invalid_path in [
        "../outside-maintenance-review-cycle.json",
        "./.check-repo-output/current-maintenance-review-cycle.json",
        "C:/outside-maintenance-review-cycle.json",
        ".check-repo-output\\current-maintenance-review-cycle.json",
        ".check-repo-output//current-maintenance-review-cycle.json",
        ".check-repo-output/not-a-cycle.json",
    ] {
        assert!(prepare_maintenance_cycle_rebind(root, exact, invalid_path).is_err());
    }

    let (_, mut rebind) = prepare_maintenance_cycle_rebind(root, exact, current_cycle).unwrap();
    rebind.flag = "-OtherCycle".into();
    assert!(replacement_authority_effective_command(exact, Some(&rebind)).is_err());
}

#[cfg(unix)]
#[test]
fn maintenance_cycle_rebind_rejects_symlink_components() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("current-maintenance-review-cycle.json"),
        "{}\n",
    )
    .unwrap();
    symlink(outside.path(), dir.path().join("linked")).unwrap();
    let command = "pwsh -NoProfile -File scripts/check-repo.ps1 -MaintenanceOrchestrationCycle old-maintenance-review-cycle.json";
    assert!(
        prepare_maintenance_cycle_rebind(
            dir.path(),
            command,
            "linked/current-maintenance-review-cycle.json",
        )
        .is_err()
    );
}

#[test]
fn lifecycle_only_replacement_rejects_inexact_stale_and_unlisted_transfers() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let authority = archived_direct_authority_success(&store, dir.path());
    let old = store
        .start("old", &[("preserve exact contract".into(), true)])
        .unwrap();
    let missing = store
        .start("missing", &[("different contract".into(), true)])
        .unwrap();
    assert!(
        store
            .authorize_replacement(
                &missing.id,
                std::slice::from_ref(&old.id),
                &authority.id,
                live_replacement_authority(
                    dir.path(),
                    &missing.id,
                    std::slice::from_ref(&old.id),
                    &authority.id,
                ),
            )
            .unwrap_err()
            .to_string()
            .contains("精确并集")
    );

    let replacement = store
        .start("replacement", &[("preserve exact contract".into(), true)])
        .unwrap();
    let mut substituted = live_replacement_authority(
        dir.path(),
        &replacement.id,
        std::slice::from_ref(&old.id),
        &authority.id,
    );
    substituted.command = "cargo test --all".into();
    substituted.invocation_sha256 = replacement_authority_invocation_sha256(
        &substituted.command,
        &replacement.id,
        &authority.id,
        std::slice::from_ref(&old.id),
        substituted.repeat,
    );
    assert!(
        store
            .authorize_replacement(
                &replacement.id,
                std::slice::from_ref(&old.id),
                &authority.id,
                substituted,
            )
            .unwrap_err()
            .to_string()
            .contains("同命令 direct-authority")
    );
    let mut unstable = live_replacement_authority(
        dir.path(),
        &replacement.id,
        std::slice::from_ref(&old.id),
        &authority.id,
    );
    unstable.runs[1].workspace_fingerprint_after = "c".repeat(64);
    assert!(
        store
            .authorize_replacement(
                &replacement.id,
                std::slice::from_ref(&old.id),
                &authority.id,
                unstable,
            )
            .unwrap_err()
            .to_string()
            .contains("重复稳定仓库 gate")
    );
    let mut failing = live_replacement_authority(
        dir.path(),
        &replacement.id,
        std::slice::from_ref(&old.id),
        &authority.id,
    );
    failing.runs[0].exit_code = 1;
    assert!(
        store
            .authorize_replacement(
                &replacement.id,
                std::slice::from_ref(&old.id),
                &authority.id,
                failing,
            )
            .unwrap_err()
            .to_string()
            .contains("重复稳定仓库 gate")
    );
    let authorized = store
        .authorize_replacement(
            &replacement.id,
            std::slice::from_ref(&old.id),
            &authority.id,
            live_replacement_authority(
                dir.path(),
                &replacement.id,
                std::slice::from_ref(&old.id),
                &authority.id,
            ),
        )
        .unwrap();
    let unlisted = store
        .start(
            "unlisted same text",
            &[("preserve exact contract".into(), true)],
        )
        .unwrap();
    assert!(store.supersede(&unlisted.id, &authorized.id).is_err());

    let stale_root = tempfile::tempdir().unwrap();
    let stale_store = GoalStore::new(stale_root.path());
    let stale_authority = archived_direct_authority_success(&stale_store, stale_root.path());
    let stale_old = stale_store
        .start("stale old", &[("preserve stale".into(), true)])
        .unwrap();
    let stale_old = stale_store
        .record_plan(
            &stale_old.id,
            PlanReceiptSubmission {
                changed_paths: vec!["lib.rs".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["lib.rs".into()],
                recommended_checks: vec!["cargo test --workspace --all-targets".into()],
            },
        )
        .unwrap();
    let stale_replacement = stale_store
        .start("stale replacement", &[("preserve stale".into(), true)])
        .unwrap();
    let stale_only = live_replacement_authority(
        stale_root.path(),
        &stale_replacement.id,
        std::slice::from_ref(&stale_old.id),
        &stale_authority.id,
    );
    fs::write(
        stale_root.path().join("lib.rs"),
        "pub fn value() -> i32 { 3 }",
    )
    .unwrap();
    assert!(
        stale_store
            .authorize_replacement(
                &stale_replacement.id,
                std::slice::from_ref(&stale_old.id),
                &stale_authority.id,
                stale_only,
            )
            .is_err()
    );
    let authorized = stale_store
        .authorize_replacement(
            &stale_replacement.id,
            std::slice::from_ref(&stale_old.id),
            &stale_authority.id,
            live_replacement_authority(
                stale_root.path(),
                &stale_replacement.id,
                std::slice::from_ref(&stale_old.id),
                &stale_authority.id,
            ),
        )
        .unwrap();
    assert_eq!(
        authorized
            .replacement_authority
            .as_ref()
            .unwrap()
            .source_delta_paths,
        vec!["lib.rs"]
    );
    let mut legacy_value = serde_json::to_value(&authorized).unwrap();
    legacy_value["replacement_authority"]
        .as_object_mut()
        .unwrap()
        .remove("live_authority");
    let legacy_readable: Goal = serde_json::from_value(legacy_value).unwrap();
    assert!(legacy_readable.current_schema_error().is_some());

    let mut tampered_predecessor = stale_store.get(&stale_old.id).unwrap().unwrap();
    tampered_predecessor.plan_receipts[0].review_priority = "broad".into();
    tampered_predecessor.plan_receipts[0].plan_sha256 =
        plan_receipt_sha256(&tampered_predecessor.plan_receipts[0]);
    write_json(
        &stale_root
            .path()
            .join(GOALS_DIR)
            .join(format!("{}.json", stale_old.id)),
        &tampered_predecessor,
    )
    .unwrap();
    let current_fingerprint = workspace_fingerprint(stale_root.path()).unwrap();
    assert!(
        replacement_authority_error(&authorized, stale_root.path(), &current_fingerprint)
            .unwrap()
            .contains("合约或 lifecycle 已失效")
    );

    let unscoped_root = tempfile::tempdir().unwrap();
    let unscoped_store = GoalStore::new(unscoped_root.path());
    let unscoped_authority =
        archived_direct_authority_success(&unscoped_store, unscoped_root.path());
    let unscoped_old = unscoped_store
        .start("unscoped old", &[("preserve unscoped".into(), true)])
        .unwrap();
    let unscoped_replacement = unscoped_store
        .start(
            "unscoped replacement",
            &[("preserve unscoped".into(), true)],
        )
        .unwrap();
    fs::write(
        unscoped_root.path().join("lib.rs"),
        "pub fn value() -> i32 { 4 }",
    )
    .unwrap();
    assert!(
        unscoped_store
            .authorize_replacement(
                &unscoped_replacement.id,
                std::slice::from_ref(&unscoped_old.id),
                &unscoped_authority.id,
                live_replacement_authority(
                    unscoped_root.path(),
                    &unscoped_replacement.id,
                    std::slice::from_ref(&unscoped_old.id),
                    &unscoped_authority.id,
                ),
            )
            .unwrap_err()
            .to_string()
            .contains("未被 predecessor plan 覆盖")
    );

    let indirect_root = tempfile::tempdir().unwrap();
    let indirect_store = GoalStore::new(indirect_root.path());
    let indirect_authority = indirect_store
        .start(
            "non-authority success",
            &[("not a repository gate".into(), true)],
        )
        .unwrap();
    let indirect_authority =
        close_non_code_success(&indirect_store, indirect_root.path(), &indirect_authority);
    let indirect_authority = indirect_store
        .archive(&indirect_authority.id, "no direct authority", false)
        .unwrap();
    let indirect_old = indirect_store
        .start("indirect old", &[("preserve indirect".into(), true)])
        .unwrap();
    let indirect_replacement = indirect_store
        .start(
            "indirect replacement",
            &[("preserve indirect".into(), true)],
        )
        .unwrap();
    assert!(
        indirect_store
            .authorize_replacement(
                &indirect_replacement.id,
                std::slice::from_ref(&indirect_old.id),
                &indirect_authority.id,
                live_replacement_authority(
                    indirect_root.path(),
                    &indirect_replacement.id,
                    std::slice::from_ref(&indirect_old.id),
                    &indirect_authority.id,
                ),
            )
            .unwrap_err()
            .to_string()
            .contains("direct-authority")
    );
}

#[test]
fn lifecycle_only_replacement_proof_rejects_cross_workspace_reuse() {
    let source = tempfile::tempdir().unwrap();
    let source_store = GoalStore::new(source.path());
    let authority = archived_direct_authority_success(&source_store, source.path());
    let old = source_store
        .start("old", &[("preserve identity".into(), true)])
        .unwrap();
    let replacement = source_store
        .start("replacement", &[("preserve identity".into(), true)])
        .unwrap();
    let authorized = source_store
        .authorize_replacement(
            &replacement.id,
            std::slice::from_ref(&old.id),
            &authority.id,
            live_replacement_authority(
                source.path(),
                &replacement.id,
                std::slice::from_ref(&old.id),
                &authority.id,
            ),
        )
        .unwrap();

    let target = tempfile::tempdir().unwrap();
    fs::write(
        target.path().join("lib.rs"),
        fs::read(source.path().join("lib.rs")).unwrap(),
    )
    .unwrap();
    fs::create_dir_all(target.path().join(GOALS_DIR)).unwrap();
    for id in [&authority.id, &old.id, &authorized.id] {
        fs::copy(
            source.path().join(GOALS_DIR).join(format!("{id}.json")),
            target.path().join(GOALS_DIR).join(format!("{id}.json")),
        )
        .unwrap();
    }
    let fingerprint = workspace_fingerprint(target.path()).unwrap();
    assert!(
        replacement_authority_error(&authorized, target.path(), &fingerprint)
            .unwrap()
            .contains("workspace identity")
    );
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
        Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED)
    );
    assert_eq!(
        proof.migration.as_deref(),
        Some(INTEGRITY_QUARANTINE_MIGRATION)
    );
    assert_eq!(quarantined.lifecycle_proof_error(dir.path()), None);

    // quarantine 是单向 evidence 降级：`goal current` 不得清掉隔离标记与
    // `[invalid proof: ...]` 审计原因，否则一条命令就能把不可信历史重铸为
    // 普通可信记录。
    let error = store.mark_current(&historical.id).unwrap_err().to_string();
    assert!(error.contains("不能恢复"), "{error}");
    let untouched = GoalStore::load_goal_file(&path).unwrap().unwrap();
    assert_eq!(
        untouched
            .lifecycle_proof
            .as_ref()
            .and_then(|proof| proof.receipt_policy.as_deref()),
        Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED)
    );
    assert!(
        untouched
            .lifecycle_reason
            .as_deref()
            .unwrap_or_default()
            .contains("invalid proof"),
        "隔离原因必须原样保留"
    );

    // Records written by the earlier narrow quarantine policy remain readable
    // and untrusted after the generalized policy is introduced.
    let mut legacy_quarantine = quarantined.clone();
    legacy_quarantine.lifecycle_proof = Some(issue_lifecycle_proof(
        &legacy_quarantine,
        proof.workspace_fingerprint.clone(),
        Some(QUARANTINED_HISTORY_MIGRATION.into()),
        Some(RECEIPT_POLICY_QUARANTINED.into()),
    ));
    assert_eq!(legacy_quarantine.lifecycle_proof_error(dir.path()), None);

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
        .is_some_and(|error| error.contains("untrusted history quarantine"))
    );
    assert!(
        store
            .quarantine_invalid_history(&superseded.id, "cannot hide current work")
            .is_err()
    );
}

#[test]
fn invalid_archived_success_can_be_quarantined_without_becoming_authority() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let started = store
        .start(
            "invalid archived receipt",
            &[("preserve failed historical evidence".into(), true)],
        )
        .unwrap();
    let current = close_non_code_success(&store, dir.path(), &started);
    let archived = store
        .archive(&current.id, "archive before policy drift", false)
        .unwrap();
    assert_eq!(archived.lifecycle_proof_error(dir.path()), None);

    // Reproduce a later integrity failure without altering the claimed result:
    // the validation is no longer marked non-code, yet it declares no changed
    // scope. Reissue only the historical lifecycle envelope so the failure is
    // specifically in the receipt ledger, not a stale contract hash.
    let path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", archived.id));
    let mut invalid = archived;
    invalid.requirements[0].validations[0].non_code = false;
    let old_proof = invalid.lifecycle_proof.clone().unwrap();
    invalid.lifecycle_proof = Some(issue_lifecycle_proof(
        &invalid,
        old_proof.workspace_fingerprint,
        old_proof.migration,
        old_proof.receipt_policy,
    ));
    write_json(&path, &invalid).unwrap();
    assert!(
        invalid
            .lifecycle_proof_error(dir.path())
            .is_some_and(|error| error.contains("success receipt proof 无效"))
    );
    let requirements = invalid.requirements.clone();

    let quarantined = store
        .quarantine_invalid_history(&invalid.id, "retain invalid evidence as history")
        .unwrap();
    assert_eq!(quarantined.requirements, requirements);
    assert!(
        quarantined
            .lifecycle_reason
            .as_deref()
            .is_some_and(|value| value.contains("archive before policy drift"))
    );
    let proof = quarantined.lifecycle_proof.as_ref().unwrap();
    assert_eq!(
        proof.receipt_policy.as_deref(),
        Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED)
    );
    assert_eq!(
        proof.migration.as_deref(),
        Some(INTEGRITY_QUARANTINE_MIGRATION)
    );
    assert_eq!(quarantined.lifecycle_proof_error(dir.path()), None);

    let mut predecessor = store
        .start(
            "must not trust quarantine",
            &[("finish current work".into(), true)],
        )
        .unwrap();
    predecessor.lifecycle = GoalLifecycle::Superseded;
    predecessor.superseded_by = Some(quarantined.id.clone());
    let current_fingerprint = workspace_fingerprint(dir.path()).unwrap();
    assert!(
        supersession_error(
            &predecessor,
            &[quarantined],
            dir.path(),
            &current_fingerprint,
        )
        .is_some_and(|error| error.contains("untrusted history quarantine"))
    );
}

#[test]
fn current_and_valid_archived_goals_cannot_misuse_integrity_quarantine() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());

    let current = store
        .start("current success", &[("prove current work".into(), true)])
        .unwrap();
    let current = close_non_code_success(&store, dir.path(), &current);
    assert!(
        store
            .quarantine_invalid_history(&current.id, "hide current success")
            .unwrap_err()
            .to_string()
            .contains("已归档")
    );

    let archived = store
        .archive(&current.id, "valid archived success", false)
        .unwrap();
    assert_eq!(archived.lifecycle_proof_error(dir.path()), None);
    let path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", archived.id));
    let before = fs::read(&path).unwrap();
    let error = store
        .quarantine_invalid_history(&archived.id, "downgrade valid proof")
        .unwrap_err()
        .to_string();
    assert!(error.contains("仍然有效"));
    assert_eq!(fs::read(path).unwrap(), before);
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

/// typed `--must-proof` 义务是不可变合约的一部分：supersede/replacement 的
/// must 转移比较必须区分 proof_kind，文本同名的普通 must 不能顶替 typed must。
/// `None` 与显式 `generic` 在校验语义（proof_kind_matches）里等价，共用一键。
#[test]
fn must_transfer_key_distinguishes_typed_proof_obligations() {
    let requirement_with = |proof_kind: Option<ProofKind>| Requirement {
        id: "req_1".into(),
        text: "install the tool binary".into(),
        kind: RequirementKind::Must,
        proof_kind,
        status: RequirementStatus::Open,
        evidence: None,
        validations: Vec::new(),
        impacts: Vec::new(),
    };
    let plain = requirement_with(None);
    let typed = requirement_with(Some(ProofKind::Installation));
    let generic = requirement_with(Some(ProofKind::Generic));
    assert_ne!(
        lifecycle::must_transfer_key(&plain),
        lifecycle::must_transfer_key(&typed)
    );
    assert_eq!(
        lifecycle::must_transfer_key(&plain),
        lifecycle::must_transfer_key(&generic)
    );
}

#[path = "tests/workflow.rs"]
mod workflow;
