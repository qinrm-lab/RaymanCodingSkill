use super::*;

fn planned_non_code_success(store: &GoalStore, root: &Path) -> Goal {
    fs::write(root.join("a.txt"), "a0").unwrap();
    let goal = store
        .start(
            "planned historical success",
            &[("preserve history".into(), true)],
        )
        .unwrap();
    let goal = store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["a.txt".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    close_non_code_success(store, root, &goal)
}

fn refresh_first_validation_contract(goal: &mut Goal) {
    let contract_sha256 = validation_contract_sha256(goal, "req_1").unwrap();
    goal.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap()
        .contract_sha256 = contract_sha256;
}

fn as_pre_rollout_legacy_plan(mut goal: Goal) -> Goal {
    goal.created_at = "2026-08-05T10:00:00Z".into();
    goal.baseline.as_mut().unwrap().recorded_at = "2026-08-05T10:05:00Z".into();
    goal.plan_publication_policy = None;
    goal.plan_publish_intent = None;
    let receipt = &mut goal.plan_receipts[0];
    receipt.recorded_at = "2026-08-05T10:10:00Z".into();
    receipt.publication = None;
    receipt.plan_sha256 = plan_receipt_sha256(receipt);
    refresh_first_validation_contract(&mut goal);
    goal
}

#[test]
fn legacy_success_archive_does_not_repair_corrupt_current_lifecycle_proof() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = store
        .start(
            "legacy current success",
            &[("preserve history".into(), true)],
        )
        .unwrap();
    let mut goal = close_non_code_success(&store, dir.path(), &goal);
    goal.created_at = "2026-08-05T10:00:00Z".into();
    goal.plan_publication_policy = None;
    let contract_sha256 = validation_contract_sha256(&goal, "req_1").unwrap();
    goal.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap()
        .contract_sha256 = contract_sha256;
    goal.lifecycle_proof = Some(LifecycleProof {
        recorded_at: now_iso(),
        workspace_fingerprint: workspace_fingerprint(dir.path()).unwrap(),
        contract_sha256: "0".repeat(64),
        migration: Some(INTEGRITY_QUARANTINE_MIGRATION.into()),
        receipt_policy: Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED.into()),
    });
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();

    let error = store
        .archive(&goal.id, "must not wash current lifecycle proof", false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("目标合约无效"), "error={error}");
    let retained = GoalStore::load_goal_file(&path).unwrap().unwrap();
    assert_eq!(retained.lifecycle, GoalLifecycle::Current);
    assert_eq!(retained.lifecycle_proof, goal.lifecycle_proof);
}

#[test]
fn legacy_success_archive_preserves_a_valid_plan_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();
    let plan_ledger = goal.plan_receipts.clone();
    let requirements = goal.requirements.clone();

    let archived = store
        .archive(&goal.id, "retire valid pre-rollout plan", false)
        .unwrap();
    assert_eq!(archived.lifecycle, GoalLifecycle::Archived);
    assert_eq!(archived.status, GoalStatus::Success);
    assert_eq!(archived.plan_receipts, plan_ledger);
    assert_eq!(archived.requirements, requirements);
    assert!(archived.current_schema_error().is_none());
    assert!(archived.lifecycle_proof_error(dir.path()).is_none());

    let before = fs::read(&path).unwrap();
    let error = store.mark_current(&goal.id).unwrap_err().to_string();
    assert!(error.contains("legacy plan chain"), "error={error}");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn legacy_success_archive_rejects_current_security_gaps_atomically() {
    for case in ["unplanned_delta", "unsafe_command", "missing_review"] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));

        match case {
            "unplanned_delta" => {
                fs::write(dir.path().join("unplanned.txt"), "not in the legacy plan").unwrap();
            }
            "unsafe_command" => {
                goal.requirements[0].validations[0].command =
                    "pwsh -NoProfile -File missing-validation.ps1".into();
            }
            "missing_review" => {
                goal.plan_receipts[0].review_priority = "high".into();
                goal.plan_receipts[0].plan_sha256 = plan_receipt_sha256(&goal.plan_receipts[0]);
            }
            _ => unreachable!(),
        }
        let command = goal.requirements[0].validations[0].command.clone();
        goal.requirements[0].validations[0].receipt = Some(successful_receipt(
            dir.path(),
            &goal,
            "req_1",
            &command,
            &[],
            true,
        ));

        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let before = fs::read(&path).unwrap();

        let error = store
            .archive(&goal.id, &format!("reject {case}"), false)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("success receipt"),
            "case={case} error={error}"
        );
        assert_eq!(fs::read(&path).unwrap(), before, "case={case}");
    }
}

#[test]
fn legacy_success_historical_fingerprint_exclusion_searches_remaining_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    let mut later = goal.requirements[0].validations[0].clone();
    for (validation, fingerprint) in [
        (&mut goal.requirements[0].validations[0], "0".repeat(64)),
        (&mut later, "f".repeat(64)),
    ] {
        let receipt = validation.receipt.as_mut().unwrap();
        receipt.workspace_fingerprint_before = fingerprint.clone();
        receipt.workspace_fingerprint_after = fingerprint;
    }
    goal.requirements[0].validations.push(later);

    assert_eq!(
        historical_success_fingerprint_excluding(
            &goal,
            dir.path(),
            ReceiptValidationPolicy::CurrentV2,
            Some(&"0".repeat(64)),
        ),
        Some("f".repeat(64))
    );
}

