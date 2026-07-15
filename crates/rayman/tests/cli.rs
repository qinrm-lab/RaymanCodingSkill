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

/// Run with a deterministic PATH prefix.  Doctor uses this to prove the same
/// command-resolution path an interactive caller would observe.
fn run_with_path(
    dir: &Path,
    args: &[&str],
    path_prefix: &[&Path],
    pathext: Option<&str>,
) -> Output {
    let mut entries = path_prefix
        .iter()
        .map(|path| path.to_path_buf())
        .collect::<Vec<_>>();
    if let Some(parent_path) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&parent_path));
    }
    let path = std::env::join_paths(entries).expect("PATH entries must be representable");
    let mut command = Command::new(BIN);
    command.args(args).current_dir(dir).env("PATH", path);
    if let Some(pathext) = pathext {
        command.env("PATHEXT", pathext);
    }
    let output = command.output().expect("无法启动 rayman 二进制");
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

/// Current-schema goals need a receipt produced by the CLI itself. `rustc
/// --version` is a harmless direct argv invocation available in the test
/// toolchain; no shell is involved.
fn validate_goal(root: &Path, id: &str, req: &str, message: &str, changed: &[&str]) -> Value {
    let command = if let Some(path) = changed.iter().find(|path| path.ends_with(".rs")) {
        std::fs::create_dir_all(root.join("target/rayman-validation")).unwrap();
        format!("rustc --crate-type lib {path} --out-dir target/rayman-validation")
    } else if changed
        .iter()
        .any(|path| path.ends_with("Cargo.toml") || path.ends_with("Cargo.lock"))
    {
        "cargo check --quiet".into()
    } else {
        "rustc --version".into()
    };
    let mut args = vec![
        "goal",
        "validate",
        id,
        "--req",
        req,
        "-m",
        message,
        "--command",
        command.as_str(),
    ];
    for path in changed {
        args.extend(["--changed", *path]);
    }
    if changed.is_empty() {
        args.push("--non-code");
    }
    run_json(root, &args)
}

fn write(dir: &Path, rel: &str, body: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn generate_lockfile(root: &Path) {
    let status = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(root)
        .status()
        .expect("cargo must be available to build the fixture lockfile");
    assert!(status.success());
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
fn standard_check_blocks_active_must_requirements_without_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &["goal", "start", "wire impact", "--must", "record evidence"],
    );

    // Bare `check` is the readiness gate, so its default profile must be the
    // same fail-closed standard behavior as `--profile standard`.
    let default_profile = run(root, &["check"]);
    assert_eq!(default_profile.status, 1);
    assert!(
        default_profile.stdout.contains("active goal") && default_profile.stdout.contains("must"),
        "stdout={}",
        default_profile.stdout
    );
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("缺少验证 receipt")
            && standard.stdout.contains("standard blockers: 2"),
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1, "stdout={}", standard.stdout);
    assert!(
        standard.stdout.contains("legacy goal goal_legacy")
            && standard.stdout.contains("仍为 current"),
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
fn doctor_verifies_installed_identity_in_an_ordinary_managed_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "SKILL.md", "ordinary workspace canonical skill\n");
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &format!("skill_sha256: {skill_hash}\n"),
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
    assert_eq!(report["repo_release"]["checked"], false);
    assert_eq!(report["repo_release"]["status"], "not_checked_by_doctor");
}

#[cfg(windows)]
#[test]
fn doctor_rejects_an_earlier_windows_path_wrapper() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "SKILL.md", "ordinary workspace canonical skill\n");
    let skill_hash = rayman::hash::sha256_file(&root.join("SKILL.md")).unwrap();
    write(
        root,
        ".RaymanCodingSkill/workspace_skill.yaml",
        &format!("skill_sha256: {skill_hash}\n"),
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

    let standard = run(root, &["check", "--profile", "standard"]);
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
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

    let standard = run(root, &["check", "--profile", "standard"]);
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

    let standard = run(root, &["check", "--profile", "standard"]);
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
    let project_map = root.join(".RaymanCodingSkill/context/project_map.json");
    assert!(
        !project_map.exists(),
        "read-only map queries must not create a cache"
    );
    let refreshed = run(root, &["map", "refresh"]);
    assert_eq!(refreshed.status, 0, "stderr={}", refreshed.stderr);
    assert!(project_map.exists());
}

