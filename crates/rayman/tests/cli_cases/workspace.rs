use super::*;

#[test]
fn workspace_root_is_discovered_from_a_subdirectory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/a.rs", "fn a() {}");
    // 在根建立索引 → 产生根级 .RaymanCodingSkill。
    run(root, &["context", "refresh"]);
    assert!(root.join(".RaymanCodingSkill").is_dir());

    // 从子目录运行：应复用祖先工作区，不在子目录另建状态。
    let sub = root.join("src");
    let status = run_json(&sub, &["context", "status"]);
    assert_eq!(status["status"], "ready");
    assert!(
        !sub.join(".RaymanCodingSkill").exists(),
        "从子目录运行不应在子目录另建 .RaymanCodingSkill（会分裂状态）"
    );
}

#[test]
fn workspace_activation_is_explicit_and_orphan_state_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join(".RaymanCodingSkill/goals")).unwrap();

    let status = run_raw(root, &["--format", "json", "workspace", "status"]);
    assert_eq!(status.status, 0);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(status["status"], "orphan_state");
    assert_eq!(status["active"], false);
    let blocked = run_raw(root, &["context", "refresh"]);
    assert_ne!(blocked.status, 0);
    assert!(!root.join(".RaymanCodingSkill/context/index.json").exists());

    let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();
    let activated = run_raw(
        root,
        &[
            "workspace",
            "activate",
            "--skill-file",
            skill.to_str().unwrap(),
            "--yes",
        ],
    );
    assert_eq!(activated.status, 0, "{}", activated.stderr);
    let active = run_json(root, &["workspace", "status"]);
    assert_eq!(active["active"], true);
    assert_eq!(
        active["actual_bundle_sha256"],
        active["expected_bundle_sha256"]
    );
    assert_eq!(
        active["actual_bundle_sha256"],
        active["running_bundle_sha256"]
    );
    assert!(
        std::fs::read_to_string(root.join(".RaymanCodingSkill/workspace_skill.yaml"))
            .unwrap()
            .contains("bundle_sha256:")
    );
    assert_eq!(run_raw(root, &["context", "refresh"]).status, 0);
}

