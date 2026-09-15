use super::*;

#[test]
fn goal_success_close_is_refused_without_must_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "add parser",
            "--must",
            "implement",
            "--should",
            "docs",
        ],
    );
    let id = goal["id"].as_str().unwrap().to_string();

    // 无证据关闭 success：非零退出 + 明确报错。
    let denied = run(root, &["goal", "close", &id]);
    assert_eq!(denied.status, 1);
    assert!(
        denied.stderr.contains("未完成") || denied.stderr.contains("evidence"),
        "stderr={}",
        denied.stderr
    );

    // partial 允许。
    assert_eq!(
        run(root, &["goal", "close", &id, "--status", "partial"]).status,
        0
    );

    // Typed evidence alone is still insufficient; an executed receipt closes it.
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "evidence",
                &id,
                "--req",
                "req_1",
                "-m",
                "src/parser.rs done"
            ]
        )
        .status,
        0
    );
    assert_eq!(run(root, &["goal", "close", &id]).status, 1);
    validate_goal(root, &id, "req_1", "executed receipt", &[]);
    let closed = run(root, &["goal", "close", &id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);
    assert_eq!(run_json(root, &["goal", "show", &id])["status"], "success");
}

#[test]
fn goal_evidence_changed_unknown_path_records_impact_without_writing_project_map_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let project_map = root.join(".RaymanCodingSkill/context/project_map.json");
    assert!(!project_map.exists());
    let goal = run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );
    let id = goal["id"].as_str().unwrap();

    let recorded = run_json(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "missing file changed",
            "--changed",
            "no/such.rs",
            "--validated",
            "cargo test --all",
        ],
    );
    assert!(
        recorded["requirements"][0]["impacts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|impact| impact["changed_path"] == "no/such.rs"),
        "recorded={recorded}"
    );
    assert!(!project_map.exists());
}

#[test]
fn goal_evidence_changed_requires_validation_and_standard_accepts_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"impact-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[cfg(test)]\nmod tests { #[test] fn answer_is_42() { assert_eq!(super::answer(), 42); } }\n",
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );
    let id = goal["id"].as_str().unwrap();

    let missing_validation = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "src/lib.rs changed",
            "--changed",
            "src/lib.rs",
        ],
    );
    assert_eq!(missing_validation.status, 1);
    assert!(
        missing_validation.stderr.contains("--validated"),
        "stderr={}",
        missing_validation.stderr
    );

    let recorded = run_json(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "src/lib.rs changed; cargo test --all passed",
            "--changed",
            "src/lib.rs",
            "--validated",
            "cargo test --all",
        ],
    );
    assert_eq!(
        recorded["requirements"][0]["validations"][0]["command"],
        "cargo test --all"
    );
    assert_eq!(
        recorded["requirements"][0]["impacts"][0]["changed_path"],
        "src/lib.rs"
    );
    assert!(
        recorded["requirements"][0]["impacts"][0]["recommendation_basis"]
            .as_str()
            .unwrap()
            .contains("heuristic")
    );
    let validated = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "executed validation receipt",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --quiet",
        ],
    );
    assert_eq!(validated.status, 0, "stderr={}", validated.stderr);
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );
}

