//! 端到端集成测试：驱动真实的 `rayman` 二进制在临时工作区跑完整流程。
//! 这些测试补足单元测试无法覆盖的东西——真实进程、真实退出码、真实文件系统状态。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_rayman");

struct Output {
    status: i32,
    stdout: String,
    stderr: String,
}

/// 在 `dir` 下运行 `rayman <args...>`，返回退出码与输出。
fn run(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("无法启动 rayman 二进制");
    Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// 运行并解析 JSON 输出（用 --format json）。
fn run_json(dir: &Path, args: &[&str]) -> Value {
    let mut full = vec!["--format", "json"];
    full.extend_from_slice(args);
    let output = run(dir, &full);
    assert_eq!(
        output.status, 0,
        "命令应成功: {args:?}\nstderr={}",
        output.stderr
    );
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|error| panic!("输出不是 JSON: {error}\n{}", output.stdout))
}

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn state_snapshot(root: &Path) -> BTreeMap<String, (u64, std::time::SystemTime, Vec<u8>)> {
    fn visit(
        base: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, (u64, std::time::SystemTime, Vec<u8>)>,
    ) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => panic!("无法读取状态目录 {}: {error}", dir.display()),
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(base, &path, out);
            } else if metadata.is_file() {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(
                    rel,
                    (
                        metadata.len(),
                        metadata.modified().unwrap(),
                        std::fs::read(&path).unwrap(),
                    ),
                );
            }
        }
    }

    let state_root = root.join(".RaymanCodingSkill");
    let mut out = BTreeMap::new();
    if state_root.exists() {
        visit(&state_root, &state_root, &mut out);
    }
    out
}

#[test]
fn context_refresh_caches_fingerprints_and_reuses_unchanged_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/a.rs", "pub fn a() {}");
    write(root, "src/b.rs", "pub fn b() {}");

    let first = run_json(root, &["context", "refresh"]);
    assert_eq!(first["total"], 2);
    assert_eq!(first["rehashed"], 2);
    assert_eq!(first["reused"], 0);

    // 不改文件：第二次全部复用，零重算——这是核心性能保证。
    let second = run_json(root, &["context", "refresh"]);
    assert_eq!(second["reused"], 2);
    assert_eq!(second["rehashed"], 0);

    // 改一个文件：只有它被重算。
    write(root, "src/a.rs", "pub fn a() { /* changed */ }");
    let third = run_json(root, &["context", "refresh"]);
    assert_eq!(third["rehashed"], 1);
    assert_eq!(third["reused"], 1);
}

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
        denied.stderr.contains("缺少证据"),
        "stderr={}",
        denied.stderr
    );

    // partial 允许。
    assert_eq!(
        run(root, &["goal", "close", &id, "--status", "partial"]).status,
        0
    );

    // 记录 must 证据后允许 success。
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
    let closed = run(root, &["goal", "close", &id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);
    assert_eq!(run_json(root, &["goal", "show", &id])["status"], "success");
}