#[test]
fn legacy_success_historical_fingerprint_requires_bound_high_review() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    let historical_fingerprint = "a".repeat(64);
    goal.plan_receipts[0].review_priority = "high".into();
    goal.plan_receipts[0].plan_sha256 = plan_receipt_sha256(&goal.plan_receipts[0]);
    let receipt = goal.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap();
    receipt.workspace_fingerprint_before = historical_fingerprint.clone();
    receipt.workspace_fingerprint_after = historical_fingerprint.clone();

    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();
    let before = fs::read(&path).unwrap();
    let error = store
        .archive(&goal.id, "missing historical final review", false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("success receipt"), "error={error}");
    assert_eq!(fs::read(&path).unwrap(), before);

    goal.review_receipts.push(ReviewReceipt {
        recorded_at: "2026-08-05T10:20:00Z".into(),
        source_fingerprint: historical_fingerprint.clone(),
        reviewer: "security-review".into(),
        summary: "reviewed the historical final fingerprint".into(),
    });
    write_json(&path, &goal).unwrap();
    let archived = store
        .archive(&goal.id, "retire reviewed historical success", false)
        .unwrap();
    assert_eq!(
        archived
            .lifecycle_proof
            .as_ref()
            .unwrap()
            .workspace_fingerprint,
        historical_fingerprint
    );
    assert!(archived.lifecycle_proof_error(dir.path()).is_none());
}

#[test]
fn legacy_success_historical_fingerprint_rejects_scope_outside_plan() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    let impacts = vec![impact("outside.txt")];
    goal.requirements[0].impacts = impacts;
    let mut validation =
        current_validation(&goal, "req_1", dir.path(), "git status", &["outside.txt"]);
    let historical_fingerprint = "b".repeat(64);
    let receipt = validation.receipt.as_mut().unwrap();
    receipt.workspace_fingerprint_before = historical_fingerprint.clone();
    receipt.workspace_fingerprint_after = historical_fingerprint;
    goal.requirements[0].validations = vec![validation];

    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();
    let before = fs::read(&path).unwrap();
    let error = store
        .archive(&goal.id, "reject historical scope outside plan", false)
        .unwrap_err()
        .to_string();
    assert!(error.contains("success receipt"), "error={error}");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn legacy_success_archive_rejects_plan_tampering_atomically() {
    for case in [
        "post_rollout",
        "v16_chronology",
        "legacy_chronology",
        "bad_plan_hash",
        "retained_publication",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let modern = planned_non_code_success(&store, dir.path());
        let mut goal = if case == "v16_chronology" {
            modern.clone()
        } else {
            as_pre_rollout_legacy_plan(modern.clone())
        };
        match case {
            "post_rollout" => {
                goal.created_at = "2026-08-05T10:30:01Z".into();
                goal.baseline.as_mut().unwrap().recorded_at = "2026-08-05T10:31:00Z".into();
                goal.plan_receipts[0].recorded_at = "2026-08-05T10:32:00Z".into();
                goal.updated_at = "2026-08-05T10:33:00Z".into();
                goal.plan_receipts[0].plan_sha256 = plan_receipt_sha256(&goal.plan_receipts[0]);
                refresh_first_validation_contract(&mut goal);
            }
            "v16_chronology" => goal.updated_at = "1970-01-01T00:00:00Z".into(),
            "legacy_chronology" => goal.updated_at = "2026-08-05T10:09:59Z".into(),
            "bad_plan_hash" => goal.plan_receipts[0].plan_sha256 = "0".repeat(64),
            "retained_publication" => {
                goal.plan_receipts[0].publication = modern.plan_receipts[0].publication.clone();
                goal.plan_receipts[0].plan_sha256 = plan_receipt_sha256(&goal.plan_receipts[0]);
            }
            _ => unreachable!(),
        }
        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let before = fs::read(&path).unwrap();

        let error = store
            .archive(&goal.id, &format!("reject {case}"), false)
            .unwrap_err()
            .to_string();
        if matches!(case, "v16_chronology" | "legacy_chronology") {
            assert!(
                error.contains("时间顺序") || error.contains("updated_at"),
                "case={case} error={error}"
            );
        } else {
            assert!(error.contains("目标合约无效"), "case={case} error={error}");
        }
        assert_eq!(fs::read(&path).unwrap(), before, "case={case}");
    }
}

#[test]
fn legacy_plan_chronology_rejects_every_reversed_edge() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut valid = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    valid.lifecycle = GoalLifecycle::Archived;
    valid.status = GoalStatus::Partial;
    valid.lifecycle_reason = Some("chronology fixture".into());
    valid.updated_at = "2026-08-05T10:25:00Z".into();
    let baseline_fingerprint = valid
        .baseline
        .as_ref()
        .unwrap()
        .workspace_fingerprint
        .clone();
    let base_sha256 = valid.plan_receipts[0].plan_sha256.clone();
    let mut first = PlanExtensionReceipt {
        recorded_at: "2026-08-05T10:15:00Z".into(),
        previous_plan_sha256: base_sha256,
        changed_paths: vec!["a.txt".into(), "b.txt".into()],
        review_priority: "normal".into(),
        impacted_paths: vec!["a.txt".into(), "b.txt".into()],
        recommended_checks: Vec::new(),
        publication: None,
        extension_sha256: String::new(),
    };
    first.extension_sha256 = plan_extension_sha256(&baseline_fingerprint, &first);
    let mut second = PlanExtensionReceipt {
        recorded_at: "2026-08-05T10:20:00Z".into(),
        previous_plan_sha256: first.extension_sha256.clone(),
        changed_paths: vec!["a.txt".into(), "b.txt".into(), "c.txt".into()],
        review_priority: "normal".into(),
        impacted_paths: vec!["a.txt".into(), "b.txt".into(), "c.txt".into()],
        recommended_checks: Vec::new(),
        publication: None,
        extension_sha256: String::new(),
    };
    second.extension_sha256 = plan_extension_sha256(&baseline_fingerprint, &second);
    valid.plan_receipts[0].extensions = vec![first, second];
    assert!(plan_chain_error(&valid).is_none());

    for case in [
        "baseline_before_created",
        "receipt_before_baseline",
        "first_extension_before_receipt",
        "second_extension_before_first",
        "updated_before_last_extension",
    ] {
        let mut reversed = valid.clone();
        match case {
            "baseline_before_created" => {
                reversed.baseline.as_mut().unwrap().recorded_at = "2026-08-05T09:59:59Z".into();
            }
            "receipt_before_baseline" => {
                reversed.plan_receipts[0].recorded_at = "2026-08-05T10:04:59Z".into();
            }
            "first_extension_before_receipt" => {
                reversed.plan_receipts[0].extensions[0].recorded_at = "2026-08-05T10:09:59Z".into();
            }
            "second_extension_before_first" => {
                reversed.plan_receipts[0].extensions[1].recorded_at = "2026-08-05T10:14:59Z".into();
            }
            "updated_before_last_extension" => {
                reversed.updated_at = "2026-08-05T10:19:59Z".into();
            }
            _ => unreachable!(),
        }
        let error = plan_chain_error(&reversed).unwrap();
        assert!(error.contains("时间顺序"), "case={case} error={error}");
    }
}