#[test]
fn goal_validate_failure_never_records_a_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "prove failure", "--must", "validate"],
    );
    let id = goal["id"].as_str().unwrap();

    let failed = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "expected failure",
            "--changed",
            "src/lib.rs",
            "--command",
            "rustc --crate-type lib src/lib.rs --out-dir missing-validation-output",
        ],
    );
    assert_eq!(
        failed.status, 1,
        "stdout={} stderr={}",
        failed.stdout, failed.stderr
    );
    assert!(
        failed.stderr.contains("不会写入 receipt"),
        "stderr={}",
        failed.stderr
    );
    let shown = run_json(root, &["goal", "show", id]);
    assert_eq!(shown["requirements"][0]["status"], "open");
    assert!(shown["requirements"][0]["evidence"].is_null());
    assert!(
        shown["requirements"][0]["validations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn typed_validated_claim_cannot_replace_an_executed_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "prove receipt", "--must", "validate"],
    );
    let id = goal["id"].as_str().unwrap();
    let claimed = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "typed claim only",
            "--validated",
            "cargo test --all",
        ],
    );
    assert_eq!(claimed.status, 0, "stderr={}", claimed.stderr);
    let close = run(root, &["goal", "close", id]);
    assert_eq!(close.status, 1);
    assert!(close.stderr.contains("validation receipt"));

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("仍为 active"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn source_change_after_receipt_invalidates_standard_readiness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "bind receipt", "--must", "validate"],
    );
    let id = goal["id"].as_str().unwrap();
    validate_goal(root, id, "req_1", "executed receipt", &[]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);
    assert_eq!(
        run(root, &["check", "--profile", "standard", "--goal", id]).status,
        0
    );

    write(root, "src/lib.rs", "pub fn answer() -> i32 { 43 }\n");
    run_json(root, &["context", "refresh"]);
    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard
            .stdout
            .contains("没有绑定当前工作区的成功 validation receipt"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn goal_validate_rejects_forged_shell_and_zero_test_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"receipt-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[cfg(test)]\nmod tests { #[test] fn answer_is_42() { assert_eq!(super::answer(), 42); } }\n",
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "secure receipt",
            "--must",
            "validate source",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    for command in [
        "echo cargo test",
        "cargo test || rustc --version",
        "sh -c 'cargo test'",
        "cargo test --no-run",
        "cargo test -- --list",
        "cargo test nonexistent_filter",
    ] {
        let failed = run(
            root,
            &[
                "goal",
                "validate",
                id,
                "--req",
                "req_1",
                "-m",
                "must not record",
                "--changed",
                "src/lib.rs",
                "--command",
                command,
            ],
        );
        assert_eq!(
            failed.status, 1,
            "command={command}\nstdout={}\nstderr={}",
            failed.stdout, failed.stderr
        );
    }
    let still_open = run_json(root, &["goal", "show", id]);
    assert_eq!(still_open["requirements"][0]["status"], "open");
    assert!(
        still_open["requirements"][0]["validations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // The intentionally executed zero-test command may populate ignored build
    // artifacts; refresh proves current content before the real validation.
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
            "one test actually passed",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --quiet",
        ],
    );
    let validation = &validated["requirements"][0]["validations"][0];
    assert_eq!(validation["impact_paths"][0], "src/lib.rs");
    assert!(validation["receipt"]["passed_tests"].as_u64().unwrap() >= 1);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);
    assert_eq!(run(root, &["check", "--profile", "standard"]).status, 0);
}

#[test]
fn typed_relevance_cannot_be_combined_with_an_unscoped_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"split-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "split receipt",
            "--must",
            "validate source",
        ],
    );
    let id = goal["id"].as_str().unwrap();
    validate_goal(root, id, "req_1", "unscoped receipt", &[]);
    let typed = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "typed cargo claim",
            "--changed",
            "src/lib.rs",
            "--validated",
            "cargo test",
        ],
    );
    assert_eq!(typed.status, 0, "stderr={}", typed.stderr);
    assert_eq!(run(root, &["goal", "close", id]).status, 1);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("同一条当前成功 receipt"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn goal_plan_and_review_receipts_close_a_real_two_file_delta() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.txt", "a0");
    write(root, "b.txt", "b0");
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &["goal", "start", "planned", "--must", "ship planned delta"],
    );
    let id = started["id"].as_str().unwrap();
    let planned = run_json(root, &["goal", "plan", id, "a.txt", "b.txt", "--check"]);
    assert_eq!(planned["plan_receipts"][0]["changed_paths"][0], "a.txt");

    write(root, "a.txt", "a1");
    write(root, "b.txt", "b1");
    run_json(root, &["context", "refresh"]);
    let reviewed = run_json(
        root,
        &[
            "goal",
            "review",
            id,
            "--reviewer",
            "integration-review",
            "-m",
            "reviewed final source snapshot",
        ],
    );
    assert!(reviewed["review_receipts"][0]["source_fingerprint"].is_string());
    validate_goal(
        root,
        id,
        "req_1",
        "validated exact planned delta",
        &["a.txt", "b.txt"],
    );
    let closed = run_json(root, &["goal", "close", id]);
    assert_eq!(closed["status"], "success");
}

