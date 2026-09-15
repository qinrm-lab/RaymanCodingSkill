use super::*;

#[test]
fn update_status_is_activation_exempt_and_read_only_outside_a_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let output = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "status"],
    );
    assert_eq!(output.status, 0, "stderr={}", output.stderr);
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(report["status"], "status");
    assert_eq!(report["state"]["auto_check"], true);
    assert_eq!(report["state"]["auto_install"], false);
    assert_eq!(report["state_written"], false);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
    assert!(!user.path().join("Rayman/update").exists());
}

#[cfg(not(windows))]
#[test]
fn non_windows_update_check_reports_unsupported_without_cache_or_workspace_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let output = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "check"],
    );
    assert_eq!(output.status, 0, "stderr={}", output.stderr);
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        report["observation"]["status"]["status"],
        "unsupported_platform"
    );
    assert_eq!(report["state_written"], false);
    assert_eq!(report["install_ready"], false);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
    assert!(!user.path().join("Rayman/update").exists());
}

#[cfg(not(windows))]
#[test]
fn non_windows_due_poll_stays_unsupported_even_with_install_consent() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let configured = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--auto-install", "--yes"],
    );
    assert_eq!(configured.status, 0, "stderr={}", configured.stderr);

    let output = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "poll"],
    );
    assert_eq!(output.status, 0, "stderr={}", output.stderr);
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(report["status"], "polled");
    assert_eq!(report["checked"], true);
    assert_eq!(report["state_written"], true);
    assert_eq!(
        report["observation"]["status"]["status"],
        "unsupported_platform"
    );
    assert_eq!(report["install_authorized"], true);
    assert_eq!(report["install_ready"], false);
    assert!(report.get("worker_launch").is_none());
    assert!(report.get("install_error").is_none());
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());

    let state_path = user.path().join("Rayman/update/update.json");
    let persisted: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(persisted["auto_check"], true);
    assert_eq!(persisted["auto_install"], true);
    assert!(persisted["last_attempted_at"].as_str().is_some());
    assert_eq!(persisted["last_successful_observation"], Value::Null);
}

#[test]
fn update_configure_requires_an_exact_selector_and_yes_without_workspace_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let no_selector = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--yes"],
    );
    assert_ne!(no_selector.status, 0);
    assert!(!user.path().join("Rayman/update/update.json").exists());

    let no_confirmation = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--no-auto-check"],
    );
    assert_ne!(no_confirmation.status, 0);
    assert!(!user.path().join("Rayman/update/update.json").exists());

    let configured = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &[
            "--format",
            "json",
            "update",
            "configure",
            "--auto-install",
            "--yes",
        ],
    );
    assert_eq!(configured.status, 0, "stderr={}", configured.stderr);
    let report: Value = serde_json::from_str(&configured.stdout).unwrap();
    assert_eq!(report["state"]["auto_check"], true);
    assert_eq!(report["state"]["auto_install"], true);
    assert_eq!(report["install_ready"], false);
    assert!(user.path().join("Rayman/update/update.json").is_file());
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
}

#[test]
fn disabled_update_poll_is_zero_network_and_does_not_touch_workspace_state() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let disabled = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--no-auto-check", "--yes"],
    );
    assert_eq!(disabled.status, 0, "stderr={}", disabled.stderr);
    let state_path = user.path().join("Rayman/update/update.json");
    let before = std::fs::read(&state_path).unwrap();

    let poll = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "poll"],
    );
    assert_eq!(poll.status, 0, "stderr={}", poll.stderr);
    let report: Value = serde_json::from_str(&poll.stdout).unwrap();
    assert_eq!(report["status"], "not_due");
    assert_eq!(report["checked"], false);
    assert_eq!(report["state_written"], false);
    assert_eq!(std::fs::read(&state_path).unwrap(), before);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
}

#[test]
fn corrupt_update_state_is_preserved_and_never_becomes_install_consent() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let update_dir = user.path().join("Rayman/update");
    std::fs::create_dir_all(&update_dir).unwrap();
    let state_path = update_dir.join("update.json");
    let corrupt = b"{ not trusted state";
    std::fs::write(&state_path, corrupt).unwrap();

    let poll = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "poll"],
    );
    assert_eq!(poll.status, 0, "stderr={}", poll.stderr);
    let report: Value = serde_json::from_str(&poll.stdout).unwrap();
    assert_eq!(report["status"], "state_error");
    assert_eq!(report["checked"], false);
    assert_eq!(report["state_written"], false);
    assert_eq!(report["install_authorized"], false);
    assert_eq!(std::fs::read(&state_path).unwrap(), corrupt);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
}
