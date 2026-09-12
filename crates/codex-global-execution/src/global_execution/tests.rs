use super::*;

#[test]
fn recovery_requests_require_commit_capability_and_original_intent() {
    let mut r = registration();
    let mut q = request(&r);
    q.operation = Operation::RecoverCommit {
        original_request_id: "0".repeat(32),
        candidate_sha256: "a".repeat(64),
    };
    validate_request(&q, &r, 1001).unwrap();
    assert!(Ledger::default().accept(&q, &r, 1001).is_err());
    let mut invalid = q.clone();
    if let Operation::RecoverCommit {
        original_request_id,
        ..
    } = &mut invalid.operation
    {
        *original_request_id = invalid.request_id.clone();
    }
    assert!(validate_request(&invalid, &r, 1001).is_err());
    if let Operation::RecoverCommit {
        original_request_id,
        candidate_sha256,
    } = &mut invalid.operation
    {
        *original_request_id = "../outside".into();
        *candidate_sha256 = "bad".into();
    }
    assert!(validate_request(&invalid, &r, 1001).is_err());
    r.capabilities.local_commit = false;
    r.capabilities.added_files = false;
    r.capabilities.deleted_files = false;
    q.registration_sha256 = r.digest().unwrap();
    assert!(validate_request(&q, &r, 1001).is_err());
}

pub(super) fn registration() -> Registration {
    Registration {
        schema_version: 1,
        project_id: "a".repeat(32),
        worktree_id: "b".repeat(32),
        owner_sid: "S-1-5-21-1-2-3-1001".into(),
        root_identity: "c".repeat(64),
        git_common_identity: "d".repeat(64),
        object_id_length: 40,
        policy_sha256: "e".repeat(64),
        capabilities: Capabilities {
            local_commit: true,
            added_files: true,
            deleted_files: true,
            formal_state: true,
            install_adapters: ["rayman".into()].into(),
        },
    }
}
fn version(c: char) -> FileVersion {
    FileVersion {
        raw_sha256: c.to_string().repeat(64),
        git_blob_oid: c.to_string().repeat(40),
        mode: 0o100644,
    }
}
pub(super) fn request(r: &Registration) -> Request {
    Request {
        schema_version: 1,
        request_id: "f".repeat(32),
        project_id: r.project_id.clone(),
        worktree_id: r.worktree_id.clone(),
        registration_sha256: r.digest().unwrap(),
        source_sha256: "1".repeat(64),
        created_at: 1000,
        expires_at: 1120,
        operation: Operation::Commit {
            expected_head: "2".repeat(40),
            expected_index_sha256: "3".repeat(64),
            message: "修复跨项目提交".into(),
            changes: vec![
                Change {
                    path: "src/added.rs".into(),
                    before: None,
                    after: Some(version('4')),
                },
                Change {
                    path: "src/deleted.rs".into(),
                    before: Some(version('5')),
                    after: None,
                },
                Change {
                    path: "src/modified.rs".into(),
                    before: Some(version('6')),
                    after: Some(version('7')),
                },
            ],
            hook_receipt: None,
        },
    }
}

#[test]
fn exact_added_modified_deleted_commit_is_valid_and_canonical() {
    let r = registration();
    let q = request(&r);
    validate_request(&q, &r, 1000).unwrap();
    let compact = serde_json::to_vec(&q).unwrap();
    let pretty = serde_json::to_vec_pretty(&q).unwrap();
    assert_eq!(
        decode_request(&compact).unwrap().digest().unwrap(),
        decode_request(&pretty).unwrap().digest().unwrap()
    );
    assert_eq!(
        decode_request(&pretty).unwrap().digest().unwrap(),
        q.digest().unwrap()
    );
}

#[test]
fn project_worktree_and_protected_policy_substitution_are_rejected() {
    let r = registration();
    let q = request(&r);
    let mut other = r.clone();
    other.project_id = "9".repeat(32);
    assert!(validate_request(&q, &other, 1000).is_err());
    other = r.clone();
    other.worktree_id = "9".repeat(32);
    assert!(validate_request(&q, &other, 1000).is_err());
    other = r.clone();
    other.policy_sha256 = "9".repeat(64);
    assert!(validate_request(&q, &other, 1000).is_err());
    other = r.clone();
    other.owner_sid = "S-1-5-21-1-2-3-1002".into();
    assert!(validate_request(&q, &other, 1000).is_err());
}

