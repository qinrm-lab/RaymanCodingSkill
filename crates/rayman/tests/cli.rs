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
        "skill: raymancodingskill\nenabled: true\nskill_file: SKILL.md\nskill_sha256: {skill_hash}\ncli_contract: {}\ncli_version: {}\n",
        rayman::CLI_CONTRACT,
        rayman::CLI_VERSION,
    )
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
        "canonical skill before upgrade\n",
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
    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_legacy"],
    );
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

    let standard = run(
        root,
        &["check", "--profile", "standard", "--goal", "goal_legacy"],
    );
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
    write(root, "SKILL.md", "ordinary workspace canonical skill\n");
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
    write(root, "SKILL.md", "ordinary workspace canonical skill\n");
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
    assert_eq!(report["state_write"]["probed"], true);
    assert_eq!(report["state_write"]["writable"], true);
    assert_eq!(report["activation_metadata"]["applicable"], true);
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

    let inspect_en = run_raw(root, &["--language", "en", "workspace", "inspect"]);
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
    write(root, "SKILL.md", "ordinary workspace canonical skill\n");
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
    write(root, "SKILL.md", "ordinary workspace canonical skill\n");
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
fn map_plan_check_passes_broad_change_without_supported_package() {
    // Real-world basis: dogfooding rayman against a 792-file, 60k-line C# repo showed
    // this heuristic hard-blocking a well-tested change because it only understands
    // modeled package shapes. Outside Cargo/pyproject packages it must be advisory.
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
        plan.stdout
            .contains("no Cargo or pyproject package detected"),
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

    let standard = run(
        root,
        &[
            "check",
            "--profile",
            "standard",
            "--goal",
            "goal_legacy_active",
        ],
    );
    assert_eq!(standard.status, 1, "stdout={}", standard.stdout);
    assert!(standard.stdout.contains("legacy goal goal_legacy_active"));
    assert!(!standard.stdout.contains("合约无效"));
}

