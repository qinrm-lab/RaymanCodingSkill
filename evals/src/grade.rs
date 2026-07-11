//! 客观评分与命令执行：在工作区里跑命令，捕获输出、带超时；退出 0 记为成功。
//!
//! `run_shell` 同时服务 agent 的 `run` 工具与隐藏的评分命令。三点关键：
//! - 环境白名单：子进程环境清空后按白名单重建。模型生成的命令输出会随对话历史发给
//!   第三方后端，绝不能让它读到 ANTHROPIC_API_KEY 等父进程密钥。
//! - `EnvPolicy`：with_skill 组把 `rayman` 所在目录前置 PATH，让技能声称的“rayman 可用”
//!   名副其实；control 组反向剔除 PATH 里暴露 `rayman` 的目录。它只是 PATH hygiene，
//!   不能把未隔离宿主 shell 变成安全或可比较的实验边界。
//! - `timeout`：agent 生成的命令可能挂起（交互提示、死循环），超时后杀整棵进程树，
//!   否则 cargo/rustc 孤儿会持有文件锁，殃及后续评分与工作区准备。

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use serde::Serialize;
use tempfile::{Builder as TempFileBuilder, NamedTempFile};

/// agent `run` 工具的默认超时（首次 cargo 编译可能偏慢）。
pub const RUN_TIMEOUT: Duration = Duration::from_secs(240);
/// 评分命令的超时。
pub const GRADE_TIMEOUT: Duration = Duration::from_secs(300);

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
    /// 从 PATH 剔除暴露 rayman 命令的目录（control 组的 best-effort PATH hygiene）。
    pub exclude_rayman: bool,
    /// 控制组若在 trial 工作区顶层发现可由 Windows 当前目录解析到的 rayman wrapper，
    /// 记录本次 trial 已失去组别隔离。Arc 让 agent 工具循环和 run_trial 共享同一事实。
    control_workspace_violation: Option<Arc<AtomicBool>>,
}

impl EnvPolicy {
    pub fn with_rayman(dir: PathBuf) -> Self {
        Self {
            extra_path: Some(dir),
            exclude_rayman: false,
            control_workspace_violation: None,
        }
    }

    pub fn without_rayman() -> Self {
        Self {
            extra_path: None,
            exclude_rayman: true,
            control_workspace_violation: Some(Arc::new(AtomicBool::new(false))),
        }
    }

    fn check_control_workspace(&self, workspace: &Path) -> Result<(), String> {
        if !self.exclude_rayman {
            return Ok(());
        }
        if let Err(error) = ensure_no_top_level_rayman_command(workspace) {
            if let Some(violation) = &self.control_workspace_violation {
                violation.store(true, Ordering::Relaxed);
            }
            return Err(error);
        }
        Ok(())
    }

    /// `cmd` permits a single compound command to create, invoke, and remove a CWD
    /// wrapper before the postflight scan sees it. For the control arm, reject every
    /// command text that names a rayman executable/wrapper at all. This deliberately
    /// favors a false-negative trial over silently restoring the treated capability;
    /// it remains a narrow integrity guard, not a shell sandbox.
    fn check_control_command(&self, command: &str) -> Result<(), String> {
        if !self.exclude_rayman || !control_command_mentions_rayman(command) {
            return Ok(());
        }
        if let Some(violation) = &self.control_workspace_violation {
            violation.store(true, Ordering::Relaxed);
        }
        Err("控制组拒绝包含 rayman 命令/包装器名称的 shell 命令".into())
    }

    pub fn control_workspace_violation(&self) -> bool {
        self.control_workspace_violation
            .as_ref()
            .is_some_and(|violation| violation.load(Ordering::Relaxed))
    }
}

/// 评分命令的结果类别。
///
/// `TimedOut` 与 `InfrastructureError` 不能当成模型未完成任务：前者无法区分被测
/// 修改导致的挂起与评测环境异常，后者则根本没有得到可用的评分结果。上层将这两种
/// 情况记为 `Outcome::Error`，从 evaluable 分母排除，但保留在 ITT 分母和报告中。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GradeOutcome {
    /// 工作区准备失败，评分命令没有执行。
    NotRun,
    /// 评分命令以 0 退出。
    Passed,
    /// 评分命令以非 0 退出。
    Failed,
    /// 评分命令超过明确的时限后被终止。
    TimedOut,
    /// 临时文件、spawn 或 wait 等评测基础设施失败。
    InfrastructureError,
}

impl GradeOutcome {
    pub fn passed(self) -> bool {
        matches!(self, Self::Passed)
    }

