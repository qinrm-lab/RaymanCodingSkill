//! 端到端集成测试：驱动真实的 `rayman` 二进制在临时工作区跑完整流程。
//! 这些测试补足单元测试无法覆盖的东西——真实进程、真实退出码、真实文件系统状态。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_rayman");

fn rayman_command() -> Command {
    let mut command = Command::new(BIN);
    // Text assertions are intentionally Chinese unless a test explicitly
    // overrides the language through argv or an environment fixture.
    command.env("RAYMAN_LANG", "zh-CN");
    command
}

struct Output {
    status: i32,
    stdout: String,
    stderr: String,
}

fn current_activation_contract(skill_hash: &str) -> String {
    format!(
        "skill: raymancodingskill\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {skill_hash}\nbundle_sha256: {}\ncli_contract: {}\ncli_version: {}\n",
        rayman::workspace::running_canonical_skill_bundle_sha256(),
        rayman::CLI_CONTRACT,
        rayman::CLI_VERSION,
    )
}

fn write_canonical_bundle(root: &Path) {
    write(
        root,
        "SKILL.md",
        std::str::from_utf8(include_bytes!("../assets/canonical-skill.md")).unwrap(),
    );
    write(
        root,
        "AGENT_CONTRACT.md",
        std::str::from_utf8(include_bytes!("../assets/canonical-agent-contract.md")).unwrap(),
    );
    write(
        root,
        "references/workflow-contract.md",
        std::str::from_utf8(include_bytes!("../assets/canonical-workflow-contract.md")).unwrap(),
    );
}

/// 在 `dir` 下运行 `rayman <args...>`，返回退出码与输出。
fn run_raw(dir: &Path, args: &[&str]) -> Output {
    let output = rayman_command()
        .args(args)
        .current_dir(dir)
        .output()
        .expect("无法启动 rayman 二进制");
    Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("rayman stdout 必须是有效 UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("rayman stderr 必须是有效 UTF-8"),
    }
}

fn run_raw_with_stdin(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = rayman_command()
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("无法启动 rayman 二进制");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("rayman stdout 必须是有效 UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("rayman stderr 必须是有效 UTF-8"),
    }
}

/// Run with a deterministic PATH prefix.  Doctor uses this to prove the same
/// command-resolution path an interactive caller would observe.
fn run(dir: &Path, args: &[&str]) -> Output {
    let activation_exempt = matches!(
        args.first().copied(),
        Some("workspace" | "doctor" | "assets" | "state")
    );
    if !activation_exempt {
        let status = run_raw(dir, &["--format", "json", "workspace", "status"]);
        let active = serde_json::from_str::<Value>(&status.stdout)
            .ok()
            .and_then(|value| value["active"].as_bool())
            .unwrap_or(false);
        if !active {
            let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("SKILL.md")
                .canonicalize()
                .unwrap();
            let skill = skill.to_str().unwrap();
            let activated = run_raw(
                dir,
                &["workspace", "activate", "--skill-file", skill, "--yes"],
            );
            assert_eq!(
                activated.status, 0,
                "fixture activation failed: {}",
                activated.stderr
            );
        }
    }
    run_raw(dir, args)
}

fn run_with_path(
    dir: &Path,
    args: &[&str],
    path_prefix: &[&Path],
    pathext: Option<&str>,
) -> Output {
    run_with_path_and_env(dir, args, path_prefix, pathext, &[])
}

fn run_with_path_and_env(
    dir: &Path,
    args: &[&str],
    path_prefix: &[&Path],
    pathext: Option<&str>,
    environment: &[(&str, &str)],
) -> Output {
    let mut entries = path_prefix
        .iter()
        .map(|path| path.to_path_buf())
        .collect::<Vec<_>>();
    if let Some(parent_path) = std::env::var_os("PATH") {
        entries.extend(std::env::split_paths(&parent_path));
    }
    let path = std::env::join_paths(entries).expect("PATH entries must be representable");
    let mut command = rayman_command();
    command.args(args).current_dir(dir).env("PATH", path);
    command.envs(environment.iter().copied());
    if let Some(pathext) = pathext {
        command.env("PATHEXT", pathext);
    }
    let output = command.output().expect("无法启动 rayman 二进制");
    Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("rayman stdout 必须是有效 UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("rayman stderr 必须是有效 UTF-8"),
    }
}