#[test]
fn expiry_future_and_integer_overflow_fail_closed() {
    let r = registration();
    let q = request(&r);
    assert!(validate_request(&q, &r, 1120).is_err());
    assert!(validate_request(&q, &r, 969).is_err());
    validate_request(&q, &r, 970).unwrap();
    for (created, expires) in [
        (1000, 1000),
        (1000, 1121),
        (i64::MIN, i64::MAX),
        (i64::MAX, i64::MIN),
    ] {
        let mut bad = q.clone();
        bad.created_at = created;
        bad.expires_at = expires;
        assert!(validate_request(&bad, &r, 1000).is_err());
    }
}

#[test]
fn duplicate_case_alias_unknown_fields_and_arbitrary_execution_are_rejected() {
    let r = registration();
    let value = serde_json::to_value(request(&r)).unwrap();
    let original = serde_json::to_string(&value).unwrap();
    for extra in [
        "\"schema_version\":1,",
        "\"SCHEMA_VERSION\":1,",
        "\"argv\":[\"whoami\"],",
        "\"program\":\"pwsh\",",
    ] {
        let text = format!("{{{extra}{}", &original[1..]);
        assert!(decode_request(text.as_bytes()).is_err(), "{extra}");
    }
    for key in ["argv", "program", "repository_root", "script"] {
        let mut bad = value.clone();
        bad["operation"][key] = "ignored".into();
        assert!(
            decode_request(&serde_json::to_vec(&bad).unwrap()).is_err(),
            "{key}"
        );
    }
    let mut bad = value;
    bad["operation"]["kind"] = "execute".into();
    assert!(decode_request(&serde_json::to_vec(&bad).unwrap()).is_err());
    assert!(decode_request(b"{\"x\":{\"a\":1,\"A\":2}}").is_err());
    assert!(decode_request(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
    assert!(decode_request(b"\xef\xbb\xbf{}").is_err());
    assert!(decode_request(b"{} {}").is_err());
}

#[test]
fn windows_aliases_traversal_and_managed_state_paths_are_rejected() {
    for path in [
        "",
        "/a",
        "a/",
        "a//b",
        "../a",
        "a/../b",
        "a\\b",
        "a:b",
        "C:/a",
        "a\0b",
        "a\nb",
        "a.",
        "a ",
        "A/NUL.txt",
        "COM1",
        "lpt².txt",
        "a/.GIT/config",
        ".agent-checkpoints/a",
        ".RaymanCodingSkill/goals/a",
        "a/*",
    ] {
        assert!(validate_relative_path(path).is_err(), "{path:?}");
    }
    for path in ["src/模块.rs", ".editorconfig", "docs/COM10.md", "a/b.c"] {
        validate_relative_path(path).unwrap();
    }
}

#[test]
fn case_collisions_order_empty_and_nonordinary_changes_fail_closed() {
    let r = registration();
    for case in 0..6 {
        let mut q = request(&r);
        let Operation::Commit { changes, .. } = &mut q.operation else {
            unreachable!()
        };
        match case {
            0 => changes.swap(0, 1),
            1 => {
                changes[0].path = "A.rs".into();
                changes[1].path = "a.rs".into();
            }
            2 => changes.clear(),
            3 => changes[0].after = None,
            4 => changes[0].after.as_mut().unwrap().mode = 0o120000,
            5 => changes[2].after = changes[2].before.clone(),
            _ => unreachable!(),
        }
        assert!(validate_request(&q, &r, 1000).is_err(), "case {case}");
    }
}

#[test]
fn capability_restrictions_apply_even_with_a_fresh_registration_hash() {
    for kind in 0..3 {
        let mut r = registration();
        match kind {
            0 => {
                r.capabilities.local_commit = false;
                r.capabilities.added_files = false;
                r.capabilities.deleted_files = false;
            }
            1 => r.capabilities.added_files = false,
            2 => r.capabilities.deleted_files = false,
            _ => unreachable!(),
        }
        assert!(validate_request(&request(&r), &r, 1000).is_err());
    }
}

#[test]
fn formal_state_and_installation_use_typed_enrolled_operations() {
    let r = registration();
    let mut q = request(&r);
    q.operation = Operation::State {
        task_id: "8".repeat(32),
        expected_revision: 0,
        mutation: StateMutation::Begin {
            title: "维护任务".into(),
        },
    };
    validate_request(&q, &r, 1000).unwrap();
    let Operation::State {
        expected_revision, ..
    } = &mut q.operation
    else {
        unreachable!()
    };
    *expected_revision = 1;
    assert!(validate_request(&q, &r, 1000).is_err());
    q.operation = Operation::Install {
        adapter_id: "rayman".into(),
        artifact_sha256: "a".repeat(64),
    };
    validate_request(&q, &r, 1000).unwrap();
    q.operation = Operation::Install {
        adapter_id: "unregistered".into(),
        artifact_sha256: "a".repeat(64),
    };
    assert!(validate_request(&q, &r, 1000).is_err());
}

#[test]
fn state_cas_replay_and_project_namespaces_preserve_other_tasks() {
    let r = registration();
    let mut q = request(&r);
    q.operation = Operation::State {
        task_id: "8".repeat(32),
        expected_revision: 0,
        mutation: StateMutation::Begin {
            title: "原始任务".into(),
        },
    };
    let original = Ledger::default();
    let first = original.accept(&q, &r, 1000).unwrap();
    assert!(original.tasks.is_empty());
    assert_eq!(first.next.revision, 1);
    assert!(!first.execute_effect);
    let replay = first.next.accept(&q, &r, 1001).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.next.revision, 1);
    assert!(first.next.accept(&q, &r, 99999).unwrap().replayed);
    assert!(original.accept(&q, &r, 99999).is_err());
    assert_eq!(
        replay.result,
        RecordedResult::StateApplied { task_revision: 1 }
    );
    let mut drift = q.clone();
    drift.source_sha256 = "9".repeat(64);
    assert!(first.next.accept(&drift, &r, 1001).is_err());
    q.request_id = "1".repeat(32);
    q.operation = Operation::State {
        task_id: "8".repeat(32),
        expected_revision: 1,
        mutation: StateMutation::Progress {
            stage: "build".into(),
            message: "编译中".into(),
        },
    };
    let second = first.next.accept(&q, &r, 1001).unwrap();
    q.request_id = "2".repeat(32);
    assert!(second.next.accept(&q, &r, 1002).is_err());
    assert_eq!(second.next.tasks.values().next().unwrap().revision, 2);
    let mut other = r.clone();
    other.project_id = "9".repeat(32);
    let mut other_q = request(&other);
    other_q.request_id = "3".repeat(32);
    other_q.operation = Operation::State {
        task_id: "8".repeat(32),
        expected_revision: 0,
        mutation: StateMutation::Begin {
            title: "独立项目".into(),
        },
    };
    let third = second.next.accept(&other_q, &other, 1002).unwrap();
    assert_eq!(third.next.tasks.len(), 2);
    assert_eq!(second.next.tasks.len(), 1);
}

#[test]
fn interrupted_effect_never_runs_again_and_terminal_result_is_immutable() {
    let r = registration();
    let q = request(&r);
    let first = Ledger::default().accept(&q, &r, 1000).unwrap();
    assert!(first.execute_effect);
    let crash_bytes = serde_json::to_vec(&first.next).unwrap();
    let restored: Ledger = serde_json::from_slice(&crash_bytes).unwrap();
    let retry = restored.accept(&q, &r, 1001).unwrap();
    assert!(!retry.execute_effect);
    assert_eq!(retry.result, RecordedResult::RecoveryRequired);
    let outcome = RecordedResult::EffectSucceeded {
        outcome_sha256: "9".repeat(64),
    };
    assert!(
        restored
            .record_effect_result(&q.request_id, &"0".repeat(64), outcome.clone())
            .is_err()
    );
    let completed = restored
        .record_effect_result(&q.request_id, &q.digest().unwrap(), outcome.clone())
        .unwrap();
    assert_eq!(completed.accept(&q, &r, 1001).unwrap().result, outcome);
    assert_eq!(
        completed
            .record_effect_result(&q.request_id, &q.digest().unwrap(), outcome)
            .unwrap()
            .revision,
        completed.revision
    );
    assert!(
        completed
            .record_effect_result(
                &q.request_id,
                &q.digest().unwrap(),
                RecordedResult::EffectFailed {
                    error_code: "overwrite".into()
                }
            )
            .is_err()
    );
}

#[test]
fn stage_reuse_binds_environment_tools_scope_policy_and_artifacts() {
    let binding = StageBinding {
        registration_sha256: "1".repeat(64),
        source_sha256: "2".repeat(64),
        stage: "source_fresh".into(),
        scope_sha256: "3".repeat(64),
        policy_sha256: "4".repeat(64),
        environment_sha256: "5".repeat(64),
        toolchain_sha256: "6".repeat(64),
        artifacts: [("cli".into(), "7".repeat(64))].into(),
    };
    let digest = binding.digest().unwrap();
    for case in 0..7 {
        let mut other = binding.clone();
        match case {
            0 => other.registration_sha256 = "a".repeat(64),
            1 => other.source_sha256 = "a".repeat(64),
            2 => other.scope_sha256 = "a".repeat(64),
            3 => other.policy_sha256 = "a".repeat(64),
            4 => other.environment_sha256 = "a".repeat(64),
            5 => other.toolchain_sha256 = "a".repeat(64),
            6 => {
                other.artifacts.insert("cli".into(), "a".repeat(64));
            }
            _ => unreachable!(),
        }
        assert_ne!(digest, other.digest().unwrap());
    }
}

#[test]
fn multi_project_preflight_is_activation_exempt_and_never_claims_authority() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let paths = vec![first.path().to_path_buf(), second.path().to_path_buf()];
    let report = preflight(&paths, false).unwrap();
    assert_eq!(report.projects.len(), 2);
    assert!(!report.privileged_operations_executed);
    assert!(
        report
            .projects
            .iter()
            .all(|p| !p.commit_ready && !p.installation_ready && !p.formal_state.probed)
    );
    assert!(!first.path().join(".RaymanCodingSkill").exists());
    assert!(!second.path().join(".RaymanCodingSkill").exists());
    assert!(preflight(&[first.path().to_path_buf(), first.path().join(".")], false).is_err());
    assert!(preflight(&[], false).is_err());
}

