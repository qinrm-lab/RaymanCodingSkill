use super::*;

use sha2::{Digest, Sha256};

fn forged_authority_gate_binding_sha256(
    policy: &str,
    entrypoint: &str,
    dependency_sha256: &BTreeMap<String, String>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"rayman.authority-gate-binding.v1");
    hasher.update([0]);
    hasher.update(policy.as_bytes());
    hasher.update([0]);
    hasher.update(entrypoint.as_bytes());
    hasher.update([0]);
    for (path, hash) in dependency_sha256 {
        hasher.update(path.as_bytes());
        hasher.update([0]);
        hasher.update(hash.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn receipt(goal: &Goal, root: &Path, command: &str) -> AuthorityReceipt {
    let fingerprint = workspace_fingerprint(root).unwrap();
    let impact_scopes = Vec::new();
    AuthorityReceipt {
        requirement_id: "req_1".into(),
        command: command.into(),
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint.clone(),
        repeat: 2,
        impact_scopes: impact_scopes.clone(),
        non_code: true,
        workspace_snapshot: false,
        invocation_sha256: authority_invocation_sha256(command, "req_1", 2, &impact_scopes, true),
        contract_sha256: validation_contract_sha256(goal, "req_1").unwrap(),
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

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
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
    fs::write(root.join("first.rs"), "pub fn first() -> i32 { 1 }\n").unwrap();
    let later_fingerprint = workspace_fingerprint(root).unwrap();
    assert_ne!(later_fingerprint, fingerprint);
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
fn lifecycle_only_replacement_read_side_rejects_workspace_drift() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    let authority = archived_direct_authority_success(&store, root);
    let predecessor = store
        .start("planned predecessor", &[("preserve delta".into(), true)])
        .unwrap();
    let predecessor = store
        .record_plan(
            &predecessor.id,
            PlanReceiptSubmission {
                changed_paths: vec!["lib.rs".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["lib.rs".into()],
                recommended_checks: vec!["cargo test --workspace --all-targets".into()],
            },
        )
        .unwrap();
    let replacement = store
        .start("replacement", &[("preserve delta".into(), true)])
        .unwrap();
    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 1 }\n").unwrap();
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
    let authorized_fingerprint = workspace_fingerprint(root).unwrap();

    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 2 }\n").unwrap();
    assert!(
        replacement_authority_error(&authorized, root, &authorized_fingerprint)
            .is_some_and(|error| error.contains("fingerprint")),
        "a replacement proof must fail closed when indexed bytes drift"
    );
}

#[test]
fn lifecycle_only_replacement_rejects_final_source_drift_without_writing_proof() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    let authority = archived_direct_authority_success(&store, root);
    let predecessor = store
        .start("planned predecessor", &[("preserve delta".into(), true)])
        .unwrap();
    let predecessor = store
        .record_plan(
            &predecessor.id,
            PlanReceiptSubmission {
                changed_paths: vec!["lib.rs".into()],
                review_priority: "normal".into(),
                impacted_paths: vec!["lib.rs".into()],
                recommended_checks: vec!["cargo test --workspace --all-targets".into()],
            },
        )
        .unwrap();
    let replacement = store
        .start("replacement", &[("preserve delta".into(), true)])
        .unwrap();
    fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 3 }\n").unwrap();
    let replacement_path = root
        .join(GOALS_DIR)
        .join(format!("{}.json", replacement.id));
    let before = fs::read(&replacement_path).unwrap();
    let live_authority = live_replacement_authority(
        root,
        &replacement.id,
        std::slice::from_ref(&predecessor.id),
        &authority.id,
    );

    let error = store
        .authorize_replacement_with_before_confirm(
            &replacement.id,
            std::slice::from_ref(&predecessor.id),
            &authority.id,
            live_authority,
            || {
                fs::write(root.join("lib.rs"), "pub fn value() -> i32 { 4 }\n").unwrap();
            },
        )
        .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("workspace changed before lifecycle-only replacement proof publication")
    );
    assert_eq!(fs::read(&replacement_path).unwrap(), before);
    assert!(
        store
            .get(&replacement.id)
            .unwrap()
            .unwrap()
            .replacement_authority
            .is_none()
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

#[test]
fn captured_replacement_recomputes_the_complete_powershell_gate_binding() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let store = GoalStore::new(root);
    write(
        root,
        "scripts/check-repo.ps1",
        ". (Join-Path $PSScriptRoot 'helper.ps1')\n",
    );
    write(root, "scripts/helper.ps1", "Write-Output 'real helper'\n");
    let command = "pwsh -NoProfile -File scripts/check-repo.ps1";
    let authority = archived_direct_authority_success_for_command(&store, root, command);
    let predecessor = store
        .start(
            "unfinished predecessor",
            &[("preserve exact authority".into(), true)],
        )
        .unwrap();
    let replacement = store
        .start("replacement", &[("preserve exact authority".into(), true)])
        .unwrap();
    let predecessor_ids = vec![predecessor.id.clone()];
    let fingerprint = workspace_fingerprint(root).unwrap();
    let live = ReplacementAuthorityReceipt {
        command: command.into(),
        command_rebind: None,
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint.clone(),
        repeat: 2,
        invocation_sha256: replacement_authority_invocation_sha256(
            command,
            &replacement.id,
            &authority.id,
            &predecessor_ids,
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
    };
    let authorized = store
        .authorize_replacement(&replacement.id, &predecessor_ids, &authority.id, live)
        .unwrap();
    let current = workspace_baseline(root).unwrap();
    let captured_files = current
        .files
        .keys()
        .map(|key| (key.clone(), fs::read(root.join(key)).unwrap()))
        .collect::<BTreeMap<_, _>>();
    let source = crate::source_state::inspect(root);
    let maintenance_artifact_hashes = BTreeMap::new();
    let workspace_identity = workspace_identity(root);
    let decision = GoalDecisionContext::captured_with_readiness_state(
        root,
        Some(&current),
        &captured_files,
        &source,
        &maintenance_artifact_hashes,
        &workspace_identity,
    );
    let goals = store.list().unwrap();
    assert_eq!(
        replacement_authority_error_with_context(&authorized, &decision, &goals),
        None
    );

    // The serialized binding is internally well-formed and still matches the
    // authority baseline, but it omits the literal helper from the real gate
    // closure.  Only a capture-only recomputation can detect this.
    let mut forged = authorized.clone();
    let binding = forged
        .replacement_authority
        .as_mut()
        .unwrap()
        .authority_gate_binding
        .as_mut()
        .unwrap();
    let entrypoint_hash = binding
        .dependency_sha256
        .get(&binding.entrypoint)
        .cloned()
        .unwrap();
    binding.dependency_sha256 = BTreeMap::from([(binding.entrypoint.clone(), entrypoint_hash)]);
    binding.binding_sha256 = forged_authority_gate_binding_sha256(
        &binding.policy,
        &binding.entrypoint,
        &binding.dependency_sha256,
    );
    let proof_sha256 =
        replacement_authority_proof_sha256(forged.replacement_authority.as_ref().unwrap());
    forged.replacement_authority.as_mut().unwrap().proof_sha256 = proof_sha256;
    assert!(
        replacement_authority_error_with_context(&forged, &decision, &goals)
            .is_some_and(|error| error.contains("captured authority gate closure")),
        "a well-formed incomplete binding must not survive a captured readiness decision"
    );
}

#[test]
fn changed_constant_success_gate_cannot_be_its_own_only_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "scripts/check-repo.ps1", "throw 'real gate'\n");
    let store = GoalStore::new(root);
    let mut goal = store
        .start("repair gate", &[("keep gate honest".into(), true)])
        .unwrap();

    write(root, "scripts/check-repo.ps1", "exit 0\n");
    let command = "pwsh -NoProfile -File scripts/check-repo.ps1";
    goal.authority_receipts.push(receipt(&goal, root, command));
    let fingerprint = workspace_fingerprint(root).unwrap();

    assert!(!has_current_stable_authority_receipt(
        &goal,
        root,
        &fingerprint
    ));
}