fn run_with_exact_path_and_env(
    dir: &Path,
    args: &[&str],
    exact_path: &Path,
    pathext: Option<&str>,
    environment: &[(&str, &str)],
) -> Output {
    let mut command = rayman_command();
    command
        .args(args)
        .current_dir(dir)
        .env("PATH", exact_path)
        .envs(environment.iter().copied());
    if let Some(pathext) = pathext {
        command.env("PATHEXT", pathext);
    }
    let output = command.output().expect("无法启动 rayman 二进制");
    Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("rayman stdout 必须是有效 UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("rayman stderr 必须是有效 UTF-8"),
    }
}

#[cfg(windows)]
fn run_binary_with_env(
    binary: &Path,
    dir: &Path,
    args: &[&str],
    environment: &[(&str, &str)],
) -> Output {
    let output = Command::new(binary)
        .args(args)
        .current_dir(dir)
        .env("RAYMAN_LANG", "zh-CN")
        .env_remove("CARGO_TARGET_DIR")
        .envs(environment.iter().copied())
        .output()
        .expect("无法启动指定 rayman 二进制");
    Output {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout).expect("rayman stdout 必须是有效 UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("rayman stderr 必须是有效 UTF-8"),
    }
}

#[cfg(windows)]
struct NestedValidationProbe {
    _temp: tempfile::TempDir,
    executable: PathBuf,
}

#[cfg(windows)]
impl NestedValidationProbe {
    fn build() -> Self {
        const SOURCE: &str = r##"
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};

fn fail(message: impl AsRef<str>) -> ! {
    let message = message.as_ref();
    if let Some(trace) = env::var_os("RAYMAN_NESTED_TRACE")
        && let Ok(mut trace) = OpenOptions::new().create(true).append(true).open(trace)
    {
        let _ = writeln!(trace, "failure\t{}", message.replace(['\r', '\n'], " "));
    }
    eprintln!("nested validation probe failed: {message}");
    process::exit(91);
}

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| fail(format!("missing {name}")))
}

fn verify_lease_environment(temp: &Path, nested: &Path) {
    if !temp.is_dir() || !nested.is_dir() {
        fail("managed temp or nested validation root was not probed before spawn");
    }
    if temp.file_name().and_then(|name| name.to_str()) != Some("t")
        || nested.file_name().and_then(|name| name.to_str()) != Some("n")
        || temp.parent() != nested.parent()
    {
        fail(format!(
            "managed temp and nested validation root are not lease siblings: temp={} nested={}",
            temp.display(),
            nested.display()
        ));
    }
}

fn append_trace(phase: &str, temp: &Path, nested: &Path) {
    let trace = required_path("RAYMAN_NESTED_TRACE");
    let mut trace = OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace)
        .unwrap_or_else(|error| fail(format!("cannot open trace: {error}")));
    writeln!(trace, "{phase}\t{}\t{}", temp.display(), nested.display())
        .unwrap_or_else(|error| fail(format!("cannot write trace: {error}")));
}

