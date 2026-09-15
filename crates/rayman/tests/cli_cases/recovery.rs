// The state-audit allowlist is hand-maintained and has now drifted from what
// the CLI itself writes three times: the pending lock, the autosave lock, and
// `checkpoints/` from the `--dir` remedy the workflow reference prescribes for
// a workspace-only sandbox. Drive the real commands instead of restating the
// list, so the next writer that lands in `.RaymanCodingSkill/` fails here.

// `temp scratch` prints a path meant to be pasted into another command — the
// workflow contract points patch files at it. A Windows `\?\` verbatim
// prefix makes many tools refuse it; pytest lease paths were normalized for
// that reason and scratch was the remaining leak.

use super::*;

#[test]
fn checkpoint_verify_state_audit_and_recursive_temp_status_are_exposed_by_cli() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let checkpoint_dir = tempfile::tempdir().unwrap();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");

    let checkpoint_dir = checkpoint_dir.path().to_str().unwrap();
    let saved = run_json(
        root,
        &["checkpoint", "--dir", checkpoint_dir, "save", "--keep", "1"],
    );
    let id = saved["id"].as_str().unwrap();
    let verified = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "verify", id]);
    assert_eq!(verified["status"], "complete");
    assert!(verified["file_count"].as_u64().unwrap() >= 1);

    let temp_before = run_json(root, &["temp", "status"]);
    assert_eq!(temp_before["traversal_error_count"], 0);
    let run_root = root.join(".RaymanCodingSkill/tmp/run");
    assert!(!run_root.exists());

    write(root, ".RaymanCodingSkill/tmp/run/nested/a.bin", "abc");
    write(root, ".RaymanCodingSkill/tmp/run/b.bin", "d");
    let temp_status = run_json(root, &["temp", "status"]);
    assert!(run_root.join("nested/a.bin").is_file());
    assert!(run_root.join("b.bin").is_file());
    assert_eq!(
        temp_status["entry_count"].as_u64().unwrap(),
        temp_before["entry_count"].as_u64().unwrap() + 1
    );
    assert_eq!(
        temp_status["file_count"].as_u64().unwrap(),
        temp_before["file_count"].as_u64().unwrap() + 2
    );
    assert_eq!(
        temp_status["directory_count"].as_u64().unwrap(),
        temp_before["directory_count"].as_u64().unwrap() + 2
    );
    assert_eq!(
        temp_status["total_bytes"].as_u64().unwrap(),
        temp_before["total_bytes"].as_u64().unwrap() + 4
    );
    assert_eq!(temp_status["traversal_error_count"], 0);

    let clean_audit = run_json(root, &["state", "audit", "--check"]);
    assert_eq!(clean_audit["clean"], true);
    assert_eq!(clean_audit["operationally_healthy"], true);
    write(
        root,
        ".RaymanCodingSkill/.pending.json.rayman-1234-7.tmp",
        "",
    );
    let recovery_warning = run_json(root, &["state", "audit", "--check"]);
    assert_eq!(recovery_warning["clean"], true);
    assert_eq!(recovery_warning["operationally_healthy"], false);
    assert!(
        recovery_warning["recovery_warnings"]
            .as_array()
            .is_some_and(|warnings| !warnings.is_empty())
    );
    std::fs::remove_file(root.join(".RaymanCodingSkill/.pending.json.rayman-1234-7.tmp")).unwrap();
    write(root, ".RaymanCodingSkill/research/retired.json", "{}");
    let blocked_audit = run(root, &["state", "audit", "--check"]);
    assert_eq!(blocked_audit.status, 1);
    assert!(
        root.join(".RaymanCodingSkill/research/retired.json")
            .exists()
    );
}

#[test]
fn checkpoint_save_is_lossless_by_default_and_prune_requires_yes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let checkpoint_dir = tempfile::tempdir().unwrap();
    let checkpoint_dir = checkpoint_dir.path().to_str().unwrap();
    write(root, "src/lib.rs", "pub fn value() -> i32 { 1 }\n");

    let first = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "save"]);
    assert_eq!(first["retention_applied"], false);
    assert_eq!(first["pruned"], 0);
    write(root, "src/lib.rs", "pub fn value() -> i32 { 2 }\n");
    let second = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "save"]);
    assert_eq!(second["retention_applied"], false);
    assert_eq!(second["pruned"], 0);

    let before = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "list"]);
    assert_eq!(before.as_array().unwrap().len(), 2);
    let refused = run(
        root,
        &[
            "checkpoint",
            "--dir",
            checkpoint_dir,
            "prune",
            "--keep",
            "1",
        ],
    );
    assert_eq!(refused.status, 1);
    assert!(refused.stderr.contains("--yes"));
    let after_refusal = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "list"]);
    assert_eq!(after_refusal.as_array().unwrap().len(), 2);

    let pruned = run_json(
        root,
        &[
            "checkpoint",
            "--dir",
            checkpoint_dir,
            "prune",
            "--keep",
            "1",
            "--yes",
        ],
    );
    assert_eq!(pruned["pruned"], 1);
}

#[test]
fn temp_scratch_status_and_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let scratch = run(root, &["temp", "scratch", "build cache"]);
    assert_eq!(scratch.status, 0);
    let dir = scratch.stdout.trim();
    assert!(Path::new(dir).is_dir());

    assert_eq!(run_json(root, &["temp", "status"])["exists"], true);
    assert_eq!(run(root, &["temp", "cleanup"]).status, 0);
    assert_eq!(run_json(root, &["temp", "status"])["exists"], false);
}