#[test]
fn workspace_activation_rejects_the_previous_cli_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();
    let activated = run_raw(
        root,
        &[
            "workspace",
            "activate",
            "--skill-file",
            skill.to_str().unwrap(),
            "--yes",
        ],
    );
    assert_eq!(activated.status, 0, "{}", activated.stderr);

    let activation_path = root.join(".RaymanCodingSkill/workspace_skill.yaml");
    let previous_identity = std::fs::read_to_string(&activation_path)
        .unwrap()
        .replace(rayman::CLI_CONTRACT, "rayman-cli-contract-v15")
        .replace(
            &format!("cli_version: {}", rayman::CLI_VERSION),
            "cli_version: 2.9.0",
        );
    std::fs::write(&activation_path, previous_identity).unwrap();

    let status = run_raw(root, &["--format", "json", "workspace", "status"]);
    assert_eq!(status.status, 0, "{}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(status["status"], "invalid");
    assert_eq!(status["active"], false);
    assert_eq!(status["cli_contract"], "rayman-cli-contract-v15");
    assert_eq!(status["cli_version"], "2.9.0");
    assert_eq!(status["running_cli_contract"], rayman::CLI_CONTRACT);
    assert_eq!(status["running_cli_version"], rayman::CLI_VERSION);
    assert!(
        status["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| { issue.as_str().unwrap().contains("cli_contract") })
    );
    assert!(
        status["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| { issue.as_str().unwrap().contains("cli_version") })
    );
    let activation_before = std::fs::read(&activation_path).unwrap();
    let write_attempt = run_raw(root, &["context", "refresh"]);
    assert_ne!(write_attempt.status, 0, "stdout={}", write_attempt.stdout);
    assert_eq!(std::fs::read(&activation_path).unwrap(), activation_before);
    assert!(
        !root.join(".RaymanCodingSkill/context.json").exists(),
        "a previous-contract binding must not authorize a v16 state write"
    );
}

#[test]
fn workspace_rebind_requires_yes_and_preserves_drifted_contract_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let activation_path = make_rebind_eligible_identity_drift(root);
    write(
        root,
        ".RaymanCodingSkill/goals/untouched.json",
        "managed sentinel\n",
    );
    let activation_before = std::fs::read(&activation_path).unwrap();
    let state_before = state_snapshot(root);

    let rejected = run_raw(root, &["workspace", "rebind"]);

    assert_ne!(rejected.status, 0, "stdout={}", rejected.stdout);
    assert!(rejected.stderr.contains("--yes"), "{}", rejected.stderr);
    assert_eq!(std::fs::read(&activation_path).unwrap(), activation_before);
    assert_eq!(state_snapshot(root), state_before);
}

#[test]
fn workspace_rebind_repairs_only_hash_and_cli_identity_drift() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let activation_path = make_rebind_eligible_identity_drift(root);
    write(
        root,
        ".RaymanCodingSkill/goals/untouched.json",
        "managed goal sentinel\n",
    );
    write(
        root,
        ".RaymanCodingSkill/context/untouched.json",
        "managed context sentinel\n",
    );
    let config_before = std::fs::read_to_string(&activation_path).unwrap();
    let skill_file_before = config_before
        .lines()
        .find(|line| line.starts_with("skill_file:"))
        .unwrap()
        .to_string();
    let other_state_before = managed_state_without_activation(root);

    let rebound = run_raw(root, &["--format", "json", "workspace", "rebind", "--yes"]);

    assert_eq!(
        rebound.status, 0,
        "stdout={} stderr={}",
        rebound.stdout, rebound.stderr
    );
    let mut rebound_report: Value = serde_json::from_str(&rebound.stdout).unwrap();
    assert_eq!(rebound_report["changed"], true);
    assert_eq!(rebound_report["status"], "active");
    assert_eq!(rebound_report["active"], true);
    assert_eq!(
        rebound_report["cli_contract"],
        Value::String(rayman::CLI_CONTRACT.to_string())
    );
    assert_eq!(
        rebound_report["cli_version"],
        Value::String(rayman::CLI_VERSION.to_string())
    );
    assert_eq!(
        rebound_report["expected_sha256"],
        rebound_report["actual_sha256"]
    );
    let retained_evidence = rebound_report
        .as_object_mut()
        .unwrap()
        .remove("retained_evidence");
    #[cfg(target_os = "linux")]
    {
        let retained = retained_evidence
            .expect("Linux rebind must report retained evidence")
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(retained.len(), 1);
        assert_eq!(
            retained[0]["action"],
            "workspace rebind preserved prior activation"
        );
        let retained_path = Path::new(retained[0]["path"].as_str().unwrap());
        assert!(
            retained_path.canonicalize().unwrap().starts_with(
                root.join(".RaymanCodingSkill/tmp/activation-retained")
                    .canonicalize()
                    .unwrap()
            )
        );
        assert!(retained[0]["sha256"].as_str().is_some());
        assert!(retained[0]["metadata_sha256"].as_str().is_some());
        assert!(retained[0]["identity"].as_str().is_some());
    }
    #[cfg(not(target_os = "linux"))]
    assert!(retained_evidence.is_none());

    let changed = rebound_report
        .as_object_mut()
        .unwrap()
        .remove("changed")
        .unwrap();
    assert_eq!(changed, true);
    let status = run_raw(root, &["--format", "json", "workspace", "status"]);
    assert_eq!(status.status, 0, "{}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(
        rebound_report, status,
        "rebind JSON must be workspace status plus changed"
    );

    let config_after = std::fs::read_to_string(&activation_path).unwrap();
    let skill_file_after = config_after
        .lines()
        .find(|line| line.starts_with("skill_file:"))
        .unwrap();
    assert_eq!(skill_file_after, skill_file_before);
    for prefix in ["skill:", "enabled:"] {
        assert_eq!(
            config_after.lines().find(|line| line.starts_with(prefix)),
            config_before.lines().find(|line| line.starts_with(prefix)),
            "rebind changed non-identity field {prefix}"
        );
    }
    assert!(config_after.contains(&format!("cli_contract: {}", rayman::CLI_CONTRACT)));
    assert!(config_after.contains(&format!("cli_version: {}", rayman::CLI_VERSION)));
    assert!(
        !config_after
            .lines()
            .any(|line| line == "cli_contract: rayman-cli-contract-v1")
    );
    assert!(
        !config_after
            .lines()
            .any(|line| line == "cli_version: 0.1.0")
    );
    let other_state_after = managed_state_without_activation(root);
    #[cfg(target_os = "linux")]
    let other_state_after = {
        let mut state = other_state_after;
        remove_linux_retained_activation(&mut state, config_before.as_bytes());
        state
    };
    assert_eq!(other_state_after, other_state_before);
}