#[test]
fn map_topology_and_impact_include_cargo_path_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/core/src/lib.rs",
        "pub fn core_api() -> i32 { 1 }\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::core_api() }\n",
    );
    write(
        root,
        "crates/app/tests/app_test.rs",
        "use app::app_api;\n#[test]\nfn app_works() { assert_eq!(app_api(), 1); }\n",
    );
    run_json(root, &["context", "refresh"]);

    let topology = run_json(root, &["map", "topology"]);
    assert_eq!(topology["packages"].as_array().unwrap().len(), 2);
    assert!(
        topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "app" && dependency["to_package"] == "core"
            }),
        "topology={topology}"
    );

    let impact = run_json(root, &["map", "impact", "crates/core/src/lib.rs"]);
    assert_eq!(impact["package"], "core");
    assert!(
        impact["package_dependents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["from_package"] == "app"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p core"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p app"),
        "impact={impact}"
    );
}

#[test]
fn map_topology_includes_workspace_inherited_path_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n\n[workspace.dependencies]\ncore = { path = \"crates/core\" }\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/core/src/lib.rs",
        "pub fn core_api() -> i32 { 1 }\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { workspace = true }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::core_api() }\n",
    );
    run_json(root, &["context", "refresh"]);

    let topology = run_json(root, &["map", "topology"]);
    assert!(
        topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "app"
                    && dependency["from_root_path"] == "crates/app"
                    && dependency["to_package"] == "core"
                    && dependency["to_root_path"] == "crates/core"
            }),
        "topology={topology}"
    );

    let impact = run_json(root, &["map", "impact", "crates/core/src/lib.rs"]);
    assert!(
        impact["package_dependents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| dependency["from_package"] == "app"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p core"),
        "impact={impact}"
    );
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p app"),
        "impact={impact}"
    );
}

#[test]
fn map_topology_includes_dotted_workspace_inherited_path_dependents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n\n[workspace.dependencies]\ncore.path = \"crates/core\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "crates/core/src/lib.rs",
        "pub fn core_api() -> i32 { 1 }\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[target.'cfg(windows)'.dependencies]\ncore.workspace = true\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::core_api() }\n",
    );
    run_json(root, &["context", "refresh"]);

    let topology = run_json(root, &["map", "topology"]);
    assert!(
        topology["package_dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dependency| {
                dependency["from_package"] == "app"
                    && dependency["from_root_path"] == "crates/app"
                    && dependency["to_package"] == "core"
                    && dependency["to_root_path"] == "crates/core"
            }),
        "topology={topology}"
    );
}

#[test]
fn map_plan_check_blocks_broad_source_change_without_test_anchor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(root, "src/evaluator.rs", "pub fn eval() -> i32 { 1 }\n");
    run_json(root, &["context", "refresh"]);

    let plan = run(
        root,
        &[
            "map",
            "plan",
            "src/lib.rs",
            "src/parser.rs",
            "src/evaluator.rs",
            "--check",
        ],
    );
    assert_eq!(plan.status, 1);
    assert!(
        plan.stdout.contains("no same-package candidate test"),
        "stdout={}",
        plan.stdout
    );
}

#[test]
fn map_plan_check_passes_broad_non_rust_change_without_cargo_workspace() {
    // Real-world basis: dogfooding rayman against a 792-file, 60k-line C# repo showed
    // this heuristic hard-blocking a well-tested change because it only understands
    // Cargo/Rust project shapes. Outside a detected Cargo workspace it must be advisory.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/A.cs", "public class A {}\n");
    write(root, "src/B.cs", "public class B {}\n");
    write(root, "src/C.cs", "public class C {}\n");
    run_json(root, &["context", "refresh"]);

    let plan = run(
        root,
        &["map", "plan", "src/A.cs", "src/B.cs", "src/C.cs", "--check"],
    );
    assert_eq!(plan.status, 0, "stdout={}", plan.stdout);
    assert!(
        plan.stdout.contains("no Cargo workspace detected"),
        "stdout={}",
        plan.stdout
    );
}

#[test]
fn map_plan_check_blocks_package_broad_change_without_indexed_test_anchor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/core/src/lib.rs", "pub mod a;\npub mod b;\n");
    write(root, "crates/core/src/a.rs", "pub fn a() -> i32 { 1 }\n");
    write(root, "crates/core/src/b.rs", "pub fn b() -> i32 { 2 }\n");
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::a::a() + core::b::b() }\n",
    );
    run_json(root, &["context", "refresh"]);

    let plan = run(
        root,
        &[
            "map",
            "plan",
            "crates/core/src/lib.rs",
            "crates/core/src/a.rs",
            "crates/core/src/b.rs",
            "--check",
        ],
    );
    assert_eq!(plan.status, 1);
    assert!(
        plan.stdout
            .contains("no same-package candidate test target")
            && plan.stdout.contains("indexed package test anchor"),
        "stdout={}",
        plan.stdout
    );
    assert!(
        plan.stdout.contains("cargo test -p core") && plan.stdout.contains("cargo test -p app"),
        "stdout={}",
        plan.stdout
    );
}

