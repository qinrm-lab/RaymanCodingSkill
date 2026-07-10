//! 客观评分与命令执行：在工作区里跑命令，捕获输出、带超时；退出 0 记为成功。
//!
//! `run_shell` 同时服务 agent 的 `run` 工具与隐藏的评分命令。三点关键：
//! - 环境白名单：子进程环境清空后按白名单重建。模型生成的命令输出会随对话历史发给
//!   第三方后端，绝不能让它读到 ANTHROPIC_API_KEY 等父进程密钥。
//! - `EnvPolicy`：with_skill 组把 `rayman` 所在目录前置 PATH，让技能声称的“rayman 可用”
//!   名副其实；control 组反向剔除 PATH 里含 rayman 的目录，保证控制组调不到它。
//! - `timeout`：agent 生成的命令可能挂起（交互提示、死循环），超时后杀整棵进程树，
//!   否则 cargo/rustc 孤儿会持有文件锁，殃及后续评分与工作区准备。

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

/// agent `run` 工具的默认超时（首次 cargo 编译可能偏慢）。
pub const RUN_TIMEOUT: Duration = Duration::from_secs(240);
/// 评分命令的超时。
pub const GRADE_TIMEOUT: Duration = Duration::from_secs(300);

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn rayman_exe() -> &'static str {
    if cfg!(windows) {
        "rayman.exe"
    } else {
        "rayman"
    }
}

/// 子进程 PATH 策略（其余环境变量一律按白名单透传，见 `env_allowed`）。
#[derive(Debug, Clone, Default)]
pub struct EnvPolicy {
    /// 前置到 PATH 的目录（with_skill 组注入 rayman 所在目录）。
    pub extra_path: Option<PathBuf>,
    /// 从 PATH 剔除含 rayman 可执行文件的目录（control 组，确保调不到 rayman）。
    pub exclude_rayman: bool,
}

impl EnvPolicy {
    pub fn with_rayman(dir: PathBuf) -> Self {
        Self {
            extra_path: Some(dir),
            exclude_rayman: false,
        }
    }

    pub fn without_rayman() -> Self {
        Self {
            extra_path: None,
            exclude_rayman: true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GradeResult {
    pub passed: bool,
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

/// 在 `workspace` 里跑一条命令行；子进程环境按白名单重建，PATH 按 `env` 策略重组，并施加 `timeout`。
pub fn run_shell(
    workspace: &Path,
    command: &str,
    env: &EnvPolicy,
    timeout: Duration,
) -> GradeResult {
    let (out_path, err_path) = match temp_pair() {
        Ok(pair) => pair,
        Err(error) => return exec_error(command, &format!("无法创建临时文件: {error}")),
    };

    let mut cmd = shell(command);
    cmd.current_dir(workspace);
    apply_env(&mut cmd, env);
    let (out_file, err_file) = match (File::create(&out_path), File::create(&err_path)) {
        (Ok(o), Ok(e)) => (o, e),
        _ => return exec_error(command, "无法打开临时输出文件"),
    };
    cmd.stdout(out_file).stderr(err_file);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            cleanup(&out_path, &err_path);
            return exec_error(command, &format!("无法执行命令 `{command}`: {error}"));
        }
    };

    let start = Instant::now();
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(-1),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    kill_tree(&mut child);
                    timed_out = true;
                    break -1;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                cleanup(&out_path, &err_path);
                return exec_error(command, &format!("等待命令失败: {error}"));
            }
        }
    };

    let stdout = truncate(&read_lossy(&out_path));
    let mut stderr = truncate(&read_lossy(&err_path));
    cleanup(&out_path, &err_path);
    if timed_out {
        stderr = format!("命令超时（>{}s），已终止。\n{stderr}", timeout.as_secs());
    }

    GradeResult {
        passed: !timed_out && exit == 0,
        exit,
        stdout,
        stderr,
    }
}

fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        // 独立进程组：超时后对整组发 SIGKILL 才能带走 cargo/rustc 孙进程。
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        cmd
    }
}

