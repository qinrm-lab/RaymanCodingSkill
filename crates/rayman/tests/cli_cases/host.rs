use super::*;

#[test]
fn language_selection_preserves_utf8_unicode_paths_and_json_contract() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("中文工作区🙂");
    std::fs::create_dir_all(&root).unwrap();
    write(&root, "src/中文模块.rs", "pub fn 中文函数() {}");

    let chinese = run(&root, &["--language", "zh-CN", "context", "status"]);
    assert_eq!(chinese.status, 0, "stderr={}", chinese.stderr);
    assert!(chinese.stdout.contains("上下文索引:"), "{}", chinese.stdout);
    assert!(!chinese.stdout.contains('\u{fffd}'), "{}", chinese.stdout);

    let english = run(&root, &["context", "status", "--lang", "en"]);
    assert_eq!(english.status, 0, "stderr={}", english.stderr);
    assert!(
        english.stdout.contains("Context index:"),
        "{}",
        english.stdout
    );
    assert!(!english.stdout.contains('\u{fffd}'), "{}", english.stdout);

    let english_goal = run(
        &root,
        &[
            "--language",
            "en",
            "goal",
            "start",
            "中文目标🙂",
            "--must",
            "prove it",
        ],
    );
    assert_eq!(english_goal.status, 0, "{}", english_goal.stderr);
    assert!(
        english_goal.stdout.contains("Goal goal_")
            && english_goal.stdout.contains("created (1 requirements)"),
        "{}",
        english_goal.stdout
    );
    assert!(
        !english_goal.stdout.contains("个需求"),
        "{}",
        english_goal.stdout
    );

    let unicode_path = run(
        &root,
        &["--language", "zh-CN", "temp", "scratch", "中文-資料-🙂"],
    );
    assert_eq!(unicode_path.status, 0, "stderr={}", unicode_path.stderr);
    assert!(
        unicode_path.stdout.contains("中文工作区🙂")
            && unicode_path.stdout.contains("中文-資料-🙂"),
        "{}",
        unicode_path.stdout
    );

    let chinese_json = run_raw(
        &root,
        &[
            "--format",
            "json",
            "--language",
            "zh-CN",
            "workspace",
            "inspect",
        ],
    );
    let english_json = run_raw(
        &root,
        &[
            "--format",
            "json",
            "--language",
            "en",
            "workspace",
            "inspect",
        ],
    );
    assert_eq!(chinese_json.status, 0, "stderr={}", chinese_json.stderr);
    assert_eq!(english_json.status, 0, "stderr={}", english_json.stderr);
    let chinese_value: Value = serde_json::from_str(&chinese_json.stdout).unwrap();
    let english_value: Value = serde_json::from_str(&english_json.stdout).unwrap();
    assert_eq!(chinese_value, english_value);
    let contains_han = |text: &str| {
        text.chars().any(|character| {
            matches!(character as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff)
        })
    };
    for arguments in [
        vec!["--language", "en", "--help"],
        vec!["--language", "en", "workspace", "--help"],
        vec!["--language", "en", "codex-hook", "--help"],
        vec!["--language", "en", "checkpoint", "--help"],
        vec!["--language", "en", "autosave", "--help"],
        vec!["--language", "en", "context", "--help"],
        vec!["--language", "en", "map", "--help"],
        vec!["--language", "en", "goal", "--help"],
        vec!["--language", "en", "state", "--help"],
        vec!["--language", "en", "temp", "--help"],
        vec!["--language", "en", "doctor", "--help"],
        vec!["--language", "en", "check", "--help"],
        vec!["--language", "en", "prepare", "--help"],
        vec!["--language", "en", "finish", "--help"],
    ] {
        let help = run_raw(&root, &arguments);
        assert_eq!(help.status, 0, "stderr={}", help.stderr);
        assert!(!contains_han(&help.stdout), "{}", help.stdout);
    }
    let parse_error = run_raw(&root, &["--language", "en", "--definitely-invalid"]);
    assert_ne!(parse_error.status, 0);
    assert!(!contains_han(&parse_error.stderr), "{}", parse_error.stderr);
    for arguments in [
        vec!["--language", "en", "context", "refresh"],
        vec!["--language", "en", "map", "summary"],
        vec!["--language", "en", "map", "quality"],
        vec!["--language", "en", "state", "audit"],
        vec!["--language", "en", "assets"],
        vec!["--language", "en", "doctor"],
    ] {
        let output = run(&root, &arguments);
        assert_eq!(output.status, 0, "stderr={}", output.stderr);
        let fixed_stdout = output.stdout.replace("src/中文模块.rs", "<dynamic-path>");
        let fixed_stderr = output.stderr.replace("src/中文模块.rs", "<dynamic-path>");
        assert!(!contains_han(&fixed_stdout), "{}", output.stdout);
        assert!(!contains_han(&fixed_stderr), "{}", output.stderr);
    }
    for arguments in [
        vec!["--language", "en", "map", "file", "missing.rs"],
        vec!["--language", "en", "goal", "show", "goal_missing"],
        vec!["--language", "en", "checkpoint", "verify", "missing"],
        vec!["--language", "en", "doctor", "--check"],
        vec!["--language", "en", "autosave", "status"],
    ] {
        let output = run(&root, &arguments);
        let fixed_stdout = output.stdout.replace("src/中文模块.rs", "<dynamic-path>");
        let fixed_stderr = output.stderr.replace("src/中文模块.rs", "<dynamic-path>");
        assert!(!contains_han(&fixed_stdout), "{}", output.stdout);
        assert!(!contains_han(&fixed_stderr), "{}", output.stderr);
    }
}