#[test]
fn map_plan_check_accepts_package_test_anchors_for_broad_source_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/core\", \"crates/app\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/core/Cargo.toml",
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/core/src/lib.rs", "pub mod a;\npub mod b;\n");
    write(root, "crates/core/src/a.rs", "pub fn a() -> i32 { 1 }\n");
    write(root, "crates/core/src/b.rs", "pub fn b() -> i32 { 2 }\n");
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    write(
        root,
        "crates/app/src/lib.rs",
        "pub fn app_api() -> i32 { core::a::a() + core::b::b() }\n",
    );
    write(
        root,
        "crates/app/tests/app_test.rs",
        "use app::app_api;\n#[test]\nfn app_works() { assert_eq!(app_api(), 3); }\n",
    );
    run_json(root, &["context", "refresh"]);

    let plan = run_json(
        root,
        &[
            "map",
            "plan",
            "crates/core/src/lib.rs",
            "crates/core/src/a.rs",
            "crates/core/src/b.rs",
            "--check",
        ],
    );
    assert_eq!(plan["ready"], true, "plan={plan}");
    assert!(
        plan["blockers"].as_array().unwrap().is_empty(),
        "plan={plan}"
    );
    assert!(
        plan["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p core"),
        "plan={plan}"
    );
    assert!(
        plan["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p app"),
        "plan={plan}"
    );
}

#[test]
fn map_quality_check_blocks_multi_source_project_without_tests() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(root, "src/lib.rs", "pub mod parser;\npub mod evaluator;\n");
    write(root, "src/parser.rs", "pub fn parse() -> i32 { 1 }\n");
    write(root, "src/evaluator.rs", "pub fn eval() -> i32 { 1 }\n");
    run_json(root, &["context", "refresh"]);

    let quality = run_json(root, &["map", "quality"]);
    assert_eq!(quality["ready"], false);
    assert_eq!(quality["error_count"], 1);
    assert!(
        quality["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["kind"] == "multi_source_project_without_tests"
                    && finding["severity"] == "error"
            }),
        "quality={quality}"
    );

    let quality_check = run(root, &["map", "quality", "--check"]);
    assert_eq!(quality_check.status, 1);
    assert!(
        quality_check
            .stdout
            .contains("multi_source_project_without_tests"),
        "stdout={}",
        quality_check.stdout
    );

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard
            .stdout
            .contains("quality multi_source_project_without_tests"),
        "stdout={}",
        standard.stdout
    );
}