#[test]
fn goal_evidence_is_refused_after_a_success_closure() {
    // evidence-only completion 是文档化的一层，但它写出的 validation 没有 receipt；
    // 允许它改写已关闭的 success 目标会让一条人工声明污染已完成的证据链。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(root, &["goal", "start", "closed", "--must", "validate"]);
    let id = goal["id"].as_str().unwrap();
    validate_goal(root, id, "req_1", "executed receipt", &[]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let late = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "late hand-written attestation",
        ],
    );
    assert_eq!(late.status, 1, "stdout={}", late.stdout);
    assert!(
        late.stderr.contains("已关闭为 success"),
        "stderr={}",
        late.stderr
    );
}

#[test]
fn unknown_ids_exit_nonzero_across_every_goal_subcommand() {
    // show / pending resolve 此前对未知 id 静默 exit 0（JSON 还输出裸 null），
    // 而 evidence/validate/close 对同一 id 都 exit 1。脚本用
    // `goal show $ID && ...` 判断存在性时会把"不存在"当成"查到了"。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);

    assert_eq!(run(root, &["goal", "show", "goal_missing"]).status, 1);
    assert_eq!(
        run(root, &["goal", "pending", "resolve", "pending_missing"]).status,
        1
    );
    assert_eq!(run(root, &["goal", "close", "goal_missing"]).status, 1);
}

#[test]
fn an_unmodelled_language_still_rejects_a_self_evidently_unrelated_command() {
    // 相关性检查只对 Rust/Python 建模，其余语言此前完全 fail-open：一条
    // `rayman --version` 就能当作 main.go 变更的交付证据。下限是拒掉自证
    // 无关的探针，同时不能误伤真实的 go test / make test / npm test。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "main.go", "package main\n\nfunc main() {}\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(root, &["goal", "start", "go work", "--must", "ship"]);
    let id = goal["id"].as_str().unwrap();

    // 多一个无意义参数不能把探针洗成"真实命令"。
    for probe in ["--version", "--help", "--no-pager --version"] {
        let rejected = run(
            root,
            &[
                "goal",
                "validate",
                id,
                "--req",
                "req_1",
                "-m",
                "shipped",
                "--changed",
                "main.go",
                "--command",
                &format!("rayman {probe}"),
            ],
        );
        assert_eq!(
            rejected.status, 1,
            "probe={probe} stdout={}",
            rejected.stdout
        );
    }
    let echoed = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "shipped",
            "--changed",
            "main.go",
            "--command",
            "echo done",
        ],
    );
    assert_eq!(echoed.status, 1, "stdout={}", echoed.stdout);

    // 真实命令仍然被接受——下限不是把未建模生态一律拒之门外。
    let accepted = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "shipped",
            "--changed",
            "main.go",
            "--command",
            "rustc --print sysroot",
        ],
    );
    assert_eq!(accepted.status, 0, "stderr={}", accepted.stderr);
}