#[test]
fn legacy_plan_chronology_cannot_be_laundered_by_partial_close() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    goal.status = GoalStatus::Active;
    goal.updated_at = "2026-08-05T10:09:59Z".into();
    refresh_first_validation_contract(&mut goal);
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();
    let before = fs::read(&path).unwrap();

    let error = store.close(&goal.id, "partial").unwrap_err().to_string();
    assert!(error.contains("时间顺序"), "error={error}");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn plan_chronology_cannot_be_laundered_by_other_goal_mutators() {
    for case in [
        "legacy_review",
        "legacy_evidence",
        "v16_close",
        "v16_archive",
        "v16_work_package",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let modern = planned_non_code_success(&store, dir.path());
        let mut goal = if case.starts_with("legacy_") {
            as_pre_rollout_legacy_plan(modern)
        } else {
            modern
        };
        goal.status = match case {
            "legacy_review" => GoalStatus::Success,
            "v16_archive" => GoalStatus::Partial,
            _ => GoalStatus::Active,
        };
        goal.updated_at = if case.starts_with("legacy_") {
            "2026-08-05T10:09:59Z".into()
        } else {
            "1970-01-01T00:00:00Z".into()
        };
        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let before = fs::read(&path).unwrap();

        let result = match case {
            "legacy_review" => store.record_review(&goal.id, "reviewer", "must stay atomic"),
            "legacy_evidence" => store.record_evidence_with_context(
                &goal.id,
                "req_1",
                "must stay atomic",
                Vec::new(),
                Vec::new(),
            ),
            "v16_close" => store.close(&goal.id, "partial"),
            "v16_archive" => store.archive(&goal.id, "must stay atomic", false),
            "v16_work_package" => store.add_work_package(
                &goal.id,
                "chronology",
                "must stay atomic",
                None,
                vec!["req_1".into()],
                false,
            ),
            _ => unreachable!(),
        };
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("时间顺序") || error.contains("updated_at"),
            "case={case} error={error}"
        );
        assert_eq!(fs::read(&path).unwrap(), before, "case={case}");
    }
}

#[test]
fn legacy_success_archive_migrations_cannot_bless_an_invalid_receipt() {
    for migration in ["unreceipted", "receipt_v1"] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
        goal.created_at = if migration == "unreceipted" {
            "2026-07-13T10:00:00Z".into()
        } else {
            "2026-07-17T10:00:00Z".into()
        };
        refresh_first_validation_contract(&mut goal);
        goal.requirements[0].validations[0]
            .receipt
            .as_mut()
            .unwrap()
            .contract_sha256 = "0".repeat(64);
        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let before = fs::read(&path).unwrap();

        let result = if migration == "unreceipted" {
            store.archive_with_receipt_policy(
                &goal.id,
                "invalid receipt must remain invalid",
                true,
                None,
            )
        } else {
            store.archive_with_receipt_policy(
                &goal.id,
                "invalid receipt must remain invalid",
                false,
                Some(RECEIPT_POLICY_V1),
            )
        };
        assert!(result.is_err(), "migration={migration}");
        assert_eq!(fs::read(&path).unwrap(), before, "migration={migration}");
    }
}