fn checked_output(label: &str, output: std::io::Result<Output>) -> Output {
    let output = output.unwrap_or_else(|error| fail(format!("cannot start {label}: {error}")));
    if !output.status.success() {
        fail(format!(
            "{label} failed with {:?}: stdout={} stderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    output
}

fn goal_id(output: &Output) -> String {
    let text = String::from_utf8_lossy(&output.stdout);
    let marker = "\"id\": \"";
    let start = text
        .find(marker)
        .map(|index| index + marker.len())
        .unwrap_or_else(|| fail(format!("goal start output has no id: {text}")));
    let tail = &text[start..];
    let end = tail
        .find('"')
        .unwrap_or_else(|| fail(format!("goal id is unterminated: {text}")));
    tail[..end].to_string()
}

fn inner() {
    let temp = required_path("TEMP");
    let nested = required_path("RAYMAN_VALIDATION_TEMP_ROOT");
    verify_lease_environment(&temp, &nested);
    append_trace("inner", &temp, &nested);
    println!("nested validation child passed");
}

fn outer() {
    let temp = required_path("TEMP");
    let nested = required_path("RAYMAN_VALIDATION_TEMP_ROOT");
    verify_lease_environment(&temp, &nested);
    append_trace("outer", &temp, &nested);

    let workspace = temp.join(format!("nested-workspace-{}", process::id()));
    fs::create_dir(&workspace)
        .unwrap_or_else(|error| fail(format!("cannot create nested workspace: {error}")));
    fs::write(workspace.join("README.md"), b"nested validation workspace\n")
        .unwrap_or_else(|error| fail(format!("cannot seed nested workspace: {error}")));

    let rayman = required_path("RAYMAN_NESTED_RAYMAN");
    let skill = required_path("RAYMAN_NESTED_SKILL");
    checked_output(
        "nested workspace activation",
        Command::new(&rayman)
            .args(["workspace", "activate", "--skill-file"])
            .arg(&skill)
            .arg("--yes")
            .current_dir(&workspace)
            .output(),
    );
    let started = checked_output(
        "nested goal start",
        Command::new(&rayman)
            .args([
                "--format",
                "json",
                "goal",
                "start",
                "nested validation reentry",
                "--must-proof",
                "generic::nested direct child completes",
            ])
            .current_dir(&workspace)
            .output(),
    );
    let id = goal_id(&started);
    fs::write(
        workspace.join("README.md"),
        b"nested validation workspace changed\n",
    )
    .unwrap_or_else(|error| fail(format!("cannot change nested workspace: {error}")));
    checked_output(
        "nested context refresh",
        Command::new(&rayman)
            .args(["--format", "json", "context", "refresh"])
            .current_dir(&workspace)
            .output(),
    );
    let current = env::current_exe()
        .unwrap_or_else(|error| fail(format!("cannot resolve probe executable: {error}")));
    let logical_command = format!("\"{}\" inner", current.display());
    checked_output(
        "nested goal validate",
        Command::new(&rayman)
            .args([
                "--format",
                "json",
                "goal",
                "validate",
                &id,
                "--req",
                "req_1",
                "-m",
                "nested validation completed",
                "--command",
                &logical_command,
                "--changed",
                "README.md",
            ])
            .current_dir(&workspace)
            .output(),
    );
    println!("nested validation reentry passed");
}

fn main() {
    match env::args().nth(1).as_deref() {
        Some("outer") => outer(),
        Some("inner") => inner(),
        other => fail(format!("unexpected mode: {other:?}")),
    }
}
"##;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("nested-validation-probe.rs");
        let executable = temp.path().join("nested-validation-probe.exe");
        std::fs::write(&source, SOURCE).unwrap();
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .arg("--edition=2024")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("must start rustc for nested validation probe");
        assert!(
            output.status.success(),
            "nested validation probe did not compile: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self {
            _temp: temp,
            executable,
        }
    }
}

#[cfg(windows)]
fn nested_validation_trace(path: &Path) -> Vec<(String, PathBuf, PathBuf)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let phase = fields.next().expect("nested trace phase");
            let temp = fields.next().expect("nested trace temp");
            let nested = fields.next().expect("nested trace root");
            assert!(
                fields.next().is_none(),
                "unexpected nested trace row: {line}"
            );
            (
                phase.to_string(),
                PathBuf::from(temp),
                PathBuf::from(nested),
            )
        })
        .collect()
}

struct NativePytestProbe {
    _temp: tempfile::TempDir,
    bin_dir: PathBuf,
}

impl NativePytestProbe {
    fn build() -> Self {
        const SOURCE: &str = r##"
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

fn fail(message: impl AsRef<str>) -> ! {
    eprintln!("pytest probe rejected invocation: {}", message.as_ref());
    process::exit(86);
}

fn required_path(name: &str) -> PathBuf {
    env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| fail(format!("missing {name}")))
}

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let pytest_start = if args.first().map(String::as_str) == Some("-m")
        && args.get(1).map(String::as_str) == Some("pytest")
    {
        2
    } else if args.first().is_some_and(|arg| arg.starts_with("-3"))
        && args.get(1).map(String::as_str) == Some("-m")
        && args.get(2).map(String::as_str) == Some("pytest")
    {
        3
    } else {
        0
    };
    let pytest = &args[pytest_start..];
    let separator = pytest
        .iter()
        .position(|argument| argument == "--")
        .unwrap_or(pytest.len());
    let options = &pytest[..separator];