#[test]
fn workspace_rebind_is_idempotent_when_activation_is_already_current() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    activate_rebind_fixture(root);
    write(
        root,
        ".RaymanCodingSkill/goals/untouched.json",
        "managed sentinel\n",
    );
    let activation_path = rebind_activation_path(root);
    let activation_before = std::fs::read(&activation_path).unwrap();
    let state_before = state_snapshot(root);

    let rebound = run_raw(root, &["--format", "json", "workspace", "rebind", "--yes"]);

    assert_eq!(
        rebound.status, 0,
        "stdout={} stderr={}",
        rebound.stdout, rebound.stderr
    );
    let mut rebound_report: Value = serde_json::from_str(&rebound.stdout).unwrap();
    assert_eq!(rebound_report["changed"], false);
    assert_eq!(rebound_report["active"], true);
    rebound_report
        .as_object_mut()
        .unwrap()
        .remove("changed")
        .unwrap();
    let status = run_raw(root, &["--format", "json", "workspace", "status"]);
    assert_eq!(status.status, 0, "{}", status.stderr);
    assert_eq!(
        rebound_report,
        serde_json::from_str::<Value>(&status.stdout).unwrap()
    );
    assert_eq!(std::fs::read(&activation_path).unwrap(), activation_before);
    assert_eq!(state_snapshot(root), state_before);
}

#[test]
fn workspace_ensure_current_reports_current_activation_without_writing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    activate_rebind_fixture(root);
    write(
        root,
        ".RaymanCodingSkill/goals/untouched.json",
        "managed sentinel\n",
    );
    let activation_path = rebind_activation_path(root);
    let activation_before = std::fs::read(&activation_path).unwrap();
    let state_before = state_snapshot(root);

    let report = run_raw(root, &["--format", "json", "workspace", "ensure-current"]);

    assert_eq!(
        report.status, 0,
        "stdout={} stderr={}",
        report.stdout, report.stderr
    );
    let report: Value = serde_json::from_str(&report.stdout).unwrap();
    assert_eq!(report["status"], "active");
    assert_eq!(report["activation"]["active"], true);
    assert_eq!(report["changed"], false);
    assert_eq!(
        report["migration_scope"],
        "current_workspace_activation_identity_only"
    );
    assert_eq!(report["activation_identity_changed"], false);
    assert_eq!(report["project_files_changed"], false);
    assert_eq!(report["other_workspaces_scanned"], false);
    assert_eq!(std::fs::read(&activation_path).unwrap(), activation_before);
    assert_eq!(state_snapshot(root), state_before);

    let applied = run_raw(
        root,
        &["--format", "json", "workspace", "ensure-current", "--yes"],
    );
    assert_eq!(
        applied.status, 0,
        "stdout={} stderr={}",
        applied.stdout, applied.stderr
    );
    let applied: Value = serde_json::from_str(&applied.stdout).unwrap();
    assert_eq!(applied["status"], "active");
    assert_eq!(applied["changed"], false);
    assert_eq!(applied["activation_identity_changed"], false);
    assert_eq!(applied["project_files_changed"], false);
    assert_eq!(applied["other_workspaces_scanned"], false);
    assert_eq!(std::fs::read(&activation_path).unwrap(), activation_before);
    assert_eq!(state_snapshot(root), state_before);
}