#[cfg(windows)]
#[test]
fn generic_validation_child_can_run_nested_goal_validate_from_its_managed_temp() {
    let probe = NestedValidationProbe::build();
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "README.md", "generic nested validation fixture\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "generic nested validation",
            "--must-proof",
            "generic::run a nested validation",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    let evidence_home = tempfile::tempdir().unwrap();
    let host_temp_root = evidence_home.path().join("host-temp");
    std::fs::create_dir_all(&host_temp_root).unwrap();
    let canonical_host_temp_root = canonical_display_path(&host_temp_root);
    let trace = evidence_home.path().join("nested-validation.trace");
    let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();
    let logical_command = format!("\"{}\" outer", probe.executable.display());
    let output = run_with_path_and_env(
        root,
        &[
            "--format",
            "json",
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "generic child reentered goal validate",
            "--command",
            &logical_command,
            "--changed",
            "README.md",
        ],
        &[],
        None,
        &[
            (
                "RAYMAN_VALIDATION_TEMP_ROOT",
                host_temp_root.to_str().unwrap(),
            ),
            ("RAYMAN_NESTED_RAYMAN", BIN),
            ("RAYMAN_NESTED_SKILL", skill.to_str().unwrap()),
            ("RAYMAN_NESTED_TRACE", trace.to_str().unwrap()),
        ],
    );
    assert_eq!(
        output.status,
        0,
        "stdout={}\nstderr={}\ntrace={}",
        output.stdout,
        output.stderr,
        std::fs::read_to_string(&trace).unwrap_or_default()
    );

    let records = nested_validation_trace(&trace);
    assert_eq!(
        records
            .iter()
            .map(|(phase, _, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["outer", "inner"],
        "records={records:?}"
    );
    let (_, outer_temp, outer_nested) = &records[0];
    let (_, inner_temp, inner_nested) = &records[1];
    let outer_lease_root = outer_temp.parent().unwrap();
    let inner_lease_root = inner_temp.parent().unwrap();
    assert!(
        outer_lease_root.starts_with(canonical_host_temp_root.join("v")),
        "outer lease escaped configured root: {}",
        outer_lease_root.display()
    );
    assert_eq!(outer_nested, &outer_lease_root.join("n"));
    assert!(
        inner_lease_root.starts_with(outer_nested.join("v")),
        "inner lease escaped the parent nested root: {}",
        inner_lease_root.display()
    );
    assert_eq!(inner_nested, &inner_lease_root.join("n"));
    for path in [outer_temp, outer_nested, inner_temp, inner_nested] {
        assert!(
            !path.exists(),
            "successful nested validation left {}",
            path.display()
        );
    }
    assert!(!host_temp_root.join(".RaymanCodingSkill").exists());

    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.join(".RaymanCodingSkill/goals")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    visit_json_strings(&persisted, &mut |text| {
        for (_, temp, nested) in &records {
            assert!(!text.contains(temp.to_string_lossy().as_ref()), "{text}");
            assert!(!text.contains(nested.to_string_lossy().as_ref()), "{text}");
            for path in [temp, nested] {
                let lease_id = path
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap()
                    .to_string_lossy();
                assert!(!text.contains(lease_id.as_ref()), "{text}");
            }
        }
    });
}

#[cfg(windows)]
#[test]
fn goal_validate_self_hosted_gate_uses_one_managed_target_without_rewriting_running_cli() {
    let trace_home = tempfile::tempdir().unwrap();
    // The actual Cargo invocation below protects this layout from drifting
    // back to the verbose form that exhausts MSVC's practical path budget.
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let trace = trace_home.path().join("self-hosted-target.trace");
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"self-hosted-validation-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n\n[[bin]]\nname = \"rayman\"\npath = \"src/main.rs\"\n",
    );
    write(root, "src/main.rs", "fn main() {}\n");
    write(
        root,
        "build.rs",
        r#"use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let trace = std::env::var("RAYMAN_SELF_HOST_TRACE").unwrap();
    let target = std::env::var("CARGO_TARGET_DIR").unwrap();
    let temp = std::env::var("TEMP").unwrap();
    let mut file = OpenOptions::new().create(true).append(true).open(trace).unwrap();
    writeln!(file, "build\t{target}\t{temp}").unwrap();
}
"#,
    );
    write(
        root,
        "tests/target_trace.rs",
        r#"use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

#[test]
fn records_the_managed_target() {
    assert!(Path::new(env!("CARGO_BIN_EXE_rayman")).is_file());
    let trace = std::env::var("RAYMAN_SELF_HOST_TRACE").unwrap();
    let target = std::env::var("CARGO_TARGET_DIR").unwrap();
    let temp = std::env::var("TEMP").unwrap();
    let mut file = OpenOptions::new().create(true).append(true).open(trace).unwrap();
    writeln!(file, "run\t{target}\t{temp}").unwrap();
}
"#,
    );
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "self-hosted gate",
            "--must-proof",
            "test::run the self-hosted gate",
        ],
    );
    let id = goal["id"].as_str().unwrap();

    let collision_target = root.join(".RaymanCodingSkill/tmp/collision-target");
    let copied_cli = collision_target.join("debug/rayman.exe");
    std::fs::create_dir_all(copied_cli.parent().unwrap()).unwrap();
    std::fs::copy(BIN, &copied_cli).unwrap();
    let original_bytes = std::fs::read(&copied_cli).unwrap();
    let canonical_root = canonical_display_path(root);
    let canonical_collision_target = canonical_display_path(&collision_target);
    let collision_text = collision_target.to_str().unwrap();
    let trace_text = trace.to_str().unwrap();
    let host_temp_root = trace_home.path().join("rayman-host-temp");
    let inherited_temp = trace_home.path().join("inherited-temp");
    std::fs::create_dir(&host_temp_root).unwrap();
    std::fs::create_dir(&inherited_temp).unwrap();
    let canonical_host_temp_root = canonical_display_path(&host_temp_root);
    let host_temp_text = host_temp_root.to_str().unwrap();
    let inherited_temp_text = inherited_temp.to_str().unwrap();
    let logical_command = "cargo test --workspace --all-targets";
    let output = run_binary_with_env(
        &copied_cli,
        root,
        &[
            "--format",
            "json",
            "goal",
            "validate",
            id,
            "--req",
            "req_1",
            "-m",
            "self-hosted gate stayed isolated",
            "--command",
            logical_command,
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
        ],
        &[
            ("CARGO_TARGET_DIR", collision_text),
            ("RAYMAN_SELF_HOST_TRACE", trace_text),
            ("RAYMAN_VALIDATION_TEMP_ROOT", host_temp_text),
            ("TEMP", inherited_temp_text),
            ("TMP", inherited_temp_text),
            ("TMPDIR", inherited_temp_text),
        ],
    );
    assert_eq!(
        output.status,
        0,
        "stdout={}\nstderr={}\ntrace={}",
        output.stdout,
        output.stderr,
        std::fs::read_to_string(&trace).unwrap_or_default()
    );
    assert_eq!(std::fs::read(&copied_cli).unwrap(), original_bytes);
    assert!(!collision_target.join("debug/deps").exists());
    assert!(!collision_target.join(".rustc_info.json").exists());

    let records = std::fs::read_to_string(&trace)
        .unwrap()
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let phase = fields.next().unwrap();
            let target = fields.next().unwrap();
            let temp = fields.next().unwrap();
            assert!(fields.next().is_none(), "unexpected trace row: {line}");
            (
                phase.to_string(),
                PathBuf::from(target),
                PathBuf::from(temp),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .map(|(phase, _, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["build", "run", "run"],
        "list proof and both repeats must be observable: {records:?}"
    );
    assert!(
        records.windows(2).all(|pair| pair[0].1 == pair[1].1),
        "list proof and repeats must reuse one target: {records:?}"
    );
    let process_temps = records
        .iter()
        .map(|(_, _, temp)| temp.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        process_temps.len(),
        3,
        "list proof and repeats must use independent process temp leases: {records:?}"
    );
    for process_temp in &process_temps {
        assert!(
            process_temp.starts_with(canonical_host_temp_root.join("v")),
            "validation process temp escaped the configured root: {}",
            process_temp.display()
        );
        assert_eq!(
            process_temp.file_name().and_then(|name| name.to_str()),
            Some("t"),
            "validation process temp did not use the compact child alias: {}",
            process_temp.display()
        );
        let process_lease = process_temp.parent().unwrap();
        let compact_process_parent = canonical_host_temp_root.join("v");
        assert_eq!(
            process_lease.parent(),
            Some(compact_process_parent.as_path()),
            "validation process lease did not use the compact parent alias: {}",
            process_temp.display()
        );
        assert!(
            process_lease
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|id| id.starts_with("v-")),
            "validation process lease id did not use its compact label: {}",
            process_lease.display()
        );
        assert!(
            !process_temp.exists(),
            "validation process lease was not released: {}",
            process_temp.display()
        );
    }
    assert!(
        !host_temp_root.join(".RaymanCodingSkill").exists(),
        "external validation root became a false workspace marker"
    );
    let managed_target = &records[0].1;
    assert!(
        managed_target.starts_with(canonical_root.join(".RaymanCodingSkill/tmp/c")),
        "unexpected managed target: {}",
        managed_target.display()
    );
    assert_eq!(
        managed_target.file_name().and_then(|name| name.to_str()),
        Some("t"),
        "managed Cargo target did not use the compact child alias: {}",
        managed_target.display()
    );
    let target_lease = managed_target.parent().unwrap();
    let compact_target_parent = canonical_root.join(".RaymanCodingSkill/tmp/c");
    assert_eq!(
        target_lease.parent(),
        Some(compact_target_parent.as_path()),
        "managed Cargo target did not use the compact parent alias: {}",
        managed_target.display()
    );
    let target_lease_id = target_lease
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap();
    let legacy_lease_id = format!(
        "cargo-{}",
        target_lease_id
            .strip_prefix("c-")
            .expect("compact Cargo lease label")
    );
    let legacy_target = canonical_root
        .join(".RaymanCodingSkill/tmp/cargo-target-leases")
        .join(legacy_lease_id)
        .join("target");
    assert!(
        legacy_target.to_string_lossy().len() >= managed_target.to_string_lossy().len() + 27,
        "compact Cargo layout did not recover the required Windows path budget: managed={} legacy={}",
        managed_target.display(),
        legacy_target.display()
    );
    assert!(!managed_target.starts_with(&canonical_collision_target));
    assert!(!managed_target.to_string_lossy().starts_with(r"\\?\"));
    assert!(
        !managed_target.exists(),
        "successful validation did not release {}",
        managed_target.display()
    );

    let returned: Value = serde_json::from_str(&output.stdout).unwrap();
    let validated: rayman::goal::Goal = serde_json::from_value(returned).unwrap();
    let validation = &validated.requirements[0].validations[0];
    let authority = &validated.authority_receipts[0];
    assert_eq!(validation.command, logical_command);
    assert_eq!(authority.command, logical_command);
    assert_eq!(authority.repeat, 2);
    assert_eq!(authority.runs.len(), 2);
    let receipt = validation.receipt.as_ref().unwrap();
    assert_eq!(receipt.listed_tests, Some(1));
    assert_eq!(receipt.passed_tests, Some(1));
    assert_eq!(
        receipt.invocation_sha256,
        rayman::goal::validation_invocation_sha256_scoped_mode(
            logical_command,
            &validation.impact_scopes,
            validation.non_code,
            validation.workspace_snapshot,
        )
    );
    assert_eq!(
        authority.invocation_sha256,
        rayman::goal::authority_invocation_sha256_mode(
            logical_command,
            "req_1",
            2,
            &authority.impact_scopes,
            authority.non_code,
            authority.workspace_snapshot,
        )
    );
    let lease_id = managed_target
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap();
    let process_lease_ids = process_temps
        .iter()
        .filter_map(|temp| temp.parent())
        .filter_map(Path::file_name)
        .filter_map(|value| value.to_str())
        .collect::<Vec<_>>();

    let persisted_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let persisted: Value =
        serde_json::from_str(&std::fs::read_to_string(persisted_path).unwrap()).unwrap();
    visit_json_strings(&persisted, &mut |text| {
        assert!(
            !text.contains(collision_text),
            "collision target leaked: {text}"
        );
        assert!(
            !text.contains(managed_target.to_string_lossy().as_ref()),
            "managed target leaked: {text}"
        );
        assert!(
            !text.contains(lease_id),
            "Cargo target lease id leaked: {text}"
        );
        assert!(
            !text.contains(copied_cli.to_string_lossy().as_ref()),
            "copied CLI leaked: {text}"
        );
        for process_temp in &process_temps {
            assert!(
                !text.contains(process_temp.to_string_lossy().as_ref()),
                "validation process temp leaked: {text}"
            );
        }
        for process_lease_id in &process_lease_ids {
            assert!(
                !text.contains(process_lease_id),
                "validation process lease id leaked: {text}"
            );
        }
    });
}

#[cfg(windows)]
#[test]
fn goal_progress_uses_and_releases_a_fresh_validation_process_temp() {
    const SOURCE: &str = r#"
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let temp = PathBuf::from(env::var_os("TEMP").expect("TEMP"));
    let tmp = PathBuf::from(env::var_os("TMP").expect("TMP"));
    let tmpdir = PathBuf::from(env::var_os("TMPDIR").expect("TMPDIR"));
    assert_eq!(temp, tmp);
    assert_eq!(temp, tmpdir);
    assert!(temp.is_dir());
    fs::write(temp.join("child-probe.txt"), b"probed").unwrap();
    fs::write(
        env::var_os("RAYMAN_PROGRESS_TEMP_TRACE").expect("trace"),
        temp.to_string_lossy().as_bytes(),
    )
    .unwrap();
}
"#;

    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "src/lib.rs", "pub fn value() -> u8 { 1 }\n");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "progress temp", "--must", "deliver"],
    );
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

    let probe_home = tempfile::tempdir().unwrap();
    let source = probe_home.path().join("progress-temp-probe.rs");
    let executable = probe_home.path().join("progress-temp-probe.exe");
    std::fs::write(&source, SOURCE).unwrap();
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let compiled = Command::new(rustc)
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "progress probe did not compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    let host_temp_root = probe_home.path().join("host-temp");
    let inherited_temp = probe_home.path().join("inherited-temp");
    std::fs::create_dir(&host_temp_root).unwrap();
    std::fs::create_dir(&inherited_temp).unwrap();
    let canonical_host_temp_root = canonical_display_path(&host_temp_root);
    let trace = probe_home.path().join("progress-temp.trace");
    let command = format!("\"{}\"", executable.display());
    let output = run_with_path_and_env(
        root,
        &[
            "--format",
            "json",
            "goal",
            "progress",
            id,
            "--package",
            "stage1",
            "-m",
            "managed process temp",
            "--command",
            &command,
        ],
        &[],
        None,
        &[
            (
                "RAYMAN_VALIDATION_TEMP_ROOT",
                host_temp_root.to_str().unwrap(),
            ),
            ("RAYMAN_PROGRESS_TEMP_TRACE", trace.to_str().unwrap()),
            ("TEMP", inherited_temp.to_str().unwrap()),
            ("TMP", inherited_temp.to_str().unwrap()),
            ("TMPDIR", inherited_temp.to_str().unwrap()),
        ],
    );
    assert_eq!(output.status, 0, "{}", output.stderr);

    let process_temp = PathBuf::from(std::fs::read_to_string(&trace).unwrap());
    assert!(
        process_temp.starts_with(canonical_host_temp_root.join("v")),
        "progress temp escaped configured host root: {}",
        process_temp.display()
    );
    assert_ne!(process_temp, inherited_temp);
    assert!(
        !process_temp.exists(),
        "progress validation process lease was not released: {}",
        process_temp.display()
    );
    assert!(!host_temp_root.join(".RaymanCodingSkill").exists());

    let persisted = std::fs::read_to_string(
        root.join(".RaymanCodingSkill/goals")
            .join(format!("{id}.json")),
    )
    .unwrap();
    assert!(!persisted.contains(process_temp.to_string_lossy().as_ref()));
    assert!(
        !persisted.contains(
            process_temp
                .parent()
                .and_then(Path::file_name)
                .unwrap()
                .to_string_lossy()
                .as_ref()
        )
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

#[cfg(windows)]
#[test]
fn pytest_validation_child_can_run_nested_goal_validate_from_its_managed_temp() {
    let pytest = NativePytestProbe::build();
    let nested = NestedValidationProbe::build();
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let id = start_pytest_validation_goal(root);
    write(root, "README.md", "pytest validation fixture changed\n");
    run_json(root, &["context", "refresh"]);
    let evidence_home = tempfile::tempdir().unwrap();
    let host_temp_root = evidence_home.path().join("pytest-host-temp");
    std::fs::create_dir(&host_temp_root).unwrap();
    let pytest_trace_path = evidence_home.path().join("pytest.trace");
    let nested_trace_path = evidence_home.path().join("nested.trace");
    let skill = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("SKILL.md")
        .canonicalize()
        .unwrap();

    let output = run_with_path_and_env(
        root,
        &[
            "--format",
            "json",
            "goal",
            "validate",
            &id,
            "--req",
            "req_1",
            "-m",
            "pytest child reentered goal validate",
            "--command",
            "pytest -q --",
            "--changed",
            "README.md",
        ],
        &[pytest.bin_dir.as_path()],
        pytest.pathext(),
        &[
            (
                "RAYMAN_VALIDATION_TEMP_ROOT",
                host_temp_root.to_str().unwrap(),
            ),
            (
                "RAYMAN_PYTEST_PROBE_LOG",
                pytest_trace_path.to_str().unwrap(),
            ),
            ("RAYMAN_PYTEST_PROBE_MODE", "success"),
            ("RAYMAN_NESTED_PROBE", nested.executable.to_str().unwrap()),
            ("RAYMAN_NESTED_RAYMAN", BIN),
            ("RAYMAN_NESTED_SKILL", skill.to_str().unwrap()),
            ("RAYMAN_NESTED_TRACE", nested_trace_path.to_str().unwrap()),
        ],
    );
    assert_eq!(
        output.status,
        0,
        "stdout={}\nstderr={}\nnested_trace={}",
        output.stdout,
        output.stderr,
        std::fs::read_to_string(&nested_trace_path).unwrap_or_default()
    );

    let pytest_records = pytest_trace(&pytest_trace_path);
    assert_eq!(
        pytest_records
            .iter()
            .map(|(phase, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["collect", "run"],
        "records={pytest_records:?}"
    );
    let pytest_run_root = pytest_records
        .iter()
        .find_map(|(phase, root)| (phase == "run").then_some(root))
        .unwrap();
    let records = nested_validation_trace(&nested_trace_path);
    assert_eq!(
        records
            .iter()
            .map(|(phase, _, _)| phase.as_str())
            .collect::<Vec<_>>(),
        ["outer", "inner"],
        "records={records:?}"
    );
    let (_, outer_temp, outer_nested) = &records[0];
    let (_, inner_temp, inner_nested) = &records[1];
    assert_eq!(outer_temp, &pytest_run_root.join("t"));
    assert_eq!(outer_nested, &pytest_run_root.join("n"));
    let inner_lease_root = inner_temp.parent().unwrap();
    assert!(
        inner_lease_root.starts_with(outer_nested.join("v")),
        "nested process lease escaped pytest lease: {}",
        inner_lease_root.display()
    );
    assert_eq!(inner_nested, &inner_lease_root.join("n"));
    for (_, lease_root) in &pytest_records {
        assert!(
            !lease_root.exists(),
            "pytest lease remained: {}",
            lease_root.display()
        );
    }
    for path in [outer_temp, outer_nested, inner_temp, inner_nested] {
        assert!(!path.exists(), "nested lease remained: {}", path.display());
    }
    assert_no_live_pytest_leases(&host_temp_root);
    assert!(!host_temp_root.join(".RaymanCodingSkill").exists());

    let persisted: Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.join(".RaymanCodingSkill/goals")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    visit_json_strings(&persisted, &mut |text| {
        assert!(!text.contains("RAYMAN_VALIDATION_TEMP_ROOT"), "{text}");
        for (_, temp, nested_root) in &records {
            assert!(!text.contains(temp.to_string_lossy().as_ref()), "{text}");
            assert!(
                !text.contains(nested_root.to_string_lossy().as_ref()),
                "{text}"
            );
            let lease_id = temp
                .parent()
                .and_then(Path::file_name)
                .unwrap()
                .to_string_lossy();
            assert!(!text.contains(lease_id.as_ref()), "{text}");
        }
    });
}

#[test]
fn goal_validate_isolates_every_pytest_process_without_receipt_leakage() {
    let probe = NativePytestProbe::build();
    let py_program = probe
        .bin_dir
        .join(format!("py{}", std::env::consts::EXE_SUFFIX))
        .to_string_lossy()
        .replace('\\', "/");
    let commands = [
        "pytest -q --".to_string(),
        "python -m pytest -q --".to_string(),
        format!("\"{py_program}\" -3.12 -m pytest -q --"),
    ];
    for (index, command) in commands.iter().enumerate() {
        let command = command.as_str();
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let id = start_pytest_validation_goal(root);
        let host_temp_root = probe._temp.path().join(format!("pytest-host-temp-{index}"));
        std::fs::create_dir(&host_temp_root).unwrap();
        #[cfg(windows)]
        let canonical_host_temp_root = canonical_display_path(&host_temp_root);
        let host_temp_text = host_temp_root.to_str().unwrap();
        let trace = probe._temp.path().join(format!("success-{index}.trace"));
        let trace_text = trace.to_str().unwrap();
        let output = run_with_path_and_env(
            root,
            &[
                "--format",
                "json",
                "goal",
                "validate",
                &id,
                "--req",
                "req_1",
                "-m",
                "managed pytest authority",
                "--command",
                command,
                "--workspace-snapshot",
                "--authority",
                "--repeat",
                "2",
            ],
            &[probe.bin_dir.as_path()],
            probe.pathext(),
            &[
                ("RAYMAN_PYTEST_PROBE_LOG", trace_text),
                ("RAYMAN_PYTEST_PROBE_MODE", "success"),
                (
                    "PYTEST_ADDOPTS",
                    "--basetemp inherited -o cache_dir=inherited",
                ),
                ("RAYMAN_VALIDATION_TEMP_ROOT", host_temp_text),
            ],
        );
        assert_eq!(
            output.status, 0,
            "command={command}\nstdout={}\nstderr={}",
            output.stdout, output.stderr
        );
        let returned: Value = serde_json::from_str(&output.stdout).unwrap();
        let goal: rayman::goal::Goal = serde_json::from_value(returned).unwrap();
        let validation = &goal.requirements[0].validations[0];
        let receipt = validation.receipt.as_ref().unwrap();
        let authority = &goal.authority_receipts[0];
        assert_eq!(validation.command, command);
        assert_eq!(authority.command, command);
        assert_eq!(authority.repeat, 2);
        assert_eq!(authority.runs.len(), 2);
        assert_eq!(
            receipt.invocation_sha256,
            rayman::goal::validation_invocation_sha256_scoped_mode(
                command,
                &validation.impact_scopes,
                validation.non_code,
                validation.workspace_snapshot,
            )
        );
        assert_eq!(
            authority.invocation_sha256,
            rayman::goal::authority_invocation_sha256_mode(
                command,
                "req_1",
                2,
                &authority.impact_scopes,
                authority.non_code,
                authority.workspace_snapshot,
            )
        );

        let records = pytest_trace(&trace);
        assert_eq!(
            records
                .iter()
                .map(|(phase, _)| phase.as_str())
                .collect::<Vec<_>>(),
            ["collect", "run", "run"],
            "command={command} records={records:?}"
        );
        let roots = records
            .iter()
            .map(|(_, lease_root)| lease_root.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            roots.len(),
            3,
            "every physical pytest process needs a new lease"
        );
        for lease_root in &roots {
            #[cfg(windows)]
            assert!(
                lease_root.starts_with(canonical_host_temp_root.join("p")),
                "Windows pytest lease escaped the configured host root: {}",
                lease_root.display()
            );
            assert!(
                !lease_root.exists(),
                "successful validation left {}",
                lease_root.display()
            );
        }
        assert_no_live_pytest_leases(if cfg!(windows) { &host_temp_root } else { root });
        assert!(
            !host_temp_root.join(".RaymanCodingSkill").exists(),
            "external pytest root became a false workspace marker"
        );

        let persisted_path = root
            .join(".RaymanCodingSkill/goals")
            .join(format!("{id}.json"));
        let persisted: Value =
            serde_json::from_str(&std::fs::read_to_string(persisted_path).unwrap()).unwrap();
        visit_json_strings(&persisted, &mut |text| {
            assert!(!text.contains("--basetemp"), "managed argv leaked: {text}");
            assert!(!text.contains("cache_dir="), "managed argv leaked: {text}");
            assert!(!text.contains("addopts="), "managed argv leaked: {text}");
            assert!(
                !text.contains("PYTHONPYCACHEPREFIX"),
                "managed environment leaked: {text}"
            );
            for lease_root in &roots {
                assert!(
                    !text.contains(lease_root.to_string_lossy().as_ref()),
                    "lease path leaked into goal JSON: {text}"
                );
                let lease_id = lease_root.file_name().unwrap().to_string_lossy();
                assert!(!text.contains(lease_id.as_ref()), "lease id leaked: {text}");
            }
        });
    }
}

#[test]
fn goal_validate_pytest_failures_cleanup_and_never_write_receipts() {
    let probe = NativePytestProbe::build();
    for (index, mode) in [
        "collect-fail",
        "collect-zero",
        "run-fail",
        "cleanup-fail",
        "run-cleanup-fail",
    ]
    .into_iter()
    .enumerate()
    {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let id = start_pytest_validation_goal(root);
        let host_temp_root = probe
            ._temp
            .path()
            .join(format!("pytest-failure-host-temp-{index}"));
        std::fs::create_dir(&host_temp_root).unwrap();
        let host_temp_text = host_temp_root.to_str().unwrap();
        let trace = probe._temp.path().join(format!("failure-{index}.trace"));
        let trace_text = trace.to_str().unwrap();
        let output = run_with_path_and_env(
            root,
            &[
                "goal",
                "validate",
                &id,
                "--req",
                "req_1",
                "-m",
                "pytest failure must not persist",
                "--command",
                "python -m pytest -q --",
                "--workspace-snapshot",
                "--authority",
                "--repeat",
                "2",
            ],
            &[probe.bin_dir.as_path()],
            probe.pathext(),
            &[
                ("RAYMAN_PYTEST_PROBE_LOG", trace_text),
                ("RAYMAN_PYTEST_PROBE_MODE", mode),
                ("RAYMAN_LANG", "zh-CN"),
                ("RAYMAN_VALIDATION_TEMP_ROOT", host_temp_text),
            ],
        );
        assert_eq!(
            output.status, 1,
            "mode={mode}\nstdout={}\nstderr={}",
            output.stdout, output.stderr
        );
        match mode {
            "collect-fail" => assert!(output.stderr.contains("exit=41"), "{}", output.stderr),
            "collect-zero" => assert!(
                output.stderr.contains("没有收集任何测试"),
                "{}",
                output.stderr
            ),
            "run-fail" => assert!(output.stderr.contains("exit=37"), "{}", output.stderr),
            "cleanup-fail" => assert!(
                output.stderr.contains("lease") && output.stderr.contains("释放"),
                "{}",
                output.stderr
            ),
            "run-cleanup-fail" => assert!(
                output.stderr.contains("exit=37") && output.stderr.contains("lease 释放失败"),
                "{}",
                output.stderr
            ),
            _ => unreachable!(),
        }

        let shown = run_json(root, &["goal", "show", &id]);
        assert_eq!(shown["requirements"][0]["status"], "open");
        assert!(
            shown["requirements"][0]["validations"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(shown["authority_receipts"].as_array().unwrap().is_empty());

        let records = pytest_trace(&trace);
        let cleanup_was_corrupted = matches!(mode, "cleanup-fail" | "run-cleanup-fail");
        for (record_index, (_, lease_root)) in records.iter().enumerate() {
            let final_record = record_index + 1 == records.len();
            if cleanup_was_corrupted && final_record {
                assert!(lease_root.exists(), "corrupt manifest must fail closed");
            } else {
                assert!(
                    !lease_root.exists(),
                    "mode={mode} left {}",
                    lease_root.display()
                );
            }
        }
        if !cleanup_was_corrupted {
            assert_no_live_pytest_leases(if cfg!(windows) { &host_temp_root } else { root });
        }
    }

    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    let id = start_pytest_validation_goal(root);
    let empty_path = tempfile::tempdir().unwrap();
    let spawn_host_temp_root = probe._temp.path().join("pytest-spawn-host-temp");
    std::fs::create_dir(&spawn_host_temp_root).unwrap();
    let spawn_host_temp_text = spawn_host_temp_root.to_str().unwrap();
    let trace = probe._temp.path().join("spawn-failure.trace");
    let trace_text = trace.to_str().unwrap();
    let output = run_with_exact_path_and_env(
        root,
        &[
            "goal",
            "validate",
            &id,
            "--req",
            "req_1",
            "-m",
            "spawn failure must release",
            "--command",
            "pytest -q --",
            "--workspace-snapshot",
            "--authority",
            "--repeat",
            "2",
        ],
        empty_path.path(),
        probe.pathext(),
        &[
            ("RAYMAN_PYTEST_PROBE_LOG", trace_text),
            ("RAYMAN_VALIDATION_TEMP_ROOT", spawn_host_temp_text),
        ],
    );
    assert_eq!(output.status, 1, "{}", output.stderr);
    assert!(output.stderr.contains("pytest"), "{}", output.stderr);
    assert!(pytest_trace(&trace).is_empty());
    assert_no_live_pytest_leases(if cfg!(windows) {
        &spawn_host_temp_root
    } else {
        root
    });
    let shown = run_json(root, &["goal", "show", &id]);
    assert!(
        shown["requirements"][0]["validations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(shown["authority_receipts"].as_array().unwrap().is_empty());
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
fn legacy_success_current_can_be_archived_through_cli() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);

    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "legacy current success",
            "--must",
            "preserve historical result",
        ],
    );
    let id = goal["id"].as_str().unwrap();
    validate_goal(root, id, "req_1", "legacy result validated", &[]);
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let goal_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&goal_path).unwrap()).unwrap();
    persisted["created_at"] = Value::String("2026-08-05T10:00:00Z".into());
    persisted["baseline"]["recorded_at"] = Value::String("2026-08-05T10:05:00Z".into());
    persisted
        .as_object_mut()
        .unwrap()
        .remove("plan_publication_policy");
    let legacy: rayman::goal::Goal = serde_json::from_value(persisted.clone()).unwrap();
    persisted["requirements"][0]["validations"][0]["receipt"]["contract_sha256"] =
        Value::String(rayman::goal::validation_contract_sha256(&legacy, "req_1").unwrap());
    std::fs::write(&goal_path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();

    let archived = run_json(
        root,
        &[
            "goal",
            "archive",
            id,
            "--reason",
            "retire pre-publication-policy success",
        ],
    );
    assert_eq!(archived["lifecycle"], "archived");
    assert_eq!(archived["status"], "success");
    assert_eq!(
        archived["lifecycle_proof"]["receipt_policy"],
        "receipt_integrity_v3"
    );
    assert_eq!(
        archived["lifecycle_proof"]["workspace_identity"],
        archived["requirements"][0]["validations"][0]["receipt"]["workspace_identity"]
    );
    assert_eq!(run(root, &["check", "--profile", "standard"]).status, 0);
}

#[test]
fn legacy_success_archive_english_multi_gap_is_fully_localized() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 41 }\n");
    run_json(root, &["context", "refresh"]);

    let goal = run_json(
        root,
        &[
            "goal",
            "start",
            "legacy English failure",
            "--must",
            "preserve historical result",
        ],
    );
    let id = goal["id"].as_str().unwrap();
    rayman::goal::GoalStore::new(root)
        .record_plan(
            id,
            rayman::goal::PlanReceiptSubmission {
                changed_paths: vec!["src/lib.rs".into()],
                review_priority: "high".into(),
                impacted_paths: vec!["src/lib.rs".into()],
                recommended_checks: Vec::new(),
            },
        )
        .unwrap();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    validate_goal(
        root,
        id,
        "req_1",
        "legacy source validated",
        &["src/lib.rs"],
    );
    run_json(
        root,
        &[
            "goal",
            "review",
            id,
            "--reviewer",
            "integration-review",
            "--message",
            "reviewed final source",
        ],
    );
    assert_eq!(run(root, &["goal", "close", id]).status, 0);

    let goal_path = root
        .join(".RaymanCodingSkill/goals")
        .join(format!("{id}.json"));
    let mut legacy: rayman::goal::Goal =
        serde_json::from_slice(&std::fs::read(&goal_path).unwrap()).unwrap();
    legacy.created_at = "2026-08-05T10:00:00Z".into();
    legacy.baseline.as_mut().unwrap().recorded_at = "2026-08-05T10:05:00Z".into();
    legacy.plan_publication_policy = None;
    legacy.review_receipts.clear();
    let plan = &mut legacy.plan_receipts[0];
    plan.recorded_at = "2026-08-05T10:10:00Z".into();
    plan.publication = None;
    plan.plan_sha256 = rayman::goal::plan_receipt_sha256(plan);
    let contract_sha256 = rayman::goal::validation_contract_sha256(&legacy, "req_1").unwrap();
    legacy.requirements[0].validations[0]
        .receipt
        .as_mut()
        .unwrap()
        .contract_sha256 = contract_sha256;
    std::fs::write(&goal_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
    write(root, "outside.txt", "unplanned post-validation change\n");
    let before = std::fs::read(&goal_path).unwrap();

    let failed = run(
        root,
        &[
            "--language",
            "en",
            "goal",
            "archive",
            id,
            "--reason",
            "reject multi-gap legacy history",
        ],
    );
    assert_eq!(failed.status, 1, "stderr={}", failed.stderr);
    let rendered = format!("{}\n{}", failed.stdout, failed.stderr);
    assert!(rendered.contains("actual changes exceed"), "{rendered}");
    assert!(rendered.contains("high-priority plan"), "{rendered}");
    assert!(
        !rendered.chars().any(|character| matches!(
            character as u32,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff
        )),
        "{rendered}"
    );
    assert_eq!(std::fs::read(&goal_path).unwrap(), before);
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
    let unknown_policy = run(
        root,
        &[
            "goal",
            "archive",
            old_id,
            "--reason",
            "invalid migration",
            "--migrate-receipt-policy",
            "unknown",
        ],
    );
    assert_eq!(unknown_policy.status, 1);
    assert!(
        unknown_policy.stderr.contains("未知历史 receipt policy"),
        "stderr={}",
        unknown_policy.stderr
    );

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
    assert_eq!(
        run(root, &["check", "--profile", "standard", "--goal", old_id]).status,
        1
    );
}

#[test]
fn lifecycle_only_replacement_cli_uses_exact_archived_authority() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"lifecycle-cli\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 41 }\n#[test]\nfn smoke() { assert_eq!(answer(), 41); }\n",
    );
    let warmup = Command::new("cargo")
        .args(["test", "--workspace", "--all-targets"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(warmup.status.success());
    run_json(root, &["context", "refresh"]);

    let authority = run_json(
        root,
        &[
            "goal",
            "start",
            "direct authority",
            "--must",
            "prove repository",
        ],
    );
    let authority_id = authority["id"].as_str().unwrap();
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn smoke() { assert_eq!(answer(), 42); }\n",
    );
    run_json(root, &["context", "refresh"]);
    validate_goal_authority(
        root,
        authority_id,
        "req_1",
        "stable direct authority",
        &["src/lib.rs"],
    );
    assert_eq!(run(root, &["goal", "close", authority_id]).status, 0);
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "archive",
                authority_id,
                "--reason",
                "direct authority complete",
            ],
        )
        .status,
        0
    );

    let old = run_json(
        root,
        &[
            "goal",
            "start",
            "unfinished",
            "--must",
            "preserve exact contract",
        ],
    );
    let old_id = old["id"].as_str().unwrap();
    let replacement = run_json(
        root,
        &[
            "goal",
            "start",
            "replacement",
            "--must",
            "preserve exact contract",
        ],
    );
    let replacement_id = replacement["id"].as_str().unwrap();
    let authorized = run_json(
        root,
        &[
            "goal",
            "authorize-replacement",
            replacement_id,
            "--supersedes",
            old_id,
            "--authority-from",
            authority_id,
            "--command",
            "cargo test --workspace --all-targets",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(authorized["status"], "success");
    assert_eq!(
        authorized["replacement_authority"]["authority_goal_id"],
        authority_id
    );
    let superseded = run_json(root, &["goal", "supersede", old_id, "--by", replacement_id]);
    assert_eq!(superseded["lifecycle"], "superseded");
    let checked = run(root, &["check", "--goal", replacement_id]);
    assert_eq!(checked.status, 0, "{}\n{}", checked.stdout, checked.stderr);
    let finished = run(root, &["finish", "--goal", replacement_id]);
    assert_eq!(
        finished.status, 0,
        "{}\n{}",
        finished.stdout, finished.stderr
    );
}

#[cfg(windows)]
#[test]
fn lifecycle_only_replacement_cli_rebinds_only_the_maintenance_cycle_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Keep this fixture authoritative even when the process TEMP happens to
    // sit below a parent Git worktree. Otherwise workspace discovery can walk
    // into the production repository before the helper activates the fixture.
    std::fs::create_dir_all(root.join(".RaymanCodingSkill")).unwrap();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"lifecycle-cycle-rebind\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 41 }\n");
    write(
        root,
        "scripts/check-repo.ps1",
        "param([string]$MaintenanceOrchestrationCycle)\nif (-not (Test-Path -LiteralPath $MaintenanceOrchestrationCycle -PathType Leaf)) { exit 11 }\nif ((Get-Content -Raw -LiteralPath $MaintenanceOrchestrationCycle) -notmatch '\"status\"\\s*:\\s*\"pass\"') { exit 12 }\n",
    );
    write(
        root,
        "target/archived-maintenance-review-cycle.json",
        "{\"status\":\"pass\",\"snapshot\":\"archived\"}\n",
    );
    write(
        root,
        "target/current-maintenance-review-cycle.json",
        "{\"status\":\"pass\",\"snapshot\":\"current\"}\n",
    );
    run_json(root, &["context", "refresh"]);

    let archived_command = "pwsh -NoProfile -File scripts/check-repo.ps1 -MaintenanceOrchestrationCycle target/archived-maintenance-review-cycle.json";
    let authority = run_json(
        root,
        &[
            "goal",
            "start",
            "cycle authority",
            "--must",
            "prove repository",
        ],
    );
    let authority_id = authority["id"].as_str().unwrap();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);
    run_json(
        root,
        &[
            "goal",
            "validate",
            authority_id,
            "--req",
            "req_1",
            "-m",
            "stable cycle authority",
            "--command",
            archived_command,
            "--changed",
            "src/lib.rs",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(run(root, &["goal", "close", authority_id]).status, 0);
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "archive",
                authority_id,
                "--reason",
                "cycle authority complete",
            ],
        )
        .status,
        0
    );

    let old = run_json(
        root,
        &[
            "goal",
            "start",
            "unfinished cycle consumer",
            "--must",
            "preserve cycle contract",
        ],
    );
    let replacement = run_json(
        root,
        &[
            "goal",
            "start",
            "replacement cycle consumer",
            "--must",
            "preserve cycle contract",
        ],
    );
    std::fs::remove_file(root.join("target/archived-maintenance-review-cycle.json")).unwrap();
    let authorized = run_json(
        root,
        &[
            "goal",
            "authorize-replacement",
            replacement["id"].as_str().unwrap(),
            "--supersedes",
            old["id"].as_str().unwrap(),
            "--authority-from",
            authority_id,
            "--command",
            archived_command,
            "--maintenance-cycle-rebind",
            "target/current-maintenance-review-cycle.json",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(authorized["status"], "success");
    assert_eq!(
        authorized["replacement_authority"]["live_authority"]["command"],
        archived_command
    );
    assert_eq!(
        authorized["replacement_authority"]["live_authority"]["command_rebind"]["current_value"],
        "target/current-maintenance-review-cycle.json"
    );
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

    let temp_before = run_json(root, &["temp", "status"]);
    assert_eq!(temp_before["traversal_error_count"], 0);
    let run_root = root.join(".RaymanCodingSkill/tmp/run");
    assert!(!run_root.exists());

    write(root, ".RaymanCodingSkill/tmp/run/nested/a.bin", "abc");
    write(root, ".RaymanCodingSkill/tmp/run/b.bin", "d");
    let temp_status = run_json(root, &["temp", "status"]);
    assert!(run_root.join("nested/a.bin").is_file());
    assert!(run_root.join("b.bin").is_file());
    assert_eq!(
        temp_status["entry_count"].as_u64().unwrap(),
        temp_before["entry_count"].as_u64().unwrap() + 1
    );
    assert_eq!(
        temp_status["file_count"].as_u64().unwrap(),
        temp_before["file_count"].as_u64().unwrap() + 2
    );
    assert_eq!(
        temp_status["directory_count"].as_u64().unwrap(),
        temp_before["directory_count"].as_u64().unwrap() + 2
    );
    assert_eq!(
        temp_status["total_bytes"].as_u64().unwrap(),
        temp_before["total_bytes"].as_u64().unwrap() + 4
    );
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
fn checkpoint_save_is_lossless_by_default_and_prune_requires_yes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let checkpoint_dir = tempfile::tempdir().unwrap();
    let checkpoint_dir = checkpoint_dir.path().to_str().unwrap();
    write(root, "src/lib.rs", "pub fn value() -> i32 { 1 }\n");

    let first = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "save"]);
    assert_eq!(first["retention_applied"], false);
    assert_eq!(first["pruned"], 0);
    write(root, "src/lib.rs", "pub fn value() -> i32 { 2 }\n");
    let second = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "save"]);
    assert_eq!(second["retention_applied"], false);
    assert_eq!(second["pruned"], 0);

    let before = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "list"]);
    assert_eq!(before.as_array().unwrap().len(), 2);
    let refused = run(
        root,
        &[
            "checkpoint",
            "--dir",
            checkpoint_dir,
            "prune",
            "--keep",
            "1",
        ],
    );
    assert_eq!(refused.status, 1);
    assert!(refused.stderr.contains("--yes"));
    let after_refusal = run_json(root, &["checkpoint", "--dir", checkpoint_dir, "list"]);
    assert_eq!(after_refusal.as_array().unwrap().len(), 2);

    let pruned = run_json(
        root,
        &[
            "checkpoint",
            "--dir",
            checkpoint_dir,
            "prune",
            "--keep",
            "1",
            "--yes",
        ],
    );
    assert_eq!(pruned["pruned"], 1);
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
fn autosave_is_reachable_as_a_top_level_command() {
    // 发布校验器不再从 --help 文本里断言命令面，所以 autosave 的 CLI 可达性
    // 需要在这里覆盖：此前它只有 in-crate 单元测试，没走过 CLI dispatch。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let status = run_json(root, &["autosave", "status"]);
    assert!(status["message"].is_string(), "status={status}");
    // 未注册的工作区没有持久化状态。
    assert!(status["state"].is_null(), "status={status}");
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

/// `goal_planning_gaps` 的分支在 check 侧此前只有 1 个有端到端证明，其余只被
/// helper 级单元测试覆盖——正是今天那三条 high 的形态（helper 全绿、调用方没接线）。
/// 这三个用例证明这些分支会真的让 `check` 拦下来。
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
fn frontier_requires_a_complete_solution_package_before_asking_user() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "owner", "--must", "finish"]);
    let id = started["id"].as_str().unwrap();
    let agent = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "local repair",
            "-m",
            "agent can still fix it",
            "--goal",
            id,
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "continue");
    assert_eq!(frontier["ask_user_allowed"], false);
    assert_eq!(frontier["execution"], "continue_foreground");
    assert_eq!(frontier["consultation"], "none");
    assert_eq!(
        run(root, &["goal", "close", id, "--status", "blocked"]).status,
        1
    );
    run(
        root,
        &["goal", "pending", "resolve", agent["id"].as_str().unwrap()],
    );

    let incomplete = run(
        root,
        &[
            "goal",
            "pending",
            "add",
            "choice",
            "-m",
            "business choice",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--capability-key",
            "owner/choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    assert_eq!(incomplete.status, 1);
    let choice = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "choice",
            "-m",
            "business choice",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both variants",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "A is safer; B is faster",
            "--resume-command",
            "rayman prepare --goal owner",
            "--auto-resume-condition",
            "choice recorded",
            "--capability-key",
            "owner/choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["ask_user_allowed"], true);
    assert_eq!(frontier["execution"], "paused_for_user");
    assert_eq!(frontier["consultation"], "ready");
    let rendered = run_json(root, &["goal", "pending", "render", "--goal", id]);
    assert!(
        rendered["text"]
            .as_str()
            .is_some_and(|text| text.contains(choice["id"].as_str().unwrap())),
        "{rendered}"
    );
    let retired = run(
        root,
        &[
            "goal",
            "pending",
            "present",
            choice["id"].as_str().unwrap(),
            "--goal",
            id,
            "--package-sha256",
            choice["package_sha256"].as_str().unwrap(),
            "--channel",
            "codex",
        ],
    );
    assert_eq!(retired.status, 1);
    assert!(retired.stderr.contains("已退役"), "{}", retired.stderr);
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["ask_user_allowed"], true);
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(
        run(root, &["goal", "close", id, "--status", "blocked"]).status,
        0
    );
}

