#![cfg(windows)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

const CLI: &str = env!("CARGO_BIN_EXE_rayman");
const WORKER: &str = env!("CARGO_BIN_EXE_rayman-update-worker");

#[derive(Serialize)]
struct Plan {
    schema_version: u32,
    transaction_id: String,
    candidate_version: String,
    cli_contract: String,
    installation_id: String,
    manifest_sha256: String,
    bundle_root: PathBuf,
    journal_path: PathBuf,
    result_path: PathBuf,
    files: Vec<PlanFile>,
}

#[derive(Serialize)]
struct PlanFile {
    role: String,
    source: PathBuf,
    destination: PathBuf,
    new_sha256: String,
    expected_current_sha256: Option<String>,
    expect_absent: bool,
    allow_existing_new: bool,
}

fn sha(path: &Path) -> String {
    rayman::hash::sha256_file(path).unwrap()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn copy(source: impl AsRef<Path>, destination: impl AsRef<Path>) {
    std::fs::copy(source, destination).unwrap();
}

fn prepare_plan(root: &Path) -> (PathBuf, Plan, Vec<u8>) {
    let repo = repo_root();
    let bundle = root.join("bundle");
    let bin = root.join("bin");
    let skill = root.join("skill");
    let references = skill.join("references");
    let install = root.join("install");
    for directory in [&bundle, &bin, &skill, &references, &install] {
        std::fs::create_dir_all(directory).unwrap();
    }

    let cli_destination = bin.join("rayman.exe");
    let worker_destination = bin.join(format!("rayman-update-worker-{}.exe", rayman::CLI_VERSION));
    copy(CLI, &cli_destination);
    copy(WORKER, &worker_destination);
    std::fs::write(skill.join("SKILL.md"), b"old skill\n").unwrap();
    std::fs::write(skill.join("AGENTS.md"), b"old contract\n").unwrap();
    std::fs::write(references.join("workflow-contract.md"), b"old workflow\n").unwrap();
    let receipt_destination = install.join("receipt.json");
    std::fs::write(&receipt_destination, b"old receipt\n").unwrap();

    let sources = [
        ("skill", repo.join("SKILL.md"), bundle.join("skill.md")),
        (
            "agent_contract",
            repo.join("AGENT_CONTRACT.md"),
            bundle.join("agents.md"),
        ),
        (
            "workflow_contract",
            repo.join("references/workflow-contract.md"),
            bundle.join("workflow.md"),
        ),
        (
            "update_worker",
            PathBuf::from(WORKER),
            bundle.join("worker.exe"),
        ),
        ("cli", PathBuf::from(CLI), bundle.join("rayman.exe")),
    ];
    for (_, source, destination) in &sources {
        copy(source, destination);
    }
    let new_receipt = bundle.join("new-receipt.json");
    std::fs::write(&new_receipt, b"new receipt\n").unwrap();

    let destinations = [
        skill.join("SKILL.md"),
        skill.join("AGENTS.md"),
        references.join("workflow-contract.md"),
        worker_destination.clone(),
        cli_destination.clone(),
        receipt_destination.clone(),
    ];
    let source_paths = [
        bundle.join("skill.md"),
        bundle.join("agents.md"),
        bundle.join("workflow.md"),
        bundle.join("worker.exe"),
        bundle.join("rayman.exe"),
        new_receipt,
    ];
    let roles = [
        "skill",
        "agent_contract",
        "workflow_contract",
        "update_worker",
        "cli",
        "install_receipt",
    ];
    let files = roles
        .into_iter()
        .enumerate()
        .map(|(index, role)| PlanFile {
            role: role.into(),
            source: source_paths[index].clone(),
            destination: destinations[index].clone(),
            new_sha256: sha(&source_paths[index]),
            expected_current_sha256: Some(sha(&destinations[index])),
            expect_absent: false,
            allow_existing_new: role == "update_worker",
        })
        .collect();
    let plan = Plan {
        schema_version: 1,
        transaction_id: "1".repeat(32),
        candidate_version: rayman::CLI_VERSION.into(),
        cli_contract: rayman::CLI_CONTRACT.into(),
        installation_id: "2".repeat(32),
        manifest_sha256: "3".repeat(64),
        bundle_root: bundle.clone(),
        journal_path: bundle.join("journal.json"),
        result_path: bundle.join("result.json"),
        files,
    };
    let plan_path = bundle.join("apply-plan.json");
    let plan_bytes = serde_json::to_vec_pretty(&plan).unwrap();
    std::fs::write(&plan_path, &plan_bytes).unwrap();
    let script = std::fs::read(repo.join("scripts/install-rayman.ps1")).unwrap();
    (plan_path, plan, script)
}

fn run_plan(plan_path: &Path, script: &[u8]) -> std::process::Output {
    let plan_hash = sha(plan_path);
    let plan_json = std::fs::read_to_string(plan_path).unwrap();
    let host_temp = plan_path
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("host-temp");
    std::fs::create_dir_all(&host_temp).unwrap();
    let mut child = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
        .env("RAYMAN_UPDATE_WORKER", "1")
        .env("RAYMAN_UPDATE_WORKER_PLAN", plan_path)
        .env("RAYMAN_UPDATE_WORKER_PLAN_SHA256", plan_hash)
        .env("RAYMAN_UPDATE_WORKER_PLAN_JSON", plan_json)
        .env("TEMP", &host_temp)
        .env("TMP", &host_temp)
        .env("TMPDIR", &host_temp)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(script).unwrap();
    child.wait_with_output().unwrap()
}

fn assert_destination_hashes(plan: &Plan, expected: &[String]) {
    for (file, expected_hash) in plan.files.iter().zip(expected) {
        assert_eq!(&sha(&file.destination), expected_hash, "{}", file.role);
    }
}

#[test]
fn verified_update_script_publishes_the_complete_tuple_and_commits_journal() {
    let temp = tempfile::tempdir().unwrap();
    let (plan_path, plan, script) = prepare_plan(temp.path());
    let output = run_plan(&plan_path, &script);
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for file in &plan.files {
        assert_eq!(sha(&file.destination), file.new_sha256, "{}", file.role);
    }
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&plan.journal_path).unwrap()).unwrap();
    assert_eq!(journal["phase"], "committed");
    assert_eq!(journal["committed"], true);
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&plan.result_path).unwrap()).unwrap();
    assert_eq!(result["status"], "installed");
    let journal_bytes = std::fs::read(&plan.journal_path).unwrap();
    let result_bytes = std::fs::read(&plan.result_path).unwrap();
    let destination_hashes: Vec<_> = plan
        .files
        .iter()
        .map(|file| sha(&file.destination))
        .collect();

    // The exact same request is the only recovery authority. A committed
    // journal is verified idempotently instead of republishing or rolling
    // back the already complete generation.
    let resumed = run_plan(&plan_path, &script);
    assert!(
        resumed.status.success(),
        "resume stdout={} stderr={}",
        String::from_utf8_lossy(&resumed.stdout),
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(std::fs::read(&plan.journal_path).unwrap(), journal_bytes);
    assert_eq!(std::fs::read(&plan.result_path).unwrap(), result_bytes);
    assert_destination_hashes(&plan, &destination_hashes);
}