#[test]
fn doctor_verifies_installed_identity_in_an_ordinary_managed_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_canonical_bundle(root);
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &current_activation_contract(&skill_hash),
    );
    let binary = std::fs::canonicalize(BIN).unwrap();
    let binary_dir = binary.parent().unwrap();

    let output = run_with_path(
        root,
        &["--format", "json", "doctor", "--check"],
        &[binary_dir],
        None,
    );

    assert_eq!(
        output.status, 0,
        "stdout={} stderr={}",
        output.stdout, output.stderr
    );
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(report["release_identity"]["ready"], true);
    assert_eq!(report["doctor_check"]["ready"], true);
    assert_eq!(report["doctor_check"]["context_requirement_present"], false);
    #[cfg(windows)]
    {
        assert!(
            report["execution_context"]["principal_fingerprint"]
                .as_str()
                .is_some_and(|value| !value.is_empty()),
            "{report}"
        );
        assert_eq!(report["execution_context"]["status"], "not_required");
    }
    #[cfg(not(windows))]
    {
        assert!(report["execution_context"]["principal_fingerprint"].is_null());
        assert_eq!(report["execution_context"]["status"], "not_applicable");
    }
    assert_eq!(report["repo_release"]["checked"], false);
    assert_eq!(report["repo_release"]["status"], "not_checked_by_doctor");
}

#[cfg(windows)]
#[test]
fn doctor_rejects_an_earlier_windows_path_wrapper() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write_canonical_bundle(root);
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &current_activation_contract(&skill_hash),
    );
    let wrapper_dir = tempfile::tempdir().unwrap();
    write(wrapper_dir.path(), "rayman.cmd", "@echo wrong wrapper\r\n");
    let binary = std::fs::canonicalize(BIN).unwrap();
    let binary_dir = binary.parent().unwrap();

    let output = run_with_path(
        root,
        &["--format", "json", "doctor", "--check"],
        &[wrapper_dir.path(), binary_dir],
        Some(".COM;.EXE;.BAT;.CMD"),
    );

    assert_ne!(output.status, 0, "stdout={}", output.stdout);
    assert!(
        output.stderr.contains("已安装身份契约不一致"),
        "stderr={}",
        output.stderr
    );
}

