use std::collections::{BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rayman_core::regression_history::{
    RegressionHistoryManager, RegressionRunRecord, RegressionStepRecord, regression_run_id,
    tail_text,
};
use rayman_core::temp::TempManager;
use rayman_core::{display_path, ensure_within, now_iso};

use crate::cli::RegressionRunProfile;

const TEMP_RUN_METADATA_FILE: &str = "metadata.json";
const SHARED_PARALLEL_TEST_LANES: usize = 4;

#[derive(Debug, Clone)]
struct RegressionStep {
    name: String,
    program: RegressionProgram,
    args: Vec<String>,
    current_dir: Option<PathBuf>,
    target_dir: Option<PathBuf>,
    env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
enum RegressionProgram {
    Cargo,
    RaymanSelf,
    Executable(PathBuf),
}

#[derive(Debug, Clone)]
struct RegressionStepResult {
    name: String,
    command: String,
    success: bool,
    exit_code: Option<i32>,
    duration_ms: u128,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CargoTestExecutable {
    executable: PathBuf,
    manifest_dir: PathBuf,
}

pub(crate) fn run_regression_profile(root: PathBuf, profile: RegressionRunProfile) -> Result<()> {
    println!("RaymanCodingSkill regression run: {profile:?}");
    let started_at = now_iso();
    let started = Instant::now();
    let mut results = Vec::new();
    let temp_run =
        TempManager::new(&root)?.run_dir(&format!("regression {profile:?} cargo target"))?;
    let target_root = temp_run.path.clone();
    let mut cleanup_removed = Vec::new();
    let mut cargo_cache_cleaned = false;
    let mut run_result = (|| -> Result<()> {
        match profile {
            RegressionRunProfile::Auto => {
                run_auto_regression_profile(&root, &target_root, &mut results)?;
                cleanup_regression_targets(
                    &root,
                    &target_root,
                    &mut cleanup_removed,
                    &mut cargo_cache_cleaned,
                )?;
                run_regression_steps(
                    &root,
                    vec![
                        rayman_step("agent eval", &["eval", "run", "--profile", "full"]),
                        rayman_step("security audit", &["security", "audit"]),
                        rayman_step("audit", &["audit"]),
                    ],
                    &mut results,
                )?;
            }
            RegressionRunProfile::Quick => {
                run_regression_steps(
                    &root,
                    vec![
                        cargo_step(
                            "format",
                            &["fmt", "--check"],
                            Some(target_root.join("format")),
                        ),
                        cargo_step(
                            "cli tests",
                            &["test", "-p", "rayman-cli"],
                            Some(target_root.join("cli")),
                        ),
                    ],
                    &mut results,
                )?;
                cleanup_regression_targets(
                    &root,
                    &target_root,
                    &mut cleanup_removed,
                    &mut cargo_cache_cleaned,
                )?;
                run_regression_steps(
                    &root,
                    vec![
                        rayman_step("agent eval", &["eval", "run"]),
                        rayman_step("security audit", &["security", "audit"]),
                        rayman_step("audit", &["audit"]),
                    ],
                    &mut results,
                )?;
            }
            RegressionRunProfile::Full => {
                run_regression_steps(
                    &root,
                    vec![
                        cargo_step(
                            "format",
                            &["fmt", "--check"],
                            Some(target_root.join("format")),
                        ),
                        cargo_step(
                            "clippy",
                            &["clippy", "--all-targets", "--", "-D", "warnings"],
                            Some(target_root.join("clippy")),
                        ),
                        cargo_step(
                            "all tests",
                            &["test", "--all"],
                            Some(target_root.join("all-tests")),
                        ),
                    ],
                    &mut results,
                )?;
                cleanup_regression_targets(
                    &root,
                    &target_root,
                    &mut cleanup_removed,
                    &mut cargo_cache_cleaned,
                )?;
                run_regression_steps(
                    &root,
                    vec![
                        rayman_step("agent eval", &["eval", "run", "--profile", "full"]),
                        rayman_step("security audit", &["security", "audit"]),
                        rayman_step("audit", &["audit"]),
                    ],
                    &mut results,
                )?;
            }
            RegressionRunProfile::SharedParallelFull => {
                run_shared_parallel_full_profile(&root, &target_root, &mut results)?;
                cleanup_regression_targets(
                    &root,
                    &target_root,
                    &mut cleanup_removed,
                    &mut cargo_cache_cleaned,
                )?;
                run_regression_steps(
                    &root,
                    vec![
                        rayman_step("agent eval", &["eval", "run", "--profile", "full"]),
                        rayman_step("security audit", &["security", "audit"]),
                        rayman_step("audit", &["audit"]),
                    ],
                    &mut results,
                )?;
            }
            RegressionRunProfile::ParallelFull => {
                run_regression_steps(
                    &root,
                    vec![cargo_step(
                        "format",
                        &["fmt", "--check"],
                        Some(target_root.join("format")),
                    )],
                    &mut results,
                )?;
                run_parallel_regression_steps(
                    &root,
                    vec![
                        cargo_step(
                            "clippy",
                            &["clippy", "--all-targets", "--", "-D", "warnings"],
                            Some(target_root.join("clippy")),
                        ),
                        cargo_step(
                            "core tests",
                            &["test", "-p", "rayman-core"],
                            Some(target_root.join("core")),
                        ),
                        cargo_step(
                            "cli tests",
                            &["test", "-p", "rayman-cli"],
                            Some(target_root.join("cli")),
                        ),
                        cargo_step(
                            "api tests",
                            &["test", "-p", "rayman-api"],
                            Some(target_root.join("api")),
                        ),
                    ],
                    &mut results,
                )?;
                cleanup_regression_targets(
                    &root,
                    &target_root,
                    &mut cleanup_removed,
                    &mut cargo_cache_cleaned,
                )?;
                run_regression_steps(
                    &root,
                    vec![
                        rayman_step("agent eval", &["eval", "run", "--profile", "full"]),
                        rayman_step("security audit", &["security", "audit"]),
                        rayman_step("audit", &["audit"]),
                    ],
                    &mut results,
                )?;
            }
        }
        Ok(())
    })();
    if run_result.is_ok() && !cargo_cache_cleaned {
        match cleanup_success_temp_targets(&root, std::slice::from_ref(&target_root)) {
            Ok(removed) => cleanup_removed = removed,
            Err(error) => {
                let _ = temp_run.fail();
                run_result = Err(error);
            }
        }
    } else if run_result.is_err() && target_root.exists() {
        let _ = temp_run.fail();
        println!("回归失败，保留临时构建缓存: {}", display_path(&target_root));
        println!("诊断后可运行: rayman temp cleanup --all-failed");
    } else if run_result.is_err() {
        println!("回归失败；cargo 临时构建缓存已在 cargo 步骤通过后清理");
    }
    record_regression_history(
        &root,
        profile,
        &started_at,
        started.elapsed().as_millis(),
        &results,
        run_result.is_ok(),
    )?;
    for path in cleanup_removed {
        println!("已清理临时构建缓存: {}", display_path(&path));
    }
    run_result?;
    println!("回归运行通过");
    Ok(())
}

fn run_auto_regression_profile(
    root: &Path,
    target_root: &Path,
    results: &mut Vec<RegressionStepResult>,
) -> Result<()> {
    if root.join("Cargo.toml").is_file() {
        let reason = "selected shared-parallel-full: Cargo.toml detected; build artifacts are reused from one managed CARGO_TARGET_DIR, test executables are run with bounded workers, and existing full/parallel-full profiles remain explicit choices";
        println!("auto regression decision: {reason}");
        push_synthetic_step(results, "auto decision", reason, true, Some(0));
        run_shared_parallel_full_profile(root, target_root, results)
    } else {
        let reason =
            "unsupported: auto regression currently requires a Cargo.toml at the workspace root";
        push_synthetic_step(results, "auto decision", reason, false, Some(1));
        bail!("{reason}");
    }
}

fn run_shared_parallel_full_profile(
    root: &Path,
    target_root: &Path,
    results: &mut Vec<RegressionStepResult>,
) -> Result<()> {
    let shared_target = target_root.join("shared");
    run_regression_steps(
        root,
        vec![
            cargo_step("format", &["fmt", "--check"], Some(shared_target.clone())),
            cargo_step(
                "clippy",
                &["clippy", "--all-targets", "--", "-D", "warnings"],
                Some(shared_target.clone()),
            ),
        ],
        results,
    )?;
    let test_build = run_regression_step(
        root,
        cargo_step(
            "test build",
            &[
                "test",
                "--workspace",
                "--all-targets",
                "--no-run",
                "--message-format=json",
            ],
            Some(shared_target.clone()),
        ),
        results,
    )?;
    let executables = cargo_test_executables_from_json(&test_build.stdout);
    let lanes = parallel_test_lanes(executables.len());
    if executables.is_empty() {
        push_synthetic_step(
            results,
            "parallel test executables",
            "no compiled test executables were reported by cargo test --no-run; doc tests still run through cargo",
            true,
            Some(0),
        );
    } else {
        let rust_test_threads = rust_test_threads_for_lanes(lanes);
        println!(
            "shared parallel test executables: count={} workers={} RUST_TEST_THREADS={}",
            executables.len(),
            lanes,
            rust_test_threads
        );
        let steps = executables
            .into_iter()
            .enumerate()
            .map(|(index, artifact)| {
                let executable = artifact.executable;
                executable_step(
                    format!(
                        "test executable {} ({})",
                        index + 1,
                        executable
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("unknown")
                    ),
                    executable,
                    artifact.manifest_dir,
                    vec![("RUST_TEST_THREADS".into(), rust_test_threads.to_string())],
                )
            })
            .collect::<Vec<_>>();
        run_bounded_parallel_regression_steps(root, steps, lanes, results)?;
    }
    run_regression_steps(
        root,
        vec![cargo_step(
            "doc tests",
            &["test", "--workspace", "--doc"],
            Some(shared_target),
        )],
        results,
    )
}

fn cleanup_regression_targets(
    root: &Path,
    target_root: &PathBuf,
    cleanup_removed: &mut Vec<PathBuf>,
    cargo_cache_cleaned: &mut bool,
) -> Result<()> {
    let removed = cleanup_success_temp_targets(root, std::slice::from_ref(target_root))?;
    cleanup_removed.extend(removed);
    *cargo_cache_cleaned = true;
    Ok(())
}

fn cargo_step(
    name: impl Into<String>,
    args: &[&str],
    target_dir: Option<PathBuf>,
) -> RegressionStep {
    RegressionStep {
        name: name.into(),
        program: RegressionProgram::Cargo,
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        current_dir: None,
        target_dir,
        env: Vec::new(),
    }
}

fn rayman_step(name: impl Into<String>, args: &[&str]) -> RegressionStep {
    RegressionStep {
        name: name.into(),
        program: RegressionProgram::RaymanSelf,
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
        current_dir: None,
        target_dir: None,
        env: Vec::new(),
    }
}

fn executable_step(
    name: impl Into<String>,
    executable: PathBuf,
    current_dir: PathBuf,
    env: Vec<(String, String)>,
) -> RegressionStep {
    RegressionStep {
        name: name.into(),
        program: RegressionProgram::Executable(executable),
        args: Vec::new(),
        current_dir: Some(current_dir),
        target_dir: None,
        env,
    }
}

fn run_regression_steps(
    root: &Path,
    steps: Vec<RegressionStep>,
    results: &mut Vec<RegressionStepResult>,
) -> Result<()> {
    for step in steps {
        run_regression_step(root, step, results)?;
    }
    Ok(())
}

fn run_regression_step(
    root: &Path,
    step: RegressionStep,
    results: &mut Vec<RegressionStepResult>,
) -> Result<RegressionStepResult> {
    let result = execute_regression_step(root, step)?;
    print_regression_step_result(&result);
    let failed = !result.success;
    let name = result.name.clone();
    let exit_code = result.exit_code;
    results.push(result.clone());
    if failed {
        bail!(
            "回归步骤失败: {} ({})",
            name,
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "terminated".into())
        );
    }
    Ok(result)
}

fn run_parallel_regression_steps(
    root: &Path,
    steps: Vec<RegressionStep>,
    all_results: &mut Vec<RegressionStepResult>,
) -> Result<()> {
    let lanes = steps.len().max(1);
    run_bounded_parallel_regression_steps(root, steps, lanes, all_results)
}

fn run_bounded_parallel_regression_steps(
    root: &Path,
    steps: Vec<RegressionStep>,
    max_lanes: usize,
    all_results: &mut Vec<RegressionStepResult>,
) -> Result<()> {
    if steps.is_empty() {
        return Ok(());
    }
    let lanes = max_lanes.max(1).min(steps.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(
        steps.into_iter().enumerate().collect::<Vec<_>>(),
    )));
    let handles = (0..lanes)
        .map(|_| {
            let root = root.to_path_buf();
            let queue = Arc::clone(&queue);
            thread::spawn(move || {
                let mut worker_results = Vec::new();
                loop {
                    let item = {
                        let mut queue = queue
                            .lock()
                            .map_err(|_| anyhow::anyhow!("并行回归队列锁异常"))?;
                        queue.pop_front()
                    };
                    let Some((index, step)) = item else {
                        break;
                    };
                    let result = execute_regression_step(&root, step)?;
                    worker_results.push((index, result));
                }
                Ok::<_, anyhow::Error>(worker_results)
            })
        })
        .collect::<Vec<_>>();
    let mut results = Vec::new();
    for handle in handles {
        let worker_results = handle
            .join()
            .map_err(|_| anyhow::anyhow!("并行回归线程异常退出"))??;
        results.extend(worker_results);
    }
    results.sort_by_key(|(index, _)| *index);
    let mut failed = Vec::new();
    for (_, result) in &results {
        print_regression_step_result(result);
        if !result.success {
            failed.push(format!(
                "{} ({})",
                result.name.as_str(),
                result
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "terminated".into())
            ));
        }
    }
    all_results.extend(results.into_iter().map(|(_, result)| result));
    if !failed.is_empty() {
        bail!("并行回归步骤失败: {}", failed.join(", "));
    }
    Ok(())
}