#[test]
fn manual_recovery_discovers_the_legacy_autosave_store() {
    let workspace = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = store.path().to_str().unwrap();
    write(root, "payload.txt", "preserved snapshot");
    let legacy = serde_json::json!({
        "active": true, "interval_min": 30, "keep": 3, "dir": dir,
        "auto_stop": true, "task_name": "RaymanCheckpoint-retired-fixture",
        "started_at": "2026-09-14T00:00:00Z"
    })
    .to_string();
    write(root, ".RaymanCodingSkill/autosave.json", &legacy);
    let saved = run_json(root, &["checkpoint", "--dir", dir, "save"]);
    let id = saved["id"].as_str().unwrap();
    write(root, "payload.txt", "later contents");
    run_json(root, &["checkpoint", "--dir", dir, "restore", id, "--yes"]);
    assert_eq!(
        std::fs::read_to_string(root.join("payload.txt")).unwrap(),
        "preserved snapshot"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".RaymanCodingSkill/autosave.json")).unwrap(),
        legacy
    );
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "manual recovery",
            "--must",
            "continue safely",
        ],
    );
    let prepared = run_json(root, &["prepare", "--goal", goal["id"].as_str().unwrap()]);
    assert_eq!(prepared["latest_verified_checkpoint"]["id"], id);
    assert_eq!(prepared["latest_verified_checkpoint"]["store_dir"], dir);
}

#[test]
fn retired_autosave_commands_preserve_workspace_and_legacy_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for args in [
        vec!["autosave", "start", "--interval", "1"],
        vec!["autosave", "tick", "--workspace", "missing-workspace"],
        vec!["autosave", "stop", "--status", "success"],
        vec!["autosave", "status"],
    ] {
        let rejected = run_raw(root, &args);
        assert_eq!(rejected.status, 1, "{}", rejected.stderr);
        assert!(
            rejected.stderr.contains("save-work-status"),
            "{}",
            rejected.stderr
        );
        assert!(!root.join(".RaymanCodingSkill").exists());
    }
    write(
        root,
        ".RaymanCodingSkill/autosave.json",
        "{\"active\":true,\"dir\":\"old-store\"}",
    );
    let before = state_snapshot(root);
    let rejected = run_raw(root, &["--language", "en", "autosave", "start"]);
    assert_eq!(rejected.status, 1);
    assert!(
        rejected.stderr.contains("is retired"),
        "{}",
        rejected.stderr
    );
    assert_eq!(state_snapshot(root), before);
    let help = run_raw(root, &["--language", "en", "--help"]);
    assert_eq!(help.status, 0);
    assert!(!help.stdout.contains("autosave"), "{}", help.stdout);
}

#[test]
fn salvage_save_cli_works_without_activation_but_never_becomes_latest() {
    let workspace = tempfile::tempdir().unwrap();
    let checkpoint_store = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "payload.txt", "emergency\n");
    let store = checkpoint_store.path().to_str().unwrap();
    let saved = run_raw(
        root,
        &[
            "--format",
            "json",
            "checkpoint",
            "salvage-save",
            "--dir",
            store,
        ],
    );
    assert_eq!(saved.status, 0, "{}", saved.stderr);
    let saved: Value = serde_json::from_str(&saved.stdout).unwrap();
    assert_eq!(saved["purpose"], "recovery_only");
    assert_eq!(saved["authoritative"], false);
    let status = run_raw(
        root,
        &["--format", "json", "checkpoint", "status", "--dir", store],
    );
    assert_eq!(status.status, 0, "{}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(status["has_checkpoint"], false);
    let listed = run_raw(
        root,
        &["--format", "json", "checkpoint", "list", "--dir", store],
    );
    let listed: Value = serde_json::from_str(&listed.stdout).unwrap();
    assert_eq!(listed[0]["purpose"], "recovery_only");
}

#[test]
fn temp_scratch_paths_do_not_leak_the_windows_verbatim_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/lib.rs",
        "pub fn a() {}
",
    );
    run_json(root, &["context", "refresh"]);

    let text = run(root, &["temp", "scratch", "patchwork"]);
    assert_eq!(text.status, 0, "stderr={}", text.stderr);
    assert!(
        !text.stdout.contains(r"\?\"),
        "text output leaked the verbatim prefix: {}",
        text.stdout
    );

    let json = run_json(root, &["temp", "scratch", "patchwork"]);
    let path = json["path"].as_str().unwrap();
    assert!(
        !path.contains(r"\?\"),
        "json output leaked the verbatim prefix: {path}"
    );
    assert!(path.contains("patchwork"), "{path}");
}

#[test]
fn state_audit_stays_clean_after_the_commands_that_write_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/lib.rs",
        "pub fn a() {}
",
    );
    write(
        root,
        "Cargo.toml",
        "[package]
name = \"sa\"
version = \"0.1.0\"
edition = \"2021\"
",
    );
    run_json(root, &["context", "refresh"]);
    assert_eq!(run(root, &["state", "audit", "--check"]).status, 0);

    let started = run_json(root, &["goal", "start", "state writers", "--must", "work"]);
    let id = started["id"].as_str().unwrap().to_string();
    run_json(
        root,
        &["goal", "pending", "add", "leftover", "-m", "detail"],
    );
    let checkpoints = root.join(".RaymanCodingSkill/checkpoints");
    let saved = run(
        root,
        &["checkpoint", "save", "--dir", checkpoints.to_str().unwrap()],
    );
    assert_eq!(saved.status, 0, "stderr={}", saved.stderr);

    let audit = run(root, &["state", "audit", "--check"]);
    assert_eq!(
        audit.status, 0,
        "state written by ordinary commands must not read as retired: {}{}",
        audit.stdout, audit.stderr
    );
    assert!(
        !audit.stdout.contains("retired entries"),
        "{}",
        audit.stdout
    );
    assert!(!id.is_empty());
}
