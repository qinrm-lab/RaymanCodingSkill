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
    for file in &plan.files {
        assert_eq!(sha(&file.destination), file.new_sha256, "{}", file.role);
    }
}