#[test]
fn frontier_requires_complete_background_authority_before_rendered_parallel_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let started = run_json(root, &["goal", "start", "mixed", "--must", "finish"]);
    let id = started["id"].as_str().unwrap();
    run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "local repair",
            "-m",
            "safe work",
            "--goal",
            id,
        ],
    );

    let partial = run(
        root,
        &[
            "goal",
            "pending",
            "add",
            "urgent choice",
            "-m",
            "owner input",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "tradeoff",
            "--resume-command",
            "rayman prepare --goal mixed",
            "--auto-resume-condition",
            "choice recorded",
            "--consultation-timing",
            "immediate",
            "--background-mechanism",
            "worktree task",
            "--background-authority-evidence",
            "user instruction codex://threads/test",
            "--capability-key",
            "mixed/urgent-choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    assert_eq!(partial.status, 1);

    let immediate = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "urgent choice",
            "-m",
            "owner input",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "tradeoff",
            "--resume-command",
            "rayman prepare --goal mixed",
            "--auto-resume-condition",
            "choice recorded",
            "--consultation-timing",
            "immediate",
            "--capability-key",
            "mixed/urgent-choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["execution"], "paused_for_user");
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(frontier["background_execution_allowed"], false);
    let rendered = run_json(root, &["goal", "pending", "render", "--current"]);
    assert!(
        rendered["text"]
            .as_str()
            .is_some_and(|text| text.contains(immediate["id"].as_str().unwrap())),
        "{rendered}"
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["consultation"], "ready");
    run(
        root,
        &[
            "goal",
            "pending",
            "resolve",
            immediate["id"].as_str().unwrap(),
        ],
    );

    let background = run_json(
        root,
        &[
            "goal",
            "pending",
            "add",
            "urgent choice",
            "-m",
            "owner input",
            "--goal",
            id,
            "--owner",
            "human",
            "--kind",
            "human_input",
            "--attempt",
            "tested both",
            "--evidence-path",
            "reports/options.md",
            "--minimum-input",
            "choose A or B",
            "--recommended",
            "choose A",
            "--alternative",
            "choose B",
            "--risk",
            "tradeoff",
            "--resume-command",
            "rayman prepare --goal mixed",
            "--auto-resume-condition",
            "choice recorded",
            "--consultation-timing",
            "immediate",
            "--background-mechanism",
            "isolated worktree task task_123",
            "--background-authority-evidence",
            "user instruction codex://threads/test",
            "--background-isolation-evidence",
            "isolated worktree task task_123",
            "--capability-key",
            "mixed/urgent-choice",
            "--boundary-class",
            "owner_decision",
        ],
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["execution"], "continue_background");
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(frontier["background_execution_allowed"], true);
    let rendered = run_json(root, &["goal", "pending", "render", "--current"]);
    assert!(
        rendered["text"]
            .as_str()
            .is_some_and(|text| text.contains(background["id"].as_str().unwrap())),
        "{rendered}"
    );
    let frontier = run_json(root, &["goal", "frontier", id]);
    assert_eq!(frontier["decision"], "ask_user");
    assert_eq!(frontier["execution"], "continue_background");
    assert_eq!(frontier["consultation"], "ready");
    assert_eq!(frontier["ask_user_allowed"], true);
}

#[test]
fn pending_render_current_matches_the_workspace_aggregate() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let goal_a = run_json(root, &["goal", "start", "aggregate A", "--must", "choose"]);
    let goal_b = run_json(root, &["goal", "start", "aggregate B", "--must", "choose"]);
    let id_a = goal_a["id"].as_str().unwrap();
    let id_b = goal_b["id"].as_str().unwrap();
    let item_a = add_complete_human_pending(root, id_a, "owner/shared", "choice A");
    let item_b = add_complete_human_pending(root, id_b, "owner/shared", "choice B");

    let aggregate = run_json(root, &["goal", "pending", "render", "--current"]);
    let partial = run_json(root, &["goal", "pending", "render", "--goal", id_a]);
    let aggregate_text = aggregate["text"].as_str().unwrap();
    assert!(aggregate_text.contains("rayman.human-boundary-aggregate.v1"));
    assert!(aggregate_text.contains("\"scope\": \"current_response_only\""));
    assert!(!aggregate_text.contains("rayman.codex-stop-candidate"));
    assert!(aggregate_text.contains(item_a["id"].as_str().unwrap()));
    assert!(aggregate_text.contains(item_b["id"].as_str().unwrap()));
    assert_eq!(aggregate["goal_ids"].as_array().unwrap().len(), 2);
    assert_ne!(aggregate["render_sha256"], partial["render_sha256"]);
    assert_eq!(run(root, &["goal", "pending", "render"]).status, 1);
    assert_ne!(
        run(
            root,
            &["goal", "pending", "render", "--goal", id_a, "--current"],
        )
        .status,
        0
    );
}