#[test]
fn standard_check_blocks_active_must_requirements_without_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );

    assert_eq!(run(root, &["check"]).status, 0);
    let standard = run(root, &["check", "--profile", "standard"]);
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("仍为 active") && standard.stdout.contains("goal close"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_blocks_done_requirement_without_structured_validation() {
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
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("缺少结构化 validated 命令")
            && standard.stdout.contains("standard blockers: 1"),
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

    let standard = run(root, &["check", "--profile", "standard"]);
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("状态为 partial")
            && standard.stdout.contains("缺少结构化 validated 命令"),
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
    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal_legacy")
            && standard.stdout.contains("缺少结构化 validated 命令"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn standard_check_accepts_legacy_goal_level_verification() {
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );
    assert!(
        standard.stdout.contains("standard warnings: 0"),
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
fn goal_evidence_changed_failure_does_not_write_project_map_cache() {
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

    let failed = run(
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
    assert_eq!(failed.status, 1);
    assert!(
        failed.stderr.contains("项目地图中没有文件"),
        "stderr={}",
        failed.stderr
    );
    assert!(!project_map.exists());
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
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);
    let before = state_snapshot(root);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
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
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard"]);
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
fn goal_evidence_changed_requires_validation_and_standard_accepts_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
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
    let closed = run(root, &["goal", "close", id]);
    assert_eq!(closed.status, 0, "stderr={}", closed.stderr);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );
}

#[test]
fn pending_items_roundtrip_and_block_check() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/a.rs", "fn a() {}");
    run(root, &["context", "refresh"]);

    // 干净工作区（无 pending、上下文 ready）→ check READY，退出 0。
    let ready = run(root, &["check"]);
    assert_eq!(
        ready.status, 0,
        "stdout={} stderr={}",
        ready.stdout, ready.stderr
    );

    // 加一个待完成项 → check BLOCKED，退出 1。
    run(
        root,
        &["goal", "pending", "add", "finish gate", "-m", "wire CI"],
    );
    let blocked = run(root, &["check"]);
    assert_eq!(blocked.status, 1);
    assert!(blocked.stdout.contains("BLOCKED"));

    // 解决后恢复 READY。
    let items = run_json(root, &["goal", "pending", "list"]);
    let pending_id = items[0]["id"].as_str().unwrap().to_string();
    run(root, &["goal", "pending", "resolve", &pending_id]);
    assert_eq!(run(root, &["check"]).status, 0);
}

#[test]
fn assets_scan_reports_obsolete_and_markers_without_deleting() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/main.rs",
        "fn main() {} // TODO: 未完成 wire up\n",
    );
    write(root, "src/old.rs.bak", "dead");

    let report = run_json(root, &["assets"]);
    assert!(
        report["obsolete"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"].as_str().unwrap().ends_with(".bak"))
    );
    let markers: Vec<&str> = report["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["marker"].as_str().unwrap())
        .collect();
    assert!(markers.contains(&"TODO"));
    assert!(markers.contains(&"未完成"));
    // 只读：文件仍在。
    assert!(root.join("src/old.rs.bak").exists());
}

#[test]
fn map_commands_report_project_structure_and_impact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(
        root,
        "src/evaluator.rs",
        "use crate::parser;\npub fn eval() -> i32 { parser::parse() }\n",
    );
    write(
        root,
        "tests/evaluator_test.rs",
        "use sample::evaluator;\n#[test]\nfn eval_works() { assert_eq!(1, 1); }\n",
    );

    run_json(root, &["context", "refresh"]);

    let summary = run_json(root, &["map", "summary"]);
    assert_eq!(summary["source_files"], 3);
    assert_eq!(summary["test_files"], 1);
    assert!(
        summary["dependencies"].as_u64().unwrap() >= 1,
        "summary={summary}"
    );

    let file = run_json(root, &["map", "file", "src/evaluator.rs"]);
    assert_eq!(file["path"], "src/evaluator.rs");
    assert!(
        file["outgoing_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["to_path"] == "src/parser.rs")
    );

    let symbols = run_json(root, &["map", "symbol", "eval"]);
    assert!(
        symbols["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|symbol| symbol["path"] == "src/evaluator.rs")
    );

    let impact = run_json(root, &["map", "impact", "src/evaluator.rs"]);
    assert!(
        impact["related_tests"]
            .as_array()
            .unwrap()
            .iter()
            .any(|test| test["path"] == "tests/evaluator_test.rs")
    );
    assert_eq!(
        impact["related_tests"][0]["basis"],
        "same_package_test_text_reference_heuristic"
    );
    assert!(
        impact["recommendation_basis"]
            .as_str()
            .unwrap()
            .contains("heuristic")
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test --all")
    );
    assert!(
        root.join(".RaymanCodingSkill/context/project_map.json")
            .exists()
    );
}

#[test]
fn map_commands_fail_closed_on_missing_or_stale_context() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");

    let missing = run(root, &["--format", "json", "map", "summary"]);
    assert_eq!(missing.status, 1);
    let missing_error: Value = serde_json::from_str(&missing.stderr)
        .unwrap_or_else(|error| panic!("stderr is not JSON: {error}\n{}", missing.stderr));
    assert!(
        missing_error["error"]
            .as_str()
            .unwrap()
            .contains("上下文索引")
    );

    run_json(root, &["context", "refresh"]);
    write(root, "src/new.rs", "pub fn new_item() {}\n");
    let stale = run(root, &["--format", "json", "map", "summary"]);
    assert_eq!(stale.status, 1);
    let stale_error: Value = serde_json::from_str(&stale.stderr)
        .unwrap_or_else(|error| panic!("stderr is not JSON: {error}\n{}", stale.stderr));
    assert!(
        stale_error["error"]
            .as_str()
            .unwrap()
            .contains("不是 ready")
    );
}

#[test]
fn map_impact_does_not_infer_related_tests_across_package_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "crates/rayman/src/lib.rs", "pub fn cli() {}\n");
    write(
        root,
        "crates/rayman/tests/cli.rs",
        "use rayman::cli;\n#[test]\nfn cli_works() { cli(); }\n",
    );
    write(
        root,
        "evals/tasks/add-feature/fixture/src/lib.rs",
        "pub fn add(left: i32, right: i32) -> i32 { left + right }\n",
    );
    run_json(root, &["context", "refresh"]);

    let impact = run_json(
        root,
        &[
            "map",
            "impact",
            "evals/tasks/add-feature/fixture/src/lib.rs",
        ],
    );
    assert!(
        impact["related_tests"].as_array().unwrap().is_empty(),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check
                .as_str()
                .unwrap()
                .contains("crates/rayman/tests/cli.rs")),
        "impact={impact}"
    );
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