fn execute_regression_step(root: &Path, step: RegressionStep) -> Result<RegressionStepResult> {
    let command = regression_command_text(&step)?;
    let mut process = match step.program {
        RegressionProgram::Cargo => ProcessCommand::new("cargo"),
        RegressionProgram::RaymanSelf => ProcessCommand::new(std::env::current_exe()?),
        RegressionProgram::Executable(ref executable) => ProcessCommand::new(executable),
    };
    let current_dir = step.current_dir.as_deref().unwrap_or(root);
    process.current_dir(current_dir).args(&step.args);
    if let Some(target_dir) = &step.target_dir {
        process.env("CARGO_TARGET_DIR", target_dir);
    }
    for (key, value) in &step.env {
        process.env(key, value);
    }
    let started = Instant::now();
    let output = process
        .output()
        .with_context(|| format!("无法执行回归步骤 `{}`: {command}", step.name))?;
    Ok(RegressionStepResult {
        name: step.name,
        command,
        success: output.status.success(),
        exit_code: output.status.code(),
        duration_ms: started.elapsed().as_millis(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn regression_command_text(step: &RegressionStep) -> Result<String> {
    let program = match step.program {
        RegressionProgram::Cargo => "cargo".to_string(),
        RegressionProgram::RaymanSelf => display_path(&std::env::current_exe()?),
        RegressionProgram::Executable(ref executable) => display_path(executable),
    };
    let mut parts = Vec::new();
    if let Some(current_dir) = &step.current_dir {
        parts.push(format!("cwd={}", display_path(current_dir)));
    }
    if let Some(target_dir) = &step.target_dir {
        parts.push(format!("CARGO_TARGET_DIR={}", display_path(target_dir)));
    }
    parts.extend(step.env.iter().map(|(key, value)| format!("{key}={value}")));
    parts.push(program);
    parts.extend(step.args.iter().cloned());
    Ok(parts.join(" "))
}

fn push_synthetic_step(
    results: &mut Vec<RegressionStepResult>,
    name: impl Into<String>,
    message: impl Into<String>,
    success: bool,
    exit_code: Option<i32>,
) {
    let name = name.into();
    let message = message.into();
    let result = RegressionStepResult {
        name,
        command: "rayman-internal".into(),
        success,
        exit_code,
        duration_ms: 0,
        stdout: message,
        stderr: String::new(),
    };
    print_regression_step_result(&result);
    results.push(result);
}

fn cargo_test_executables_from_json(stdout: &str) -> Vec<CargoTestExecutable> {
    let mut executables = BTreeSet::new();
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(|reason| reason.as_str()) != Some("compiler-artifact") {
            continue;
        }
        if !value
            .pointer("/profile/test")
            .and_then(|test| test.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let target_kind = value
            .pointer("/target/kind")
            .and_then(|kind| kind.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let runnable_target = target_kind
            .iter()
            .any(|kind| matches!(*kind, "bin" | "lib" | "test" | "example" | "bench"));
        if !runnable_target {
            continue;
        }
        let Some(executable) = value.get("executable").and_then(|path| path.as_str()) else {
            continue;
        };
        let Some(manifest_dir) = value
            .get("manifest_path")
            .and_then(|path| path.as_str())
            .and_then(|path| Path::new(path).parent())
            .map(Path::to_path_buf)
        else {
            continue;
        };
        executables.insert(CargoTestExecutable {
            executable: PathBuf::from(executable),
            manifest_dir,
        });
    }
    executables.into_iter().collect()
}

fn parallel_test_lanes(executable_count: usize) -> usize {
    if executable_count == 0 {
        return 0;
    }
    let available = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    SHARED_PARALLEL_TEST_LANES
        .min(executable_count)
        .min(available)
}

fn rust_test_threads_for_lanes(lanes: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    rust_test_threads_for_lanes_with_available(available, lanes)
}

fn rust_test_threads_for_lanes_with_available(available: usize, lanes: usize) -> usize {
    let available = available.max(1);
    if lanes == 0 {
        return available;
    }
    (available / lanes.max(1)).max(1)
}

fn print_regression_step_result(result: &RegressionStepResult) {
    let mark = if result.success { "✓" } else { "✗" };
    println!(
        "{mark} {} [{} ms] {}",
        result.name, result.duration_ms, result.command
    );
    if !result.success {
        if !result.stdout.trim().is_empty() {
            println!("--- stdout: {} ---", result.name);
            println!("{}", result.stdout.trim_end());
        }
        if !result.stderr.trim().is_empty() {
            println!("--- stderr: {} ---", result.name);
            println!("{}", result.stderr.trim_end());
        }
    }
}

fn cleanup_success_temp_targets(root: &Path, targets: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let root = root
        .canonicalize()
        .with_context(|| format!("无法解析工作区路径: {}", display_path(root)))?;
    let managed_runs_root = TempManager::new(&root)?.root().join("runs");
    for target in targets {
        let target = ensure_within(target, &root, "临时构建缓存必须位于工作区内")?;
        if target == root {
            bail!("拒绝删除工作区根目录作为临时构建缓存");
        }
        if !target.exists() {
            continue;
        }
        let managed_runs_root = managed_runs_root.canonicalize().with_context(|| {
            format!("无法解析临时运行目录: {}", display_path(&managed_runs_root))
        })?;
        if !target.is_dir() {
            bail!(
                "临时构建缓存必须是 Rayman 管理的运行目录: {}",
                display_path(&target)
            );
        }
        if target.parent() != Some(managed_runs_root.as_path()) {
            bail!(
                "临时构建缓存必须位于 Rayman 管理的 runs 目录: {}",
                display_path(&target)
            );
        }
        if !target.join(TEMP_RUN_METADATA_FILE).is_file() {
            bail!(
                "临时构建缓存缺少 Rayman 管理元数据: {}",
                display_path(&target)
            );
        }
        fs::remove_dir_all(&target)
            .with_context(|| format!("无法删除临时构建缓存: {}", display_path(&target)))?;
        removed.push(target);
    }
    Ok(removed)
}

fn record_regression_history(
    root: &Path,
    profile: RegressionRunProfile,
    started_at: &str,
    duration_ms: u128,
    results: &[RegressionStepResult],
    passed: bool,
) -> Result<()> {
    let finished_at = now_iso();
    let profile_name = profile.as_str();
    let record = RegressionRunRecord {
        id: regression_run_id(profile_name, started_at),
        profile: profile_name.into(),
        status: if passed { "passed" } else { "failed" }.into(),
        started_at: started_at.into(),
        finished_at,
        duration_ms,
        steps: results
            .iter()
            .map(|result| RegressionStepRecord {
                name: result.name.clone(),
                command: result.command.clone(),
                success: result.success,
                exit_code: result.exit_code,
                duration_ms: result.duration_ms,
                stdout_tail: tail_text(&result.stdout, 4000),
                stderr_tail: tail_text(&result.stderr, 4000),
            })
            .collect(),
    };
    let manager = RegressionHistoryManager::new(root)?;
    manager.append(&record)?;
    println!(
        "回归历史已记录: {} ({})",
        display_path(manager.history_path()),
        record.id
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_success_temp_targets_removes_target_inside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let target = root
            .join(".RaymanCodingSkill")
            .join("tmp")
            .join("runs")
            .join("regression-targets");
        let child_dir = target.join("core");
        let child_file = target.join("marker.txt");
        std::fs::create_dir_all(&child_dir).unwrap();
        std::fs::write(target.join(TEMP_RUN_METADATA_FILE), "{}").unwrap();
        std::fs::write(child_dir.join("artifact.txt"), "artifact").unwrap();
        std::fs::write(&child_file, "marker").unwrap();

        let removed = cleanup_success_temp_targets(&root, std::slice::from_ref(&target)).unwrap();

        assert!(!target.exists());
        assert!(!child_dir.exists());
        assert!(!child_file.exists());
        assert_eq!(removed.len(), 1);
        assert_eq!(display_path(&removed[0]), display_path(&target));
    }

    #[test]
    fn cleanup_success_temp_targets_rejects_outside_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_target = outside.path().join("regression-targets");
        std::fs::create_dir_all(&outside_target).unwrap();

        let result = cleanup_success_temp_targets(workspace.path(), &[outside_target]);

        assert!(result.is_err());
    }

    #[test]
    fn cleanup_success_temp_targets_rejects_workspace_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let source_backup = root.join("src").join("lib.rs.bk");
        std::fs::write(&source_backup, "source backup").unwrap();

        let result = cleanup_success_temp_targets(&root, std::slice::from_ref(&source_backup));

        assert!(result.is_err());
        assert!(source_backup.exists());
    }

    #[test]
    fn cleanup_success_temp_targets_rejects_unmanaged_workspace_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let unmanaged = root
            .join(".RaymanCodingSkill")
            .join("tmp")
            .join("not-a-run");
        std::fs::create_dir_all(&unmanaged).unwrap();

        let result = cleanup_success_temp_targets(&root, std::slice::from_ref(&unmanaged));

        assert!(result.is_err());
        assert!(unmanaged.exists());
    }

    #[test]
    fn cargo_test_executables_from_json_keeps_only_test_artifacts() {
        let stdout = r#"
{"reason":"compiler-artifact","manifest_path":"crates/rayman-core/Cargo.toml","target":{"kind":["lib"]},"profile":{"test":true},"executable":"target/debug/deps/rayman_core_test.exe"}
{"reason":"compiler-artifact","manifest_path":"crates/rayman-core/Cargo.toml","target":{"kind":["custom-build"]},"profile":{"test":true},"executable":"target/debug/build/build_script.exe"}
{"reason":"compiler-artifact","manifest_path":"crates/rayman-cli/Cargo.toml","target":{"kind":["bin"]},"profile":{"test":false},"executable":"target/debug/rayman.exe"}
{"reason":"compiler-artifact","manifest_path":"crates/rayman-cli/Cargo.toml","target":{"kind":["test"]},"profile":{"test":true},"executable":"target/debug/deps/ui_contract.exe"}
not-json
"#;

        let executables = cargo_test_executables_from_json(stdout);

        assert_eq!(
            executables,
            vec![
                CargoTestExecutable {
                    executable: PathBuf::from("target/debug/deps/rayman_core_test.exe"),
                    manifest_dir: PathBuf::from("crates/rayman-core"),
                },
                CargoTestExecutable {
                    executable: PathBuf::from("target/debug/deps/ui_contract.exe"),
                    manifest_dir: PathBuf::from("crates/rayman-cli"),
                },
            ]
        );
    }

    #[test]
    fn rust_test_threads_split_available_parallelism_across_lanes() {
        assert_eq!(rust_test_threads_for_lanes_with_available(16, 4), 4);
        assert_eq!(rust_test_threads_for_lanes_with_available(3, 4), 1);
        assert_eq!(rust_test_threads_for_lanes_with_available(8, 0), 8);
    }
}