#[test]
fn pending_render_text_is_protocol_exact_under_the_english_locale() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "README.md", "workspace");
    run_json(root, &["context", "refresh"]);
    let goal = run_json(
        root,
        &["goal", "start", "exact aggregate", "--must", "choose"],
    );
    let id = goal["id"].as_str().unwrap();
    add_complete_human_pending_with_title(root, id, "owner/exact", "秒", "choose safely");

    let expected = run_json(root, &["goal", "pending", "render", "--current"]);
    let expected = expected["text"].as_str().unwrap();
    assert!(expected.contains("秒"), "{expected}");

    let rendered = run(
        root,
        &["--language", "en", "goal", "pending", "render", "--current"],
    );
    assert_eq!(rendered.status, 0, "{}", rendered.stderr);
    assert_eq!(
        rendered.stdout.trim_end_matches(&['\r', '\n'][..]),
        expected,
        "text-mode output must remain byte-for-byte compatible with the client-neutral aggregate"
    );
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
fn map_impact_rejects_directory_inputs_instead_of_returning_empty_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(root, "src/lib.rs", "pub fn answer() -> i32 { 42 }\n");
    run_json(root, &["context", "refresh"]);

    let impact = run(root, &["map", "impact", "src"]);
    assert_eq!(impact.status, 1);
    assert!(
        impact.stderr.contains("indexed directory"),
        "{}",
        impact.stderr
    );
    assert!(impact.stderr.contains("map plan"), "{}", impact.stderr);
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

