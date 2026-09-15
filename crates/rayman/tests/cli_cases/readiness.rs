// `goal_planning_gaps` 的分支在 check 侧此前只有 1 个有端到端证明，其余只被
// helper 级单元测试覆盖——正是今天那三条 high 的形态（helper 全绿、调用方没接线）。
// 这三个用例证明这些分支会真的让 `check` 拦下来。

use super::*;

#[test]
fn context_status_transitions_missing_ready_stale() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/a.rs", "fn a() {}");

    assert_eq!(run_json(root, &["context", "status"])["status"], "missing");
    run(root, &["context", "refresh"]);
    assert_eq!(run_json(root, &["context", "status"])["status"], "ready");
    write(root, "src/b.rs", "fn b() {}");
    let stale = run_json(root, &["context", "status"]);
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["added"], serde_json::json!(["src/b.rs"]));
}

#[test]
fn unbound_standard_check_reports_active_goal_without_blocking_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );

    // Bare `check` is workspace health. It reports unfinished goals, but task
    // completion is enforced only when a goal is explicitly bound.
    let default_profile = run(root, &["--format", "json", "check"]);
    assert_eq!(
        default_profile.status, 0,
        "stderr={}",
        default_profile.stderr
    );
    let default_json: Value = serde_json::from_str(&default_profile.stdout).unwrap();
    assert_eq!(default_json["workspace_ready"], true);
    assert_eq!(default_json["task"]["requested"], false);
    assert!(
        default_json["standard"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("not task-ready")),
        "stdout={}",
        default_profile.stdout
    );
    let current = run_json(root, &["goal", "current"]);
    let id = current[0]["id"].as_str().unwrap();
    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("active goal") && standard.stdout.contains("must"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_active_goal_even_with_validated_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );
    let id = goal["id"].as_str().unwrap();
    write(
        root,
        "README.md",
        "generic nested validation fixture changed\n",
    );
    run_json(root, &["context", "refresh"]);

    let recorded = run(
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
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("仍为 active") && standard.stdout.contains("goal close"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_done_requirement_without_validation_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );
    let id = goal["id"].as_str().unwrap();

    let recorded = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "src/lib.rs changed; claimed validation",
        ],
    );
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 1, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("缺少验证 receipt")
            && standard.stdout.contains("任务阻断")
            && standard.stdout.contains("standard blockers: 0"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_done_requirement_without_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/goal_manual.json",
        r#"{
  "schema_version": 2,
  "id": "goal_manual",
  "title": "manual goal",
  "status": "success",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z",
  "requirements": [
    {
      "id": "req_1",
      "text": "manual requirement",
      "kind": "must",
      "status": "done",
      "validations": [
        {
          "command": "cargo test --all",
          "recorded_at": "2026-01-01T00:00:00Z"
        }
      ],
      "impacts": []
    }
  ]
}"#,
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_manual"],
    );
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal_manual") && standard.stdout.contains("缺少 evidence 文本"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_partial_goal_without_structured_validation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );
    let id = goal["id"].as_str().unwrap();

    let recorded = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "src/lib.rs changed; claimed validation",
        ],
    );
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);
    let closed = run(root, &["goal", "close", id, "--status", "partial"]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("状态为 partial") && standard.stdout.contains("缺少验证 receipt"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_unreadable_goal_file_instead_of_skipping_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/bad.json",
        "{ definitely not json",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal 文件不可读取"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_invalid_goals_store_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    std::fs::write(root.join(".RaymanCodingSkill/goals"), "not a directory").unwrap();

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal 文件不可读取"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_does_not_write_project_map_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let project_map = root.join(".RaymanCodingSkill/context/project_map.json");
    std::fs::write(&project_map, "sentinel project map cache").unwrap();

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );
    assert_eq!(
        std::fs::read_to_string(&project_map).unwrap(),
        "sentinel project map cache"
    );
}

#[test]
fn release_check_does_not_write_project_map_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let project_map = root.join(".RaymanCodingSkill/context/project_map.json");
    std::fs::write(&project_map, "sentinel project map cache").unwrap();

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(
        release.status, 0,
        "stdout={} stderr={}",
        release.stdout, release.stderr
    );
    assert_eq!(
        std::fs::read_to_string(&project_map).unwrap(),
        "sentinel project map cache"
    );
}