    pub fn is_evaluation_error(self) -> bool {
        matches!(
            self,
            Self::NotRun | Self::TimedOut | Self::InfrastructureError
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NotRun => "not_run",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::InfrastructureError => "infrastructure_error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GradeResult {
    pub outcome: GradeOutcome,
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
    // `cmd /C rayman` searches the current directory before PATH. PATH filtering alone
    // cannot protect the control arm from a fixture/agent-provided rayman.cmd/.bat/.exe.
    // This is a narrow experiment-integrity preflight, not a sandbox claim.
    if let Err(error) = env.check_control_command(command) {
        return exec_error(command, &error);
    }
    if let Err(error) = env.check_control_workspace(workspace) {
        return exec_error(command, &error);
    }
    let temp = match temp_pair() {
        Ok(pair) => pair,
        Err(error) => return exec_error(command, &format!("无法创建临时文件: {error}")),
    };
    let mut cmd = shell(command);
    cmd.current_dir(workspace);
    apply_env(&mut cmd, env);
    // Keep four handles to the exclusively-created files: two are inherited by the
    // child and two are held by the evaluator for reading. Reading through retained
    // handles, rather than re-opening the path after the child returns, prevents a
    // same-user host shell from swapping a discovered temp pathname under the report.
    let (out_file, mut out_reader, err_file, mut err_reader) = match (
        temp.stdout.as_file().try_clone(),
        temp.stdout.as_file().try_clone(),
        temp.stderr.as_file().try_clone(),
        temp.stderr.as_file().try_clone(),
    ) {
        (Ok(out_file), Ok(out_reader), Ok(err_file), Ok(err_reader)) => {
            (out_file, out_reader, err_file, err_reader)
        }
        _ => return exec_error(command, "无法打开临时输出文件"),
    };
    cmd.stdout(out_file).stderr(err_file);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => return exec_error(command, &format!("无法执行命令 `{command}`: {error}")),
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
                return exec_error(command, &format!("等待命令失败: {error}"));
            }
        }
    };

    let stdout = truncate(&read_lossy_file(&mut out_reader));
    let mut stderr = truncate(&read_lossy_file(&mut err_reader));
    if timed_out {
        stderr = format!("命令超时（>{}s），已终止。\n{stderr}", timeout.as_secs());
    }

    GradeResult {
        outcome: if timed_out {
            GradeOutcome::TimedOut
        } else if exit == 0 {
            GradeOutcome::Passed
        } else {
            GradeOutcome::Failed
        },
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
        .filter(|dir| !(policy.exclude_rayman && contains_rayman_command(dir)))
        .collect();
    if let Some(extra) = &policy.extra_path {
        dirs.insert(0, extra.clone());
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| parent.to_os_string())
}

/// `cmd /C rayman` consults `PATHEXT`; filtering only `rayman.exe` leaves common
/// `.cmd`/`.bat` shims (and custom PATHEXT extensions) reachable in the control arm.
/// On Windows fail closed for unreadable PATH entries and remove any entry that can
/// expose a `rayman`-named command. Unix command lookup is exact, so the regular
/// executable name is sufficient there.
fn contains_rayman_command(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return true,
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                // A failed directory entry could itself be a rayman wrapper; strip the
                // whole PATH directory rather than silently retaining it in control.
                Err(_) => return true,
            };
            if is_rayman_command_name(&entry.file_name()) {
                return true;
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        dir.join(rayman_exe()).exists()
    }
}

/// `cmd` resolves a bare command from the current directory before PATH. On Windows reject a
/// top-level regular or reparse `rayman` command of any extension, including custom PATHEXT
/// values. A directory named rayman is not executable and is intentionally not rejected.
/// Non-Windows shells do not normally search CWD for a bare command, so this is a no-op there.
pub fn ensure_no_top_level_rayman_command(workspace: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        let entries = fs::read_dir(workspace).map_err(|error| {
            format!(
                "无法枚举控制组 trial 工作区以确认不存在当前目录 rayman wrapper {}: {error}",
                workspace.display()
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "无法枚举控制组 trial 工作区以确认不存在当前目录 rayman wrapper {}: {error}",
                    workspace.display()
                )
            })?;
            if !is_rayman_command_name(&entry.file_name()) {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "无法检查控制组 trial 当前目录 rayman wrapper {}: {error}",
                    path.display()
                )
            })?;
            if metadata.is_file() || is_link_or_reparse(&metadata) {
                return Err(format!(
                    "控制组拒绝 trial 工作区顶层可由 Windows 当前目录解析的 rayman 命令/包装器: {}",
                    path.display()
                ));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = workspace;
    }
    Ok(())
}

fn is_rayman_command_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.eq_ignore_ascii_case("rayman")
        || name
            .get(.."rayman.".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("rayman."))
}