#[cfg(windows)]
#[test]
fn reenrollment_preserves_registered_install_adapter_policies() {
    let root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let context = crate::execution_context::execution_context_probe();
    let sid = context.principal_sid.unwrap();
    super::native::protect_fixture_directory(root.path(), &sid, "").unwrap();
    let pin = ProtectedDirectory::open(root.path(), &sid).unwrap();
    std::fs::write(root.path().join("worker.exe"), b"fixture worker").unwrap();
    std::fs::write(root.path().join("client.exe"), b"fixture client").unwrap();
    let install = Installation {
        schema_version: 1,
        installation_id: "a".repeat(32),
        owner_sid: sid,
        root_identity: pin.identity().into(),
        source_project_id: "c".repeat(32),
        worker_sha256: crate::hash::sha256_bytes(b"fixture worker"),
        client_sha256: crate::hash::sha256_bytes(b"fixture client"),
    };
    crate::file_io::write_json(&root.path().join("installation.json"), &install).unwrap();
    let first = enroll(root.path(), workspace.path(), None, true, true).unwrap();
    let target = std::path::PathBuf::from(context.token_profile.unwrap())
        .join("AppData/Local/rayman-enrollment-fixture-never-created.bin");
    let spec = root.path().join("fixture-adapter.json");
    crate::file_io::write_json(
        &spec,
        &serde_json::json!({"adapter_id":"fixture","targets":{"output":target}}),
    )
    .unwrap();
    let adapter = register_install_adapter(root.path(), workspace.path(), &spec).unwrap();
    let registry_path = root
        .path()
        .join(format!("project-{}.json", first.registration.worktree_id));
    let before = std::fs::read(&registry_path).unwrap();
    for publish in [false, true] {
        let repeated = enroll(root.path(), workspace.path(), None, true, publish).unwrap();
        assert_eq!(
            repeated.install_policies.get("fixture"),
            Some(&adapter.digest().unwrap())
        );
        assert_eq!(before, std::fs::read(&registry_path).unwrap());
    }
    assert!(enroll(root.path(), workspace.path(), None, false, false).is_err());
    assert_eq!(before, std::fs::read(&registry_path).unwrap());
}