#[test]
fn doctor_and_workspace_inspect_report_distinct_write_probes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_canonical_bundle(root);
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    let activation_path = root.join(".RaymanCodingSkill/workspace_skill.yaml");
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &current_activation_contract(&skill_hash),
    );
    let activation_before = std::fs::read(&activation_path).unwrap();
    let binary = std::fs::canonicalize(BIN).unwrap();
    let binary_dir = binary.parent().unwrap();

    let doctor = run_with_path(root, &["--format", "json", "doctor"], &[binary_dir], None);
    assert_eq!(
        doctor.status, 0,
        "stdout={} stderr={}",
        doctor.stdout, doctor.stderr
    );
    let report: Value = serde_json::from_str(&doctor.stdout).unwrap();
    assert_eq!(report["state_write"]["state_dir_present"], true);
    assert_eq!(report["state_write"]["probed"], false);
    assert_eq!(report["state_write"]["writable"], false);
    assert_eq!(report["activation_metadata"]["applicable"], true);
    assert_eq!(report["activation_metadata"]["probed"], false);
    assert_eq!(report["activation_metadata"]["ready"], false);

    let doctor_probe = run_with_path(
        root,
        &["--format", "json", "doctor", "--probe-writes"],
        &[binary_dir],
        None,
    );
    assert_eq!(doctor_probe.status, 0, "{}", doctor_probe.stderr);
    let report: Value = serde_json::from_str(&doctor_probe.stdout).unwrap();
    assert_eq!(report["state_write"]["probed"], true);
    assert_eq!(report["state_write"]["writable"], true);
    assert_eq!(report["activation_metadata"]["probed"], true);
    assert_eq!(report["activation_metadata"]["ready"], true);
    assert_eq!(
        report["activation_metadata"]["capability_key"],
        "activation/metadata-preserving-staging"
    );
    assert_eq!(report["activation_metadata"]["activation_unchanged"], true);
    assert_eq!(report["activation_metadata"]["cleanup_complete"], true);

    let inspect = run_raw(root, &["--format", "json", "workspace", "inspect"]);
    assert_eq!(inspect.status, 0, "{}", inspect.stderr);
    let inspect: Value = serde_json::from_str(&inspect.stdout).unwrap();
    assert_eq!(inspect["state_write"]["probed"], false);
    assert_eq!(inspect["activation_metadata"]["probed"], false);

    let inspect = run_raw(
        root,
        &["--format", "json", "workspace", "inspect", "--probe-writes"],
    );
    assert_eq!(inspect.status, 0, "{}", inspect.stderr);
    let inspect: Value = serde_json::from_str(&inspect.stdout).unwrap();
    assert_eq!(inspect["state_write"]["probed"], true);
    assert_eq!(inspect["state_write"]["writable"], true);
    assert_eq!(inspect["activation_metadata"]["probed"], true);
    assert_eq!(inspect["activation_metadata"]["ready"], true);
    assert_eq!(inspect["activation_metadata"]["phase"], "complete");
    assert!(inspect["execution_context"]["status"].is_string());

    let status = run_raw(root, &["--format", "json", "workspace", "status"]);
    assert_eq!(status.status, 0, "{}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert!(
        status.get("activation_metadata").is_none(),
        "workspace status must remain the activation report only"
    );

    let inspect_en = run_raw(
        root,
        &["--language", "en", "workspace", "inspect", "--probe-writes"],
    );
    assert_eq!(inspect_en.status, 0, "{}", inspect_en.stderr);
    assert!(
        inspect_en.stdout.contains("state-write probe: writable"),
        "stdout={}",
        inspect_en.stdout
    );
    assert!(
        inspect_en
            .stdout
            .contains("activation-metadata write probe: ready"),
        "stdout={}",
        inspect_en.stdout
    );
    assert_eq!(std::fs::read(&activation_path).unwrap(), activation_before);
    assert!(
        std::fs::read_dir(root.join(".RaymanCodingSkill"))
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| {
                !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".workspace_skill.yaml.rayman-")
            }),
        "activation metadata probe left a named sidecar"
    );

    // A workspace without a state root must be reported unprobed, not mutated.
    let bare = tempfile::tempdir().unwrap();
    let bare_inspect = run_raw(bare.path(), &["--format", "json", "workspace", "inspect"]);
    assert_eq!(bare_inspect.status, 0, "{}", bare_inspect.stderr);
    let bare_inspect: Value = serde_json::from_str(&bare_inspect.stdout).unwrap();
    assert_eq!(bare_inspect["state_write"]["state_dir_present"], false);
    assert_eq!(bare_inspect["state_write"]["probed"], false);
    assert_eq!(bare_inspect["activation_metadata"]["applicable"], false);
    assert_eq!(bare_inspect["activation_metadata"]["probed"], false);
    assert!(!bare.path().join(".RaymanCodingSkill").exists());
}