#[test]
fn a_success_closure_cannot_be_downgraded_to_reopen_evidence_writes() {
    // 「已关闭 success 不能再追加人工证据」这条守卫只看当前 status，所以
    // close --status partial 降级一次就能绕过它：降级 → 追加伪造 evidence →
    // 重新关闭为 success。success 因此必须是终态。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(root, &["goal", "start", "terminal", "--must", "validate"]);
    let id = goal["id"].as_str().unwrap();
    validate_goal(root, id, "req_1", "executed receipt", &[]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let downgraded = run(root, &["goal", "close", id, "--status", "partial"]);
    assert_eq!(
        downgraded.status, 1,
        "success 必须是终态\nstdout={}",
        downgraded.stdout
    );
    assert!(
        downgraded.stderr.contains("不能降级"),
        "stderr={}",
        downgraded.stderr
    );

    // 守卫仍然拦住直接追加，且目标状态未被改动。
    assert_eq!(
        run(
            root,
            &["goal", "evidence", id, "--req", "req_1", "-m", "fabricated"]
        )
        .status,
        1
    );
    let shown = run_json(root, &["goal", "show", id]);
    assert_eq!(shown["status"], "success");
}

#[test]
fn goal_plan_extend_is_monotonic_and_rejects_post_hoc_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for name in ["a.txt", "b.txt", "c.txt"] {
        write(root, name, "baseline");
    }
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &["goal", "start", "expand", "--must", "finish safely"],
    );
    let id = started["id"].as_str().unwrap();
    run_json(root, &["goal", "plan", id, "a.txt", "--check"]);
    write(root, "a.txt", "planned change");
    let extended = run_json(root, &["goal", "plan", id, "b.txt", "--check", "--extend"]);
    let receipt = &extended["plan_receipts"][0];
    assert_eq!(receipt["extensions"].as_array().unwrap().len(), 1);
    assert_eq!(
        receipt["extensions"][0]["changed_paths"],
        serde_json::json!(["a.txt", "b.txt"])
    );

    write(root, "c.txt", "already changed");
    let rejected = run(root, &["goal", "plan", id, "c.txt", "--check", "--extend"]);
    assert_eq!(rejected.status, 1);
    assert!(rejected.stderr.contains("事后补票"), "{}", rejected.stderr);
}

#[test]
fn authority_validation_rejects_a_gate_that_mutates_the_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "data.txt", "baseline");
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"mutating-gate\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n",
    );
    write(root, "src/lib.rs", "#[test]\nfn smoke() {}\n");
    write(
        root,
        "build.rs",
        r#"fn main() { std::fs::write("data.txt", "mutated").unwrap(); }"#,
    );
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "stable", "--must", "stable gate"]);
    let id = started["id"].as_str().unwrap();
    let command = "cargo test --workspace --all-targets";
    let rejected = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "must stay stable",
            "--command",
            command,
            "--changed",
            "data.txt",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(rejected.status, 1);
    assert!(
        rejected.stderr.contains("workspace fingerprint 漂移")
            || rejected.stderr.contains("修改了工作区内容"),
        "{}",
        rejected.stderr
    );
    let shown = run_json(root, &["goal", "show", id]);
    assert!(shown["authority_receipts"].as_array().unwrap().is_empty());
}

#[test]
fn retired_commands_fail_with_actionable_migrations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    for (args, expected) in [
        (&["audit", "--check"][..], "rayman check --profile standard"),
        (
            &["workspace-skill", "mark-used"][..],
            "rayman workspace status",
        ),
        (&["subagent", "status"][..], "v2"),
        (&["context", "os", "--check"][..], "rayman context refresh"),
        (&["context", "task"][..], "rayman prepare --goal"),
    ] {
        let output = run_raw(root, args);
        assert_eq!(output.status, 1, "args={args:?}");
        assert!(
            output.stderr.contains(expected),
            "args={args:?} stderr={}",
            output.stderr
        );
    }
}

#[test]
fn goal_package_progress_summary_and_lane_fail_closed_through_cli() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn value() -> u8 { 1 }\n");
    let goal = run_json(root, &["goal", "start", "staged cli", "--must", "deliver"]);
    let id = goal["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "package",
            "add",
            id,
            "stage1",
            "focused stage",
            "--req",
            "req_1",
        ],
    );
    let progress = run_json(
        root,
        &[
            "goal",
            "progress",
            id,
            "--package",
            "stage1",
            "-m",
            "focused check",
            "--command",
            "rustc --version",
        ],
    );
    assert_eq!(progress["authoritative"], false);
    let progress_id = progress["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "package",
            "complete",
            id,
            "stage1",
            "--progress",
            progress_id,
        ],
    );
    let summary = run_json(root, &["goal", "summary", id]);
    assert_eq!(summary["completed_packages"], 1);
    assert_eq!(summary["progress_receipts"], 1);
    assert_eq!(summary["validation_receipts"], 0);

    run_json(
        root,
        &[
            "goal",
            "lane",
            "open",
            id,
            "review",
            "--mode",
            "final-reviewer",
        ],
    );
    write(root, "src/lib.rs", "pub fn value() -> u8 { 2 }\n");
    let rejected = run(root, &["goal", "lane", "close", id, "review"]);
    assert_eq!(rejected.status, 1);
    assert!(rejected.stderr.contains("只读 lane"), "{}", rejected.stderr);
}