#[test]
fn legacy_success_archive_migrations_cannot_bypass_current_governance() {
    for case in [
        "unreceipted_unplanned",
        "receipt_v1_unplanned",
        "receipt_v1_unsafe",
        "receipt_v1_missing_delta_scope",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
        goal.created_at = if case == "unreceipted_unplanned" {
            "2026-07-13T10:00:00Z".into()
        } else {
            "2026-07-17T10:00:00Z".into()
        };
        refresh_first_validation_contract(&mut goal);

        match case {
            "unreceipted_unplanned" => {
                goal.requirements[0].validations[0].receipt = None;
                fs::write(dir.path().join("unplanned.txt"), "not in the legacy plan").unwrap();
            }
            "receipt_v1_unplanned" => {
                fs::write(dir.path().join("unplanned.txt"), "not in the legacy plan").unwrap();
            }
            "receipt_v1_unsafe" => {
                goal.requirements[0].validations[0].command =
                    "pwsh -NoProfile -File missing-validation.ps1".into();
            }
            "receipt_v1_missing_delta_scope" => {
                goal.plan_receipts[0].changed_paths = vec!["a.txt".into(), "b.txt".into()];
                goal.plan_receipts[0].impacted_paths = vec!["a.txt".into(), "b.txt".into()];
                goal.plan_receipts[0].plan_sha256 = plan_receipt_sha256(&goal.plan_receipts[0]);
                fs::write(dir.path().join("a.txt"), "a1").unwrap();
                fs::write(dir.path().join("b.txt"), "b1").unwrap();
            }
            _ => unreachable!(),
        }
        if case != "unreceipted_unplanned" {
            let impacts = if case == "receipt_v1_missing_delta_scope" {
                vec![impact("a.txt")]
            } else {
                Vec::new()
            };
            let impact_paths = impacts
                .iter()
                .map(|impact| impact.changed_path.clone())
                .collect::<Vec<_>>();
            let impact_scopes = validation_scopes_for_impacts(&impacts);
            let non_code = impacts.is_empty();
            let command = goal.requirements[0].validations[0].command.clone();
            goal.requirements[0].impacts = impacts.clone();
            goal.requirements[0].validations[0].impact_paths = impact_paths;
            goal.requirements[0].validations[0].impact_scopes = impact_scopes;
            goal.requirements[0].validations[0].non_code = non_code;
            goal.requirements[0].validations[0].receipt = Some(successful_receipt(
                dir.path(),
                &goal,
                "req_1",
                &command,
                &impacts,
                non_code,
            ));
        }
        if case == "unreceipted_unplanned" {
            assert!(pre_receipt_migration_eligible(&goal));
        } else {
            assert!(receipt_policy_v1_migration_eligible(&goal));
        }
        let current_fingerprint = workspace_fingerprint(dir.path()).unwrap();
        let migration_gaps = if case == "unreceipted_unplanned" {
            goal_retiring_legacy_success_unreceipted_migration_gaps(
                &goal,
                dir.path(),
                &current_fingerprint,
            )
        } else {
            goal_retiring_legacy_success_v1_migration_gaps(&goal, dir.path(), &current_fingerprint)
        };
        assert!(!migration_gaps.is_empty(), "case={case}");

        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let before = fs::read(&path).unwrap();
        let result = if case == "unreceipted_unplanned" {
            store.archive_with_receipt_policy(
                &goal.id,
                "migration must not repair plan omissions",
                true,
                None,
            )
        } else {
            store.archive_with_receipt_policy(
                &goal.id,
                "migration must not repair current security",
                false,
                Some(RECEIPT_POLICY_V1),
            )
        };
        let error = result.unwrap_err().to_string();
        if case == "unreceipted_unplanned" {
            assert!(
                error.contains("不能修复当前 command/plan/review 缺口"),
                "case={case} error={error}"
            );
        } else {
            assert!(
                error.contains("success receipt 未通过"),
                "case={case} error={error}"
            );
        }
        assert_eq!(fs::read(&path).unwrap(), before, "case={case}");
    }
}

#[test]
fn legacy_success_v1_migration_uses_a_complete_safe_receipt_set() {
    {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
        goal.created_at = "2026-07-17T10:00:00Z".into();
        fs::write(dir.path().join("a.txt"), "a1").unwrap();
        goal.requirements[0].impacts = vec![impact("a.txt")];
        let mut safe = current_validation(
            &goal,
            "req_1",
            dir.path(),
            "python -m pytest -q",
            &["a.txt"],
        );
        let safe_receipt = safe.receipt.as_mut().unwrap();
        safe_receipt.passed_tests = None;
        safe_receipt.listed_tests = None;
        safe_receipt.ignored_tests = None;
        safe_receipt.list_stdout_sha256 = None;
        safe_receipt.list_stderr_sha256 = None;
        let unsafe_extra = current_validation(
            &goal,
            "req_1",
            dir.path(),
            "pwsh -NoProfile -File missing-validation.ps1",
            &["outside.txt"],
        );
        goal.requirements[0].validations = vec![unsafe_extra, safe];
        let current_fingerprint = workspace_fingerprint(dir.path()).unwrap();
        assert!(
            goal_retiring_legacy_success_v1_migration_gaps(
                &goal,
                dir.path(),
                &current_fingerprint,
            )
            .is_empty()
        );

        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let archived = store
            .archive_with_receipt_policy(
                &goal.id,
                "safe proof set ignores unsafe extras",
                false,
                Some(RECEIPT_POLICY_V1),
            )
            .unwrap();
        assert_eq!(
            archived
                .lifecycle_proof
                .as_ref()
                .unwrap()
                .receipt_policy
                .as_deref(),
            Some(RECEIPT_POLICY_V1)
        );
        assert!(archived.lifecycle_proof_error(dir.path()).is_none());
    }

    {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("b.txt"), "b0").unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
        goal.created_at = "2026-07-17T10:00:00Z".into();
        goal.plan_receipts[0].changed_paths = vec!["a.txt".into(), "b.txt".into()];
        goal.plan_receipts[0].impacted_paths = vec!["a.txt".into(), "b.txt".into()];
        goal.plan_receipts[0].plan_sha256 = plan_receipt_sha256(&goal.plan_receipts[0]);
        fs::write(dir.path().join("a.txt"), "a1").unwrap();
        fs::write(dir.path().join("b.txt"), "b1").unwrap();
        goal.requirements[0].impacts = vec![impact("a.txt"), impact("b.txt")];
        let mut safe_partial = current_validation(
            &goal,
            "req_1",
            dir.path(),
            "python -m pytest -q",
            &["a.txt"],
        );
        let safe_receipt = safe_partial.receipt.as_mut().unwrap();
        safe_receipt.passed_tests = None;
        safe_receipt.listed_tests = None;
        safe_receipt.ignored_tests = None;
        safe_receipt.list_stdout_sha256 = None;
        safe_receipt.list_stderr_sha256 = None;
        let unsafe_only_for_b = current_validation(
            &goal,
            "req_1",
            dir.path(),
            "pwsh -NoProfile -File missing-validation.ps1",
            &["b.txt"],
        );
        goal.requirements[0].validations = vec![safe_partial, unsafe_only_for_b];
        let current_fingerprint = workspace_fingerprint(dir.path()).unwrap();
        assert!(
            !goal_retiring_legacy_success_v1_migration_gaps(
                &goal,
                dir.path(),
                &current_fingerprint,
            )
            .is_empty()
        );

        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();
        let before = fs::read(&path).unwrap();
        assert!(
            store
                .archive_with_receipt_policy(
                    &goal.id,
                    "unsafe receipt cannot complete missing safe scope",
                    false,
                    Some(RECEIPT_POLICY_V1),
                )
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn legacy_success_archive_v1_migration_keeps_distinct_historical_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = as_pre_rollout_legacy_plan(planned_non_code_success(&store, dir.path()));
    goal.created_at = "2026-07-17T10:00:00Z".into();
    let command = "python -m pytest -q";
    let mut validation = current_validation(&goal, "req_1", dir.path(), command, &[]);
    let historical_fingerprint = "a".repeat(64);
    let receipt = validation.receipt.as_mut().unwrap();
    receipt.workspace_fingerprint_before = historical_fingerprint.clone();
    receipt.workspace_fingerprint_after = historical_fingerprint.clone();
    receipt.passed_tests = None;
    receipt.listed_tests = None;
    receipt.ignored_tests = None;
    receipt.list_stdout_sha256 = None;
    receipt.list_stderr_sha256 = None;
    goal.requirements[0].validations = vec![validation];
    fs::write(dir.path().join("unplanned.txt"), "post-history drift").unwrap();
    let current_fingerprint = workspace_fingerprint(dir.path()).unwrap();
    assert_ne!(current_fingerprint, historical_fingerprint);
    assert!(receipt_policy_v1_migration_eligible(&goal));
    assert!(
        !goal_retiring_legacy_success_v1_migration_gaps(&goal, dir.path(), &current_fingerprint,)
            .is_empty()
    );

    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();
    let archived = store
        .archive_with_receipt_policy(
            &goal.id,
            "retain distinct historical v1 proof",
            false,
            Some(RECEIPT_POLICY_V1),
        )
        .unwrap();
    let proof = archived.lifecycle_proof.as_ref().unwrap();
    assert_eq!(proof.workspace_fingerprint, historical_fingerprint);
    assert_eq!(proof.receipt_policy.as_deref(), Some(RECEIPT_POLICY_V1));
    assert_eq!(
        proof.migration.as_deref(),
        Some(RECEIPT_POLICY_V1_MIGRATION)
    );
    assert!(archived.lifecycle_proof_error(dir.path()).is_none());
}

#[test]
fn legacy_success_historical_pre_plan_scope_keeps_the_original_one_file_boundary() {
    for paths in [vec!["a.txt"], vec!["a.txt", "b.txt"]] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = store
            .start(
                "pre-plan historical success",
                &[("preserve proof".into(), true)],
            )
            .unwrap();
        goal.created_at = "2026-07-17T10:00:00Z".into();
        goal.status = GoalStatus::Success;
        goal.plan_publication_policy = None;
        goal.requirements[0].status = RequirementStatus::Done;
        goal.requirements[0].evidence = Some("historical validation passed".into());
        goal.requirements[0].impacts = paths.iter().map(|path| impact(path)).collect();
        let validation = current_validation(&goal, "req_1", dir.path(), "git status", &paths);
        goal.requirements[0].validations = vec![validation];
        refresh_first_validation_contract(&mut goal);
        let fingerprint = workspace_fingerprint(dir.path()).unwrap();
        let gaps = goal_success_receipt_gaps_for_historical_legacy_success(
            &goal,
            dir.path(),
            &fingerprint,
            ReceiptValidationPolicy::LegacyV1,
        );
        assert_eq!(
            gaps.is_empty(),
            paths.len() == 1,
            "paths={paths:?} gaps={gaps:?}"
        );
    }
}