#[cfg(windows)]
#[test]
fn doctor_check_rejects_an_unsatisfied_untrusted_context_requirement_without_changing_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_canonical_bundle(root);
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &current_activation_contract(&skill_hash),
    );
    let binary = std::fs::canonicalize(BIN).unwrap();
    let binary_dir = binary.parent().unwrap();
    let mut entries = vec![binary_dir.to_path_buf()];
    entries.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(entries).unwrap();
    let output = rayman_command()
        .args(["--format", "json", "doctor", "--check"])
        .current_dir(root)
        .env("PATH", path)
        .env("RAYMAN_REQUIRED_PRINCIPAL", "RAYMAN_TEST\\NotTheTokenUser")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!output.status.success(), "stdout={stdout} stderr={stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["execution_context"]["status"], "principal_mismatch");
    assert_eq!(
        report["execution_context"]["requirement_source"],
        "process_environment_untrusted"
    );
    assert_eq!(report["release_identity"]["ready"], true);
    assert_eq!(report["doctor_check"]["ready"], false);
    assert_eq!(report["doctor_check"]["identity_ready"], true);
    assert_eq!(report["doctor_check"]["context_ready"], false);
    assert!(
        stderr.contains("execution-context requirement 未满足"),
        "stderr={stderr}"
    );
}

#[cfg(windows)]
#[test]
fn doctor_profile_requirement_uses_the_token_profile_not_userprofile() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write_canonical_bundle(root);
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &current_activation_contract(&skill_hash),
    );
    let binary = std::fs::canonicalize(BIN).unwrap();
    let binary_dir = binary.parent().unwrap();
    let mut entries = vec![binary_dir.to_path_buf()];
    entries.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path = std::env::join_paths(entries).unwrap();

    let baseline = rayman_command()
        .args(["--format", "json", "doctor"])
        .current_dir(root)
        .env("PATH", &path)
        .env_remove("RAYMAN_REQUIRED_SID")
        .env_remove("RAYMAN_REQUIRED_PRINCIPAL")
        .env_remove("RAYMAN_REQUIRED_PROFILE")
        .output()
        .unwrap();
    assert!(baseline.status.success());
    let baseline: Value = serde_json::from_slice(&baseline.stdout).unwrap();
    let forged_profile = root.join("forged-profile").display().to_string();
    let Some(token_profile) = baseline["execution_context"]["token_profile"]
        .as_str()
        .map(str::to_string)
    else {
        // Restricted service/sandbox identities may have a real process token
        // but no registered Windows profile directory. That is an observable
        // Unknown result, not permission to fall back to attacker-controlled
        // USERPROFILE.
        let unavailable = rayman_command()
            .args(["--format", "json", "doctor", "--check"])
            .current_dir(root)
            .env("PATH", &path)
            .env("USERPROFILE", &forged_profile)
            .env("RAYMAN_REQUIRED_PROFILE", &forged_profile)
            .env_remove("RAYMAN_REQUIRED_SID")
            .env_remove("RAYMAN_REQUIRED_PRINCIPAL")
            .output()
            .unwrap();
        assert!(!unavailable.status.success());
        let report: Value = serde_json::from_slice(&unavailable.stdout).unwrap();
        assert_eq!(report["execution_context"]["status"], "unknown");
        assert_eq!(report["execution_context"]["profile_match"], "unknown");
        assert_eq!(report["execution_context"]["token_profile"], Value::Null);
        assert_eq!(
            report["execution_context"]["environment_profile"],
            forged_profile
        );
        assert_eq!(report["doctor_check"]["ready"], false);
        return;
    };
    assert_ne!(
        token_profile.to_ascii_lowercase(),
        forged_profile.to_ascii_lowercase()
    );

    let forged = rayman_command()
        .args(["--format", "json", "doctor", "--check"])
        .current_dir(root)
        .env("PATH", &path)
        .env("USERPROFILE", &forged_profile)
        .env("RAYMAN_REQUIRED_PROFILE", &forged_profile)
        .env_remove("RAYMAN_REQUIRED_SID")
        .env_remove("RAYMAN_REQUIRED_PRINCIPAL")
        .output()
        .unwrap();
    assert!(!forged.status.success());
    let forged_report: Value = serde_json::from_slice(&forged.stdout).unwrap();
    assert_eq!(
        forged_report["execution_context"]["status"],
        "profile_mismatch"
    );
    assert_eq!(
        forged_report["execution_context"]["environment_profile"],
        forged_profile
    );
    assert_eq!(
        forged_report["execution_context"]["token_profile"],
        token_profile
    );
    assert_eq!(
        forged_report["execution_context"]["environment_profile_matches_token"],
        false
    );
    assert_eq!(forged_report["doctor_check"]["ready"], false);

    let token_bound = rayman_command()
        .args(["--format", "json", "doctor", "--check"])
        .current_dir(root)
        .env("PATH", path)
        .env("USERPROFILE", &forged_profile)
        .env("RAYMAN_REQUIRED_PROFILE", &token_profile)
        .env_remove("RAYMAN_REQUIRED_SID")
        .env_remove("RAYMAN_REQUIRED_PRINCIPAL")
        .output()
        .unwrap();
    assert!(
        token_bound.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&token_bound.stdout),
        String::from_utf8_lossy(&token_bound.stderr)
    );
    let token_bound_report: Value = serde_json::from_slice(&token_bound.stdout).unwrap();
    assert_eq!(token_bound_report["execution_context"]["status"], "match");
    assert_eq!(token_bound_report["doctor_check"]["ready"], true);
    assert_eq!(
        token_bound_report["execution_context"]["environment_profile_matches_token"],
        false
    );
}

