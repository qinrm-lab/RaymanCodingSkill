use super::*;

#[test]
fn context_refresh_reports_full_hashing_and_legacy_content_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/a.rs", "pub fn a() {}");
    write(root, "src/b.rs", "pub fn b() {}");

    let first = run_json(root, &["context", "refresh"]);
    assert_eq!(first["total"], 2);
    assert_eq!(first["files_hashed"], 4);
    assert_eq!(first["bytes_hashed"], 52);
    assert_eq!(first["content_changed"], 2);
    assert_eq!(first["content_unchanged"], 0);
    // Legacy names remain content-classification aliases for one migration
    // window. They are not evidence that a strong-hash round was skipped.
    assert_eq!(first["rehashed"], 2);
    assert_eq!(first["reused"], 0);

    // Unchanged content still receives both strong-hash capture rounds.
    let second = run_json(root, &["context", "refresh"]);
    assert_eq!(second["files_hashed"], 4);
    assert_eq!(second["bytes_hashed"], 52);
    assert_eq!(second["content_unchanged"], 2);
    assert_eq!(second["content_changed"], 0);
    assert_eq!(second["reused"], 2);
    assert_eq!(second["rehashed"], 0);

    // One changed file changes only the content classification, not hash work.
    write(root, "src/a.rs", "pub fn a() { /* changed */ }");
    let third = run_json(root, &["context", "refresh"]);
    assert_eq!(third["files_hashed"], 4);
    assert_eq!(third["content_changed"], 1);
    assert_eq!(third["content_unchanged"], 1);
    assert_eq!(third["rehashed"], 1);
    assert_eq!(third["reused"], 1);
}

#[test]
fn standard_check_reads_legacy_goal_schema_and_blocks_missing_validation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/goal_legacy.json",
        r#"{
  "id": "goal_legacy",
  "contract": {
    "goal": "legacy goal",
    "requirements": [
      {
        "id": "req_1",
        "priority": "must",
        "text": "legacy requirement",
        "status": "satisfied",
        "evidence": "claimed done",
        "validation_commands": []
      }
    ],
    "verification": [],
    "created_at": "2026-01-01T00:00:00Z"
  },
  "status": "success",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z"
}"#,
    );
    run_json(root, &["context", "refresh"]);

    let listed = run_json(root, &["goal", "list"]);
    assert_eq!(listed[0]["id"], "goal_legacy");
    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_legacy"],
    );
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("legacy goal goal_legacy")
            && standard.stdout.contains("仍为 current"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_legacy_goal_level_verification_without_a_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/goal_legacy.json",
        r#"{
  "id": "goal_legacy",
  "contract": {
    "goal": "legacy goal",
    "requirements": [
      {
        "id": "req_1",
        "priority": "must",
        "text": "legacy requirement",
        "status": "satisfied",
        "evidence": "claimed done",
        "validation_commands": []
      }
    ],
    "verification": ["cargo test --all"],
    "created_at": "2026-01-01T00:00:00Z"
  },
  "status": "success",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z"
}"#,
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_legacy"],
    );
    assert_eq!(standard.status, 1, "stdout={}", standard.stdout);
    assert!(
        standard.stdout.contains("legacy goal goal_legacy")
            && standard.stdout.contains("仍为 current"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn legacy_goal_mutation_remains_legacy_history_after_writeback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/goal_legacy_active.json",
        r#"{
  "id": "goal_legacy_active",
  "contract": {
    "goal": "legacy active goal",
    "requirements": [
      {
        "id": "req_1",
        "priority": "must",
        "text": "record legacy evidence",
        "status": "open",
        "validation_commands": []
      }
    ],
    "created_at": "2026-01-01T00:00:00Z"
  },
  "status": "active",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z"
}"#,
    );
    run_json(root, &["context", "refresh"]);

    let recorded = run(
        root,
        &[
            "goal",
            "evidence",
            "goal_legacy_active",
            "--req",
            "req_1",
            "-m",
            "historical evidence",
        ],
    );
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);
    assert_eq!(
        run(root, &["goal", "close", "goal_legacy_active"]).status,
        1
    );

    let standard = run(
        root,
        &[
            "check",
            "--profile",
            "standard",
            "--goal",
            "goal_legacy_active",
        ],
    );
    assert_eq!(standard.status, 1, "stdout={}", standard.stdout);
    assert!(standard.stdout.contains("legacy goal goal_legacy_active"));
    assert!(!standard.stdout.contains("合约无效"));
}