#[test]
fn existing_goal_mutators_keep_logical_time_monotonic_when_the_clock_rolls_back() {
    const FUTURE: &str = "2099-01-01T00:00:00Z";

    for case in ["review", "close", "work_package", "archive"] {
        let dir = tempfile::tempdir().unwrap();
        let store = GoalStore::new(dir.path());
        let mut goal = planned_non_code_success(&store, dir.path());
        if matches!(case, "close" | "work_package") {
            goal.status = GoalStatus::Active;
        }
        goal.updated_at = FUTURE.into();
        let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
        write_json(&path, &goal).unwrap();

        let updated = match case {
            "review" => store
                .record_review(
                    &goal.id,
                    "future-clock-review",
                    "clock rollback must be safe",
                )
                .unwrap(),
            "close" => store.close(&goal.id, "partial").unwrap(),
            "work_package" => store
                .add_work_package(
                    &goal.id,
                    "future_clock",
                    "clock rollback must be safe",
                    None,
                    vec!["req_1".into()],
                    false,
                )
                .unwrap(),
            "archive" => store
                .archive(&goal.id, "clock rollback must be safe", false)
                .unwrap(),
            _ => unreachable!(),
        };

        assert_eq!(updated.updated_at, FUTURE, "case={case}");
        if case == "review" {
            assert_eq!(updated.review_receipts.last().unwrap().recorded_at, FUTURE);
        }
        if case == "archive" {
            assert_eq!(
                updated.lifecycle_proof.as_ref().unwrap().recorded_at,
                FUTURE
            );
            assert!(updated.lifecycle_proof_error(dir.path()).is_none());
        }
    }
}