#[test]
fn weakening_a_literal_gate_dependency_invalidates_the_gate_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "scripts/check-repo.ps1",
        ". (Join-Path $PSScriptRoot 'repository-quality.ps1')\n",
    );
    write(
        root,
        "scripts/repository-quality.ps1",
        "throw 'quality failure'\n",
    );
    let store = GoalStore::new(root);
    let mut goal = store
        .start("repair helper", &[("keep helper honest".into(), true)])
        .unwrap();

    write(root, "scripts/repository-quality.ps1", "return\n");
    let command = "pwsh -NoProfile -File scripts/check-repo.ps1";
    goal.authority_receipts.push(receipt(&goal, root, command));
    let fingerprint = workspace_fingerprint(root).unwrap();

    assert!(!has_current_stable_authority_receipt(
        &goal,
        root,
        &fingerprint
    ));
}

#[test]
fn receipt_binding_tracks_real_dynamic_helpers_without_freezing_fixtures() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "scripts/check-repo.ps1",
        "$scripts = @('real-helper.ps1', 'missing-fixture.ps1', 'late-helper.ps1')\nforeach ($scriptName in $scripts) { & (Join-Path $PSScriptRoot $scriptName) }\n",
    );
    write(root, "scripts/real-helper.ps1", "Write-Output 'real'\n");
    let store = GoalStore::new(root);
    let goal = store
        .start("bind repository gate", &[("prove repository".into(), true)])
        .unwrap();

    write(root, "scripts/late-helper.ps1", "Write-Output 'late'\n");
    let command = "pwsh -NoProfile -File scripts/check-repo.ps1";
    let binding = authority_gate_binding_for_goal(&goal, root, command)
        .unwrap()
        .unwrap();

    assert!(
        binding
            .dependency_sha256
            .contains_key("scripts/check-repo.ps1")
    );
    assert!(
        binding
            .dependency_sha256
            .contains_key("scripts/real-helper.ps1")
    );
    assert!(
        !binding
            .dependency_sha256
            .contains_key("scripts/missing-fixture.ps1")
    );
    assert!(
        !binding
            .dependency_sha256
            .contains_key("scripts/late-helper.ps1")
    );

    let error = validate_authority_command_for_goal(root, &goal, command).unwrap_err();
    assert!(
        error.to_string().contains("scripts/late-helper.ps1"),
        "unexpected authority error: {error:#}"
    );
}