/// Windows command syntax is too rich for a complete portable parser. The control
/// arm therefore uses a deliberately conservative lexical check: any shell token
/// that names `rayman` or `rayman.*` is rejected before `cmd` receives it. It catches
/// the create→invoke→delete wrapper sequence while leaving the host-shell warning in
/// force for everything a textual guard cannot prove.
fn control_command_mentions_rayman(command: &str) -> bool {
    #[cfg(windows)]
    {
        command
            .split(|ch: char| {
                ch.is_whitespace() || matches!(ch, '&' | '|' | ';' | '<' | '>' | '(' | ')' | '=')
            })
            .map(|token| token.trim_matches(|ch| matches!(ch, '\'' | '"' | '`')))
            .filter(|token| !token.is_empty())
            .any(|token| {
                let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
                is_rayman_command_name(OsStr::new(base))
            })
    }
    #[cfg(not(windows))]
    {
        let _ = command;
        false
    }
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

struct TempOutputPair {
    stdout: NamedTempFile,
    stderr: NamedTempFile,
}

/// Random, exclusively-created output files. `tempfile` retains the open handles
/// and removes the paths on drop, closing the former predictable-name preoccupation
/// and symlink/hardlink race.
fn temp_pair() -> std::io::Result<TempOutputPair> {
    temp_pair_in(&std::env::temp_dir())
}

fn temp_pair_in(base: &Path) -> std::io::Result<TempOutputPair> {
    let stdout = TempFileBuilder::new()
        .prefix("rayman-eval-")
        .suffix(".out")
        .tempfile_in(base)?;
    let stderr = TempFileBuilder::new()
        .prefix("rayman-eval-")
        .suffix(".err")
        .tempfile_in(base)?;
    Ok(TempOutputPair { stdout, stderr })
}

fn read_lossy_file(file: &mut File) -> String {
    if file.seek(SeekFrom::Start(0)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn exec_error(command: &str, message: &str) -> GradeResult {
    let _ = command;
    GradeResult {
        outcome: GradeOutcome::InfrastructureError,
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
        assert_eq!(ok.outcome, GradeOutcome::Passed);
        let bad = run_shell(dir.path(), "exit 3", &EnvPolicy::default(), RUN_TIMEOUT);
        assert_eq!(bad.outcome, GradeOutcome::Failed);
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
        assert!(result.outcome.passed());
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
        assert!(result.outcome.passed());
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
        #[cfg(not(windows))]
        std::fs::write(rayman_dir.join(rayman_exe()), "").unwrap();
        #[cfg(windows)]
        {
            // 没有 rayman.exe：`cmd` 仍会按 PATHEXT 找到这些 wrapper，故控制组必须
            // 整目录剔除，而不能只查 .exe。
            std::fs::write(rayman_dir.join("rayman.cmd"), "@echo off").unwrap();
            std::fs::write(rayman_dir.join("rayman.bat"), "@echo off").unwrap();
            std::fs::write(rayman_dir.join("rayman.custom"), "@echo off").unwrap();
        }
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

    #[cfg(windows)]
    #[test]
    fn control_cwd_rayman_wrapper_fails_closed_before_shell_start() {
        let dir = tempfile::tempdir().unwrap();
        // Use a multi-dot custom extension to prove this is not limited to .exe/.cmd/.bat.
        std::fs::write(
            dir.path().join("RAYMAN.custom.extension"),
            "@echo invoked > %~dp0invoked.txt",
        )
        .unwrap();
        let policy = EnvPolicy::without_rayman();

        let result = run_shell(dir.path(), "rayman", &policy, RUN_TIMEOUT);

        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert!(policy.control_workspace_violation());
        assert!(result.stderr.contains("控制组拒绝"), "{}", result.stderr);
        assert!(!dir.path().join("invoked.txt").exists());
    }

    #[cfg(windows)]
    #[test]
    fn control_rejects_compound_create_invoke_delete_wrapper_command() {
        let dir = tempfile::tempdir().unwrap();
        let policy = EnvPolicy::without_rayman();
        let command = "echo @echo off > rayman.cmd & rayman & del rayman.cmd";

        let result = run_shell(dir.path(), command, &policy, RUN_TIMEOUT);

        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert!(policy.control_workspace_violation());
        assert!(
            result.stderr.contains("rayman 命令/包装器"),
            "{}",
            result.stderr
        );
        assert!(
            !dir.path().join("rayman.cmd").exists(),
            "command must be rejected before cmd can create the wrapper"
        );
    }

    #[test]
    fn temp_pair_ignores_preoccupied_legacy_predictable_paths() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_out = dir
            .path()
            .join(format!("rayman-eval-{}-0.out", std::process::id()));
        let legacy_err = dir
            .path()
            .join(format!("rayman-eval-{}-0.err", std::process::id()));
        std::fs::create_dir(&legacy_out).unwrap();
        std::fs::create_dir(&legacy_err).unwrap();

        let pair = temp_pair_in(dir.path()).unwrap();

        assert_ne!(pair.stdout.path(), legacy_out);
        assert_ne!(pair.stderr.path(), legacy_err);
        assert_ne!(pair.stdout.path(), pair.stderr.path());
        assert!(pair.stdout.path().is_file());
        assert!(pair.stderr.path().is_file());
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
        assert_eq!(result.outcome, GradeOutcome::TimedOut);
        assert!(result.stderr.contains("超时"));
        assert!(start.elapsed() < Duration::from_secs(15), "应尽快超时返回");
    }

    #[test]
    fn run_shell_classifies_spawn_failures_as_infrastructure_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing_workspace = dir.path().join("does-not-exist");
        let result = run_shell(
            &missing_workspace,
            "exit 0",
            &EnvPolicy::default(),
            RUN_TIMEOUT,
        );
        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert_eq!(result.exit, -1);
    }
}