/// 超时后杀整棵进程树：只杀 shell 会留下持有文件锁的 cargo/rustc 孤儿。
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &child.id().to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(unix)]
    // spawn 时已 process_group(0)，对负 pid 发信号即覆盖全组。
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(format!("-{}", child.id()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // 兜底：无论树杀是否成功，直接结束 shell 本身并回收，避免 wait 悬挂。
    let _ = child.kill();
    let _ = child.wait();
}

/// 清空继承环境，只透传白名单变量；PATH 单独按策略重组后写入。
fn apply_env(cmd: &mut Command, policy: &EnvPolicy) {
    cmd.env_clear();
    let mut parent_path = OsString::new();
    for (key, value) in std::env::vars_os() {
        let Some(name) = key.to_str() else { continue };
        if name.eq_ignore_ascii_case("PATH") {
            parent_path = value;
            continue;
        }
        if env_allowed(name) {
            cmd.env(&key, &value);
        }
    }
    cmd.env("PATH", compose_path(&parent_path, policy));
}

/// 白名单：cmd/sh 与 cargo/rustc 正常工作所需的最小集合；其余（尤其 *_API_KEY）一律不透传。
fn env_allowed(name: &str) -> bool {
    if cfg!(windows) {
        // Windows 环境变量名不区分大小写，统一大写比较。
        const EXACT: &[&str] = &[
            "PATHEXT",
            "COMSPEC",
            "SYSTEMROOT",
            "SYSTEMDRIVE",
            "WINDIR",
            "TEMP",
            "TMP",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "APPDATA",
            "LOCALAPPDATA",
            "PROGRAMDATA",
            "NUMBER_OF_PROCESSORS",
            "OS",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "RUSTUP_TOOLCHAIN",
        ];
        // 前缀覆盖 ProgramFiles(x86)、CommonProgramFiles(x86)、PROCESSOR_* 等变体。
        const PREFIXES: &[&str] = &["PROGRAMFILES", "COMMONPROGRAMFILES", "PROCESSOR_"];
        let upper = name.to_ascii_uppercase();
        EXACT.contains(&upper.as_str()) || PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
    } else {
        const EXACT: &[&str] = &[
            "HOME",
            "USER",
            "SHELL",
            "LANG",
            "TMPDIR",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "RUSTUP_TOOLCHAIN",
        ];
        EXACT.contains(&name) || name.starts_with("LC_")
    }
}

/// 以父进程 PATH 为基础重组子进程 PATH：先按策略剔除含 rayman 的目录，再前置 extra_path。
fn compose_path(parent: &OsStr, policy: &EnvPolicy) -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(parent)
        .filter(|dir| !(policy.exclude_rayman && dir.join(rayman_exe()).exists()))
        .collect();
    if let Some(extra) = &policy.extra_path {
        dirs.insert(0, extra.clone());
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| parent.to_os_string())
}

fn temp_pair() -> std::io::Result<(std::path::PathBuf, std::path::PathBuf)> {
    let base = std::env::temp_dir();
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    Ok((
        base.join(format!("rayman-eval-{pid}-{n}.out")),
        base.join(format!("rayman-eval-{pid}-{n}.err")),
    ))
}