#[test]
fn codex_stop_hook_blocks_active_goal_and_installs_idempotently() {
    let inactive = tempfile::tempdir().unwrap();
    let allowed = run_raw_with_stdin(
        inactive.path(),
        &["codex-hook", "stop"],
        r#"{"hook_event_name":"Stop","stop_hook_active":false}"#,
    );
    assert_eq!(allowed.status, 0, "{}", allowed.stderr);
    let allowed: Value = serde_json::from_str(&allowed.stdout).unwrap();
    assert_eq!(allowed["continue"], true);

    let workspace = tempfile::tempdir().unwrap();
    let goal = run_json(
        workspace.path(),
        &[
            "goal",
            "start",
            "whole program",
            "--must",
            "original requirement",
            "--must",
            "mid-turn addition",
        ],
    );
    let goal_id = goal["id"].as_str().unwrap();
    let blocked = run_raw_with_stdin(
        workspace.path(),
        &["codex-hook", "stop"],
        r#"{"hook_event_name":"Stop","stop_hook_active":false}"#,
    );
    assert_eq!(blocked.status, 0, "{}", blocked.stderr);
    let blocked: Value = serde_json::from_str(&blocked.stdout).unwrap();
    assert_eq!(blocked["decision"], "block");
    assert!(blocked["reason"].as_str().unwrap().contains(goal_id));

    let codex_home = tempfile::tempdir().unwrap();
    std::fs::write(
        codex_home.path().join("hooks.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other","statusMessage":"Other"}]}]}}"#,
    )
    .unwrap();
    let home = codex_home.path().to_str().unwrap();
    for _ in 0..2 {
        let installed = run_raw(
            workspace.path(),
            &[
                "--format",
                "json",
                "codex-hook",
                "install",
                "--codex-home",
                home,
                "--yes",
            ],
        );
        assert_eq!(installed.status, 0, "{}", installed.stderr);
    }
    let status = run_raw(
        workspace.path(),
        &[
            "--format",
            "json",
            "codex-hook",
            "status",
            "--codex-home",
            home,
        ],
    );
    assert_eq!(status.status, 0, "{}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(status["installed"], true);
    let hooks = std::fs::read_to_string(codex_home.path().join("hooks.json")).unwrap();
    assert!(hooks.contains("Other"));
    assert_eq!(
        hooks.matches("Rayman Owner Mode completion guard").count(),
        1
    );
}