#[test]
fn pytest_lease_cli_is_manifest_owned_and_releasable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let lease = run_json(root, &["temp", "pytest-lease", "focused tests"]);
    let id = lease["id"].as_str().unwrap();
    assert_eq!(lease["schema"], "rayman.pytest-lease.v1");
    let lease_root = Path::new(lease["root"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    let expected_root = root
        .join(".RaymanCodingSkill/tmp/leases")
        .canonicalize()
        .unwrap();
    assert!(lease_root.starts_with(expected_root));
    assert_eq!(lease["pytest_args"].as_array().unwrap().len(), 4);
    let probed = run_json(root, &["temp", "pytest-probe", id]);
    assert_eq!(probed["id"], id);
    let released = run_json(root, &["temp", "pytest-release", id]);
    assert_eq!(released["removed"], true);
    let traversal = run(root, &["temp", "pytest-probe", "../outside"]);
    assert_eq!(traversal.status, 1);
}

#[test]
fn salvage_save_cli_works_without_activation_but_never_becomes_latest() {
    let workspace = tempfile::tempdir().unwrap();
    let checkpoint_store = tempfile::tempdir().unwrap();
    let root = workspace.path();
    write(root, "payload.txt", "emergency\n");
    let store = checkpoint_store.path().to_str().unwrap();
    let saved = run_raw(
        root,
        &[
            "--format",
            "json",
            "checkpoint",
            "salvage-save",
            "--dir",
            store,
        ],
    );
    assert_eq!(saved.status, 0, "{}", saved.stderr);
    let saved: Value = serde_json::from_str(&saved.stdout).unwrap();
    assert_eq!(saved["purpose"], "recovery_only");
    assert_eq!(saved["authoritative"], false);
    let status = run_raw(
        root,
        &["--format", "json", "checkpoint", "status", "--dir", store],
    );
    assert_eq!(status.status, 0, "{}", status.stderr);
    let status: Value = serde_json::from_str(&status.stdout).unwrap();
    assert_eq!(status["has_checkpoint"], false);
    let listed = run_raw(
        root,
        &["--format", "json", "checkpoint", "list", "--dir", store],
    );
    let listed: Value = serde_json::from_str(&listed.stdout).unwrap();
    assert_eq!(listed[0]["purpose"], "recovery_only");
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
fn ordinary_changed_validation_keeps_the_legacy_scope_shape() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"changed-scope-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root, "src/lib.rs", "pub fn value() -> u8 { 1 }\n");
    generate_lockfile(root);
    run_json(root, &["context", "refresh"]);
    let started = run_json(
        root,
        &[
            "goal",
            "start",
            "ordinary change",
            "--must",
            "validate source",
        ],
    );
    let id = started["id"].as_str().unwrap();
    write(
        root,
        "src/lib.rs",
        "pub fn value() -> u8 { 2 }\n#[test]\nfn value_is_two() { assert_eq!(value(), 2); }\n",
    );
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
            "ordinary changed validation",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --quiet",
        ],
    );
    let validation = &validated["requirements"][0]["validations"][0];
    assert!(validation.get("workspace_snapshot").is_none());
    assert_eq!(validation["non_code"], false);
    assert_eq!(validation["impact_paths"][0], "src/lib.rs");
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
fn handoff_start_binds_source_goal_authority_clean_head_and_structured_stages() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"handoff-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 42); }\n",
    );
    write(root, ".gitignore", ".RaymanCodingSkill/\ntarget/\n");
    generate_lockfile(root);
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
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
    let commit = git(&["rev-parse", "HEAD"]);

    run_json(root, &["context", "refresh"]);
    let source = run_json(
        root,
        &[
            "goal",
            "start",
            "implementation",
            "--must",
            "prove implementation",
        ],
    );
    let source_id = source["id"].as_str().unwrap();
    let authority = run(
        root,
        &[
            "goal",
            "validate",
            source_id,
            "--req",
            "req_1",
            "-m",
            "stable implementation authority",
            "--changed",
            "src/lib.rs",
            "--command",
            "cargo test --all",
            "--authority",
            "--repeat",
            "2",
        ],
    );
    assert_eq!(authority.status, 0, "stderr={}", authority.stderr);
    assert_eq!(run(root, &["goal", "close", source_id]).status, 0);

    let handoff = run_json(
        root,
        &[
            "goal",
            "handoff",
            "start",
            "--from-goal",
            source_id,
            "--commit",
            &commit,
        ],
    );
    assert_eq!(handoff["handoff"]["source_goal_id"], source_id);
    assert_eq!(handoff["handoff"]["git_commit"], commit);
    assert_eq!(handoff["handoff"]["stages"].as_array().unwrap().len(), 3);
    assert_eq!(handoff["requirements"][0]["proof_kind"], "installation");
    assert_eq!(handoff["requirements"][1]["proof_kind"], "repository_gate");
    assert_eq!(handoff["requirements"][2]["proof_kind"], "source_fresh");

    write(root, "src/lib.rs", "pub fn answer() -> i32 { 43 }\n");
    let dirty = run(
        root,
        &[
            "goal",
            "handoff",
            "start",
            "--from-goal",
            source_id,
            "--commit",
            &commit,
        ],
    );
    assert_eq!(dirty.status, 1);
    assert!(
        dirty.stderr.contains("clean Git worktree"),
        "{}",
        dirty.stderr
    );
}