#[test]
fn captured_authority_gate_uses_only_the_captured_gate_and_helper_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "scripts/check-repo.ps1",
        ". (Join-Path $PSScriptRoot 'helper.ps1')\n",
    );
    write(root, "scripts/helper.ps1", "throw 'original'\n");
    let store = GoalStore::new(root);
    let goal = store
        .start(
            "preserve gate independence",
            &[("authority stays honest".into(), true)],
        )
        .unwrap();
    let captured_files = BTreeMap::from([
        (
            "scripts/check-repo.ps1".to_string(),
            fs::read(root.join("scripts/check-repo.ps1")).unwrap(),
        ),
        (
            "scripts/helper.ps1".to_string(),
            fs::read(root.join("scripts/helper.ps1")).unwrap(),
        ),
    ]);
    let current = WorkspaceBaseline {
        recorded_at: now_iso(),
        workspace_fingerprint: fingerprint_for_files(
            &captured_files
                .iter()
                .map(|(key, bytes)| (key.clone(), crate::hash::sha256_bytes(bytes)))
                .collect(),
        ),
        files: captured_files
            .iter()
            .map(|(key, bytes)| (key.clone(), crate::hash::sha256_bytes(bytes)))
            .collect(),
    };
    let decision = GoalDecisionContext::captured(root, Some(&current), &captured_files);
    let command = "pwsh -NoProfile -File scripts/check-repo.ps1";

    // The live helper becomes harmless, but the captured decision remains
    // anchored to the earlier bytes and must not reopen the live path.
    write(root, "scripts/helper.ps1", "return\n");
    assert!(validate_authority_command_for_goal_with_context(&decision, &goal, command).is_ok());
    assert!(validate_authority_command_for_goal(root, &goal, command).is_err());
}