#[test]
fn legacy_success_current_can_be_archived_through_cli() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);

    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "legacy current success",
            "--must",
            "preserve historical result",
        ],
    );
    let id = goal["id"].as_str().unwrap();
    validate_goal(root, id, "req_1", "legacy result validated", &[]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let goal_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&goal_path).unwrap()).unwrap();
    persisted["created_at"] = Value::String("2026-08-05T10:00:00Z".into());
    persisted["baseline"]["recorded_at"] = Value::String("2026-08-05T10:05:00Z".into());
    persisted
        .as_object_mut()
        .unwrap()
        .remove("plan_publication_policy");
    let legacy: rayman::goal::Goal = serde_json::from_value(persisted.clone()).unwrap();
    persisted["requirements"][0]["validations"][0]["receipt"]["contract_sha256"] =
        Value::String(rayman::goal::validation_contract_sha256(&legacy, "req_1").unwrap());
    std::fs::write(&goal_path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();

    let archived = run_json(
        root,
        &[
            "goal",
            "archive",
            id,
            "--reason",
            "retire pre-publication-policy success",
        ],
    );
    assert_eq!(archived["lifecycle"], "archived");
    assert_eq!(archived["status"], "success");
    assert_eq!(
        archived["lifecycle_proof"]["receipt_policy"],
        "receipt_integrity_v3"
    );
    assert_eq!(
        archived["lifecycle_proof"]["workspace_identity"],
        archived["requirements"][0]["validations"][0]["receipt"]["workspace_identity"]
    );
    assert_eq!(run(root, &["check", "--profile", "standard"]).status, 0);
}