    let mut basetemps = Vec::new();
    let mut cache_dirs = Vec::new();
    let mut addopts = Vec::new();
    let mut index = 0;
    while let Some(argument) = options.get(index) {
        if argument == "--basetemp" {
            basetemps.push(
                options
                    .get(index + 1)
                    .cloned()
                    .unwrap_or_else(|| fail("--basetemp has no value")),
            );
            index += 2;
            continue;
        }
        if let Some(value) = argument.strip_prefix("--basetemp=") {
            basetemps.push(value.to_string());
        }
        if argument == "-o" {
            if let Some(value) = options.get(index + 1) {
                if let Some(path) = value.strip_prefix("cache_dir=") {
                    cache_dirs.push(path.to_string());
                }
                if let Some(value) = value.strip_prefix("addopts=") {
                    addopts.push(value.to_string());
                }
            }
            index += 2;
            continue;
        }
        let inline = argument
            .strip_prefix("-o=")
            .or_else(|| argument.strip_prefix("-o").filter(|value| !value.is_empty()));
        if let Some(path) = inline.and_then(|value| value.strip_prefix("cache_dir=")) {
            cache_dirs.push(path.to_string());
        }
        if let Some(value) = inline.and_then(|value| value.strip_prefix("addopts=")) {
            addopts.push(value.to_string());
        }
        index += 1;
    }
    if basetemps.len() != 1 || cache_dirs.len() != 1 || addopts != [""] {
        fail(format!(
            "expected one basetemp/cache_dir and one empty addopts, got {}/{}/{} in {options:?}",
            basetemps.len(),
            cache_dirs.len(),
            addopts.len()
        ));
    }