#[test]
fn release_check_reports_workspace_scope_not_installed_release_contract() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);

    let report = run_json(root, &["check", "--profile", "release"]);

    assert_eq!(report["ready"], true);
    assert_eq!(report["readiness_scope"], "workspace_strict_quality");
    assert_eq!(report["release_contract"]["checked"], false);
    assert_eq!(report["release_contract"]["status"], "not_checked");
    assert!(
        report["release_contract"]["required_verifier"]
            .as_str()
            .is_some_and(|command| command.contains("RequireSourceFresh")),
        "{report}"
    );
}

#[test]
fn standard_check_does_not_change_state_tree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &["goal", "start", "docs update", "--must", "record evidence"],
    );
    let goals = run_json(root, &["goal", "list"]);
    assert!(goals[0].get("baseline").is_none());
    assert_eq!(goals[0]["schema"], "rayman.goal-compact.v1");
    assert!(goals[0]["baseline_files"].as_u64().is_some());
    assert!(goals[0]["requirements"].as_u64().is_some());
    let id = goals[0]["id"].as_str().unwrap();
    run(
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
    validate_goal(
        root,
        id,
        "req_1",
        "executed validation receipt",
        &["src/lib.rs"],
    );
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);
    let before = state_snapshot(root);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );
    assert_eq!(state_snapshot(root), before);
}

#[test]
fn release_check_does_not_change_state_tree() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &["goal", "start", "docs update", "--must", "record evidence"],
    );
    let goals = run_json(root, &["goal", "list"]);
    let id = goals[0]["id"].as_str().unwrap();
    run(
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
    validate_goal(
        root,
        id,
        "req_1",
        "executed validation receipt",
        &["src/lib.rs"],
    );
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);
    let before = state_snapshot(root);

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(
        release.status, 0,
        "stdout={} stderr={}",
        release.stdout, release.stderr
    );
    assert_eq!(state_snapshot(root), before);
}

#[test]
fn standard_check_accepts_done_requirement_with_validation_and_no_impact_warning() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "docs only\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "docs update", "--must", "record evidence"],
    );
    let id = goal["id"].as_str().unwrap();

    let recorded = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "README.md changed; docs reviewed",
            "--validated",
            "docs reviewed",
        ],
    );
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);
    validate_goal(root, id, "req_1", "executed validation receipt", &[]);
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );
    assert!(
        standard.stdout.contains("standard warnings: 1"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_irrelevant_validation_for_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "wire relevant validation",
            "--must",
            "record evidence",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    run_json(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "src/lib.rs changed; docs reviewed",
            "--changed",
            "src/lib.rs",
            "--validated",
            "docs reviewed",
        ],
    );
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 1, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("validation 不覆盖 src/lib.rs"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_accepts_rust_validation_for_cargo_manifest_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "update manifest",
            "--must",
            "record evidence",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    let recorded = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "Cargo.toml changed; cargo test --all passed",
            "--changed",
            "Cargo.toml",
            "--validated",
            "cargo test --all",
        ],
    );
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);
    validate_goal(
        root,
        id,
        "req_1",
        "executed validation receipt",
        &["Cargo.toml"],
    );
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
fn standard_check_accepts_rust_validation_for_cargo_lock_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "update lockfile",
            "--must",
            "record evidence",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    let recorded = run(
        root,
        &[
            "goal",
            "evidence",
            id,
            "--req",
            "req_1",
            "-m",
            "Cargo.lock changed; cargo check --all passed",
            "--changed",
            "Cargo.lock",
            "--validated",
            "cargo check --all",
        ],
    );
    assert_eq!(recorded.status, 0, "stderr={}", recorded.stderr);
    validate_goal(
        root,
        id,
        "req_1",
        "executed validation receipt",
        &["Cargo.lock"],
    );
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
fn release_check_fails_closed_on_corrupt_quality_config() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(root, ".RaymanCodingSkill/quality.json", "{ not json");
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(release.status, 1);
    assert!(
        release.stderr.contains("quality.json"),
        "stderr={}",
        release.stderr
    );
}