#[test]
fn handoff_start_rejects_a_retired_source_goal() {
    // 回归：goal_gate_verdict 对非 current lifecycle 只出 warning、无 blocker，故 handoff 曾能
    // 从一个已 archive 的退休实现切 release。现在 start_handoff 要求源目标 lifecycle=current。
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"handoff-retired-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root,
        "src/lib.rs",
        "pub fn answer() -> i32 { 42 }\n#[test]\nfn answer_is_valid() { assert_eq!(answer(), 42); }\n",
    );
    write(root, ".gitignore", ".RaymanCodingSkill/\ntarget/\n");
    generate_lockfile(root);
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
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
    let commit = git(&["rev-parse", "HEAD"]);

    run_json(root, &["context", "refresh"]);
    let source = run_json(
        root,
        &["goal", "start", "implementation", "--must", "prove it"],
    );
    let source_id = source["id"].as_str().unwrap();
    assert_eq!(
        run(
            root,
            &[
                "goal",
                "validate",
                source_id,
                "--req",
                "req_1",
                "-m",
                "stable implementation authority",
                "--changed",
                "src/lib.rs",
                "--command",
                "cargo test --all",
                "--authority",
                "--repeat",
                "2",
            ],
        )
        .status,
        0
    );
    assert_eq!(run(root, &["goal", "close", source_id]).status, 0);
    // Archive the proven success into its normal terminal (retired) state.
    assert_eq!(
        run(root, &["goal", "archive", source_id, "--reason", "retired"],).status,
        0
    );

    let retired = run(
        root,
        &[
            "goal",
            "handoff",
            "start",
            "--from-goal",
            source_id,
            "--commit",
            &commit,
        ],
    );
    assert_eq!(retired.status, 1, "stdout={}", retired.stdout);
    assert!(
        retired.stderr.contains("lifecycle") && retired.stderr.contains("current"),
        "{}",
        retired.stderr
    );
}