    let basetemp = PathBuf::from(&basetemps[0]);
    let lease_root = basetemp
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| fail("basetemp has no lease root"));
    let (basetemp_name, cache_name, temp_name, pycache_name, nested_name) = if cfg!(windows) {
        ("b", "c", "t", "y", "n")
    } else {
        ("basetemp", "cache", "temp", "pycache", "nested-validation")
    };
    if basetemp.file_name().and_then(|name| name.to_str()) != Some(basetemp_name) {
        fail("basetemp does not use the managed lease layout");
    }
    let expected_cache = lease_root.join(cache_name);
    if Path::new(&cache_dirs[0]) != expected_cache {
        fail("cache_dir is outside the basetemp lease");
    }
    for (name, expected) in [
        ("TEMP", lease_root.join(temp_name)),
        ("TMP", lease_root.join(temp_name)),
        ("TMPDIR", lease_root.join(temp_name)),
        ("PYTHONPYCACHEPREFIX", lease_root.join(pycache_name)),
        (
            "RAYMAN_VALIDATION_TEMP_ROOT",
            lease_root.join(nested_name),
        ),
    ] {
        let actual = required_path(name);
        if actual != expected || !actual.is_dir() {
            fail(format!("{name} is not the live managed path"));
        }
    }
    if !basetemp.is_dir() || !expected_cache.is_dir() {
        fail("managed pytest directories were not probed before spawn");
    }
    if env::var("PYTHONDONTWRITEBYTECODE").as_deref() != Ok("1") {
        fail("PYTHONDONTWRITEBYTECODE is not managed");
    }
    if env::var_os("PYTEST_ADDOPTS").is_some() {
        fail("PYTEST_ADDOPTS was inherited");
    }

    let collect_count = options
        .iter()
        .filter(|argument| argument.as_str() == "--collect-only")
        .count();
    if collect_count > 1 {
        fail("collect-only was injected more than once");
    }
    let phase = if collect_count == 1 { "collect" } else { "run" };
    let trace_path = env::var_os("RAYMAN_PYTEST_PROBE_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| fail("missing RAYMAN_PYTEST_PROBE_LOG"));
    let mut trace = OpenOptions::new()
        .create(true)
        .append(true)
        .open(trace_path)
        .unwrap_or_else(|error| fail(format!("cannot open trace: {error}")));
    writeln!(trace, "{phase}\t{}", lease_root.display())
        .unwrap_or_else(|error| fail(format!("cannot write trace: {error}")));
    drop(trace);

    let mode = env::var("RAYMAN_PYTEST_PROBE_MODE").unwrap_or_else(|_| "success".into());
    if phase == "collect" {
        if mode == "collect-fail" {
            eprintln!("collection failed by probe");
            process::exit(41);
        }
        if mode == "collect-zero" {
            println!("0 tests collected in 0.01s");
            return;
        }
        println!("1 test collected in 0.01s");
        return;
    }

    if let Some(nested_probe) = env::var_os("RAYMAN_NESTED_PROBE") {
        let output = Command::new(nested_probe)
            .arg("outer")
            .output()
            .unwrap_or_else(|error| fail(format!("cannot start nested validation probe: {error}")));
        if !output.status.success() {
            fail(format!(
                "nested validation probe failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }

    if matches!(mode.as_str(), "cleanup-fail" | "run-cleanup-fail") {
        fs::write(lease_root.join("lease.json"), b"{}")
            .unwrap_or_else(|error| fail(format!("cannot corrupt manifest: {error}")));
    }
    if matches!(mode.as_str(), "run-fail" | "run-cleanup-fail") {
        println!("1 failed in 0.01s");
        process::exit(37);
    }
    println!("1 passed in 0.01s");
}
"##;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("pytest-probe.rs");
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        std::fs::write(&source, SOURCE).unwrap();
        let compiled = temp
            .path()
            .join(format!("pytest-probe{}", std::env::consts::EXE_SUFFIX));
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .arg("--edition=2024")
            .arg(&source)
            .arg("-o")
            .arg(&compiled)
            .output()
            .expect("must start rustc for pytest probe");
        assert!(
            output.status.success(),
            "pytest probe did not compile: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for launcher in ["pytest", "python", "py"] {
            let target = bin_dir.join(format!("{launcher}{}", std::env::consts::EXE_SUFFIX));
            if std::fs::hard_link(&compiled, &target).is_err() {
                std::fs::copy(&compiled, &target).unwrap();
            }
        }
        Self {
            _temp: temp,
            bin_dir,
        }
    }

    fn pathext(&self) -> Option<&'static str> {
        cfg!(windows).then_some(".EXE")
    }
}

fn start_pytest_validation_goal(root: &Path) -> String {
    write(root, "README.md", "pytest validation fixture\n");
    write(root, "pytest.ini", "[pytest]\naddopts = -k never\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &[
            "goal",
            "start",
            "managed pytest execution",
            "--must-proof",
            "test::run managed pytest",
        ],
    )["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn pytest_trace(path: &Path) -> Vec<(String, PathBuf)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let (phase, root) = line.split_once('\t').expect("phase and lease root");
            (phase.to_string(), PathBuf::from(root))
        })
        .collect()
}

fn assert_no_live_pytest_leases(root: &Path) {
    let leases = if cfg!(windows) {
        root.join("p")
    } else {
        root.join(".RaymanCodingSkill/tmp/leases")
    };
    assert!(
        !leases.exists() || std::fs::read_dir(&leases).unwrap().next().is_none(),
        "managed pytest lease remained under {}",
        leases.display()
    );
}

