use std::fs;
use std::process::{Command, Output};

fn rayman() -> &'static str {
    env!("CARGO_BIN_EXE_rayman")
}

fn run_in_temp(args: &[&str]) -> Output {
    let temp = tempfile::tempdir().unwrap();
    Command::new(rayman())
        .args(args)
        .env("RAYMAN_DISABLE_REMINDER", "1")
        .current_dir(temp.path())
        .output()
        .unwrap()
}

#[test]
fn cli_help_lists_core_commands() {
    // @ui:cli
    let output = run_in_temp(&["--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("RaymanCodingSkill Rust CLI"));
    assert!(stdout.contains("coverage"));
    assert!(stdout.contains("audit"));
    assert!(stdout.contains("docs"));
    assert!(!stdout.contains('�'));
}

#[test]
fn cli_coverage_help_lists_gate_options() {
    let output = run_in_temp(&["coverage", "status", "--help"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("--format"));
    assert!(stdout.contains("--check"));
    assert!(stdout.contains("--output"));
}

#[test]
fn cli_json_stdout_remains_parseable_with_footer_on_stderr() {
    let output = run_in_temp(&["context", "explain"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(value["source_policy"].as_str().is_some());
    assert!(stderr.contains("辅助AI使用价值"));
    assert!(stderr.contains("实现验证纠错贡献"));
}

#[test]
fn cli_subagent_auto_start_emits_spawn_contract() {
    let output = run_in_temp(&[
        "subagent",
        "auto-start",
        "--task",
        "审计 subagent 自动开启",
        "--path",
        "crates/rayman-core/src/subagent.rs",
        "--read-only",
        "--max-lanes",
        "3",
    ]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["auto_start_ready"].as_bool(), Some(true));
    assert_eq!(
        value["auto_start_contract"]["host_tool"].as_str(),
        Some("multi_agent_v1.spawn_agent")
    );
    assert_eq!(
        value["auto_start_contract"]["authorization_mode"].as_str(),
        Some("standing_workspace_authorization")
    );
    assert_eq!(
        value["auto_start_contract"]["per_use_prompt_required"].as_bool(),
        Some(false)
    );
    assert_eq!(
        value["auto_start_contract"]["explicit_subagent_phrase_required"].as_bool(),
        Some(false)
    );
    assert!(
        value["auto_start_contract"]["start_when"]
            .as_str()
            .unwrap_or_default()
            .contains("no additional '开启subagent' phrase is required")
    );
    let lanes = value["recommended_lanes"].as_array().unwrap();
    assert_eq!(value["read_only_intent"].as_bool(), Some(true));
    assert!(lanes.iter().all(|lane| lane["read_only"] == true));
    assert!(lanes.iter().all(|lane| lane["agent_type"] != "worker"));
    assert!(
        lanes
            .iter()
            .any(|lane| lane["lane_id"] == "read_only_scope_review")
    );
    assert!(lanes.iter().all(|lane| {
        lane["record_command_template"]
            .as_str()
            .unwrap_or_default()
            .contains("--read-only")
    }));
    assert!(lanes.iter().any(|lane| {
        lane["spawn_agent_request"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("RaymanCodingSkill host subagent lane")
    }));
    assert!(
        value["auto_start_contract"]["ledger_sequence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|step| step
                .as_str()
                .unwrap_or_default()
                .contains("rayman subagent review"))
    );
}

#[test]
fn cli_goal_run_emits_host_subagent_dispatch_request() {
    let temp = tempfile::tempdir().unwrap();
    let start = Command::new(rayman())
        .args(["goal", "start", "全仓审计修复并闭环 gate"])
        .env("RAYMAN_DISABLE_REMINDER", "1")
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(start.status.success());

    let output = Command::new(rayman())
        .args(["goal", "run", "--until", "blocked"])
        .env("RAYMAN_DISABLE_REMINDER", "1")
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let line = stdout
        .lines()
        .find(|line| line.starts_with("HOST_SUBAGENT_DISPATCH_REQUEST "))
        .expect("dispatch request line");
    let json_text = line.trim_start_matches("HOST_SUBAGENT_DISPATCH_REQUEST ");
    let value: serde_json::Value = serde_json::from_str(json_text).unwrap();
    assert_eq!(value["auto_start_ready"].as_bool(), Some(true));
    assert_eq!(
        value["auto_start_contract"]["host_tool"].as_str(),
        Some("multi_agent_v1.spawn_agent")
    );
    assert_eq!(
        value["auto_start_contract"]["per_use_prompt_required"].as_bool(),
        Some(false)
    );
    assert!(
        value["request_id"]
            .as_str()
            .unwrap()
            .contains("subagent_dispatch")
    );
    assert!(
        value["recommended_lanes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|lane| lane["record_command_template"]
                .as_str()
                .unwrap_or_default()
                .contains("--goal-id"))
    );
}

#[test]
fn cli_subagent_review_accepts_documented_not_used_value() {
    let temp = tempfile::tempdir().unwrap();
    let record_output = Command::new(rayman())
        .args([
            "subagent",
            "record",
            "--agent-id",
            "agent-1",
            "--nickname",
            "Lane",
            "--task",
            "read-only audit lane",
            "--boundary",
            "read-only",
            "--read-only",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(record_output.status.success());
    let record_stdout = String::from_utf8(record_output.stdout).unwrap();
    let record: serde_json::Value = serde_json::from_str(&record_stdout).unwrap();
    let id = record["id"].as_str().unwrap();

    let result_output = Command::new(rayman())
        .args([
            "subagent",
            "result",
            "--id",
            id,
            "--status",
            "failed",
            "-m",
            "host subagent unavailable",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(result_output.status.success());

    let review_output = Command::new(rayman())
        .args([
            "subagent",
            "review",
            "--id",
            id,
            "--verdict",
            "not-used",
            "-m",
            "primary continued without this failed advisory lane",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(review_output.status.success());
    let review_stdout = String::from_utf8(review_output.stdout).unwrap();
    let review: serde_json::Value = serde_json::from_str(&review_stdout).unwrap();
    assert_eq!(review["status"].as_str(), Some("reviewed"));
    assert_eq!(
        review["primary_review"]["verdict"].as_str(),
        Some("not_used")
    );
}

#[test]
fn cli_chinese_output_is_utf8_not_mojibake() {
    let output = run_in_temp(&["session", "status"]);

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("待完成"));
    assert!(!stdout.contains('�'));
}

#[test]
fn cli_error_output_is_utf8_and_actionable() {
    let output = run_in_temp(&["temp", "cleanup"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("错误:"));
    assert!(
        stderr.contains(
            "temp cleanup requires --completed, --stale, --all-failed, or --cargo-targets"
        )
    );
    assert!(!stderr.contains('�'));
}

#[test]
fn cli_temp_cleanup_completed_deletes_managed_run() {
    let temp = tempfile::tempdir().unwrap();
    let run_dir = temp
        .path()
        .join(".RaymanCodingSkill")
        .join("tmp")
        .join("runs")
        .join("completed-run");
    fs::create_dir_all(&run_dir).unwrap();
    let metadata = serde_json::json!({
        "command": "validation",
        "workspace": temp.path().display().to_string(),
        "pid": 1,
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z",
        "status": "completed"
    });
    fs::write(
        run_dir.join("metadata.json"),
        serde_json::to_string_pretty(&metadata).unwrap(),
    )
    .unwrap();

    let output = Command::new(rayman())
        .args(["temp", "cleanup", "--completed"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!run_dir.exists());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["removed"].as_array().unwrap().len(), 1);
}

#[test]
fn cli_task_stop_reminder_trace_records_goal_and_session_stops() {
    let temp = tempfile::tempdir().unwrap();
    let trace = temp.path().join("reminder.log");

    let start_output = Command::new(rayman())
        .args(["goal", "start", "compile reminder flow"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(start_output.status.success());

    let run_output = Command::new(rayman())
        .args(["goal", "run"])
        .env("RAYMAN_REMINDER_LOG_PATH", &trace)
        .env_remove("RAYMAN_DISABLE_REMINDER")
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(run_output.status.success());

    let close_output = Command::new(rayman())
        .args([
            "session",
            "close",
            "--status",
            "partial",
            "-m",
            "record reminder trace",
        ])
        .env("RAYMAN_REMINDER_LOG_PATH", &trace)
        .env_remove("RAYMAN_DISABLE_REMINDER")
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(close_output.status.success());

    let trace_text = fs::read_to_string(trace).unwrap();
    let lines: Vec<_> = trace_text.lines().collect();
    assert_eq!(lines, vec!["goal_stopped", "session_closed"]);
}

#[test]
fn cli_task_stop_reminder_trace_records_error_command_end_once() {
    let temp = tempfile::tempdir().unwrap();
    let trace = temp.path().join("reminder-error.log");

    let output = Command::new(rayman())
        .args(["goal", "run", "--id", "missing_goal"])
        .env("RAYMAN_REMINDER_LOG_PATH", &trace)
        .env_remove("RAYMAN_DISABLE_REMINDER")
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let trace_text = fs::read_to_string(trace).unwrap();
    let lines: Vec<_> = trace_text.lines().collect();
    assert_eq!(lines, vec!["goal_stopped"]);
}

#[test]
fn cli_task_stop_reminder_trace_ignores_subagent_commands_and_scope() {
    let temp = tempfile::tempdir().unwrap();
    let subagent_trace = temp.path().join("subagent-reminder.log");

    let subagent_output = Command::new(rayman())
        .args(["subagent", "status"])
        .env("RAYMAN_REMINDER_LOG_PATH", &subagent_trace)
        .env_remove("RAYMAN_DISABLE_REMINDER")
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(subagent_output.status.success());
    assert!(!subagent_trace.exists());

    let scoped_trace = temp.path().join("scoped-subagent-reminder.log");
    let scoped_output = Command::new(rayman())
        .args(["goal", "run", "--id", "missing_goal"])
        .env("RAYMAN_REMINDER_LOG_PATH", &scoped_trace)
        .env("RAYMAN_REMINDER_SCOPE", "subagent")
        .env_remove("RAYMAN_DISABLE_REMINDER")
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(!scoped_output.status.success());
    assert!(!scoped_trace.exists());
}