#[test]
fn workspace_ensure_current_only_rebinds_eligible_identity_drift_with_yes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let activation_path = make_rebind_eligible_identity_drift(root);
    write(
        root,
        ".RaymanCodingSkill/goals/untouched.json",
        "managed sentinel\n",
    );
    let config_before = std::fs::read_to_string(&activation_path).unwrap();
    let skill_file_before = config_before
        .lines()
        .find(|line| line.starts_with("skill_file:"))
        .unwrap()
        .to_string();
    let state_before = state_snapshot(root);

    let text = run_raw(root, &["workspace", "ensure-current"]);
    assert_eq!(
        text.status, 0,
        "stdout={} stderr={}",
        text.stdout, text.stderr
    );
    assert!(text.stdout.contains("rebind_required"), "{}", text.stdout);
    assert!(text.stdout.contains("changed: false"), "{}", text.stdout);
    assert_exact_rebind_hint(
        &format!("{}\n{}", text.stdout, text.stderr),
        "ensure-current",
    );
    assert_eq!(state_snapshot(root), state_before);

    let check = run_raw(root, &["--format", "json", "workspace", "ensure-current"]);
    assert_eq!(
        check.status, 0,
        "stdout={} stderr={}",
        check.stdout, check.stderr
    );
    let check: Value = serde_json::from_str(&check.stdout).unwrap();
    assert_eq!(check["status"], "rebind_required");
    assert_eq!(check["activation"]["rebind_eligible"], true);
    assert_eq!(check["changed"], false);
    assert_eq!(state_snapshot(root), state_before);

    let applied = run_raw(
        root,
        &["--format", "json", "workspace", "ensure-current", "--yes"],
    );
    assert_eq!(
        applied.status, 0,
        "stdout={} stderr={}",
        applied.stdout, applied.stderr
    );
    let applied: Value = serde_json::from_str(&applied.stdout).unwrap();
    assert_eq!(applied["status"], "active");
    assert_eq!(applied["activation"]["active"], true);
    assert_eq!(applied["changed"], true);
    assert_eq!(applied["activation_identity_changed"], true);
    assert_eq!(applied["project_files_changed"], false);
    assert_eq!(applied["other_workspaces_scanned"], false);
    let config_after = std::fs::read_to_string(&activation_path).unwrap();
    assert_eq!(
        config_after
            .lines()
            .find(|line| line.starts_with("skill_file:"))
            .unwrap(),
        skill_file_before
    );
    let state_after = managed_state_without_activation(root);
    #[cfg(target_os = "linux")]
    let state_after = {
        let mut state = state_after;
        remove_linux_retained_activation(&mut state, config_before.as_bytes());
        state
    };
    let mut expected_state = state_before;
    expected_state.remove("workspace_skill.yaml");
    assert_eq!(
        state_after, expected_state,
        "ensure-current must not modify unrelated managed state"
    );
}

#[test]
fn workspace_ensure_current_never_scans_or_rewrites_a_sibling_workspace() {
    let parent = tempfile::tempdir().unwrap();
    let current = parent.path().join("current");
    let sibling = parent.path().join("sibling");
    std::fs::create_dir_all(&current).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    make_rebind_eligible_identity_drift(&current);
    write(&sibling, "SKILL.md", "sibling-owned skill bytes\n");
    write(
        &sibling,
        ".RaymanCodingSkill/workspace_skill.yaml",
        "sibling-owned activation bytes\n",
    );
    write(
        &sibling,
        ".RaymanCodingSkill/goals/sentinel.json",
        "sibling-owned state\n",
    );
    let sibling_skill_before = std::fs::read(sibling.join("SKILL.md")).unwrap();
    let sibling_state_before = state_snapshot(&sibling);

    let applied = run_raw(
        &current,
        &["--format", "json", "workspace", "ensure-current", "--yes"],
    );

    assert_eq!(
        applied.status, 0,
        "stdout={} stderr={}",
        applied.stdout, applied.stderr
    );
    let applied: Value = serde_json::from_str(&applied.stdout).unwrap();
    assert_eq!(applied["status"], "active");
    assert_eq!(applied["activation_identity_changed"], true);
    assert_eq!(applied["project_files_changed"], false);
    assert_eq!(applied["other_workspaces_scanned"], false);
    assert_eq!(
        std::fs::read(sibling.join("SKILL.md")).unwrap(),
        sibling_skill_before
    );
    assert_eq!(state_snapshot(&sibling), sibling_state_before);
}