#[test]
fn strict_quality_config_can_block_configured_warning_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(
        root,
        ".RaymanCodingSkill/quality.json",
        "{\n  \"block_warning_kinds\": [\"public_api_without_test_evidence\"]\n}\n",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["map", "quality", "--check"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let strict = run(root, &["map", "quality", "--profile", "strict", "--check"]);
    assert_eq!(strict.status, 1);
    assert!(
        strict
            .stdout
            .contains("configured as blocking by .RaymanCodingSkill/quality.json"),
        "stdout={}",
        strict.stdout
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
fn strict_quality_config_fails_closed_on_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(
        root,
        ".RaymanCodingSkill/quality.json",
        "{\n  \"block_warning_kind\": [\"public_api_without_test_evidence\"]\n}\n",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let strict = run(root, &["map", "quality", "--profile", "strict", "--check"]);
    assert_eq!(strict.status, 1);
    assert!(
        strict.stderr.contains("quality.json") && strict.stderr.contains("unknown field"),
        "stderr={}",
        strict.stderr
    );

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(release.status, 1);
    assert!(
        release.stderr.contains("quality.json") && release.stderr.contains("unknown field"),
        "stderr={}",
        release.stderr
    );
}

#[test]
fn strict_quality_config_fails_closed_on_unknown_warning_kinds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn api() {}\n");
    write(
        root,
        ".RaymanCodingSkill/quality.json",
        "{\n  \"block_warning_kinds\": [\"public_api_without_test_evdence\"]\n}\n",
    );
    run_json(root, &["context", "refresh"]);

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(
        standard.status, 0,
        "stdout={} stderr={}",
        standard.stdout, standard.stderr
    );

    let strict = run(root, &["map", "quality", "--profile", "strict", "--check"]);
    assert_eq!(strict.status, 1);
    assert!(
        strict.stderr.contains("quality.json")
            && strict.stderr.contains("unknown block_warning_kinds entry"),
        "stderr={}",
        strict.stderr
    );

    let release = run(root, &["check", "--profile", "release"]);
    assert_eq!(release.status, 1);
    assert!(
        release.stderr.contains("quality.json")
            && release.stderr.contains("unknown block_warning_kinds entry"),
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

    let standard = run(root, &["check", "--profile", "standard"]);
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("goal_future")
            && standard.stdout.contains("不支持的 goal schema_version=3"),
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1, "stdout={}", standard.stdout);
    assert!(standard.stdout.contains("legacy goal goal_legacy_active"));
    assert!(!standard.stdout.contains("合约无效"));
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

    let standard = run(root, &["check", "--profile", "standard"]);
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
    assert_eq!(run(root, &["check", "--profile", "standard"]).status, 0);

    write(root, "src/lib.rs", "pub fn answer() -> i32 { 43 }\n");
    run_json(root, &["context", "refresh"]);
    let standard = run(root, &["check", "--profile", "standard"]);
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

    let standard = run(root, &["check", "--profile", "standard"]);
    assert_eq!(standard.status, 1);
    assert!(
        standard.stdout.contains("同一条当前成功 receipt"),
        "stdout={}",
        standard.stdout
    );
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
    assert!(standard.stdout.contains("lifecycle=superseded"));

    let restored = run_json(root, &["goal", "current", old_id]);
    assert_eq!(restored["lifecycle"], "current");
    assert_eq!(run(root, &["check", "--profile", "standard"]).status, 1);
}

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

    write(root, ".RaymanCodingSkill/tmp/run/nested/a.bin", "abc");
    write(root, ".RaymanCodingSkill/tmp/run/b.bin", "d");
    let temp_status = run_json(root, &["temp", "status"]);
    assert_eq!(temp_status["entry_count"], 1);
    assert_eq!(temp_status["file_count"], 2);
    assert_eq!(temp_status["directory_count"], 2);
    assert_eq!(temp_status["total_bytes"], 4);
    assert_eq!(temp_status["traversal_error_count"], 0);

    let clean_audit = run_json(root, &["state", "audit", "--check"]);
    assert_eq!(clean_audit["clean"], true);
    write(root, ".RaymanCodingSkill/research/retired.json", "{}");
    let blocked_audit = run(root, &["state", "audit", "--check"]);
    assert_eq!(blocked_audit.status, 1);
    assert!(
        root.join(".RaymanCodingSkill/research/retired.json")
            .exists()
    );
}

#[test]
fn map_quality_check_passes_with_a_test_anchor() {
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
        "use sample::evaluator;\n#[test]\nfn evaluator_works() {}\n",
    );
    run_json(root, &["context", "refresh"]);

    let quality = run_json(root, &["map", "quality"]);
    assert_eq!(quality["ready"], true, "quality={quality}");
    assert_eq!(quality["error_count"], 0);

    let quality_check = run(root, &["map", "quality", "--check"]);
    assert_eq!(
        quality_check.status, 0,
        "stdout={} stderr={}",
        quality_check.stdout, quality_check.stderr
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
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/rayman\"]\nexclude = [\"evals\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/rayman/Cargo.toml",
        "[package]\nname = \"rayman\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/rayman/src/lib.rs", "pub fn cli() {}\n");
    write(
        root,
        "crates/rayman/tests/cli.rs",
        "use rayman::cli;\n#[test]\nfn cli_works() { cli(); }\n",
    );
    write(
        root,
        "evals/tasks/add-feature/fixture/Cargo.toml",
        "[package]\nname = \"task\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
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
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check
                == "cargo test --manifest-path evals/tasks/add-feature/fixture/Cargo.toml"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p task"),
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
fn map_impact_uses_manifest_path_for_duplicate_workspace_package_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/one\", \"crates/two\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/one/Cargo.toml",
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/one/src/lib.rs", "pub fn one() {}\n");
    write(
        root,
        "crates/two/Cargo.toml",
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/two/src/lib.rs", "pub fn two() {}\n");
    run_json(root, &["context", "refresh"]);

    let impact = run_json(root, &["map", "impact", "crates/one/src/lib.rs"]);
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test --manifest-path crates/one/Cargo.toml"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p shared"),
        "impact={impact}"
    );
}

#[test]
fn map_impact_uses_manifest_path_for_nested_package_under_workspace_member_glob() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
    );
    write(
        root,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/app/src/lib.rs", "pub fn app() {}\n");
    write(
        root,
        "crates/app/fixture/Cargo.toml",
        "[package]\nname = \"task\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "crates/app/fixture/src/lib.rs", "pub fn task() {}\n");
    run_json(root, &["context", "refresh"]);

    let impact = run_json(root, &["map", "impact", "crates/app/fixture/src/lib.rs"]);
    assert!(
        impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test --manifest-path crates/app/fixture/Cargo.toml"),
        "impact={impact}"
    );
    assert!(
        !impact["recommended_checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check == "cargo test -p task"),
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