/// `temp scratch` prints a path meant to be pasted into another command — the
/// workflow contract points patch files at it. A Windows `\?\` verbatim
/// prefix makes many tools refuse it; pytest lease paths were normalized for
/// that reason and scratch was the remaining leak.
#[test]
fn temp_scratch_paths_do_not_leak_the_windows_verbatim_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/lib.rs",
        "pub fn a() {}
",
    );
    run_json(root, &["context", "refresh"]);

    let text = run(root, &["temp", "scratch", "patchwork"]);
    assert_eq!(text.status, 0, "stderr={}", text.stderr);
    assert!(
        !text.stdout.contains(r"\?\"),
        "text output leaked the verbatim prefix: {}",
        text.stdout
    );

    let json = run_json(root, &["temp", "scratch", "patchwork"]);
    let path = json["path"].as_str().unwrap();
    assert!(
        !path.contains(r"\?\"),
        "json output leaked the verbatim prefix: {path}"
    );
    assert!(path.contains("patchwork"), "{path}");
}

/// The state-audit allowlist is hand-maintained and has now drifted from what
/// the CLI itself writes three times: the pending lock, the autosave lock, and
/// `checkpoints/` from the `--dir` remedy the workflow reference prescribes for
/// a workspace-only sandbox. Drive the real commands instead of restating the
/// list, so the next writer that lands in `.RaymanCodingSkill/` fails here.
#[test]
fn state_audit_stays_clean_after_the_commands_that_write_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(
        root,
        "src/lib.rs",
        "pub fn a() {}
",
    );
    write(
        root,
        "Cargo.toml",
        "[package]
name = \"sa\"
version = \"0.1.0\"
edition = \"2021\"
",
    );
    run_json(root, &["context", "refresh"]);
    assert_eq!(run(root, &["state", "audit", "--check"]).status, 0);

    let started = run_json(root, &["goal", "start", "state writers", "--must", "work"]);
    let id = started["id"].as_str().unwrap().to_string();
    run_json(
        root,
        &["goal", "pending", "add", "leftover", "-m", "detail"],
    );
    let checkpoints = root.join(".RaymanCodingSkill/checkpoints");
    let saved = run(
        root,
        &["checkpoint", "save", "--dir", checkpoints.to_str().unwrap()],
    );
    assert_eq!(saved.status, 0, "stderr={}", saved.stderr);

    let audit = run(root, &["state", "audit", "--check"]);
    assert_eq!(
        audit.status, 0,
        "state written by ordinary commands must not read as retired: {}{}",
        audit.stdout, audit.stderr
    );
    assert!(
        !audit.stdout.contains("retired entries"),
        "{}",
        audit.stdout
    );
    assert!(!id.is_empty());
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

#[test]
fn update_status_is_activation_exempt_and_read_only_outside_a_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let output = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "status"],
    );
    assert_eq!(output.status, 0, "stderr={}", output.stderr);
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(report["status"], "status");
    assert_eq!(report["state"]["auto_check"], true);
    assert_eq!(report["state"]["auto_install"], false);
    assert_eq!(report["state_written"], false);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
    assert!(!user.path().join("Rayman/update").exists());
}