#[test]
fn workspace_ensure_current_fails_closed_without_activating_manual_states() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let state_before = state_snapshot(root);

    let check = run_raw(root, &["--format", "json", "workspace", "ensure-current"]);
    assert_eq!(
        check.status, 0,
        "stdout={} stderr={}",
        check.stdout, check.stderr
    );
    let check: Value = serde_json::from_str(&check.stdout).unwrap();
    assert_eq!(check["status"], "manual_repair_required");
    assert_eq!(check["activation"]["active"], false);
    assert_eq!(check["changed"], false);
    assert_eq!(state_snapshot(root), state_before);

    let rejected = run_raw(root, &["workspace", "ensure-current", "--yes"]);
    assert_ne!(
        rejected.status, 0,
        "stdout={} stderr={}",
        rejected.stdout, rejected.stderr
    );
    assert!(
        rejected.stderr.contains("无法安全自动修复"),
        "{}",
        rejected.stderr
    );
    assert!(!root.join(".RaymanCodingSkill").exists());
    assert_eq!(state_snapshot(root), state_before);
}

#[test]
fn workspace_rebind_rejects_ineligible_contracts_without_writing_state() {
    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, ".RaymanCodingSkill/goals/orphan.json", "orphan\n");
        assert_rebind_rejected_without_state_changes(root, "orphan");
    }

    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "SKILL.md", "canonical skill\n");
        let hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
        write(
            root,
            ".RaymanCodingSkill/workspace_skill.yaml",
            &complete_rebind_contract("raymancodingskill", false, "SKILL.md", &hash),
        );
        assert_rebind_rejected_without_state_changes(root, "disabled");
    }

    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "SKILL.md", "canonical skill\n");
        let hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
        write(
            root,
            ".RaymanCodingSkill/workspace_skill.yaml",
            &complete_rebind_contract("another-skill", true, "SKILL.md", &hash),
        );
        assert_rebind_rejected_without_state_changes(root, "wrong-skill");
    }

    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "SKILL.md", "canonical skill\n");
        write(
            root,
            ".RaymanCodingSkill/workspace_skill.yaml",
            "skill raymancodingskill\nenabled: true\nskill_file: SKILL.md\n",
        );
        assert_rebind_rejected_without_state_changes(root, "malformed");
    }

    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "SKILL.md", "canonical skill\n");
        let hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
        write(
            root,
            ".RaymanCodingSkill/workspace_skill.yaml",
            &format!(
                "skill: raymancodingskill\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {hash}\ncli_contract: {}\n",
                rayman::CLI_CONTRACT
            ),
        );
        assert_rebind_rejected_without_state_changes(root, "missing-field");
    }

    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "SKILL.md", "canonical skill\n");
        write(
            root,
            ".RaymanCodingSkill/workspace_skill.yaml",
            &complete_rebind_contract("raymancodingskill", true, "SKILL.md", "not-a-sha256"),
        );
        assert_rebind_rejected_without_state_changes(root, "invalid-hash");
    }

    {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let valid_but_stale_hash = "a".repeat(64);
        write(
            root,
            ".RaymanCodingSkill/workspace_skill.yaml",
            &complete_rebind_contract(
                "raymancodingskill",
                true,
                "missing/SKILL.md",
                &valid_but_stale_hash,
            ),
        );
        assert_rebind_rejected_without_state_changes(root, "missing-file");
    }
}

#[test]
fn eligible_identity_drift_reports_recovery_without_forcing_a_stop_hook_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    make_rebind_eligible_identity_drift(root);

    let status = run_raw(root, &["workspace", "status"]);
    assert_eq!(
        status.status, 0,
        "stdout={} stderr={}",
        status.stdout, status.stderr
    );
    assert_exact_rebind_hint(&format!("{}\n{}", status.stdout, status.stderr), "status");

    let binary = std::fs::canonicalize(BIN).unwrap();
    let binary_dir = binary.parent().unwrap();
    let doctor = run_with_path(root, &["doctor", "--check"], &[binary_dir], None);
    assert_ne!(doctor.status, 0, "stdout={}", doctor.stdout);
    assert_exact_rebind_hint(&format!("{}\n{}", doctor.stdout, doctor.stderr), "doctor");

    let stop = run_raw_with_stdin(
        root,
        &["codex-hook", "stop"],
        r#"{"hook_event_name":"Stop","stop_hook_active":false}"#,
    );
    assert_eq!(
        stop.status, 0,
        "stdout={} stderr={}",
        stop.stdout, stop.stderr
    );
    let stop: Value = serde_json::from_str(&stop.stdout).unwrap();
    assert!(stop["decision"].is_null());
    assert!(stop["reason"].is_null());
    assert_eq!(stop["continue"], true);
}