#[test]
fn authority_gate_rejects_aliases_before_live_or_captured_resolution() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "scripts/check-repo.ps1", "exit 0\n");
    let captured_files = BTreeMap::from([(
        "scripts/check-repo.ps1".to_string(),
        fs::read(root.join("scripts/check-repo.ps1")).unwrap(),
    )]);
    let decision = GoalDecisionContext::captured(root, None, &captured_files);
    for script in [
        "./scripts/check-repo.ps1",
        "scripts/../scripts/check-repo.ps1",
        "scripts\\check-repo.ps1",
        "SCRIPTS/check-repo.ps1",
        "scripts/check-repo.ps1:stream",
        "scripts/check-repo.ps1 ",
        "C:/work/scripts/check-repo.ps1",
        "\\\\server\\share\\check-repo.ps1",
        "\\\\?\\C:\\work\\scripts\\check-repo.ps1",
    ] {
        let command = format!("pwsh -NoProfile -File '{script}'");
        assert!(
            validate_authority_command_with_context(&decision, &command).is_err(),
            "captured alias must fail: {script}"
        );
        assert!(
            validate_authority_command(root, &command).is_err(),
            "live alias must fail: {script}"
        );
    }
}

#[test]
fn absolute_workspace_powershell_script_is_execution_safe_but_not_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "scripts/check-repo.ps1", "exit 0\n");
    let script = root.join("scripts/check-repo.ps1");
    let command = format!("pwsh -NoProfile -File \"{}\"", script.display());
    let parsed = parse_validation_command(&command).unwrap();

    let resolved = resolve_live_powershell_script(root, &parsed)
        .unwrap()
        .expect("PowerShell file mode must resolve its workspace-local script");
    assert_eq!(resolved.logical_key, "scripts/check-repo.ps1");
    assert!(validate_command_security(root, &parsed).is_ok());
    assert!(
        validate_authority_command(root, &command).is_err(),
        "an absolute execution path must not borrow the strict authority identity"
    );
}

#[test]
fn captured_authority_rejects_nonexecuting_test_modes() {
    let root = tempfile::tempdir().unwrap();
    let captured_files = BTreeMap::new();
    let decision = GoalDecisionContext::captured(root.path(), None, &captured_files);
    for command in [
        "cargo test --workspace -- --list",
        "python -m pytest --collect-only",
    ] {
        assert!(
            validate_authority_command_with_context(&decision, command).is_err(),
            "captured authority must reject nonexecuting test mode: {command}"
        );
    }
}

#[test]
fn selector_free_locked_workspace_cargo_test_remains_independent_authority() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "scripts/check-repo.ps1", "throw 'real gate'\n");
    let store = GoalStore::new(root);
    let mut goal = store
        .start("repair gate", &[("keep gate honest".into(), true)])
        .unwrap();
    write(root, "scripts/check-repo.ps1", "exit 0\n");

    let command = "cargo test --locked --workspace --all-targets";
    assert!(validate_authority_command(root, command).is_ok());
    goal.authority_receipts.push(receipt(&goal, root, command));
    let fingerprint = workspace_fingerprint(root).unwrap();
    assert!(has_current_stable_authority_receipt(
        &goal,
        root,
        &fingerprint
    ));
}

#[test]
fn cargo_authority_rejects_package_exclude_and_libtest_selectors() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        "cargo test --workspace -p rayman",
        "cargo test --workspace -prayman",
        "cargo test --workspace --package rayman",
        "cargo test --workspace --package=rayman",
        "cargo test --workspace --exclude rayman",
        "cargo test --workspace --exclude=rayman",
        "cargo test --workspace goal::tests",
        "cargo test --workspace -- goal::tests",
        "cargo test --workspace -- --skip goal::tests",
        "cargo test --workspace -- --exact",
        "cargo test --workspace -- --ignored",
    ] {
        assert!(
            validate_authority_command(root.path(), command).is_err(),
            "narrow authority must be rejected: {command}"
        );
    }
}