#[test]
fn standard_check_rejects_a_forged_v2_success_goal_without_must_requirements() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/goal_forged.json",
        r#"{
  "schema_version": 2,
  "id": "goal_forged",
  "title": "forged success",
  "status": "success",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z",
  "requirements": []
}"#,
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_forged"],
    );
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal_forged") && standard.stdout.contains("至少需要一个 must"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_rejects_an_unknown_nonzero_goal_schema() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    write(
        root,
        ".RaymanCodingSkill/goals/goal_future.json",
        r#"{
  "schema_version": 3,
  "id": "goal_future",
  "title": "unknown schema",
  "status": "success",
  "created_at": "2026-01-01T00:00:00Z",
  "updated_at": "2026-01-01T00:00:00Z",
  "requirements": [
    {
      "id": "req_1",
      "text": "must",
      "kind": "must",
      "status": "done",
      "evidence": "claimed",
      "validations": [],
      "impacts": []
    }
  ]
}"#,
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_future"],
    );
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal_future")
            && standard.stdout.contains("不支持的 goal schema_version=3"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn check_rejects_undeclared_drift_that_close_would_also_reject() {
    // 交付门禁曾经只做逐需求的 receipt 新鲜度检查，整目标级的差量门禁只在
    // `goal close` 里跑；而 close 不会重置 status，已关闭的 success 目标可以
    // 原地反复重新验证，于是"receipts 必须共同声明真实 delta"在 check/finish
    // 上彻底失效——同一状态下 close 拒绝、check 却报 ready。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.txt", "a0");
    write(root, "b.txt", "b0");
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &["goal", "start", "drift", "--must", "ship planned delta"],
    );
    let id = started["id"].as_str().unwrap();
    run_json(root, &["goal", "plan", id, "a.txt", "b.txt", "--check"]);

    write(root, "a.txt", "a1");
    run_json(root, &["context", "refresh"]);
    validate_goal(root, id, "req_1", "validated a.txt", &["a.txt"]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);
    assert_eq!(
        run(root, &["check", "--profile", "standard", "--goal", id]).status,
        0
    );

    // b.txt 在 immutable plan 之内，所以重新 validate 会被接受，但它的实际改动
    // 从未被任何 receipt 声明过。
    write(root, "b.txt", "b1-undeclared");
    run_json(root, &["context", "refresh"]);
    validate_goal(root, id, "req_1", "revalidated only a.txt", &["a.txt"]);

    let checked = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(
        checked.status, 1,
        "check must not report ready while b.txt is undeclared\nstdout={}",
        checked.stdout
    );
    assert!(
        checked
            .stdout
            .contains("实际变更未被当前 validation receipt 声明")
            && checked.stdout.contains("b.txt"),
        "stdout={}",
        checked.stdout
    );

    let finished = run(root, &["finish", "--goal", id]);
    assert_eq!(finished.status, 1, "stdout={}", finished.stdout);

    // 对照：close 在同一状态下本来就拒绝，证明两条路径现在一致。
    assert_eq!(run(root, &["goal", "close", id]).status, 1);
}

#[test]
fn check_blocks_a_multi_file_delta_that_has_no_plan_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.txt", "a0");
    write(root, "b.txt", "b0");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "unplanned", "--must", "ship"]);
    let id = started["id"].as_str().unwrap();

    write(root, "a.txt", "a1");
    run_json(root, &["context", "refresh"]);
    validate_goal(root, id, "req_1", "validated a.txt", &["a.txt"]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    // 第二个文件让实际变更达到 2 个，而全程没有 plan receipt。
    write(root, "b.txt", "b1");
    run_json(root, &["context", "refresh"]);
    let checked = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(checked.status, 1, "stdout={}", checked.stdout);
    assert!(
        checked
            .stdout
            .contains("缺少首次修改前的 goal plan receipt"),
        "stdout={}",
        checked.stdout
    );
}

#[test]
fn check_blocks_a_change_outside_the_immutable_plan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.txt", "a0");
    write(root, "c.txt", "c0");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "scoped", "--must", "ship"]);
    let id = started["id"].as_str().unwrap();
    run_json(root, &["goal", "plan", id, "a.txt", "--check"]);

    write(root, "a.txt", "a1");
    run_json(root, &["context", "refresh"]);
    validate_goal(root, id, "req_1", "validated a.txt", &["a.txt"]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    // c.txt 从不在 plan 里。
    write(root, "c.txt", "c1");
    run_json(root, &["context", "refresh"]);
    let checked = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(checked.status, 1, "stdout={}", checked.stdout);
    assert!(
        checked.stdout.contains("实际变更超出 plan") && checked.stdout.contains("c.txt"),
        "stdout={}",
        checked.stdout
    );
}