#[test]
fn workspace_activation_contract_rejects_duplicate_and_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
    std::fs::write(
        root.join(".RaymanCodingSkill/workspace_skill.yaml"),
        "skill: raymancodingskill\nenabled: true\nenabled: true\nskill_file: SKILL.md\nskill_sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .unwrap();
    let duplicate = run_raw(root, &["workspace", "status"]);
    assert_ne!(duplicate.status, 0);
    assert!(
        duplicate.stderr.contains("重复字段"),
        "{}",
        duplicate.stderr
    );

    std::fs::write(
        root.join(".RaymanCodingSkill/workspace_skill.yaml"),
        "skill: raymancodingskill\nenabled: true\nauto_use: true\nskill_file: SKILL.md\nskill_sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
    )
    .unwrap();
    let unknown = run_raw(root, &["workspace", "status"]);
    assert_ne!(unknown.status, 0);
    assert!(unknown.stderr.contains("未知字段"), "{}", unknown.stderr);
}

#[test]
fn workspace_inspect_reports_git_head_and_dirty_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "tracked.txt", "one\n");
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "--quiet"]);
    git(&["add", "tracked.txt"]);
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

    let clean = run_raw(root, &["--format", "json", "workspace", "inspect"]);
    assert_eq!(clean.status, 0, "{}", clean.stderr);
    let clean: Value = serde_json::from_str(&clean.stdout).unwrap();
    assert_eq!(clean["source"]["available"], true);
    assert_eq!(clean["source"]["clean"], true);
    assert!(clean["source"]["head"].as_str().unwrap().len() >= 40);

    write(root, "tracked.txt", "two\n");
    write(root, "new.txt", "new\n");
    let dirty = run_raw(root, &["--format", "json", "workspace", "inspect"]);
    let dirty: Value = serde_json::from_str(&dirty.stdout).unwrap();
    assert_eq!(dirty["source"]["clean"], false);
    assert_eq!(dirty["source"]["tracked_dirty"], 1);
    assert_eq!(dirty["source"]["untracked"], 1);
}

#[test]
fn workspace_snapshot_authority_records_a_zero_delta_audit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"snapshot-audit-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 42); }\n",
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "zero delta audit",
            "--must",
            "audit workspace",
        ],
    );
    let id = started["id"].as_str().unwrap();

    let validated = run_json(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "stable zero delta repository audit",
            "--command",
            "cargo test --workspace --all-targets",
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    let validation = &validated["requirements"][0]["validations"][0];
    assert_eq!(validation["workspace_snapshot"], true);
    assert_eq!(validation["non_code"], false);
    assert_eq!(validation["impact_paths"].as_array().unwrap().len(), 0);
    let authority = &validated["authority_receipts"][0];
    assert_eq!(authority["workspace_snapshot"], true);
    assert_eq!(authority["repeat"], 2);
    assert_ne!(
        validation["receipt"]["invocation_sha256"],
        authority["invocation_sha256"]
    );
}

#[test]
fn workspace_snapshot_rejects_real_delta_before_running_the_gate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "scripts/check-repo.ps1",
        "$sentinel = Join-Path (Split-Path -Parent $PSScriptRoot) 'command-ran.txt'\n[IO.File]::WriteAllText($sentinel, 'ran')\n",
    );
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "guard snapshot audit",
            "--must",
            "audit workspace",
        ],
    );
    let id = started["id"].as_str().unwrap();
    write(root, "unexpected.txt", "real delta\n");

    let rejected = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "must not run with a real delta",
            "--command",
            "pwsh -NoProfile -File scripts/check-repo.ps1",
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(rejected.status, 1, "stdout={}", rejected.stdout);
    assert!(
        rejected.stderr.contains("goal baseline delta")
            && rejected.stderr.contains("验证命令尚未执行"),
        "{}",
        rejected.stderr
    );
    assert!(!root.join("command-ran.txt").exists());
    let unchanged = run_json(root, &["goal", "show", id]);
    assert_eq!(unchanged["requirements"][0]["status"], "open");
    assert_eq!(
        unchanged["requirements"][0]["validations"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}