#[test]
fn legacy_success_archive_english_multi_gap_is_fully_localized() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 41 }\n");
    run_json(root, &["context", "refresh"]);

    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "legacy English failure",
            "--must",
            "preserve historical result",
        ],
    );
    let id = goal["id"].as_str().unwrap();
    rayman::goal::GoalStore::new(root)
        .record_plan(
            id,
            rayman::goal::PlanReceiptSubmission {
                changed_paths: vec!["src/lib.rs".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["src/lib.rs".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    validate_goal(
        root,
        id,
        "req_1",
        "legacy source validated",
        &["src/lib.rs"],
    );
    run_json(
        root,
        &[
            "goal",
            "review",
            id,
            "--reviewer",
            "integration-review",
            "--message",
            "reviewed final source",
        ],
    );
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let goal_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let mut legacy: rayman::goal::Goal =
        serde_json::from_slice(&std::fs::read(&goal_path).unwrap()).unwrap();
    legacy.created_at = "2026-08-05T10:00:00Z".into();
    legacy.baseline.as_mut().unwrap().recorded_at = "2026-08-05T10:05:00Z".into();
    legacy.plan_publication_policy = None;
    legacy.review_receipts.clear();
    let plan = &mut legacy.plan_receipts[0];
    plan.recorded_at = "2026-08-05T10:10:00Z".into();
    plan.publication = None;
    plan.plan_sha256 = rayman::goal::plan_receipt_sha256(plan);
    let contract_sha256 = rayman::goal::validation_contract_sha256(&legacy, "req_1").unwrap();
    legacy.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap()
        .contract_sha256 = contract_sha256;
    std::fs::write(&goal_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    write(root, "outside.txt", "unplanned post-validation change\n");
    let before = std::fs::read(&goal_path).unwrap();

    let failed = run(
        root,
        &[
            "--language",
            "en",
            "goal",
            "archive",
            id,
            "--reason",
            "reject multi-gap legacy history",
        ],
    );
    assert_eq!(failed.status, 1, "stderr={}", failed.stderr);
    let rendered = format!("{}\n{}", failed.stdout, failed.stderr);
    assert!(rendered.contains("actual changes exceed"), "{rendered}");
    assert!(rendered.contains("high-priority plan"), "{rendered}");
    assert!(
        !rendered.chars().any(|character| matches!(
            character as u32,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff
        )),
        "{rendered}"
    );
    assert_eq!(std::fs::read(&goal_path).unwrap(), before);
}

#[test]
fn goal_lifecycle_preserves_history_without_hiding_unfinished_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let old = run_json(
        root,
        &["goal", "start", "old work", "--must", "preserve invariant"],
    );
    let old_id = old["id"].as_str().unwrap();

    let hidden = run(
        root,
        &["goal", "archive", old_id, "--reason", "hide blocker"],
    );
    assert_eq!(hidden.status, 1);
    let unknown_policy = run(
        root,
        &[
            "goal",
            "archive",
            old_id,
            "--reason",
            "invalid migration",
            "--migrate-receipt-policy",
            "unknown",
        ],
    );
    assert_eq!(unknown_policy.status, 1);
    assert!(
        unknown_policy.stderr.contains("未知历史 receipt policy"),
        "stderr={}",
        unknown_policy.stderr
    );

    let replacement = run_json(
        root,
        &[
            "goal",
            "start",
            "replacement",
            "--must",
            "preserve invariant",
        ],
    );
    let replacement_id = replacement["id"].as_str().unwrap();
    validate_goal(root, replacement_id, "req_1", "replacement validated", &[]);
    assert_eq!(run(root, &["goal", "close", replacement_id]).status, 0);
    let superseded = run_json(root, &["goal", "supersede", old_id, "--by", replacement_id]);
    assert_eq!(superseded["lifecycle"], "superseded");
    assert!(
        root.join(".RaymanCodingSkill/goals")
            .join(format!("{old_id}.json"))
            .is_file()
    );

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 0, "stdout={}", standard.stdout);
    assert!(
        !standard.stdout.contains(old_id) && !standard.stdout.contains("lifecycle=superseded"),
        "retired history must not feed readiness diagnostics: {}",
        standard.stdout
    );
    let retained = run_json(root, &["goal", "show", old_id]);
    assert_eq!(retained["lifecycle"], "superseded");

    let restored = run_json(root, &["goal", "current", old_id]);
    assert_eq!(restored["lifecycle"], "current");
    assert_eq!(restored["schema"], "rayman.goal-compact.v1");
    assert!(restored.get("baseline").is_none());
    assert!(restored.get("validation_receipts").is_none());
    assert!(restored["baseline_files"].as_u64().is_some());
    assert_eq!(
        run(root, &["check", "--profile", "standard", "--goal", old_id]).status,
        1
    );
}

#[test]
fn lifecycle_only_replacement_cli_uses_exact_archived_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"lifecycle-cli\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 41 }\n#[test]\nfn smoke() { assert_eq!(answer(), 41); }\n",
    );
    let warmup = Command::new("cargo")
        .args(["test", "--workspace", "--all-targets"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(warmup.status.success());
    run_json(root, &["context", "refresh"]);

    let authority = run_json(
        root,
        &[
            "goal",
            "start",
            "direct authority",
            "--must",
            "prove repository",
        ],
    );
    let authority_id = authority["id"].as_str().unwrap();
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn smoke() { assert_eq!(answer(), 42); }\n",
    );
    run_json(root, &["context", "refresh"]);
    validate_goal_authority(
        root,
        authority_id,
        "req_1",
        "stable direct authority",
        &["src/lib.rs"],
    );
    assert_eq!(run(root, &["goal", "close", authority_id]).status, 0);
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "archive",
                authority_id,
                "--reason",
                "direct authority complete",
            ],
        )
        .status,
        0
    );

    let old = run_json(
        root,
        &[
            "goal",
            "start",
            "unfinished",
            "--must",
            "preserve exact contract",
        ],
    );
    let old_id = old["id"].as_str().unwrap();
    let replacement = run_json(
        root,
        &[
            "goal",
            "start",
            "replacement",
            "--must",
            "preserve exact contract",
        ],
    );
    let replacement_id = replacement["id"].as_str().unwrap();
    let authorized = run_json(
        root,
        &[
            "goal",
            "authorize-replacement",
            replacement_id,
            "--supersedes",
            old_id,
            "--authority-from",
            authority_id,
            "--command",
            "cargo test --workspace --all-targets",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(authorized["status"], "success");
    assert_eq!(
        authorized["replacement_authority"]["authority_goal_id"],
        authority_id
    );
    let superseded = run_json(root, &["goal", "supersede", old_id, "--by", replacement_id]);
    assert_eq!(superseded["lifecycle"], "superseded");
    let checked = run(root, &["check", "--goal", replacement_id]);
    assert_eq!(checked.status, 0, "{}\n{}", checked.stdout, checked.stderr);
    let finished = run(root, &["finish", "--goal", replacement_id]);
    assert_eq!(
        finished.status, 0,
        "{}\n{}",
        finished.stdout, finished.stderr
    );
}

#[cfg(windows)]
#[test]
fn lifecycle_only_replacement_cli_rebinds_only_the_maintenance_cycle_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Keep this fixture authoritative even when the process TEMP happens to
    // sit below a parent Git worktree. Otherwise workspace discovery can walk
    // into the production repository before the helper activates the fixture.
    std::fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"lifecycle-cycle-rebind\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 41 }\n");
    write(
        root,
        "scripts/check-repo.ps1",
        "param([string]$MaintenanceOrchestrationCycle)\nif (-not (Test-Path -LiteralPath $MaintenanceOrchestrationCycle -PathType Leaf)) { exit 11 }\nif ((Get-Content -Raw -LiteralPath $MaintenanceOrchestrationCycle) -notmatch '\"status\"\\s*:\\s*\"pass\"') { exit 12 }\n",
    );
    write(
        root,
        "target/archived-maintenance-review-cycle.json",
        "{\"status\":\"pass\",\"snapshot\":\"archived\"}\n",
    );
    write(
        root,
        "target/current-maintenance-review-cycle.json",
        "{\"status\":\"pass\",\"snapshot\":\"current\"}\n",
    );
    run_json(root, &["context", "refresh"]);

    let archived_command = "pwsh -NoProfile -File scripts/check-repo.ps1 -MaintenanceOrchestrationCycle target/archived-maintenance-review-cycle.json";
    let authority = run_json(
        root,
        &[
            "goal",
            "start",
            "cycle authority",
            "--must",
            "prove repository",
        ],
    );
    let authority_id = authority["id"].as_str().unwrap();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &[
            "goal",
            "validate",
            authority_id,
            "--req",
            "req_1",
            "-m",
            "stable cycle authority",
            "--command",
            archived_command,
            "--changed",
            "src/lib.rs",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(run(root, &["goal", "close", authority_id]).status, 0);
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "archive",
                authority_id,
                "--reason",
                "cycle authority complete",
            ],
        )
        .status,
        0
    );

    let old = run_json(
        root,
        &[
            "goal",
            "start",
            "unfinished cycle consumer",
            "--must",
            "preserve cycle contract",
        ],
    );
    let replacement = run_json(
        root,
        &[
            "goal",
            "start",
            "replacement cycle consumer",
            "--must",
            "preserve cycle contract",
        ],
    );
    std::fs::remove_file(root.join("target/archived-maintenance-review-cycle.json")).unwrap();
    let authorized = run_json(
        root,
        &[
            "goal",
            "authorize-replacement",
            replacement["id"].as_str().unwrap(),
            "--supersedes",
            old["id"].as_str().unwrap(),
            "--authority-from",
            authority_id,
            "--command",
            archived_command,
            "--maintenance-cycle-rebind",
            "target/current-maintenance-review-cycle.json",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(authorized["status"], "success");
    assert_eq!(
        authorized["replacement_authority"]["live_authority"]["command"],
        archived_command
    );
    assert_eq!(
        authorized["replacement_authority"]["live_authority"]["command_rebind"]["current_value"],
        "target/current-maintenance-review-cycle.json"
    );
}

#[test]
fn ordinary_changed_validation_keeps_the_legacy_scope_shape() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"changed-scope-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn value() -> u8 { 1 }\n");
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "ordinary change",
            "--must",
            "validate source",
        ],
    );
    let id = started["id"].as_str().unwrap();
    write(
        root,
        "src/lib.rs",
        "pub fn value() -> u8 { 2 }\n#[test]\nfn value_is_two() { assert_eq!(value(), 2); }\n",
    );
    run_json(root, &["context", "refresh"]);

    let validated = run_json(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "ordinary changed validation",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --quiet",
        ],
    );
    let validation = &validated["requirements"][0]["validations"][0];
    assert!(validation.get("workspace_snapshot").is_none());
    assert_eq!(validation["non_code"], false);
    assert_eq!(validation["impact_paths"][0], "src/lib.rs");
}