#[test]
fn committed_update_journal_rejects_changed_plan_identity_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let (plan_path, mut plan, script) = prepare_plan(temp.path());
    let first = run_plan(&plan_path, &script);
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let committed_hashes: Vec<_> = plan
        .files
        .iter()
        .map(|file| sha(&file.destination))
        .collect();

    // The same paths and bytes are insufficient recovery authority. Changing
    // the transaction identity produces a different verified plan hash, which
    // must not reuse or rewrite the committed journal.
    plan.transaction_id = "4".repeat(32);
    std::fs::write(&plan_path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    let replay = run_plan(&plan_path, &script);
    assert!(!replay.status.success());
    assert!(
        String::from_utf8_lossy(&replay.stderr)
            .contains("Existing update journal does not match the verified plan"),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&replay.stdout),
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_destination_hashes(&plan, &committed_hashes);
}

#[test]
fn committed_update_journal_rejects_envelope_state_and_destination_drift() {
    let temp = tempfile::tempdir().unwrap();
    let (plan_path, plan, script) = prepare_plan(temp.path());
    let first = run_plan(&plan_path, &script);
    assert!(
        first.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let journal_bytes = std::fs::read(&plan.journal_path).unwrap();
    let committed_hashes: Vec<_> = plan
        .files
        .iter()
        .map(|file| sha(&file.destination))
        .collect();

    let sentinel = temp.path().join("outside-sentinel.txt");
    std::fs::write(&sentinel, b"outside sentinel\n").unwrap();
    let mut drifted_entry: serde_json::Value = serde_json::from_slice(&journal_bytes).unwrap();
    drifted_entry["entries"][0]["destination"] =
        serde_json::Value::String(sentinel.to_string_lossy().into_owned());
    std::fs::write(
        &plan.journal_path,
        serde_json::to_vec_pretty(&drifted_entry).unwrap(),
    )
    .unwrap();
    let envelope = run_plan(&plan_path, &script);
    assert!(!envelope.status.success());
    assert!(String::from_utf8_lossy(&envelope.stderr).contains("entry 0 drifted"));
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"outside sentinel\n");
    assert_destination_hashes(&plan, &committed_hashes);

    let mut unknown_field: serde_json::Value = serde_json::from_slice(&journal_bytes).unwrap();
    unknown_field["unexpected"] = serde_json::Value::Bool(true);
    std::fs::write(
        &plan.journal_path,
        serde_json::to_vec_pretty(&unknown_field).unwrap(),
    )
    .unwrap();
    let schema = run_plan(&plan_path, &script);
    assert!(!schema.status.success());
    assert!(
        String::from_utf8_lossy(&schema.stderr)
            .contains("does not match the verified plan envelope")
    );
    assert_destination_hashes(&plan, &committed_hashes);

    let mut inconsistent: serde_json::Value = serde_json::from_slice(&journal_bytes).unwrap();
    inconsistent["entries"][0]["completed"] = serde_json::Value::Bool(false);
    std::fs::write(
        &plan.journal_path,
        serde_json::to_vec_pretty(&inconsistent).unwrap(),
    )
    .unwrap();
    let state = run_plan(&plan_path, &script);
    assert!(!state.status.success());
    assert!(
        String::from_utf8_lossy(&state.stderr)
            .contains("completion state is not a fixed-role prefix")
    );
    assert_destination_hashes(&plan, &committed_hashes);

    std::fs::write(&plan.journal_path, &journal_bytes).unwrap();
    std::fs::write(
        &plan.files[0].destination,
        b"concurrent destination drift\n",
    )
    .unwrap();
    let destination = run_plan(&plan_path, &script);
    assert!(!destination.status.success());
    assert!(
        String::from_utf8_lossy(&destination.stderr)
            .contains("Committed update journal destination drifted")
    );
    assert_eq!(
        std::fs::read(&plan.files[0].destination).unwrap(),
        b"concurrent destination drift\n"
    );
    for (file, committed_hash) in plan
        .files
        .iter()
        .skip(1)
        .zip(committed_hashes.iter().skip(1))
    {
        assert_eq!(&sha(&file.destination), committed_hash, "{}", file.role);
    }
}