#[test]
fn cargo_authority_accepts_selector_free_options_and_does_not_parse_libtest_as_cargo() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        "cargo test --workspace",
        "cargo test --locked --workspace --all-targets",
        "cargo +stable test --all --all-targets --locked",
        "cargo test --workspace -- --test-threads 1",
        "cargo test --workspace -- --format terse",
        "cargo test --workspace -- -prayman",
    ] {
        assert!(
            validate_authority_command(root.path(), command).is_ok(),
            "selector-free authority must remain accepted: {command}"
        );
    }
}

#[test]
fn goal_bound_preflight_rejects_before_a_changed_gate_can_execute() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "scripts/check-repo.ps1", "throw 'real gate'\n");
    let store = GoalStore::new(root);
    let goal = store
        .start("repair gate", &[("keep gate honest".into(), true)])
        .unwrap();
    write(
        root,
        "scripts/check-repo.ps1",
        "[IO.File]::WriteAllText((Join-Path $PSScriptRoot '../gate-ran.txt'), 'ran')\nexit 0\n",
    );

    let rejected = validate_authority_command_for_goal(
        root,
        &goal,
        "pwsh -NoProfile -File scripts/check-repo.ps1",
    )
    .unwrap_err();
    assert!(
        rejected
            .to_string()
            .contains("refusing a self-validating authority gate")
    );
    assert!(rejected.to_string().contains("scripts/check-repo.ps1"));
    assert!(!root.join("gate-ran.txt").exists());
}

#[test]
fn receipt_store_rechecks_goal_bound_gate_independence() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "scripts/check-repo.ps1", "throw 'real gate'\n");
    let store = GoalStore::new(root);
    let goal = store
        .start("repair gate", &[("keep gate honest".into(), true)])
        .unwrap();
    write(root, "scripts/check-repo.ps1", "exit 0\n");

    let command = "pwsh -NoProfile -File scripts/check-repo.ps1";
    let impacts = vec![impact("scripts/check-repo.ps1")];
    let impact_scopes = validation_scopes_for_impacts(&impacts);
    let fingerprint = workspace_fingerprint(root).unwrap();
    let contract_sha256 = validation_contract_sha256(&goal, "req_1").unwrap();
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
        contract_sha256: contract_sha256.clone(),
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
    let validation_receipt = ValidationReceipt {
        exit_code: 0,
        cwd: root.display().to_string(),
        workspace_identity: workspace_identity(root),
        workspace_fingerprint_before: fingerprint.clone(),
        workspace_fingerprint_after: fingerprint,
        stdout_sha256: "a".repeat(64),
        stderr_sha256: "b".repeat(64),
        invocation_sha256: validation_invocation_sha256_scoped(command, &impact_scopes, false),
        passed_tests: None,
        listed_tests: None,
        ignored_tests: None,
        list_stdout_sha256: None,
        list_stderr_sha256: None,
        contract_sha256,
    };

    let rejected = store
        .record_authority_validation_receipt(
            &goal.id,
            "req_1",
            AuthorityReceiptSubmission {
                validation: ValidationReceiptSubmission {
                    evidence: "forged direct receipt".into(),
                    command: command.into(),
                    receipt: validation_receipt,
                    impacts,
                    non_code: false,
                },
                authority,
            },
        )
        .unwrap_err();
    assert!(
        rejected
            .to_string()
            .contains("refusing a self-validating authority gate")
    );
    let unchanged = store.get(&goal.id).unwrap().unwrap();
    assert_eq!(unchanged.requirements[0].status, RequirementStatus::Open);
    assert!(unchanged.requirements[0].validations.is_empty());
    assert!(unchanged.authority_receipts.is_empty());
}