#[cfg(not(windows))]
#[test]
fn non_windows_update_check_reports_unsupported_without_cache_or_workspace_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let output = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "check"],
    );
    assert_eq!(output.status, 0, "stderr={}", output.stderr);
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        report["observation"]["status"]["status"],
        "unsupported_platform"
    );
    assert_eq!(report["state_written"], false);
    assert_eq!(report["install_ready"], false);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
    assert!(!user.path().join("Rayman/update").exists());
}

#[cfg(not(windows))]
#[test]
fn non_windows_due_poll_reports_unsupported_without_notification_or_worker() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let output = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "poll"],
    );
    assert_eq!(output.status, 0, "stderr={}", output.stderr);
    let report: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(report["status"], "polled");
    assert_eq!(report["checked"], true);
    assert_eq!(report["state_written"], true);
    assert_eq!(
        report["observation"]["status"]["status"],
        "unsupported_platform"
    );
    assert_eq!(report["install_authorized"], false);
    assert_eq!(report["install_ready"], false);
    assert!(report.get("worker_launch").is_none());
    assert!(report.get("install_error").is_none());
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());

    let state_path = user.path().join("Rayman/update/update.json");
    let persisted: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(persisted["auto_check"], true);
    assert_eq!(persisted["auto_install"], false);
    assert!(persisted["last_attempted_at"].as_str().is_some());
    assert_eq!(persisted["last_successful_observation"], Value::Null);
}

#[test]
fn update_configure_requires_an_exact_selector_and_yes_without_workspace_writes() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let no_selector = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--yes"],
    );
    assert_ne!(no_selector.status, 0);
    assert!(!user.path().join("Rayman/update/update.json").exists());

    let no_confirmation = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--no-auto-check"],
    );
    assert_ne!(no_confirmation.status, 0);
    assert!(!user.path().join("Rayman/update/update.json").exists());

    let configured = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &[
            "--format",
            "json",
            "update",
            "configure",
            "--auto-install",
            "--yes",
        ],
    );
    assert_eq!(configured.status, 0, "stderr={}", configured.stderr);
    let report: Value = serde_json::from_str(&configured.stdout).unwrap();
    assert_eq!(report["state"]["auto_check"], true);
    assert_eq!(report["state"]["auto_install"], true);
    assert_eq!(report["install_ready"], false);
    assert!(user.path().join("Rayman/update/update.json").is_file());
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
}

#[test]
fn disabled_update_poll_is_zero_network_and_does_not_touch_workspace_state() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();

    let disabled = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["update", "configure", "--no-auto-check", "--yes"],
    );
    assert_eq!(disabled.status, 0, "stderr={}", disabled.stderr);
    let state_path = user.path().join("Rayman/update/update.json");
    let before = std::fs::read(&state_path).unwrap();

    let poll = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "poll"],
    );
    assert_eq!(poll.status, 0, "stderr={}", poll.stderr);
    let report: Value = serde_json::from_str(&poll.stdout).unwrap();
    assert_eq!(report["status"], "not_due");
    assert_eq!(report["checked"], false);
    assert_eq!(report["state_written"], false);
    assert_eq!(std::fs::read(&state_path).unwrap(), before);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
}

#[test]
fn corrupt_update_state_is_preserved_and_never_becomes_install_consent() {
    let workspace = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let update_dir = user.path().join("Rayman/update");
    std::fs::create_dir_all(&update_dir).unwrap();
    let state_path = update_dir.join("update.json");
    let corrupt = b"{ not trusted state";
    std::fs::write(&state_path, corrupt).unwrap();

    let poll = run_update_with_user_root(
        workspace.path(),
        user.path(),
        &["--format", "json", "update", "poll"],
    );
    assert_eq!(poll.status, 0, "stderr={}", poll.stderr);
    let report: Value = serde_json::from_str(&poll.stdout).unwrap();
    assert_eq!(report["status"], "state_error");
    assert_eq!(report["checked"], false);
    assert_eq!(report["state_written"], false);
    assert_eq!(report["install_authorized"], false);
    assert_eq!(std::fs::read(&state_path).unwrap(), corrupt);
    assert!(!workspace.path().join(".RaymanCodingSkill").exists());
}
