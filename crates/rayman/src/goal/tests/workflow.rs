use super::*;

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
fn pending_readiness_preserves_retired_history_and_keeps_active_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let current = goals
        .start("current boundary", &[("finish current work".into(), true)])
        .unwrap();
    let mut retired = goals
        .start("retired boundary", &[("old owner choice".into(), true)])
        .unwrap();
    retired.lifecycle = GoalLifecycle::Archived;

    let pending = PendingStore::new(dir.path());
    let current_item = pending
        .add_capability_bound(
            complete_human_submission(&current.id, "current owner choice"),
            Some("owner/current-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let retired_item = pending
        .add_capability_bound(
            complete_human_submission(&retired.id, "historical owner choice"),
            Some("owner/historical-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let unbound_item = pending.add("legacy local work", "still active").unwrap();

    let report = pending.readiness(&[current, retired]).unwrap();
    assert_eq!(
        report
            .active
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec![current_item.id.as_str(), unbound_item.id.as_str()]
    );
    assert_eq!(report.historical, vec![retired_item]);
    assert_eq!(
        pending.list().unwrap().len(),
        3,
        "scoping must not delete history"
    );
}

#[test]
fn pending_readiness_fails_closed_when_bound_goal_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let pending = PendingStore::new(dir.path());
    pending
        .add_capability_bound(
            complete_human_submission("goal_missing", "missing owner"),
            Some("owner/missing-goal".into()),
            Some("owner_decision".into()),
        )
        .unwrap();

    let error = pending.readiness(&[]).unwrap_err().to_string();
    assert!(error.contains("goal_missing"), "{error}");
}

fn complete_human_submission(goal_id: &str, detail: &str) -> PendingSubmission {
    PendingSubmission {
        title: "owner decision".into(),
        detail: detail.into(),
        goal_id: Some(goal_id.into()),
        owner: PendingOwner::Human,
        kind: PendingKind::HumanInput,
        attempts: vec!["completed every safe local path".into()],
        evidence_paths: vec!["reports/decision.md".into()],
        minimum_input: Some("choose A or B".into()),
        recommended_action: Some("choose A".into()),
        alternatives: vec!["choose B".into()],
        risk: Some("the product behavior differs".into()),
        resume_command: Some("rayman prepare --goal owner".into()),
        auto_resume_condition: Some("the owner choice is recorded".into()),
        consultation_timing: ConsultationTiming::Immediate,
        background_mechanism: None,
        background_authority_evidence: None,
        background_isolation_evidence: None,
    }
}

#[test]
fn capability_bound_pending_is_idempotent_and_rejects_contract_drift() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let goal = goals
        .start("stable boundary", &[("obtain owner choice".into(), true)])
        .unwrap();
    let pending = PendingStore::new(dir.path());
    let submission = complete_human_submission(&goal.id, "two incompatible directions");

    let first = pending
        .add_capability_bound(
            submission.clone(),
            Some("owner/decision".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let replay = pending
        .add_capability_bound(
            submission.clone(),
            Some("OWNER/DECISION".into()),
            Some("OWNER_DECISION".into()),
        )
        .unwrap();
    assert_eq!(replay.id, first.id);
    assert_eq!(replay.package_sha256, first.package_sha256);
    assert_eq!(pending.list().unwrap().len(), 1);

    let mut drifted = submission;
    drifted.detail = "silently changed question".into();
    let error = pending
        .add_capability_bound(
            drifted,
            Some("owner/decision".into()),
            Some("owner_decision".into()),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("contract conflict"), "{error}");
    assert_eq!(pending.list().unwrap(), std::slice::from_ref(&first));

    let second_goal = goals
        .start("second boundary", &[("obtain owner choice".into(), true)])
        .unwrap();
    let second = pending
        .add_capability_bound(
            complete_human_submission(&second_goal.id, "independent goal choice"),
            Some("owner/decision".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    assert_ne!(second.id, first.id);
    assert_eq!(pending.list().unwrap().len(), 2);
}

#[test]
fn legacy_migration_is_explicit_atomic_auditable_and_does_not_self_present() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let goal = goals
        .start("present once", &[("obtain owner choice".into(), true)])
        .unwrap();
    let pending = PendingStore::new(dir.path());
    let item = pending
        .add_capability_bound(
            complete_human_submission(&goal.id, "legacy-compatible package"),
            Some("owner/legacy-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let path = dir.path().join(PENDING_PATH);

    // Simulate a readable v0 package with a historical agent assertion. It
    // remains non-authoritative until the explicit hash-bound migration.
    let mut legacy: PendingList = read_json(&path).unwrap().unwrap();
    legacy.items[0].contract_version = 0;
    legacy.items[0].capability_key = None;
    legacy.items[0].boundary_class = None;
    legacy.items[0].legacy_migration = None;
    let legacy_sha256 = legacy.items[0].expected_package_sha256().unwrap();
    legacy.items[0].package_sha256 = Some(legacy_sha256.clone());
    legacy.items[0].legacy_agent_assertion_untrusted = Some(LegacyAgentPresentationAssertion {
        presented_at: now_iso(),
        package_sha256: legacy_sha256.clone(),
        channel: "codex".into(),
        reference: Some("historical-agent-claim".into()),
    });
    write_json(&path, &legacy).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(pending.list().unwrap()[0].contract_version, 0);
    assert_eq!(fs::read(&path).unwrap(), before);
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::Continue);
    assert!(!frontier.ask_user_allowed);
    assert!(
        pending
            .render_for_goals(std::slice::from_ref(&goal))
            .is_err()
    );

    assert!(
        pending
            .migrate_legacy(
                &item.id,
                &goal.id,
                &"0".repeat(64),
                "owner/legacy-choice",
                "owner_decision",
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);

    let migrated = pending
        .migrate_legacy(
            &item.id,
            &goal.id,
            &legacy_sha256,
            "owner/legacy-choice",
            "owner_decision",
        )
        .unwrap();
    let proof = migrated.legacy_migration.as_ref().unwrap();
    assert_eq!(proof.from_contract_version, 0);
    assert_eq!(proof.legacy_package_sha256, legacy_sha256);
    assert_eq!(proof.goal_id, goal.id);
    assert_eq!(proof.capability_key, "owner/legacy-choice");
    assert_eq!(
        Some(proof.new_package_sha256.as_str()),
        migrated.package_sha256.as_deref()
    );
    assert!(migrated.legacy_agent_assertion_untrusted.is_some());
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::AskUser);
    assert_eq!(frontier.consultation, FrontierConsultation::Ready);
    let rendered = pending.render_for_goals(&[goal]).unwrap();
    assert!(rendered.text.contains(&item.id));
}

#[test]
fn pending_capability_identity_is_unique_only_within_one_goal() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let first_goal = goals
        .start("first", &[("first choice".into(), true)])
        .unwrap();
    let second_goal = goals
        .start("second", &[("second choice".into(), true)])
        .unwrap();
    let pending = PendingStore::new(dir.path());
    pending
        .add_capability_bound(
            complete_human_submission(&first_goal.id, "first package"),
            Some("shared/decision".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    pending
        .add_capability_bound(
            complete_human_submission(&second_goal.id, "second package"),
            Some("shared/decision".into()),
            Some("owner_decision".into()),
        )
        .unwrap();

    assert_eq!(pending.list().unwrap().len(), 2);

    let path = dir.path().join(PENDING_PATH);
    let mut tampered: PendingList = read_json(&path).unwrap().unwrap();
    tampered.items[1].goal_id = Some(first_goal.id.clone());
    tampered.items[1].package_sha256 = Some(tampered.items[1].expected_package_sha256().unwrap());
    write_json(&path, &tampered).unwrap();
    let error = pending.list().unwrap_err().to_string();
    assert!(error.contains("(goal_id, capability_key) 重复"), "{error}");
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
    let choice = pending
        .add_capability_bound(
            PendingSubmission {
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
            },
            Some("owner/choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::AskUser);
    assert!(frontier.ask_user_allowed);
    assert_eq!(frontier.execution, FrontierExecution::PausedForUser);
    assert_eq!(frontier.consultation, FrontierConsultation::Ready);
    let rendered = pending
        .render_for_goals(std::slice::from_ref(&goal))
        .unwrap();
    assert!(rendered.text.contains(&choice.id));
    assert_eq!(pending.frontier(&goal).unwrap(), frontier);
    assert_eq!(
        goals.close(&goal.id, "blocked").unwrap().status,
        GoalStatus::Blocked
    );
}

#[test]
fn structured_frontier_renders_current_candidates_without_persisting_presentation() {
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
        .add_capability_bound(
            human_submission(ConsultationTiming::Deferred, None, None, None),
            Some("owner/deferred-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.decision, FrontierDecision::Continue);
    assert!(!frontier.ask_user_allowed);
    assert_eq!(frontier.execution, FrontierExecution::ContinueForeground);
    assert_eq!(frontier.consultation, FrontierConsultation::Deferred);
    assert!(!frontier.background_execution_allowed);

    assert!(
        pending
            .add_capability_bound(
                human_submission(
                    ConsultationTiming::Immediate,
                    Some("worktree task".into()),
                    Some("user instruction codex://threads/test".into()),
                    None,
                ),
                Some("owner/partial-background".into()),
                Some("owner_decision".into()),
            )
            .is_err(),
        "partial background proof must fail closed"
    );

    let immediate = pending
        .add_capability_bound(
            human_submission(ConsultationTiming::Immediate, None, None, None),
            Some("owner/immediate-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.execution, FrontierExecution::PausedForUser);
    assert_eq!(frontier.consultation, FrontierConsultation::Ready);
    assert!(frontier.ask_user_allowed);
    let rendered = pending
        .render_for_goals(std::slice::from_ref(&goal))
        .unwrap();
    assert!(rendered.text.contains(&immediate.id));
    assert_eq!(pending.frontier(&goal).unwrap(), frontier);
    pending.resolve(&immediate.id).unwrap();

    let background = pending
        .add_capability_bound(
            human_submission(
                ConsultationTiming::Immediate,
                Some("isolated worktree task task_123".into()),
                Some("user instruction codex://threads/test".into()),
                Some("isolated worktree task task_123".into()),
            ),
            Some("owner/background-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let frontier = pending.frontier(&goal).unwrap();
    assert_eq!(frontier.execution, FrontierExecution::ContinueBackground);
    assert_eq!(frontier.consultation, FrontierConsultation::Ready);
    assert!(frontier.background_execution_allowed);
    let rendered = pending
        .render_for_goals(std::slice::from_ref(&goal))
        .unwrap();
    assert!(rendered.text.contains(&background.id));
    assert_eq!(pending.frontier(&goal).unwrap(), frontier);
    assert!(pending.resolve(&deferred.id).unwrap());
}

#[test]
fn aggregate_render_is_deterministic_complete_and_goal_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let goal_a = goals
        .start("aggregate A", &[("choose".into(), true)])
        .unwrap();
    let goal_b = goals
        .start("aggregate B", &[("choose".into(), true)])
        .unwrap();
    let pending = PendingStore::new(dir.path());
    let item_a = pending
        .add_capability_bound(
            complete_human_submission(&goal_a.id, "choice A"),
            Some("owner/shared-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let item_b = pending
        .add_capability_bound(
            complete_human_submission(&goal_b.id, "choice B"),
            Some("owner/shared-choice".into()),
            Some("owner_decision".into()),
        )
        .unwrap();

    let first = pending
        .render_for_goals(&[goal_b.clone(), goal_a.clone()])
        .unwrap();
    let replay = pending
        .render_for_goals(&[goal_a.clone(), goal_b.clone()])
        .unwrap();
    let partial = pending
        .render_for_goals(std::slice::from_ref(&goal_a))
        .unwrap();

    assert_eq!(first, replay);
    let mut expected_goal_ids = vec![goal_a.id, goal_b.id];
    expected_goal_ids.sort();
    assert_eq!(first.goal_ids, expected_goal_ids);
    assert_eq!(first.pending_ids.len(), 2);
    assert!(first.text.contains("rayman.human-boundary-aggregate.v1"));
    assert!(first.text.contains("\"scope\": \"current_response_only\""));
    assert!(!first.text.contains("rayman.codex-stop-candidate"));
    assert!(first.text.contains(&item_a.id));
    assert!(first.text.contains(&item_b.id));
    assert_ne!(partial.text, first.text);
    assert_ne!(partial.render_sha256, first.render_sha256);
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
        workspace_snapshot: false,
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
fn state_lock_contention_excludes_acl_denial_and_only_retries_os_lock_conflicts() {
    assert!(is_state_lock_contention(&std::io::Error::from(
        std::io::ErrorKind::WouldBlock
    )));
    for code in [32, 33] {
        assert!(is_state_lock_contention(
            &std::io::Error::from_raw_os_error(code)
        ));
    }
    assert!(!is_state_lock_contention(
        &std::io::Error::from_raw_os_error(5)
    ));
    assert!(!is_state_lock_contention(&std::io::Error::from(
        std::io::ErrorKind::PermissionDenied
    )));
    assert!(!is_state_lock_contention(&std::io::Error::from(
        std::io::ErrorKind::NotFound
    )));
}

#[test]
fn state_lock_file_is_stable_and_reusable_after_unlock() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("goal.json");
    {
        let _lock = acquire_state_lock(&target).unwrap();
    }
    let lock_path = dir.path().join(".goal.json.rayman.lock");
    assert!(
        lock_path.is_file(),
        "stable OS lock file must remain after unlock"
    );
    let _reacquired = acquire_state_lock(&target).unwrap();
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
    // `resolve` is the only removal path, so it must stay usable on an invalid
    // file — but removing an id that is not there must still not rewrite it.
    assert!(!store.resolve("pending_x").unwrap());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn pending_store_rejects_hand_tampered_incomplete_solution_package() {
    let dir = tempfile::tempdir().unwrap();
    let goals = GoalStore::new(dir.path());
    let goal = goals
        .start("tamper target", &[("obtain owner choice".into(), true)])
        .unwrap();
    let store = PendingStore::new(dir.path());
    let item = store
        .add_capability_bound(
            PendingSubmission {
                title: "owner choice".into(),
                detail: "two incompatible requirements".into(),
                goal_id: Some(goal.id),
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
            },
            Some("owner/tamper-target".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let path = dir.path().join(PENDING_PATH);
    let mut tampered: PendingList = read_json(&path).unwrap().unwrap();
    tampered.items[0].recommended_action = None;
    write_json(&path, &tampered).unwrap();

    assert!(store.list().is_err());
    // Every reading path stays fail-closed, but `resolve` — the only removal
    // path — must be able to take the invalid item out. Gating it on the same
    // load-time validation left `check` blocked with no CLI way to clear the
    // record that blocked it.
    assert!(store.resolve(&item.id).unwrap());
    assert!(
        store.list().unwrap().is_empty(),
        "removing the invalid item must restore a readable store"
    );
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

/// `start_with_specs` used `any`, so one valid must let a blank sibling
/// through. `current_schema_error` — which every gate re-runs on read —
/// rejects empty requirement text, so the store reported a goal created that
/// no reader would ever accept and no command could retire.
#[test]
fn goal_start_rejects_a_requirement_the_read_path_would_reject() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());

    let error = store
        .start(
            "mixed",
            &[("real work".into(), true), ("   ".into(), false)],
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("空的 requirement"),
        "unexpected error: {error}"
    );
    assert!(store.list().unwrap().is_empty(), "nothing may be persisted");

    let goal = store.start("clean", &[("real work".into(), true)]).unwrap();
    assert!(goal.current_schema_error().is_none());
}

/// The non-executing-mode guard was an exact-literal list, so pytest's `--co`
/// alias and its several collect/plan-only modes produced a "successful"
/// receipt that ran no tests at all.
#[test]
fn pytest_collect_only_aliases_cannot_produce_a_test_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for mode in [
        "--collect-only",
        "--co",
        "--setup-only",
        "--setup-plan",
        "--fixtures",
        "--markers",
    ] {
        let command = format!("pytest {mode}");
        let parsed = validation::parse_validation_command(&command).unwrap();
        assert!(
            validation::validate_command_security(root, &parsed).is_err(),
            "{mode} must not be accepted as an executing test command"
        );
    }
    // A real run stays acceptable.
    let parsed = validation::parse_validation_command("pytest -q").unwrap();
    assert!(validation::validate_command_security(root, &parsed).is_ok());
}

/// `close_lane` was the only lane mutator with no lifecycle guard, so the lane
/// ledger of a superseded goal could still be rewritten.
#[test]
fn close_lane_refuses_a_retired_goal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    let store = GoalStore::new(root);
    let goal = store.start("lanes", &[("work".into(), true)]).unwrap();
    store
        .open_lane(&goal.id, "lane1", LaneMode::Writer, vec!["a.txt".into()])
        .unwrap();

    // Retire the record the same way `supersede`/`archive` persist it, without
    // standing up a gate-ready replacement just to reach the guard.
    let path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{}.json", goal.id));
    let mut raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    raw["lifecycle"] = serde_json::json!("archived");
    std::fs::write(&path, serde_json::to_string(&raw).unwrap()).unwrap();

    let error = store.close_lane(&goal.id, "lane1").unwrap_err();
    assert!(
        error.to_string().contains("关闭 lane"),
        "unexpected error: {error}"
    );
}

/// Abandoned work had no disposal path: `archive` demanded success and
/// `supersede` demanded a replacement that was already gate-ready success, so
/// a goal whose baseline no longer matched reality could only be left dangling.
/// Two consecutive real sessions stopped recording anything because of it.
#[test]
fn an_honestly_closed_goal_can_be_retired_without_ever_claiming_success() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);

    {
        let status = "partial";
        let goal = store
            .start(status, &[("abandoned work".into(), true)])
            .unwrap();
        // An active goal must still state its real outcome first.
        let active = store.archive(&goal.id, "abandoned", false).unwrap_err();
        assert!(
            active.to_string().contains("active goal 不能直接归档"),
            "unexpected error: {active}"
        );

        store.close(&goal.id, status).unwrap();
        let archived = store
            .archive(&goal.id, "abandoned: baseline drifted", false)
            .map_err(|e| format!("archive failed for {status}: {e:#}"))
            .unwrap();

        assert_eq!(archived.lifecycle, GoalLifecycle::Archived);
        assert_ne!(archived.status, GoalStatus::Success);
        // The retired record must stay schema-valid, or it becomes exactly the
        // unremovable state this change exists to eliminate.
        assert!(
            archived.current_schema_error().is_none(),
            "archived record must remain valid: {:?}",
            archived.current_schema_error()
        );

        // `blocked` follows the same rule; the invariant is what both share.
        let mut as_blocked = archived.clone();
        as_blocked.status = GoalStatus::Blocked;
        assert!(as_blocked.current_schema_error().is_none());
        // `active` never becomes a valid archived record.
        let mut as_active = archived.clone();
        as_active.status = GoalStatus::Active;
        assert!(as_active.current_schema_error().is_some());
    }
}

#[test]
fn pending_initial_publication_can_be_archived_losslessly_as_partial() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), "a0").unwrap();
    let store = GoalStore::new(root);
    let goal = store
        .start("pending initial", &[("ship".into(), true)])
        .unwrap();
    let submission = PlanReceiptSubmission {
        changed_paths: vec!["a.txt".into()],
        review_priority: "high".into(),
        impacted_paths: vec!["a.txt".into()],
        recommended_checks: vec!["focused".into()],
    };
    store
        .record_plan_with_before_confirm(&goal.id, submission, || {
            fs::write(root.join("a.txt"), "raced").unwrap();
        })
        .unwrap_err();
    let pending = store.get(&goal.id).unwrap().unwrap();
    let intent = pending.plan_publish_intent.clone();
    let plans = pending.plan_receipts.clone();

    store.close(&goal.id, "partial").unwrap();
    let archived = store
        .archive(
            &goal.id,
            "publication raced; retain exact forensic state",
            false,
        )
        .unwrap();
    assert_eq!(archived.lifecycle, GoalLifecycle::Archived);
    assert_eq!(archived.status, GoalStatus::Partial);
    assert_eq!(archived.plan_publish_intent, intent);
    assert_eq!(archived.plan_receipts, plans);
    assert!(archived.current_schema_error().is_none());
    assert!(archived.lifecycle_proof_error(root).is_none());

    let current = store.mark_current(&goal.id).unwrap();
    assert!(
        current.current_schema_error().is_some(),
        "restoring the record must restore the pending-publication blocker"
    );
}

#[test]
fn pending_extension_publication_can_be_archived_losslessly_as_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), "a0").unwrap();
    fs::write(root.join("b.txt"), "b0").unwrap();
    let store = GoalStore::new(root);
    let goal = store
        .start("pending extension", &[("ship".into(), true)])
        .unwrap();
    store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["a.txt".into()],
                recommended_checks: vec!["base".into()],
            },
        )
        .unwrap();
    fs::write(root.join("a.txt"), "planned").unwrap();
    store
        .extend_plan_with_before_confirm(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["b.txt".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["b.txt".into()],
                recommended_checks: vec!["extension".into()],
            },
            || {
                fs::write(root.join("b.txt"), "raced").unwrap();
            },
        )
        .unwrap_err();
    PendingStore::new(root)
        .add_capability_bound(
            complete_human_submission(&goal.id, "external direction is required"),
            Some("owner/publication-race".into()),
            Some("owner_decision".into()),
        )
        .unwrap();
    let pending = store.get(&goal.id).unwrap().unwrap();
    let intent = pending.plan_publish_intent.clone();
    let plans = pending.plan_receipts.clone();

    store.close(&goal.id, "blocked").unwrap();
    let archived = store
        .archive(
            &goal.id,
            "blocked publication retired without repair",
            false,
        )
        .unwrap();
    assert_eq!(archived.status, GoalStatus::Blocked);
    assert_eq!(archived.plan_publish_intent, intent);
    assert_eq!(archived.plan_receipts, plans);
    assert!(archived.current_schema_error().is_none());
    assert!(archived.lifecycle_proof_error(root).is_none());
    assert!(
        goal_gate_verdict(
            &archived,
            &store.list().unwrap(),
            root,
            Some(&workspace_fingerprint(root).unwrap()),
        )
        .blockers
        .is_empty(),
        "retired non-success history must leave readiness without becoming authority"
    );
}

/// Retiring a non-success goal must never become a completion or authority
/// bypass: every consumer of an archived record also requires success.
#[test]
fn a_retired_non_success_goal_is_never_accepted_as_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    let goal = store.start("abandoned", &[("work".into(), true)]).unwrap();
    store.close(&goal.id, "partial").unwrap();
    let archived = store
        .archive(&goal.id, "abandoned: baseline drifted", false)
        .unwrap();

    let all = store.list().unwrap();
    let fingerprint = workspace_fingerprint(root).unwrap();
    let verdict = goal_gate_verdict(&archived, &all, root, Some(&fingerprint));
    // A retired record leaves readiness entirely rather than satisfying it.
    assert_ne!(archived.status, GoalStatus::Success);
    assert!(
        verdict.blockers.is_empty(),
        "a retired record must not block the workspace: {:?}",
        verdict.blockers
    );
    // Every consumer of an archived record additionally requires success, so a
    // retired partial can never stand in for one. `quarantine_invalid_history`
    // is the cheapest of those gates to exercise directly.
    let quarantined = store.quarantine_invalid_history(&goal.id, "probe");
    assert!(
        quarantined.is_err(),
        "a retired non-success record must not be usable as historical evidence"
    );
}