#[test]
fn changed_repository_gate_is_rejected_before_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "scripts/check-repo.ps1", "throw 'real gate'\n");
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "repair repository gate",
            "--must-proof",
            "repository_gate::keep the gate independent",
        ],
    );
    let id = started["id"].as_str().unwrap();
    write(
        root,
        "scripts/check-repo.ps1",
        "$sentinel = Join-Path (Split-Path -Parent $PSScriptRoot) 'command-ran.txt'\n[IO.File]::WriteAllText($sentinel, 'ran')\nexit 0\n",
    );

    let rejected = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "a changed gate cannot validate itself",
            "--changed",
            "scripts/check-repo.ps1",
            "--command",
            "pwsh -NoProfile -File scripts/check-repo.ps1",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(rejected.status, 1, "stdout={}", rejected.stdout);
    assert!(
        rejected
            .stderr
            .contains("refusing a self-validating authority gate")
            && rejected.stderr.contains("scripts/check-repo.ps1"),
        "{}",
        rejected.stderr
    );
    assert!(!root.join("command-ran.txt").exists());
    let unchanged = run_json(root, &["goal", "show", id]);
    assert_eq!(unchanged["requirements"][0]["status"], "open");
    assert!(
        unchanged["requirements"][0]["validations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        unchanged["authority_receipts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn typed_must_proof_requires_a_matching_validation_command_kind() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"typed-proof-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
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
            "atomic typed proof",
            "--must-proof",
            "test::run the test suite",
            "--must-proof",
            "documentation::validate the agent contract",
        ],
    );
    let id = started["id"].as_str().unwrap();
    assert_eq!(started["requirements"][0]["proof_kind"], "test");
    assert_eq!(started["requirements"][1]["proof_kind"], "documentation");

    let wrong = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "a build is not a test proof",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo check --quiet",
        ],
    );
    assert_eq!(wrong.status, 1);
    assert!(
        wrong.stderr.contains("proof kind mismatch"),
        "stderr={}",
        wrong.stderr
    );
    let still_open = run_json(root, &["goal", "show", id]);
    assert_eq!(still_open["requirements"][0]["status"], "open");

    let test_receipt = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "test proof",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --quiet",
        ],
    );
    assert_eq!(test_receipt.status, 0, "stderr={}", test_receipt.stderr);

    let wrong_for_docs = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_2",
            "-m",
            "tests cannot prove the documentation contract",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --quiet",
        ],
    );
    assert_eq!(wrong_for_docs.status, 1);
    assert!(wrong_for_docs.stderr.contains("proof kind mismatch"));
}

#[test]
fn repository_gate_typed_must_accepts_selector_free_cargo_only_with_authority_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"typed-authority-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
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
            "selector-free cargo authority",
            "--must-proof",
            "repository_gate::prove the full workspace authority",
        ],
    );
    let id = started["id"].as_str().unwrap();

    let plain_test = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "plain workspace tests are still a test proof",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --locked --workspace --all-targets",
        ],
    );
    assert_eq!(plain_test.status, 1);
    assert!(
        plain_test.stderr.contains("proof kind mismatch"),
        "stderr={}",
        plain_test.stderr
    );

    let authority = run(
        root,
        &[
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "selector-free workspace cargo authority",
            "--changed",
            "src/lib.rs",
            "--authority",
            "--repeat",
            "2",
            "--command",
            "cargo test --locked --workspace --all-targets",
        ],
    );
    assert_eq!(authority.status, 0, "stderr={}", authority.stderr);

    let closed = run(
        root,
        &[
            "goal", "close", id, "--status", "success", "--format", "json",
        ],
    );
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);
}
