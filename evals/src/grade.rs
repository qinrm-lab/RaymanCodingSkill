//! 客观评分与命令执行：在工作区里跑命令，捕获输出、带超时；退出 0 记为成功。
//!
//! `run_shell` 同时服务 agent 的 `run` 工具与隐藏的评分命令。两点关键：
//! - `extra_path`：with_skill 组把 `rayman` 所在目录注入 PATH，让技能声称“rayman 可用”名副其实。
//! - `timeout`：agent 生成的命令可能挂起（交互提示、死循环），超时后杀掉，避免整轮卡死。

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

/// agent `run` 工具的默认超时（首次 cargo 编译可能偏慢）。
pub const RUN_TIMEOUT: Duration = Duration::from_secs(240);
/// 评分命令的超时。
pub const GRADE_TIMEOUT: Duration = Duration::from_secs(300);

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct GradeResult {
    pub passed: bool,
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
}

/// 在 `workspace` 里跑一条命令行；可选把 `extra_path` 前置到 PATH，并施加 `timeout`。
pub fn run_shell(
    workspace: &Path,
    command: &str,
    extra_path: Option<&Path>,
    timeout: Duration,
) -> GradeResult {
    let (out_path, err_path) = match temp_pair() {
        Ok(pair) => pair,
        Err(error) => return exec_error(command, &format!("无法创建临时文件: {error}")),
    };

    let mut cmd = shell(command);
    cmd.current_dir(workspace);
    if let Some(dir) = extra_path {
        cmd.env("PATH", prepend_path(dir));
    }
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
                    let _ = child.kill();
                    let _ = child.wait();
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
        cmd
    }
}

fn prepend_path(dir: &Path) -> std::ffi::OsString {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    std::env::join_paths(paths).unwrap_or_else(|_| existing.clone())
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
        let ok = run_shell(dir.path(), "exit 0", None, RUN_TIMEOUT);
        assert!(ok.passed);
        let bad = run_shell(dir.path(), "exit 3", None, RUN_TIMEOUT);
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
        let result = run_shell(dir.path(), cmd, Some(&marker), RUN_TIMEOUT);
        assert!(result.passed);
        assert!(
            result.stdout.contains("rayman-bin"),
            "注入的目录应出现在子进程 PATH: {}",
            result.stdout
        );
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
        let result = run_shell(dir.path(), cmd, None, Duration::from_secs(1));
        assert!(!result.passed);
        assert!(result.stderr.contains("超时"));
        assert!(start.elapsed() < Duration::from_secs(15), "应尽快超时返回");
    }
}