fn read_lossy(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

fn cleanup(a: &Path, b: &Path) {
    let _ = std::fs::remove_file(a);
    let _ = std::fs::remove_file(b);
}

fn exec_error(command: &str, message: &str) -> GradeResult {
    let _ = command;
    GradeResult {
        passed: false,
        exit: -1,
        stdout: String::new(),
        stderr: message.to_string(),
    }
}

fn truncate(text: &str) -> String {
    const MAX: usize = 4000;
    if text.len() <= MAX {
        return text.to_string();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(MAX)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…(truncated)\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_shell_captures_success_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let ok = run_shell(dir.path(), "exit 0", &EnvPolicy::default(), RUN_TIMEOUT);
        assert!(ok.passed);
        let bad = run_shell(dir.path(), "exit 3", &EnvPolicy::default(), RUN_TIMEOUT);
        assert!(!bad.passed);
        assert_eq!(bad.exit, 3);
    }

    #[test]
    fn run_shell_injects_extra_path_into_child_env() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("rayman-bin");
        std::fs::create_dir_all(&marker).unwrap();
        let cmd = if cfg!(windows) {
            "echo %PATH%"
        } else {
            "echo $PATH"
        };
        let policy = EnvPolicy::with_rayman(marker);
        let result = run_shell(dir.path(), cmd, &policy, RUN_TIMEOUT);
        assert!(result.passed);
        assert!(
            result.stdout.contains("rayman-bin"),
            "注入的目录应出现在子进程 PATH: {}",
            result.stdout
        );
    }

    #[test]
    fn run_shell_hides_parent_secrets_from_child() {
        // set_var 在 2024 edition 是 unsafe（与并发 getenv 有竞态），测试进程里可接受。
        unsafe { std::env::set_var("FAKE_SECRET_KEY", "fake-secret-value-123") };
        let dir = tempfile::tempdir().unwrap();
        // Windows 下未定义的 %VAR% 原样回显，Unix 下展开为空——统一断言“值不出现”。
        let cmd = if cfg!(windows) {
            "echo [%FAKE_SECRET_KEY%]"
        } else {
            "echo [$FAKE_SECRET_KEY]"
        };
        let result = run_shell(dir.path(), cmd, &EnvPolicy::default(), RUN_TIMEOUT);
        assert!(result.passed);
        assert!(
            !result.stdout.contains("fake-secret-value-123"),
            "子进程不应读到父进程密钥: {}",
            result.stdout
        );
    }

    #[test]
    fn env_allowlist_blocks_secrets_and_keeps_toolchain_vars() {
        assert!(!env_allowed("ANTHROPIC_API_KEY"));
        assert!(!env_allowed("DEEPSEEK_API_KEY"));
        assert!(env_allowed("CARGO_HOME"));
        if cfg!(windows) {
            assert!(env_allowed("PathExt"));
            assert!(env_allowed("ProgramFiles(x86)"));
            assert!(env_allowed("PROCESSOR_ARCHITECTURE"));
        } else {
            assert!(env_allowed("HOME"));
            assert!(env_allowed("LC_ALL"));
        }
    }

    #[test]
    fn compose_path_strips_rayman_dirs_for_control() {
        let dir = tempfile::tempdir().unwrap();
        let rayman_dir = dir.path().join("has-rayman");
        std::fs::create_dir_all(&rayman_dir).unwrap();
        std::fs::write(rayman_dir.join(rayman_exe()), "").unwrap();
        let clean_dir = dir.path().join("clean");
        std::fs::create_dir_all(&clean_dir).unwrap();

        let parent = std::env::join_paths([rayman_dir.clone(), clean_dir.clone()]).unwrap();
        let composed = compose_path(&parent, &EnvPolicy::without_rayman());
        let dirs: Vec<PathBuf> = std::env::split_paths(&composed).collect();
        assert!(
            !dirs.contains(&rayman_dir),
            "control 组应剔除含 rayman 的目录"
        );
        assert!(dirs.contains(&clean_dir), "无 rayman 的目录应保留");

        // 默认策略不剔除。
        let kept = compose_path(&parent, &EnvPolicy::default());
        let dirs: Vec<PathBuf> = std::env::split_paths(&kept).collect();
        assert!(dirs.contains(&rayman_dir));
    }

    #[test]
    fn run_shell_times_out_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        // 睡 30 秒但只给 1 秒超时 → 应被杀掉并标记失败。
        let cmd = if cfg!(windows) {
            // ping 作为跨平台“sleep”：ping 本机 30 次约 30 秒。
            "ping 127.0.0.1 -n 30 > NUL"
        } else {
            "sleep 30"
        };
        let start = Instant::now();
        let result = run_shell(
            dir.path(),
            cmd,
            &EnvPolicy::default(),
            Duration::from_secs(1),
        );
        assert!(!result.passed);
        assert!(result.stderr.contains("超时"));
        assert!(start.elapsed() < Duration::from_secs(15), "应尽快超时返回");
    }
}