#[test]
fn initial_plan_publication_is_not_earlier_than_a_future_goal_baseline() {
    const CREATED: &str = "2099-01-01T00:00:00Z";
    const BASELINE: &str = "2099-01-02T00:00:00Z";

    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a0").unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = store
        .start(
            "future baseline plan",
            &[("preserve chronology".into(), true)],
        )
        .unwrap();
    goal.created_at = CREATED.into();
    goal.updated_at = CREATED.into();
    goal.baseline.as_mut().unwrap().recorded_at = BASELINE.into();
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();

    let planned = store
        .record_plan(
            &goal.id,
            PlanReceiptSubmission {
                changed_paths: vec!["a.txt".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["a.txt".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    let receipt = &planned.plan_receipts[0];
    let publication = receipt.publication.as_ref().unwrap();

    assert_eq!(receipt.recorded_at, BASELINE);
    assert_eq!(publication.published_at, BASELINE);
    assert_eq!(publication.committed_at.as_deref(), Some(BASELINE));
    assert_eq!(planned.updated_at, BASELINE);
    assert!(plan_chain_error(&planned).is_none());
}

#[test]
fn mark_current_keeps_the_retired_lifecycle_proof_as_a_time_lower_bound() {
    const UPDATED: &str = "2099-01-01T00:00:00Z";
    const PROOF: &str = "2099-01-02T00:00:00Z";

    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = planned_non_code_success(&store, dir.path());
    let mut archived = store
        .archive(&goal.id, "future lifecycle proof", false)
        .unwrap();
    let old_proof = archived.lifecycle_proof.take().unwrap();
    archived.updated_at = UPDATED.into();
    archived.lifecycle_proof = Some(issue_lifecycle_proof_at(
        &archived,
        old_proof.workspace_fingerprint,
        old_proof.migration,
        old_proof.receipt_policy,
        PROOF.into(),
    ));
    assert!(archived.lifecycle_proof_error(dir.path()).is_none());
    let path = dir
        .path()
        .join(GOALS_DIR)
        .join(format!("{}.json", archived.id));
    write_json(&path, &archived).unwrap();

    let current = store.mark_current(&archived.id).unwrap();
    assert_eq!(current.updated_at, PROOF);
    assert!(current.lifecycle_proof.is_none());
}

#[test]
fn archive_uses_the_latest_timestamp_from_the_complete_goal_ledger() {
    const FUTURE_VALIDATION: &str = "2099-02-01T00:00:00Z";

    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let mut goal = planned_non_code_success(&store, dir.path());
    goal.requirements[0].validations[0].recorded_at = FUTURE_VALIDATION.into();
    let path = dir.path().join(GOALS_DIR).join(format!("{}.json", goal.id));
    write_json(&path, &goal).unwrap();

    let archived = store
        .archive(&goal.id, "bind the complete ledger", false)
        .unwrap();
    assert_eq!(archived.updated_at, FUTURE_VALIDATION);
    assert_eq!(
        archived.lifecycle_proof.as_ref().unwrap().recorded_at,
        FUTURE_VALIDATION
    );
    assert!(archived.lifecycle_proof_error(dir.path()).is_none());
}

#[test]
fn incoming_authority_time_is_validated_and_bounds_the_atomic_goal_event() {
    const FUTURE_AUTHORITY: &str = "2099-03-01T00:00:00Z";

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 1 }").unwrap();
    let store = GoalStore::new(root);
    let goal = store
        .start("future authority", &[("prove repository".into(), true)])
        .unwrap();
    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 2 }").unwrap();
    let command = "cargo test --workspace --all-targets";
    let impacts = vec![impact("lib.rs")];
    let impact_scopes = validation_scopes_for_impacts(&impacts);
    let fingerprint = workspace_fingerprint(root).unwrap();
    let contract_sha256 = validation_contract_sha256(&goal, "req_1").unwrap();
    let submission = |recorded_at: &str| {
        let runs = (0..2)
            .map(|_| AuthorityRunReceipt {
                exit_code: 0,
                workspace_fingerprint_before: fingerprint.clone(),
                workspace_fingerprint_after: fingerprint.clone(),
                stdout_sha256: "a".repeat(64),
                stderr_sha256: "b".repeat(64),
            })
            .collect::<Vec<_>>();
        AuthorityReceiptSubmission {
            validation: ValidationReceiptSubmission {
                evidence: "stable direct authority".into(),
                command: command.into(),
                receipt: successful_receipt(root, &goal, "req_1", command, &impacts, false),
                impacts: impacts.clone(),
                non_code: false,
            },
            authority: AuthorityReceipt {
                requirement_id: "req_1".into(),
                command: command.into(),
                recorded_at: recorded_at.into(),
                workspace_fingerprint: fingerprint.clone(),
                repeat: 2,
                impact_scopes: impact_scopes.clone(),
                non_code: false,
                invocation_sha256: authority_invocation_sha256(
                    command,
                    "req_1",
                    2,
                    &impact_scopes,
                    false,
                ),
                contract_sha256: contract_sha256.clone(),
                runs,
            },
        }
    };
    let path = store.goal_path(&goal.id).unwrap();
    let before = fs::read(&path).unwrap();

    let error = store
        .record_authority_validation_receipt(&goal.id, "req_1", submission("not-a-timestamp"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("RFC3339"), "error={error}");
    assert_eq!(fs::read(&path).unwrap(), before);

    let recorded = store
        .record_authority_validation_receipt(&goal.id, "req_1", submission(FUTURE_AUTHORITY))
        .unwrap();
    assert_eq!(recorded.updated_at, FUTURE_AUTHORITY);
    assert_eq!(
        recorded.requirements[0].validations[0].recorded_at,
        FUTURE_AUTHORITY
    );
    assert_eq!(recorded.authority_receipts[0].recorded_at, FUTURE_AUTHORITY);
}

#[test]
fn malformed_lifecycle_proof_time_can_still_be_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let goal = planned_non_code_success(&store, dir.path());
    let mut archived = store.archive(&goal.id, "valid archive", false).unwrap();
    archived.lifecycle_proof.as_mut().unwrap().recorded_at = "not-a-timestamp".into();
    let path = store.goal_path(&goal.id).unwrap();
    write_json(&path, &archived).unwrap();
    assert!(
        archived
            .lifecycle_proof_error(dir.path())
            .is_some_and(|error| error.contains("RFC3339"))
    );

    let quarantined = store
        .quarantine_invalid_history(&goal.id, "retain malformed proof as untrusted")
        .unwrap();
    assert_eq!(
        quarantined
            .lifecycle_proof
            .as_ref()
            .and_then(|proof| proof.receipt_policy.as_deref()),
        Some(RECEIPT_POLICY_INTEGRITY_QUARANTINED)
    );
    assert!(
        quarantined
            .lifecycle_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("RFC3339"))
    );
    assert!(quarantined.lifecycle_proof_error(dir.path()).is_none());
}

#[test]
fn replacement_and_supersession_times_follow_every_referenced_goal() {
    const LIVE: &str = "2099-04-01T00:00:00Z";
    const PREDECESSOR: &str = "2099-04-02T00:00:00Z";
    const AUTHORITY: &str = "2099-04-03T00:00:00Z";

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    let mut authority = archived_direct_authority_success(&store, root);
    let old_proof = authority.lifecycle_proof.take().unwrap();
    authority.lifecycle_proof = Some(issue_lifecycle_proof_at(
        &authority,
        old_proof.workspace_fingerprint,
        old_proof.migration,
        old_proof.receipt_policy,
        AUTHORITY.into(),
    ));
    write_json(&store.goal_path(&authority.id).unwrap(), &authority).unwrap();
    assert!(authority.lifecycle_proof_error(root).is_none());

    let mut predecessor = store
        .start("future predecessor", &[("preserve alpha".into(), true)])
        .unwrap();
    predecessor.updated_at = PREDECESSOR.into();
    write_json(&store.goal_path(&predecessor.id).unwrap(), &predecessor).unwrap();
    let replacement = store
        .start("future replacement", &[("preserve alpha".into(), true)])
        .unwrap();
    let predecessor_ids = vec![predecessor.id.clone()];
    let mut live =
        live_replacement_authority(root, &replacement.id, &predecessor_ids, &authority.id);
    live.recorded_at = LIVE.into();

    let authorized = store
        .authorize_replacement(&replacement.id, &predecessor_ids, &authority.id, live)
        .unwrap();
    assert_eq!(authorized.updated_at, AUTHORITY);
    assert_eq!(
        authorized
            .replacement_authority
            .as_ref()
            .unwrap()
            .recorded_at,
        AUTHORITY
    );
    let fingerprint = workspace_fingerprint(root).unwrap();
    assert_eq!(
        replacement_authority_error(&authorized, root, &fingerprint),
        None
    );

    let superseded = store.supersede(&predecessor.id, &authorized.id).unwrap();
    let authorized = store.get(&authorized.id).unwrap().unwrap();
    assert_eq!(superseded.updated_at, AUTHORITY);
    assert_eq!(
        superseded.lifecycle_proof.as_ref().unwrap().recorded_at,
        AUTHORITY
    );
    assert!(superseded.lifecycle_proof_error(root).is_none());
    assert_eq!(
        replacement_authority_error(&authorized, root, &fingerprint),
        None
    );
    assert_eq!(
        supersession_error(&superseded, &[authorized], root, &fingerprint),
        None
    );
}

fn authorized_lifecycle_only_replacement_fixture() -> (tempfile::TempDir, GoalStore, Goal, Goal) {
    let dir = tempfile::tempdir().unwrap();
    let store = GoalStore::new(dir.path());
    let authority = archived_direct_authority_success(&store, dir.path());
    let predecessor = store
        .start("unfinished predecessor", &[("preserve alpha".into(), true)])
        .unwrap();
    let replacement = store
        .start(
            "lifecycle-only replacement",
            &[("preserve alpha".into(), true)],
        )
        .unwrap();
    let authorized = store
        .authorize_replacement(
            &replacement.id,
            std::slice::from_ref(&predecessor.id),
            &authority.id,
            live_replacement_authority(
                dir.path(),
                &replacement.id,
                std::slice::from_ref(&predecessor.id),
                &authority.id,
            ),
        )
        .unwrap();
    (dir, store, predecessor, authorized)
}

fn refresh_replacement_authority_hash(goal: &mut Goal) {
    let proof_sha256 = replacement_authority_proof_sha256(
        goal.replacement_authority
            .as_ref()
            .expect("replacement proof exists"),
    );
    goal.replacement_authority
        .as_mut()
        .expect("replacement proof exists")
        .proof_sha256 = proof_sha256;
}

#[test]
fn replacement_authority_read_side_rejects_rehashed_timestamp_forgery() {
    const PROOF_AT: &str = "2099-04-03T00:00:00Z";
    const BEFORE_PROOF: &str = "2099-04-02T00:00:00Z";
    const AFTER_PROOF: &str = "2099-04-04T00:00:00Z";

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    let mut authority = archived_direct_authority_success(&store, root);
    let old_authority_proof = authority.lifecycle_proof.take().unwrap();
    authority.lifecycle_proof = Some(issue_lifecycle_proof_at(
        &authority,
        old_authority_proof.workspace_fingerprint,
        old_authority_proof.migration,
        old_authority_proof.receipt_policy,
        PROOF_AT.into(),
    ));
    write_json(&store.goal_path(&authority.id).unwrap(), &authority).unwrap();

    let predecessor = store
        .start("timestamp predecessor", &[("preserve alpha".into(), true)])
        .unwrap();
    let replacement = store
        .start("timestamp replacement", &[("preserve alpha".into(), true)])
        .unwrap();
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
    let fingerprint = workspace_fingerprint(root).unwrap();
    assert_eq!(
        authorized
            .replacement_authority
            .as_ref()
            .unwrap()
            .recorded_at,
        PROOF_AT
    );

    let mut malformed_proof = authorized.clone();
    malformed_proof
        .replacement_authority
        .as_mut()
        .unwrap()
        .recorded_at = "not-a-timestamp".into();
    refresh_replacement_authority_hash(&mut malformed_proof);
    assert!(
        replacement_authority_error(&malformed_proof, root, &fingerprint)
            .is_some_and(|error| error.contains("replacement_authority.recorded_at")
                && error.contains("RFC3339"))
    );

    let mut malformed_live = authorized.clone();
    malformed_live
        .replacement_authority
        .as_mut()
        .unwrap()
        .live_authority
        .recorded_at = "not-a-timestamp".into();
    refresh_replacement_authority_hash(&mut malformed_live);
    assert!(
        replacement_authority_error(&malformed_live, root, &fingerprint).is_some_and(|error| error
            .contains("live_authority.recorded_at")
            && error.contains("RFC3339"))
    );

    let mut reversed_live = authorized.clone();
    reversed_live
        .replacement_authority
        .as_mut()
        .unwrap()
        .live_authority
        .recorded_at = AFTER_PROOF.into();
    refresh_replacement_authority_hash(&mut reversed_live);
    assert!(
        replacement_authority_error(&reversed_live, root, &fingerprint).is_some_and(|error| error
            .contains("live_authority.recorded_at")
            && error.contains("不得晚于"))
    );

    let mut proof_after_goal = authorized.clone();
    proof_after_goal
        .replacement_authority
        .as_mut()
        .unwrap()
        .recorded_at = AFTER_PROOF.into();
    refresh_replacement_authority_hash(&mut proof_after_goal);
    assert!(
        replacement_authority_error(&proof_after_goal, root, &fingerprint)
            .is_some_and(|error| error.contains("goal.updated_at"))
    );

    let mut proof_before_baseline = authorized.clone();
    proof_before_baseline.baseline.as_mut().unwrap().recorded_at = AFTER_PROOF.into();
    proof_before_baseline.updated_at = AFTER_PROOF.into();
    refresh_replacement_authority_hash(&mut proof_before_baseline);
    assert!(
        replacement_authority_error(&proof_before_baseline, root, &fingerprint)
            .is_some_and(|error| error.contains("baseline.recorded_at"))
    );

    let mut proof_before_authority = authorized;
    proof_before_authority
        .replacement_authority
        .as_mut()
        .unwrap()
        .recorded_at = BEFORE_PROOF.into();
    refresh_replacement_authority_hash(&mut proof_before_authority);
    assert!(
        replacement_authority_error(&proof_before_authority, root, &fingerprint).is_some_and(
            |error| error.contains("lifecycle-only authority") && error.contains("不得晚于")
        )
    );
}

#[test]
fn current_predecessor_may_advance_after_replacement_authorization() {
    let (dir, store, predecessor, authorized) = authorized_lifecycle_only_replacement_fixture();
    let root = dir.path();
    let mut advanced = store.get(&predecessor.id).unwrap().unwrap();
    advanced.updated_at = "2099-05-01T00:00:00Z".into();
    write_json(&store.goal_path(&advanced.id).unwrap(), &advanced).unwrap();
    assert_eq!(advanced.lifecycle, GoalLifecycle::Current);
    assert!(advanced.current_schema_error().is_none());

    let fingerprint = workspace_fingerprint(root).unwrap();
    assert_eq!(
        replacement_authority_error(&authorized, root, &fingerprint),
        None
    );
}

#[test]
fn restored_current_replacement_allows_proof_before_updated_at() {
    const RESTORED_AT: &str = "2099-06-01T00:00:00Z";

    let (dir, store, predecessor, authorized) = authorized_lifecycle_only_replacement_fixture();
    let root = dir.path();
    store.supersede(&predecessor.id, &authorized.id).unwrap();
    let mut archived = store
        .archive(&authorized.id, "retire replacement before restore", false)
        .unwrap();
    let old_lifecycle_proof = archived.lifecycle_proof.take().unwrap();
    archived.updated_at = RESTORED_AT.into();
    archived.lifecycle_proof = Some(issue_lifecycle_proof_at(
        &archived,
        old_lifecycle_proof.workspace_fingerprint,
        old_lifecycle_proof.migration,
        old_lifecycle_proof.receipt_policy,
        RESTORED_AT.into(),
    ));
    write_json(&store.goal_path(&archived.id).unwrap(), &archived).unwrap();
    assert!(archived.lifecycle_proof_error(root).is_none());

    let restored = store.mark_current(&archived.id).unwrap();
    let proof_recorded_at = &restored.replacement_authority.as_ref().unwrap().recorded_at;
    assert!(
        plan_timestamp("replacement proof", proof_recorded_at).unwrap()
            < plan_timestamp("restored goal", &restored.updated_at).unwrap()
    );
    let fingerprint = workspace_fingerprint(root).unwrap();
    assert_eq!(
        replacement_authority_error(&restored, root, &fingerprint),
        None
    );
}

#[test]
fn supersession_read_side_rejects_predecessor_before_replacement_authority() {
    const FORGED_SUPERSESSION_AT: &str = "1970-01-01T00:00:00Z";

    let (dir, store, predecessor, authorized) = authorized_lifecycle_only_replacement_fixture();
    let root = dir.path();
    let mut forged = store.supersede(&predecessor.id, &authorized.id).unwrap();
    let old_lifecycle_proof = forged.lifecycle_proof.take().unwrap();
    forged.updated_at = FORGED_SUPERSESSION_AT.into();
    forged.lifecycle_proof = Some(issue_lifecycle_proof_at(
        &forged,
        old_lifecycle_proof.workspace_fingerprint,
        old_lifecycle_proof.migration,
        old_lifecycle_proof.receipt_policy,
        FORGED_SUPERSESSION_AT.into(),
    ));
    write_json(&store.goal_path(&forged.id).unwrap(), &forged).unwrap();
    assert!(forged.lifecycle_proof_error(root).is_none());

    let replacement = store.get(&authorized.id).unwrap().unwrap();
    let fingerprint = workspace_fingerprint(root).unwrap();
    assert!(
        replacement_authority_error(&replacement, root, &fingerprint)
            .is_some_and(|error| error.contains("supersession") && error.contains("不得早于"))
    );
    assert!(
        supersession_error(&forged, &[replacement], root, &fingerprint)
            .is_some_and(|error| error.contains("supersession") && error.contains("不得早于"))
    );
}