fn visit_json_strings(value: &Value, visitor: &mut impl FnMut(&str)) {
    match value {
        Value::String(text) => visitor(text),
        Value::Array(items) => {
            for item in items {
                visit_json_strings(item, visitor);
            }
        }
        Value::Object(fields) => {
            for item in fields.values() {
                visit_json_strings(item, visitor);
            }
        }
        _ => {}
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

fn add_complete_human_pending(
    root: &Path,
    goal_id: &str,
    capability_key: &str,
    detail: &str,
) -> Value {
    add_complete_human_pending_with_title(root, goal_id, capability_key, "choice", detail)
}

fn add_complete_human_pending_with_title(
    root: &Path,
    goal_id: &str,
    capability_key: &str,
    title: &str,
    detail: &str,
) -> Value {
    let args = vec![
        "goal".to_string(),
        "pending".into(),
        "add".into(),
        title.into(),
        "-m".into(),
        detail.into(),
        "--goal".into(),
        goal_id.into(),
        "--owner".into(),
        "human".into(),
        "--kind".into(),
        "human_input".into(),
        "--attempt".into(),
        "completed every safe local path".into(),
        "--evidence-path".into(),
        "reports/options.md".into(),
        "--minimum-input".into(),
        "choose A or B".into(),
        "--recommended".into(),
        "choose A".into(),
        "--alternative".into(),
        "choose B".into(),
        "--risk".into(),
        "behavior differs".into(),
        "--resume-command".into(),
        format!("rayman prepare --goal {goal_id}"),
        "--auto-resume-condition".into(),
        "owner records choice".into(),
        "--consultation-timing".into(),
        "immediate".into(),
        "--capability-key".into(),
        capability_key.into(),
        "--boundary-class".into(),
        "owner_decision".into(),
    ];
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_json(root, &args)
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
        // 不能用 `rustc --version`：纯 version/help 查询现在被相关性下限判定为
        // 自证无关的探针，正是这个 helper 要绕开的东西。`--print sysroot` 是一条
        // 真实执行、退出 0、且不依赖工作区是不是 git 仓库的命令。
        "rustc --print sysroot".into()
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

fn validate_goal_authority(
    root: &Path,
    id: &str,
    req: &str,
    message: &str,
    changed: &[&str],
) -> Value {
    let command = "cargo test --workspace --all-targets".to_string();
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
        "--authority",
        "--repeat",
        "2",
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

#[cfg(windows)]
fn canonical_display_path(path: &Path) -> PathBuf {
    PathBuf::from(rayman::pathfmt::display_path(&path.canonicalize().unwrap()))
}

fn rebind_activation_path(root: &Path) -> std::path::PathBuf {
    root.join(".RaymanCodingSkill/workspace_skill.yaml")
}

fn activate_rebind_fixture(root: &Path) -> std::path::PathBuf {
    let skill_path = root.join("skill-fixtures/canonical SKILL.md");
    write(
        root,
        "skill-fixtures/canonical SKILL.md",
        std::str::from_utf8(include_bytes!("../assets/canonical-skill.md")).unwrap(),
    );
    write(
        root,
        "skill-fixtures/AGENT_CONTRACT.md",
        std::str::from_utf8(include_bytes!("../assets/canonical-agent-contract.md")).unwrap(),
    );
    write(
        root,
        "skill-fixtures/references/workflow-contract.md",
        std::str::from_utf8(include_bytes!("../assets/canonical-workflow-contract.md")).unwrap(),
    );
    let activated = run_raw(
        root,
        &[
            "workspace",
            "activate",
            "--skill-file",
            skill_path.to_str().unwrap(),
            "--yes",
        ],
    );
    assert_eq!(
        activated.status, 0,
        "fixture activation failed: stdout={} stderr={}",
        activated.stdout, activated.stderr
    );
    skill_path
}

fn make_rebind_eligible_identity_drift(root: &Path) -> std::path::PathBuf {
    let skill_path = activate_rebind_fixture(root);
    let activation_path = rebind_activation_path(root);
    let original = std::fs::read_to_string(&activation_path).unwrap();
    let current_contract = format!("cli_contract: {}", rayman::CLI_CONTRACT);
    let current_version = format!("cli_version: {}", rayman::CLI_VERSION);
    let stale = original
        .replace(&current_contract, "cli_contract: rayman-cli-contract-v1")
        .replace(&current_version, "cli_version: 0.1.0");
    assert_ne!(
        stale, original,
        "fixture must replace the running CLI identity"
    );
    assert!(stale.contains("cli_contract: rayman-cli-contract-v1"));
    assert!(stale.contains("cli_version: 0.1.0"));

    std::fs::write(&skill_path, include_bytes!("../assets/canonical-skill.md")).unwrap();
    std::fs::write(&activation_path, stale).unwrap();
    activation_path
}

fn complete_rebind_contract(
    skill: &str,
    enabled: bool,
    skill_file: &str,
    skill_sha256: &str,
) -> String {
    format!(
        "skill: {skill}\nenabled: {enabled}\nskill_file: {skill_file}\nskill_sha256: {skill_sha256}\ncli_contract: {}\ncli_version: {}\n",
        rayman::CLI_CONTRACT,
        rayman::CLI_VERSION
    )
}

fn managed_state_without_activation(
    root: &Path,
) -> BTreeMap<String, (u64, std::time::SystemTime, Vec<u8>)> {
    let mut snapshot = state_snapshot(root);
    snapshot.remove("workspace_skill.yaml");
    snapshot
}

#[cfg(target_os = "linux")]
fn remove_linux_retained_activation(
    snapshot: &mut BTreeMap<String, (u64, std::time::SystemTime, Vec<u8>)>,
    expected_bytes: &[u8],
) {
    let retained = snapshot
        .keys()
        .filter(|path| path.starts_with("tmp/activation-retained/"))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        retained.len(),
        1,
        "Linux rebind must retain exactly one prior activation: {retained:?}"
    );
    let (_, _, bytes) = snapshot.remove(&retained[0]).unwrap();
    assert_eq!(bytes, expected_bytes);
}

fn assert_rebind_rejected_without_state_changes(root: &Path, case: &str) {
    let activation_path = rebind_activation_path(root);
    // Rebind must acquire the shared activation lock before rejecting a parsed contract.
    // Seed that stable lifecycle file so the snapshot checks managed data bytes rather
    // than treating first-use lock creation as an application-state mutation.
    drop(rayman::state_lock::acquire_state_lock(&activation_path).unwrap());
    let activation_before = std::fs::read(&activation_path).ok();
    let state_before = state_snapshot(root);
    let rejected = run_raw(root, &["--format", "json", "workspace", "rebind", "--yes"]);
    assert_ne!(
        rejected.status, 0,
        "case={case} stdout={} stderr={}",
        rejected.stdout, rejected.stderr
    );
    assert_eq!(
        std::fs::read(&activation_path).ok(),
        activation_before,
        "case={case} changed workspace_skill.yaml"
    );
    assert_eq!(
        state_snapshot(root),
        state_before,
        "case={case} changed managed state"
    );

    // `ensure-current --yes` must not turn any manual-repair state into a
    // convenient activation path.  Unlike `rebind`, it rejects before taking
    // a lock; this asserts both commands leave the parsed contract and every
    // pre-existing managed file unchanged.
    let ensure_rejected = run_raw(
        root,
        &["--format", "json", "workspace", "ensure-current", "--yes"],
    );
    assert_ne!(
        ensure_rejected.status, 0,
        "ensure-current case={case} stdout={} stderr={}",
        ensure_rejected.stdout, ensure_rejected.stderr
    );
    assert_eq!(
        std::fs::read(&activation_path).ok(),
        activation_before,
        "ensure-current case={case} changed workspace_skill.yaml"
    );
    assert_eq!(
        state_snapshot(root),
        state_before,
        "ensure-current case={case} changed managed state"
    );
}

fn assert_exact_rebind_hint(surface: &str, source: &str) {
    assert!(
        surface.contains("rayman workspace rebind --yes"),
        "{source} did not provide the exact rebind command: {surface}"
    );
    assert!(
        !surface.contains("workspace activate --skill-file"),
        "{source} incorrectly routed eligible identity drift through activation: {surface}"
    );
}

#[test]
fn workspace_install_bind_is_hidden_confirmed_and_path_stable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let canonical = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();
    let canonical_text = canonical.to_str().unwrap();

    let help = run_raw(root, &["workspace", "--help"]);
    assert_eq!(help.status, 0, "{}", help.stderr);
    assert!(!help.stdout.contains("install-bind"), "{}", help.stdout);

    let rejected = run_raw(
        root,
        &["workspace", "install-bind", "--skill-file", canonical_text],
    );
    assert_ne!(rejected.status, 0);
    assert!(rejected.stderr.contains("--yes"), "{}", rejected.stderr);
    let binding = root.join(".RaymanCodingSkill/workspace_skill.yaml");
    assert!(!binding.exists());

    let created = run_raw(
        root,
        &[
            "--format",
            "json",
            "workspace",
            "install-bind",
            "--skill-file",
            canonical_text,
            "--yes",
        ],
    );
    assert_eq!(created.status, 0, "{}", created.stderr);
    let created_json: Value = serde_json::from_str(&created.stdout).unwrap();
    assert_eq!(created_json["active"], true);
    assert_eq!(created_json["changed"], true);
    let created_bytes = std::fs::read(&binding).unwrap();

    let no_op = run_raw(
        root,
        &[
            "--format",
            "json",
            "workspace",
            "install-bind",
            "--skill-file",
            canonical_text,
            "--yes",
        ],
    );
    assert_eq!(no_op.status, 0, "{}", no_op.stderr);
    let no_op_json: Value = serde_json::from_str(&no_op.stdout).unwrap();
    assert_eq!(no_op_json["changed"], false);
    assert_eq!(std::fs::read(&binding).unwrap(), created_bytes);

    let stale = std::fs::read_to_string(&binding)
        .unwrap()
        .replace(
            &format!("cli_contract: {}", rayman::CLI_CONTRACT),
            "cli_contract: rayman-cli-contract-v1",
        )
        .replace(
            &format!("cli_version: {}", rayman::CLI_VERSION),
            "cli_version: 0.1.0",
        );
    std::fs::write(&binding, stale).unwrap();
    let rebound = run_raw(
        root,
        &[
            "--format",
            "json",
            "workspace",
            "install-bind",
            "--skill-file",
            canonical_text,
            "--yes",
        ],
    );
    assert_eq!(rebound.status, 0, "{}", rebound.stderr);
    let rebound_json: Value = serde_json::from_str(&rebound.stdout).unwrap();
    assert_eq!(rebound_json["changed"], true);
    assert_eq!(rebound_json["active"], true);

    let alternate = root.join("alternate-SKILL.md");
    std::fs::write(&alternate, include_bytes!("../assets/canonical-skill.md")).unwrap();
    write(
        root,
        "AGENTS.md",
        include_str!("../assets/canonical-agent-contract.md"),
    );
    write(
        root,
        "references/workflow-contract.md",
        include_str!("../assets/canonical-workflow-contract.md"),
    );
    let binding_before_path_change = std::fs::read(&binding).unwrap();
    let path_change = run_raw(
        root,
        &[
            "workspace",
            "install-bind",
            "--skill-file",
            alternate.to_str().unwrap(),
            "--yes",
        ],
    );
    assert_ne!(path_change.status, 0);
    assert!(
        path_change.stderr.contains("path change"),
        "{}",
        path_change.stderr
    );
    assert_eq!(std::fs::read(&binding).unwrap(), binding_before_path_change);
}

fn run_update_with_user_root(root: &Path, user_root: &Path, args: &[&str]) -> Output {
    let user_root = user_root.to_str().unwrap();
    run_with_path_and_env(
        root,
        args,
        &[],
        None,
        &[
            ("RAYMAN_INTERNAL_TEST_UPDATE_ROOT", user_root),
            ("LOCALAPPDATA", user_root),
            ("XDG_DATA_HOME", user_root),
            ("HOME", user_root),
            ("USERPROFILE", user_root),
        ],
    )
}

#[path = "cli_cases/goal_evidence.rs"]
mod goal_evidence;
#[path = "cli_cases/host.rs"]
mod host;
#[path = "cli_cases/lifecycle.rs"]
mod lifecycle;
#[path = "cli_cases/navigation.rs"]
mod navigation;
#[path = "cli_cases/pending.rs"]
mod pending;
#[path = "cli_cases/project_map.rs"]
mod project_map;
#[path = "cli_cases/readiness.rs"]
mod readiness;
#[path = "cli_cases/recovery.rs"]
mod recovery;
#[path = "cli_cases/update.rs"]
mod update;
#[path = "cli_cases/validation_process.rs"]
mod validation_process;
#[path = "cli_cases/workspace.rs"]
mod workspace;