#[test]
fn check_blocks_a_baseline_less_goal_instead_of_reporting_ready() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.txt", "a0");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "no-baseline", "--must", "ship"]);
    let id = started["id"].as_str().unwrap();
    write(root, "a.txt", "a1");
    run_json(root, &["context", "refresh"]);
    validate_goal(root, id, "req_1", "validated a.txt", &["a.txt"]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);
    assert_eq!(
        run(root, &["check", "--profile", "standard", "--goal", id]).status,
        0
    );

    // 旧版本写下的 v2 记录就是这个形态：加 baseline 字段时没有升 schema 版本，
    // 且字段是 #[serde(default)] Option，所以纯升级路径即可产生它。
    let goal_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&goal_path).unwrap()).unwrap();
    stored.as_object_mut().unwrap().remove("baseline");
    std::fs::write(&goal_path, serde_json::to_string_pretty(&stored).unwrap()).unwrap();

    let checked = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(checked.status, 1, "stdout={}", checked.stdout);
    assert!(
        checked.stdout.contains("缺少开工 baseline"),
        "stdout={}",
        checked.stdout
    );
}

#[test]
fn check_blocks_a_goal_whose_baseline_manifest_was_tampered_with() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "a.txt", "a0");
    write(root, "keep.txt", "k0");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "tampered", "--must", "ship"]);
    let id = started["id"].as_str().unwrap();
    write(root, "a.txt", "a1");
    run_json(root, &["context", "refresh"]);
    validate_goal(root, id, "req_1", "validated a.txt", &["a.txt"]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    // baseline.files 与 baseline.workspace_fingerprint 必须自洽；手改文件清单
    // 就能伪造"什么都没变"，所以门禁要先验证这一对是否匹配。
    let goal_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&goal_path).unwrap()).unwrap();
    stored["baseline"]["files"]
        .as_object_mut()
        .unwrap()
        .remove("keep.txt")
        .expect("baseline must have recorded keep.txt");
    std::fs::write(&goal_path, serde_json::to_string_pretty(&stored).unwrap()).unwrap();

    // 实际由更靠前的 schema 重校验拦下（goal_planning_gaps 里同义的那个分支因此
    // 是够不到的兜底）。这里断言真实生效的那一层，不去断言被遮住的文案。
    let checked = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(checked.status, 1, "stdout={}", checked.stdout);
    assert!(
        checked
            .stdout
            .contains("baseline fingerprint 与文件清单不匹配"),
        "stdout={}",
        checked.stdout
    );
}

#[test]
fn check_blocks_a_high_priority_plan_whose_review_went_stale() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let planned: Vec<String> = (0..8).map(|i| format!("f{i}.txt")).collect();
    for path in &planned {
        write(root, path, "v0");
    }
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "wide", "--must", "ship"]);
    let id = started["id"].as_str().unwrap();

    let mut plan_args = vec!["goal", "plan", id];
    plan_args.extend(planned.iter().map(String::as_str));
    plan_args.push("--check");
    let plan = run_json(root, &plan_args);
    assert_eq!(
        plan["plan_receipts"][0]["review_priority"], "high",
        "8 个受影响路径必须落到 high 档，否则这个用例测不到 review 绑定"
    );

    for path in &planned {
        write(root, path, "v1");
    }
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &[
            "goal",
            "review",
            id,
            "--reviewer",
            "integration",
            "-m",
            "reviewed the wide change",
        ],
    );
    let changed: Vec<&str> = planned.iter().map(String::as_str).collect();
    validate_goal(root, id, "req_1", "validated the wide change", &changed);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);
    assert_eq!(
        run(root, &["check", "--profile", "standard", "--goal", id]).status,
        0
    );

    // 源码再动一次，之前那份 review receipt 就不再绑定当前 fingerprint。
    write(root, "f0.txt", "v2");
    run_json(root, &["context", "refresh"]);
    let checked = run(root, &["check", "--profile", "standard", "--goal", id]);
    assert_eq!(checked.status, 1, "stdout={}", checked.stdout);
    assert!(
        checked
            .stdout
            .contains("high-priority plan 缺少绑定最终源码 fingerprint 的 review receipt"),
        "stdout={}",
        checked.stdout
    );
}