#[test]
fn handoff_start_binds_source_goal_authority_clean_head_and_structured_stages() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"handoff-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 42); }\n",
    );
    write(root, ".gitignore", ".RaymanCodingSkill/\ntarget/\n");
    generate_lockfile(root);
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Rayman Test",
        "-c",
        "user.email=rayman@example.invalid",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ]);
    let commit = git(&["rev-parse", "HEAD"]);

    run_json(root, &["context", "refresh"]);
    let source = run_json(
        root,
        &[
            "goal",
            "start",
            "implementation",
            "--must",
            "prove implementation",
        ],
    );
    let source_id = source["id"].as_str().unwrap();
    let authority = run(
        root,
        &[
            "goal",
            "validate",
            source_id,
            "--req",
            "req_1",
            "-m",
            "stable implementation authority",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --all",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(authority.status, 0, "stderr={}", authority.stderr);
    assert_eq!(run(root, &["goal", "close", source_id]).status, 0);

    let handoff = run_json(
        root,
        &[
            "goal",
            "handoff",
            "start",
            "--from-goal",
            source_id,
            "--commit",
            &commit,
        ],
    );
    assert_eq!(handoff["handoff"]["source_goal_id"], source_id);
    assert_eq!(handoff["handoff"]["git_commit"], commit);
    assert_eq!(handoff["handoff"]["stages"].as_array().unwrap().len(), 4);
    assert_eq!(
        handoff["handoff"]["audit_policy"],
        "complete_repository_audit_v1"
    );
    assert_eq!(handoff["requirements"][0]["proof_kind"], "installation");
    assert_eq!(handoff["requirements"][1]["proof_kind"], "repository_audit");
    assert_eq!(handoff["requirements"][3]["proof_kind"], "repository_gate");
    assert_eq!(handoff["requirements"][2]["proof_kind"], "source_fresh");

    let rejected = run(
        root,
        &[
            "goal",
            "validate",
            handoff["id"].as_str().unwrap(),
            "--req",
            "req_2",
            "--message",
            "Cargo is not a complete audit",
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
            "--command",
            "cargo test --all",
        ],
    );
    assert_eq!(rejected.status, 1, "{}", rejected.stdout);
    let unchanged = run_json(root, &["goal", "show", handoff["id"].as_str().unwrap()]);
    assert!(
        unchanged["requirements"][1]["validations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let final_authority = run(
        root,
        &[
            "goal",
            "validate",
            handoff["id"].as_str().unwrap(),
            "--req",
            "req_4",
            "--message",
            "independent final authority",
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
            "--command",
            "cargo test --all",
        ],
    );
    assert_eq!(final_authority.status, 0, "{}", final_authority.stderr);
    let separated = run_json(root, &["goal", "show", handoff["id"].as_str().unwrap()]);
    assert_eq!(separated["requirements"][3]["status"], "done");
    assert_eq!(separated["requirements"][1]["status"], "open");
    let partial_check = run(
        root,
        &[
            "check",
            "--goal",
            handoff["id"].as_str().unwrap(),
            "--format",
            "json",
        ],
    );
    assert!(
        !partial_check
            .stdout
            .contains("handoff final authority receipt is missing or stale"),
        "{}",
        partial_check.stdout
    );

    write(root, "src/lib.rs", "pub fn answer() -> i32 { 43 }\n");
    let dirty = run(
        root,
        &[
            "goal",
            "handoff",
            "start",
            "--from-goal",
            source_id,
            "--commit",
            &commit,
        ],
    );
    assert_eq!(dirty.status, 1);
    assert!(
        dirty.stderr.contains("clean Git worktree"),
        "{}",
        dirty.stderr
    );
}

#[test]
fn handoff_start_rejects_a_retired_source_goal() {
    // 回归：goal_gate_verdict 对非 current lifecycle 只出 warning、无 blocker，故 handoff 曾能
    // 从一个已 archive 的退休实现切 release。现在 start_handoff 要求源目标 lifecycle=current。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"handoff-retired-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 42); }\n",
    );
    write(root, ".gitignore", ".RaymanCodingSkill/\ntarget/\n");
    generate_lockfile(root);
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Rayman Test",
        "-c",
        "user.email=rayman@example.invalid",
        "commit",
        "--quiet",
        "-m",
        "fixture",
    ]);
    let commit = git(&["rev-parse", "HEAD"]);

    run_json(root, &["context", "refresh"]);
    let source = run_json(
        root,
        &["goal", "start", "implementation", "--must", "prove it"],
    );
    let source_id = source["id"].as_str().unwrap();
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "validate",
                source_id,
                "--req",
                "req_1",
                "-m",
                "stable implementation authority",
                "--changed",
                "src/lib.rs",
                "--command",
                "cargo test --all",
                "--authority",
                "--repeat",
                "2",
            ],
        )
        .status,
        0
    );
    assert_eq!(run(root, &["goal", "close", source_id]).status, 0);
    // Archive the proven success into its normal terminal (retired) state.
    assert_eq!(
        run(root, &["goal", "archive", source_id, "--reason", "retired"],).status,
        0
    );

    let retired = run(
        root,
        &[
            "goal",
            "handoff",
            "start",
            "--from-goal",
            source_id,
            "--commit",
            &commit,
        ],
    );
    assert_eq!(retired.status, 1, "stdout={}", retired.stdout);
    assert!(
        retired.stderr.contains("lifecycle") && retired.stderr.contains("current"),
        "{}",
        retired.stderr
    );
}