#[test]
fn task_bound_check_prepare_and_finish_distinguish_task_from_workspace_readiness() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"authority-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 42); }\n",
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);

    let retired = run_json(
        root,
        &[
            "goal",
            "start",
            "retired boundary",
            "--must",
            "obtain owner input",
        ],
    );
    let retired_id = retired["id"].as_str().unwrap();
    let retired_pending = add_complete_human_pending(
        root,
        retired_id,
        "owner/retired-readiness",
        "historical owner choice",
    );
    assert_eq!(
        run(root, &["goal", "close", retired_id, "--status", "blocked"],).status,
        0
    );
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "archive",
                retired_id,
                "--reason",
                "retired boundary retained",
            ],
        )
        .status,
        0
    );

    let unbound = run(
        root,
        &[
            "--format",
            "json",
            "check",
            "--profile",
            "standard",
            "--require-current-goal",
        ],
    );
    assert_eq!(unbound.status, 1);
    let unbound: Value = serde_json::from_str(&unbound.stdout).unwrap();
    assert_eq!(unbound["workspace_ready"], true);
    assert_eq!(unbound["task"]["ready"], false);
    assert_eq!(unbound["pending"], 0);
    assert_eq!(unbound["historical_pending"], 1);

    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "bound delivery",
            "--must",
            "validate the real source delta",
        ],
    );
    let id = started["id"].as_str().unwrap();
    let prepared = run_json(root, &["prepare", "--goal", id]);
    assert!(prepared.get("ready").is_none(), "{prepared}");
    assert_eq!(prepared["readiness"]["scope"], "goal_workspace_snapshot");
    assert!(
        prepared["readiness"]["workspace_fingerprint"]
            .as_str()
            .is_some_and(|value| value.len() == 64),
        "{prepared}"
    );
    assert!(
        prepared["readiness"]["goal_state_sha256"]
            .as_str()
            .is_some_and(|value| value.len() == 64),
        "{prepared}"
    );
    assert_eq!(prepared["goal_id"], id);

    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 43 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 43); }\n",
    );
    run_json(root, &["context", "refresh"]);
    validate_goal(
        root,
        id,
        "req_1",
        "compiled the bound source delta",
        &["src/lib.rs"],
    );
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let no_authority = run(root, &["finish", "--goal", id]);
    assert_eq!(no_authority.status, 1);
    assert!(
        no_authority.stderr.contains("稳定 authority receipt"),
        "stderr={}",
        no_authority.stderr
    );
    validate_goal_authority(
        root,
        id,
        "req_1",
        "authority gate stayed stable twice",
        &["src/lib.rs"],
    );

    let finished = run_json(root, &["finish", "--goal", id]);
    assert_eq!(finished["workspace_ready"], true);
    assert_eq!(finished["task"]["goal_id"], id);
    assert_eq!(finished["task"]["ready"], true);
    assert_eq!(finished["ready"], true);
    assert_eq!(finished["pending"], 0);
    assert_eq!(finished["historical_pending"], 1);
    assert!(finished["context_refresh"].is_object());

    let current_pending =
        add_complete_human_pending(root, id, "owner/current-readiness", "current owner choice");
    let blocked_by_current = run(root, &["--format", "json", "finish", "--goal", id]);
    assert_eq!(
        blocked_by_current.status, 1,
        "{}",
        blocked_by_current.stdout
    );
    let blocked_by_current: Value = serde_json::from_str(&blocked_by_current.stdout).unwrap();
    assert_eq!(blocked_by_current["pending"], 1);
    assert_eq!(blocked_by_current["historical_pending"], 1);
    run(
        root,
        &[
            "goal",
            "pending",
            "resolve",
            current_pending["id"].as_str().unwrap(),
        ],
    );

    let unbound_pending = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "legacy active work",
            "-m",
            "still active",
        ],
    );
    assert_eq!(run(root, &["finish", "--goal", id]).status, 1);
    run(
        root,
        &[
            "goal",
            "pending",
            "resolve",
            unbound_pending["id"].as_str().unwrap(),
        ],
    );
    let listed = run_json(root, &["goal", "pending", "list"]);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], retired_pending["id"]);
}
