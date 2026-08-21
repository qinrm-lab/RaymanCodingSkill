//! 客观评分与命令执行：在工作区里跑命令，捕获输出、带超时；退出 0 记为成功。
//!
//! `run_shell` 同时服务 agent 的 `run` 工具与隐藏的评分命令。三点关键：
//! - 环境白名单：子进程环境清空后按白名单重建。模型生成的命令输出会随对话历史发给
//!   第三方后端，绝不能让它读到 ANTHROPIC_API_KEY 等父进程密钥。
//! - `EnvPolicy`：with_skill 组把 `rayman` 所在目录前置 PATH，让技能声称的“rayman 可用”
//!   名副其实；control 组反向剔除 PATH 里暴露 `rayman` 的目录。它只是 PATH hygiene，
//!   不能把未隔离宿主 shell 变成安全或可比较的实验边界。
//! - `timeout`：agent 生成的命令可能挂起（交互提示、死循环）。Windows 只在 suspended
//!   child 成功绑定 kill-on-close Job Object 后执行；Job 创建/绑定失败不回退。其他平台
//!   没有本二进制可证明可靠的 descendant supervisor，因此在 shell spawn 前 fail closed。

use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;

/// agent `run` 工具的默认超时（首次 cargo 编译可能偏慢）。
pub const RUN_TIMEOUT: Duration = Duration::from_secs(240);
/// 评分命令的超时。
pub const GRADE_TIMEOUT: Duration = Duration::from_secs(300);
/// stdout/stderr each retain only this many trailing bytes while the pipes are drained.
const OUTPUT_CAPTURE_BYTES: usize = 16 * 1024;
const UNSUPPORTED_SHELL_MESSAGE: &str = "拒绝宿主 shell 执行：此平台没有评测器可证明可靠的 descendant supervisor；Unix process group 可被 setsid/新 session 绕过。请在 Windows Job Object 路径运行，或在评测器外提供并审计真正的隔离/监督器后再实现对应后端";

/// Whether this build has a process-containment primitive strong enough for arbitrary shell
/// descendants. Windows Job Objects are mandatory; there is deliberately no Unix process-group
/// fallback because a child can call `setsid` without using a recognizable launcher string.
pub fn shell_execution_supported() -> bool {
    cfg!(windows)
}

pub fn shell_containment_mode() -> &'static str {
    if cfg!(windows) {
        "windows_job_object_kill_on_close_required"
    } else {
        "shell_refused_no_reliable_descendant_supervisor"
    }
}

pub fn shell_execution_refusal_reason() -> Option<&'static str> {
    (!shell_execution_supported()).then_some(UNSUPPORTED_SHELL_MESSAGE)
}

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
    /// 控制组若在 trial 工作区顶层观察到可由 Windows 当前目录解析到的 rayman wrapper，
    /// 记录本次 trial 的组别隔离已不可判定：wrapper 已经落盘，评测器无法证明它没有被
    /// 用到。只有这一类**已经发生的事实**会作废整轮 trial。
    ///
    /// 被拒绝的单条命令不在此登记：它根本没有交给 `cmd`，隔离并未丢失，作废整轮只会
    /// 让 Error 系统性偏向控制组并污染 A/B 比较。Arc 让 agent 工具循环和 run_trial
    /// 共享同一事实；OnceLock 记住首个原因，供报告如实描述。
    control_isolation_loss: Option<Arc<OnceLock<String>>>,
}

impl EnvPolicy {
    pub fn with_rayman(dir: PathBuf) -> Self {
        Self {
            extra_path: Some(dir),
            exclude_rayman: false,
            control_isolation_loss: None,
        }
    }

    pub fn without_rayman() -> Self {
        Self {
            extra_path: None,
            exclude_rayman: true,
            control_isolation_loss: Some(Arc::new(OnceLock::new())),
        }
    }

    fn check_control_workspace(&self, workspace: &Path) -> Result<(), String> {
        if !self.exclude_rayman {
            return Ok(());
        }
        if let Err(error) = ensure_no_top_level_rayman_command(workspace) {
            if let Some(slot) = &self.control_isolation_loss {
                let _ = slot.set(error.clone());
            }
            return Err(error);
        }
        Ok(())
    }

    /// `cmd` permits a single compound command to create, invoke, and remove a CWD
    /// wrapper before the postflight scan sees it, and it rewrites the command text
    /// (percent expansion, caret escapes) before tokenizing. The control arm therefore
    /// classifies the command text and refuses anything it cannot prove stays away from
    /// a rayman executable. It remains a narrow integrity guard, not a shell sandbox.
    ///
    /// A refused command never reaches `cmd`, so the arm is still isolated afterwards.
    /// The refusal is reported to the agent as a failed tool call and deliberately does
    /// **not** mark the trial as having lost isolation: only a rayman wrapper actually
    /// observed inside the control workspace does that.
    fn check_control_command(&self, command: &str, workspace: &Path) -> Result<(), String> {
        if !self.exclude_rayman {
            return Ok(());
        }
        match classify_control_command(command, self, workspace) {
            ControlCommandVerdict::Allowed => Ok(()),
            ControlCommandVerdict::NamesRayman { evidence } => Err(format!(
                "控制组拒绝执行该命令：cmd 解析后会出现 rayman 命令/包装器名称（{evidence}）。\
                 命令未执行，本 trial 的组别隔离未受影响"
            )),
            ControlCommandVerdict::UnresolvableExpansion { evidence } => Err(format!(
                "控制组拒绝执行该命令：命令使用了评测器无法解析为字面量的 cmd 变量替换语法\
                 （{evidence}），因此无法证明它不会展开出 rayman。命令未执行，本 trial 的组别隔离未受影响"
            )),
            ControlCommandVerdict::DelayedExpansion { evidence } => Err(format!(
                "控制组拒绝执行该命令：命令显式启用了 cmd 延迟展开（{evidence}）并使用了 `!` 引用。\
                 `!VAR!` 的取值到执行时才确定，可由同一行更早的 `set` 拼出 rayman，评测器不归约它。\
                 命令未执行，本 trial 的组别隔离未受影响"
            )),
        }
    }

    /// 本 trial 是否已观察到组别隔离丢失，附带首个原因文本。
    pub fn control_isolation_loss(&self) -> Option<String> {
        self.control_isolation_loss
            .as_ref()
            .and_then(|slot| slot.get().cloned())
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
    let mut captures = ThreadCaptureFactory;
    run_shell_with_capture_factory(workspace, command, env, timeout, &mut captures)
}

fn run_shell_with_capture_factory(
    workspace: &Path,
    command: &str,
    env: &EnvPolicy,
    timeout: Duration,
    captures: &mut dyn CaptureFactory,
) -> GradeResult {
    if let Some(reason) = shell_execution_refusal_reason() {
        return exec_error(command, reason);
    }
    // `cmd /C rayman` searches the current directory before PATH. PATH filtering alone
    // cannot protect the control arm from a fixture/agent-provided rayman.cmd/.bat/.exe.
    // This is a narrow experiment-integrity preflight, not a sandbox claim.
    if let Err(error) = env.check_control_command(command, workspace) {
        return exec_error(command, &error);
    }
    if let Err(error) = env.check_control_workspace(workspace) {
        return exec_error(command, &error);
    }
    if let Err(error) = reject_uncontained_launch(command) {
        return exec_error(command, &error);
    }
    let mut cmd = shell(command);
    cmd.current_dir(workspace);
    apply_env(&mut cmd, env);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (mut child, containment) = match spawn_contained(&mut cmd) {
        Ok(spawned) => spawned,
        Err(error) => return exec_error(command, &format!("无法执行命令 `{command}`: {error}")),
    };
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let cleanup_error = terminate_drop_and_reap(child, containment);
            return exec_error(
                command,
                &with_cleanup_error("无法捕获命令 stdout".into(), cleanup_error),
            );
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            drop(stdout);
            let cleanup_error = terminate_drop_and_reap(child, containment);
            return exec_error(
                command,
                &with_cleanup_error("无法捕获命令 stderr".into(), cleanup_error),
            );
        }
    };
    let stdout_capture = match captures.spawn(Box::new(stdout), "stdout") {
        Ok(capture) => capture,
        Err(error) => {
            let cleanup_error = terminate_drop_and_reap(child, containment);
            return exec_error(
                command,
                &with_cleanup_error(format!("无法启动 stdout 捕获线程: {error}"), cleanup_error),
            );
        }
    };
    let stderr_capture = match captures.spawn(Box::new(stderr), "stderr") {
        Ok(capture) => capture,
        Err(error) => {
            // Do not join an already-running reader while a descendant can still retain its
            // inherited pipe. Explicit termination, Job drop, and Child drop must all happen
            // first; otherwise a capture-thread creation failure can hang this error path.
            let cleanup_error = terminate_drop_and_reap(child, containment);
            let reader_error = stdout_capture.finish().err();
            let mut message =
                with_cleanup_error(format!("无法启动 stderr 捕获线程: {error}"), cleanup_error);
            if let Some(reader_error) = reader_error {
                message.push_str(&format!("; 已有 stdout 捕获线程失败: {reader_error}"));
            }
            return exec_error(command, &message);
        }
    };

    let start = Instant::now();
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.code().unwrap_or(-1)),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    break Ok(-1);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => break Err(format!("等待命令失败: {error}")),
        }
    };

    // Always terminate the containment boundary, even after a successful/non-zero root
    // exit. A shell can otherwise return while background descendants retain pipe/file
    // handles and mutate later trials. The mandatory Windows Job Object makes cleanup a normal
    // postcondition rather than a timeout-only best effort.
    // Closing the Windows job with KILL_ON_JOB_CLOSE is an independent backstop if the
    // explicit termination API reported an error. The Child is also reaped and dropped before
    // joining pipe readers so no evaluator-owned process/pipe state can extend their lifetime.
    let cleanup_error = terminate_drop_and_reap(child, containment);

    let stdout_result = stdout_capture.finish();
    let stderr_result = stderr_capture.finish();
    let stdout = match stdout_result {
        Ok(output) => output,
        Err(error) => return exec_error(command, &format!("读取 stdout 失败: {error}")),
    };
    let mut stderr = match stderr_result {
        Ok(output) => output,
        Err(error) => return exec_error(command, &format!("读取 stderr 失败: {error}")),
    };
    if let Some(error) = cleanup_error {
        return exec_error(
            command,
            &format!("无法确认命令进程树已清理: {error}; stderr: {stderr}"),
        );
    }
    let exit = match exit {
        Ok(exit) => exit,
        Err(error) => return exec_error(command, &format!("{error}; stderr: {stderr}")),
    };
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
        cmd
    }
}

/// Terminate and drop every process-lifetime guard before any reader join. This ordering is a
/// safety invariant, not cleanup cosmetics: a descendant retaining stdout/stderr would keep a
/// reader blocked if Job drop were delayed until function return.
fn terminate_drop_and_reap(
    mut child: Child,
    mut containment: ProcessContainment,
) -> Option<io::Error> {
    let cleanup_error = containment.terminate().err();
    drop(containment);
    let _ = child.kill();
    let _ = child.wait();
    drop(child);
    cleanup_error
}

fn with_cleanup_error(mut message: String, cleanup_error: Option<io::Error>) -> String {
    if let Some(error) = cleanup_error {
        message.push_str(&format!("; containment cleanup reported: {error}"));
    }
    message
}

/// Reject launchers whose purpose is to move work outside the descendant boundary.
/// Ordinary background jobs remain allowed only on Windows because the mandatory Job Object
/// owns and terminates them after every root exit. Schedulers/services would create work outside
/// that boundary, so recognizable launchers are additionally rejected before the shell.
#[cfg(windows)]
fn reject_uncontained_launch(command: &str) -> Result<(), String> {
    let mut forbidden = None;
    'scan: for token in cmd_tokens(command) {
        for candidate in command_name_candidates(token) {
            let base = candidate.to_ascii_lowercase();
            if matches!(
                base.as_str(),
                "schtasks"
                    | "schtasks.exe"
                    | "wmic"
                    | "wmic.exe"
                    | "sc"
                    | "sc.exe"
                    | "at"
                    | "at.exe"
            ) {
                forbidden = Some(base);
                break 'scan;
            }
        }
    }
    match forbidden {
        Some(launcher) => Err(format!(
            "拒绝可将进程移出评测进程树的后台/调度启动器 `{launcher}`"
        )),
        None => Ok(()),
    }
}

#[cfg(not(windows))]
fn reject_uncontained_launch(_command: &str) -> Result<(), String> {
    // `run_shell_with_capture_factory` refuses this platform before reaching this guard.
    Ok(())
}

fn spawn_contained(cmd: &mut Command) -> io::Result<(Child, ProcessContainment)> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        let job = WindowsJob::new()?;
        // Suspend before any shell byte executes, then bind the process to the job. This
        // closes the spawn->AssignProcessToJobObject race in which a fast shell could
        // otherwise create an untracked child first.
        cmd.creation_flags(CREATE_SUSPENDED);
        let mut child = cmd.spawn()?;
        if let Err(error) = job.assign_and_resume(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((
            child,
            ProcessContainment {
                job,
                terminated: false,
            },
        ))
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            UNSUPPORTED_SHELL_MESSAGE,
        ))
    }
}

struct ProcessContainment {
    #[cfg(windows)]
    job: WindowsJob,
    terminated: bool,
}

impl ProcessContainment {
    fn terminate(&mut self) -> io::Result<()> {
        if self.terminated {
            return Ok(());
        }
        #[cfg(windows)]
        self.job.terminate()?;
        self.terminated = true;
        Ok(())
    }
}

impl Drop for ProcessContainment {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

#[cfg(windows)]
struct WindowsJob {
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl WindowsJob {
    fn new() -> io::Result<Self> {
        use std::mem::size_of;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        // SAFETY: null security/name pointers request a private job with default security.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self { handle };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: handle is a live job and the information pointer/length match the class.
        let configured = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        // SAFETY: both handles remain owned/live for the duration of this call.
        let assigned =
            unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle().cast()) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        resume_suspended_process(child.id())
    }

    fn terminate(&self) -> io::Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        // SAFETY: handle is a live private job owned by self.
        if unsafe { TerminateJobObject(self.handle, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;

        // KILL_ON_JOB_CLOSE is the panic/early-return backstop for every assigned child.
        // SAFETY: handle was returned by CreateJobObjectW and is closed exactly once.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(windows)]
fn resume_suspended_process(pid: u32) -> io::Result<()> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    // SAFETY: flags/pid follow CreateToolhelp32Snapshot's documented contract.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut resumed = false;
    // SAFETY: snapshot is live and entry points to a correctly-sized writable structure.
    let mut has_entry = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
    while has_entry {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: the thread id came from the live system snapshot.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                // SAFETY: snapshot is still live and owned by this function.
                let _ = unsafe { CloseHandle(snapshot) };
                return Err(io::Error::last_os_error());
            }
            // SAFETY: thread is a live handle opened with THREAD_SUSPEND_RESUME.
            let previous = unsafe { ResumeThread(thread) };
            // SAFETY: thread is closed exactly once after ResumeThread.
            let _ = unsafe { CloseHandle(thread) };
            if previous == u32::MAX {
                // SAFETY: snapshot is still live and owned by this function.
                let _ = unsafe { CloseHandle(snapshot) };
                return Err(io::Error::last_os_error());
            }
            resumed = true;
            break;
        }
        // SAFETY: snapshot/entry remain valid for the next enumeration step.
        has_entry = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
    }
    // SAFETY: snapshot is closed exactly once after enumeration.
    let _ = unsafe { CloseHandle(snapshot) };
    if !resumed {
        return Err(io::Error::other(format!(
            "找不到 suspended child {pid} 的主线程"
        )));
    }
    Ok(())
}

struct BoundedCapture {
    thread: JoinHandle<io::Result<BoundedBytes>>,
}

trait CaptureFactory {
    fn spawn(
        &mut self,
        reader: Box<dyn Read + Send>,
        stream: &'static str,
    ) -> io::Result<BoundedCapture>;
}

struct ThreadCaptureFactory;

impl CaptureFactory for ThreadCaptureFactory {
    fn spawn(
        &mut self,
        reader: Box<dyn Read + Send>,
        stream: &'static str,
    ) -> io::Result<BoundedCapture> {
        BoundedCapture::spawn(reader, stream)
    }
}

impl BoundedCapture {
    fn spawn(reader: impl Read + Send + 'static, stream: &'static str) -> io::Result<Self> {
        let thread = std::thread::Builder::new()
            .name(format!("rayman-eval-{stream}"))
            .spawn(move || read_bounded(reader))?;
        Ok(Self { thread })
    }

    fn finish(self) -> io::Result<String> {
        self.thread
            .join()
            .map_err(|_| io::Error::other("输出捕获线程 panic"))?
            .map(BoundedBytes::into_string)
    }
}

struct BoundedBytes {
    tail: Vec<u8>,
    truncated: bool,
}

impl BoundedBytes {
    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= OUTPUT_CAPTURE_BYTES {
            self.tail.clear();
            self.tail
                .extend_from_slice(&bytes[bytes.len() - OUTPUT_CAPTURE_BYTES..]);
            self.truncated = true;
            return;
        }
        let overflow = self
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(OUTPUT_CAPTURE_BYTES);
        if overflow > 0 {
            self.tail.drain(..overflow);
            self.truncated = true;
        }
        self.tail.extend_from_slice(bytes);
    }

    fn into_string(self) -> String {
        let tail = String::from_utf8_lossy(&self.tail);
        if self.truncated {
            format!("…(stream truncated to last {OUTPUT_CAPTURE_BYTES} bytes)\n{tail}")
        } else {
            tail.into_owned()
        }
    }
}

fn read_bounded(mut reader: impl Read) -> io::Result<BoundedBytes> {
    let mut capture = BoundedBytes {
        tail: Vec::with_capacity(OUTPUT_CAPTURE_BYTES),
        truncated: false,
    };
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        capture.push(&chunk[..read]);
    }
    Ok(capture)
}

/// 清空继承环境，只透传白名单变量；PATH 单独按策略重组后写入。
fn apply_env(cmd: &mut Command, policy: &EnvPolicy) {
    cmd.env_clear();
    for (key, value) in child_env(policy) {
        cmd.env(key, value);
    }
}

/// 子进程实际会收到的完整环境块。控制组的命令文本检查用同一份数据解析 `%VAR%`，
/// 这样“命令展开后会不会指向 rayman”与子进程真实行为基于同一事实，而不是猜测。
fn child_env(policy: &EnvPolicy) -> Vec<(OsString, OsString)> {
    let mut vars = Vec::new();
    let mut parent_path = OsString::new();
    for (key, value) in std::env::vars_os() {
        let Some(name) = key.to_str() else { continue };
        if name.eq_ignore_ascii_case("PATH") {
            parent_path = value;
            continue;
        }
        if env_allowed(name) {
            vars.push((key, value));
        }
    }
    vars.push((OsString::from("PATH"), compose_path(&parent_path, policy)));
    vars
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

#[cfg(windows)]
fn is_rayman_command_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name.eq_ignore_ascii_case("rayman")
        || name
            .get(.."rayman.".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("rayman."))
}

/// 控制组对一条命令文本的判定。
///
/// 分成四类而不是二分，是因为“命令确实指向 rayman”“命令用了评测器读不懂的展开语法”
/// “命令显式打开了延迟展开”是不同的事实，错误文案必须如实区分；三者都只拒绝该条命令，
/// 都不作废整轮 trial。
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlCommandVerdict {
    /// 没有任何 token 能命名 rayman 可执行文件/包装器。
    Allowed,
    /// 原文、去插入符文本或按子进程环境展开后的文本里出现了命名 rayman 的 token。
    NamesRayman { evidence: String },
    /// 命令使用了评测器无法归约为字面量的 cmd 变量替换语法，无法证明它不触达 rayman。
    UnresolvableExpansion { evidence: String },
    /// 命令显式打开了 cmd 延迟展开且含 `!`。`!VAR!` 在执行那一刻才取值，可以由同一行更早的
    /// `set` 拼出任意命令名，评测器不归约它。
    DelayedExpansion { evidence: String },
}

/// Classify one control-arm command by the text `cmd` would actually execute.
///
/// **This is not a complete model of `cmd`, and no such claim should be made for it.** An
/// earlier revision of this comment asserted it covered "every reachable spelling" and named
/// the only residual gap as "a lone `%`". That was false, and an audit counted the false
/// completeness claim itself as the defect: `cmd /V:ON /C "set p=…&set q=…&!p!!q!"` was
/// classified `Allowed` and really did execute the installed rayman. What follows is the
/// honest inventory.
///
/// Modeled, because `cmd` rewrites the command line before tokenizing it:
/// - caret escapes are removed (`^r^a^y^m^a^n`);
/// - `%VAR%` is resolved against the exact environment the child would receive, so a
///   reference that names rayman, or whose *value* does, is refused;
/// - `%VAR:…%` substring/replace syntax is refused as unresolvable, since it can synthesize
///   an arbitrary string from an arbitrary value;
/// - every resulting spelling is tokenized with the shared `cmd` delimiter set
///   (`CMD_NAME_TERMINATORS` / `strip_cmd_token_prefixes` / `command_name_candidates`),
///   each measured character-by-character against a real `cmd`;
/// - delayed expansion is refused rather than modeled — see `delayed_expansion_switch`.
///   Reducing `!VAR!` would mean simulating `set` assignments and `cmd`'s two-phase
///   expansion order across a compound line, i.e. rebuilding an interpreter; that is exactly
///   the unbounded modeling that produced the false completeness claim in the first place.
///   Delayed expansion is off by default, so its only command-line-reachable form carries an
///   explicit `/V` opt-in, which is cheap and exact to detect.
///
/// Known and deliberately accepted gaps — the guard allows these:
/// - a lone `%` (a `for` loop variable, a literal percent) cannot be reduced to a literal,
///   because its value only exists at runtime;
/// - the *contents* of a script the agent writes and then invokes are never inspected. The
///   guard sees `build.cmd`, not what is inside it, and a batch file may enable delayed
///   expansion and assemble any name it likes;
/// - any route that reaches rayman without naming it in the command text at all — for
///   example a program that resolves it on its own.
///
/// Refusals are kept narrow on purpose. An earlier revision rejected any command containing
/// `%` or `^`, which made ordinary syntax (`echo %CD%`, `if %ERRORLEVEL% neq 0 …`,
/// `for /f %i in (…)`) unusable for the control arm only. That is a measurement bug: those
/// refusals invalidated whole trials and biased errors toward one arm.
///
/// So this is one layer of an integrity guard, not a shell sandbox. It is combined with
/// rayman removed from the control arm's `PATH`, a fail-closed scan for any rayman-named
/// command in the trial CWD before every spawn, and a rescan after the agent finishes.
#[cfg(windows)]
fn classify_control_command(
    command: &str,
    policy: &EnvPolicy,
    workspace: &Path,
) -> ControlCommandVerdict {
    let env = child_env(policy);
    let decaretted = strip_caret_escapes(command);
    let mut variants = vec![command.to_string(), decaretted.clone()];
    for source in [command, decaretted.as_str()] {
        match expand_percent_references(source, &env, workspace) {
            Ok(expanded) => {
                variants.push(strip_caret_escapes(&expanded));
                variants.push(expanded);
            }
            Err(verdict) => return verdict,
        }
    }
    for variant in &variants {
        if let Some(token) = rayman_naming_token(variant) {
            return ControlCommandVerdict::NamesRayman {
                evidence: format!("token `{token}`"),
            };
        }
    }
    // `!` 与开关都出现才算证据：单独的 `!` 在 `cmd /C` 下是字面量，单独的 `/V:ON` 没有可展开
    // 的引用。两者都要求，良性的 `echo Done!` 与不含 `!` 的 `/V:ON` 才不会被误拒。
    for variant in &variants {
        if !variant.contains('!') {
            continue;
        }
        if let Some(switch) = delayed_expansion_switch(variant) {
            return ControlCommandVerdict::DelayedExpansion {
                evidence: format!("开关 `{switch}` 与 `!` 引用同时出现"),
            };
        }
    }
    ControlCommandVerdict::Allowed
}

#[cfg(not(windows))]
fn classify_control_command(
    _command: &str,
    _policy: &EnvPolicy,
    _workspace: &Path,
) -> ControlCommandVerdict {
    // Non-Windows shells do not search CWD for a bare command and this platform refuses
    // shell execution before the guard runs; there is nothing to normalize here.
    ControlCommandVerdict::Allowed
}

/// `cmd` 会在这些字符处结束一个命令名 token。
///
/// 取值来自在本机 cmd 上对 ASCII 0x01–0x7F 逐字符实测（前导位置与终止位置各扫一轮），
/// 不是凭印象：除空白外（`char::is_whitespace` 已覆盖 TAB/LF/VT/FF/CR/SPACE，实测四种控制
/// 字符都确实分隔），`cmd` 在 `&` `|` `;` `<` `>` `(` `)` `=` `,` 处结束命令名。
///
/// `,` 曾只写进 `reject_uncontained_launch` 而漏在 `rayman_naming_token` 之外，于是
/// `,rayman` 整体成为一个 token 并绕过检测。这正是本常量存在的理由：同一语义只允许有一份
/// 定义，两处守卫必须切分得一模一样。
#[cfg(windows)]
const CMD_NAME_TERMINATORS: &[char] = &['&', '|', ';', '<', '>', '(', ')', '=', ','];

/// `cmd` 在命令名之前会吃掉的字符：引号，以及 `@`。
///
/// 实测 `@rayman`、`@@rayman`、`@"rayman"` 都会执行 rayman，而 `rayman@x` **不会**——`@`
/// 只是前缀，不是分隔符，所以只从头部剥离。`^` 不在此列：它由 `strip_caret_escapes` 生成的
/// 候选变体覆盖。
#[cfg(windows)]
fn strip_cmd_token_prefixes(token: &str) -> &str {
    let quotes = |ch: char| matches!(ch, '\'' | '"' | '`');
    token
        .trim_matches(quotes)
        .trim_start_matches('@')
        .trim_matches(quotes)
}

/// 按 `cmd` 的命令名切分规则把命令文本拆成 token。两处守卫共用这一份实现。
#[cfg(windows)]
fn cmd_tokens(command: &str) -> impl Iterator<Item = &str> {
    command
        .split(|ch: char| ch.is_whitespace() || CMD_NAME_TERMINATORS.contains(&ch))
        .map(strip_cmd_token_prefixes)
        .filter(|token| !token.is_empty())
}

/// 一个 token 可能解析成的命令名候选。
///
/// `/` 的行为依赖上下文，实测两种都存在：`sub/probe` 执行的是 `sub`（`/` 结束了命令名，
/// 余下部分成为参数 `/probe`），而 `C:\dir/probe` 执行的是 `C:\dir\probe`（token 已经是
/// 路径时整体解析）。因此两种读法都必须检查——只查 basename 会漏掉 `rayman/x`，只查第一个
/// `/` 之前的部分会漏掉 `C:\dir/rayman`。
///
/// 把 `/` 直接并入 `CMD_NAME_TERMINATORS` 也能堵住 `rayman/x`，但那会让 `type
/// crates/rayman/src/main.rs` 这类纯参数路径被误判为提及 rayman；控制组的误拒会系统性地
/// 把错误偏向一侧，正是本守卫要避免的。
#[cfg(windows)]
fn command_name_candidates(token: &str) -> [&str; 2] {
    let basename = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let before_first_slash = token.split('/').next().unwrap_or(token);
    [basename, before_first_slash]
}

/// 命令里第一个能命名 rayman 可执行文件/包装器的 token（按 `cmd` 的分隔符切分）。
#[cfg(windows)]
fn rayman_naming_token(command: &str) -> Option<&str> {
    cmd_tokens(command).find(|token| {
        command_name_candidates(token)
            .into_iter()
            .any(|name| is_rayman_command_name(OsStr::new(name)))
    })
}

/// 命令是否显式打开了 cmd 延迟展开，若是则返回作为证据的开关写法。
///
/// 实测事实：`cmd /C` 下延迟展开**默认关闭**，`!p!` 只是字面量；命令行上唯一能打开它的写法
/// 是给嵌套 `cmd` 传 `/V` 或 `/V:ON`（`/v:on` 等价，`/V:OFF` 不打开，`-V:ON` 无效）。
/// 命令行上的 `setlocal enabledelayedexpansion` 实测**不**生效（只在批处理脚本内部生效），
/// 但仍一并识别：它出现在命令文本里时，意图明确且代价为零。
///
/// 这里不做边界扫描以外的解析：只要文本里出现 `/v` 开关且该 token 不是 `/v:off`，就算打开。
/// 不去判断这个 `/v` 是否真的挂在某个 `cmd` 上——那需要再引入一层拼写建模（`%COMSPEC%`、
/// `c^m^d`、`start cmd …`），只会重新制造本次审计要消灭的那种“看起来完备”的解析。
/// 代价是一处已知误拒：既用 `/V` 当开关、又含字面 `!` 的命令（如 `find /v "x" & echo hi!`）
/// 会被拒。调用方只在文本同时含 `!` 时才查开关，所以 `echo Done!` 这类命令不受影响；
/// 且被拒的单条命令不作废整轮 trial。
#[cfg(windows)]
fn delayed_expansion_switch(text: &str) -> Option<String> {
    let lowered = text.to_ascii_lowercase();
    if lowered.contains("enabledelayedexpansion") {
        return Some("setlocal enabledelayedexpansion".to_string());
    }
    let mut cursor = 0;
    while let Some(offset) = lowered[cursor..].find("/v") {
        let start = cursor + offset;
        let rest = &lowered[start + "/v".len()..];
        if let Some(tail) = rest.strip_prefix(':') {
            let value: String = tail
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric())
                .collect();
            if value != "off" {
                return Some(format!("/v:{value}"));
            }
        } else if !rest.starts_with(|ch: char| ch.is_ascii_alphanumeric()) {
            return Some("/v".to_string());
        }
        cursor = start + "/v".len();
    }
    None
}

/// `cmd` 会吃掉未被引号保护的 `^` 并原样保留其后一个字符，因此 `^r^a^y^m^a^n` 会执行到
/// `rayman`。检测时对整段文本无条件去插入符：这只会多产生一个候选拼写，不会掩盖任何写法，
/// 因为原文同样参与扫描。
#[cfg(windows)]
fn strip_caret_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '^' {
            if let Some(escaped) = chars.next() {
                out.push(escaped);
            }
            continue;
        }
        out.push(ch);
    }
    out
}

/// 把 `%VAR%` 引用替换成子进程真实会看到的取值，让 token 扫描看到 `cmd` 实际执行的文本。
///
/// - 引用名本身命名 rayman（`%RAYMAN%`、`%rayman.exe%`）→ 直接判为指向 rayman。
/// - `%VAR:...%`（子串/替换语法）能从任意变量值里拼出任意字符串，评测器不归约它 → 判为不可解析。
/// - 已定义变量 → 代入真实取值（控制组的 PATH 已剔除 rayman 目录）。
/// - 未定义变量 → 代入空串。`cmd` 实际会原样保留 `%FOO%`，而原文变体已经覆盖了那种拼写；
///   折叠成空串只会多命中（更保守），不会漏掉任何拼写。
/// - 落单的 `%`（`for /f %i`、字面百分号）→ 原样保留，不视为可疑。
#[cfg(windows)]
fn expand_percent_references(
    text: &str,
    env: &[(OsString, OsString)],
    workspace: &Path,
) -> Result<String, ControlCommandVerdict> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '%' {
            out.push(chars[index]);
            index += 1;
            continue;
        }
        let closing = chars[index + 1..]
            .iter()
            .position(|&ch| ch == '%')
            .map(|offset| index + 1 + offset);
        let Some(end) = closing else {
            // 落单的 `%`：`cmd` 不会把它当环境变量引用，原样保留供 token 扫描。
            out.push('%');
            index += 1;
            continue;
        };
        let name: String = chars[index + 1..end].iter().collect();
        if name.is_empty() {
            // `%%` 在命令行上就是一个字面百分号。
            out.push('%');
            index = end + 1;
            continue;
        }
        if is_rayman_command_name(OsStr::new(name.trim())) {
            return Err(ControlCommandVerdict::NamesRayman {
                evidence: format!("变量引用 `%{name}%`"),
            });
        }
        if name.contains(':') {
            return Err(ControlCommandVerdict::UnresolvableExpansion {
                evidence: format!("`%{name}%`"),
            });
        }
        if let Some(value) = child_env_value(&name, env, workspace) {
            out.push_str(&value);
        }
        index = end + 1;
    }
    Ok(out)
}

/// 子进程里 `%NAME%` 的取值：白名单环境块，外加 `cmd` 自行解析、评测器已知的动态变量。
#[cfg(windows)]
fn child_env_value(name: &str, env: &[(OsString, OsString)], workspace: &Path) -> Option<String> {
    if name.eq_ignore_ascii_case("CD") {
        return Some(workspace.display().to_string());
    }
    env.iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn run_shell_captures_success_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        let ok = run_shell(dir.path(), "exit 0", &EnvPolicy::default(), RUN_TIMEOUT);
        assert_eq!(ok.outcome, GradeOutcome::Passed);
        let bad = run_shell(dir.path(), "exit 3", &EnvPolicy::default(), RUN_TIMEOUT);
        assert_eq!(bad.outcome, GradeOutcome::Failed);
        assert_eq!(bad.exit, 3);
    }

    #[cfg(windows)]
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

    #[cfg(windows)]
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
        assert!(result.stderr.contains("控制组拒绝"), "{}", result.stderr);
        assert!(!dir.path().join("invoked.txt").exists());
    }

    /// 工作区里真的落了一个 rayman wrapper：这是**已经发生**的隔离事实，必须 latch 到
    /// 整轮，即使触发它的那条命令同时被拒绝。
    #[cfg(windows)]
    #[test]
    fn control_latches_isolation_loss_only_for_a_wrapper_present_in_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rayman.cmd"), "@echo off\r\n").unwrap();
        let policy = EnvPolicy::without_rayman();

        let result = run_shell(dir.path(), "dir", &policy, RUN_TIMEOUT);

        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        let loss = policy
            .control_isolation_loss()
            .expect("落盘的 wrapper 必须记入本轮隔离丢失");
        assert!(loss.contains("rayman"), "{loss}");
    }

    #[cfg(windows)]
    #[test]
    fn control_rejects_compound_create_invoke_delete_wrapper_command() {
        let dir = tempfile::tempdir().unwrap();
        let policy = EnvPolicy::without_rayman();
        let command = "echo @echo off > rayman.cmd & rayman & del rayman.cmd";

        let result = run_shell(dir.path(), command, &policy, RUN_TIMEOUT);

        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert!(
            result.stderr.contains("rayman 命令/包装器"),
            "{}",
            result.stderr
        );
        assert!(
            !dir.path().join("rayman.cmd").exists(),
            "command must be rejected before cmd can create the wrapper"
        );
        // 命令没有执行过，隔离没有丢，因此不作废整轮 trial。
        assert_eq!(policy.control_isolation_loss(), None);
    }

    #[cfg(windows)]
    #[test]
    fn control_rejects_variable_and_caret_hidden_rayman_commands() {
        for command in [
            "set x=rayman & %x%",
            "^r^a^y^m^a^n",
            "%RAYMAN%",
            "%rayman.exe% --version",
            "ray^man.cmd",
            // 子串/替换语法能从任意变量值里拼出任意字符串，评测器不归约它。
            "%COMSPEC:~0,6%",
            "%COMSPEC:cmd=rayman%",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let policy = EnvPolicy::without_rayman();
            let result = run_shell(dir.path(), command, &policy, RUN_TIMEOUT);
            assert_eq!(
                result.outcome,
                GradeOutcome::InfrastructureError,
                "`{command}` 必须被拒绝"
            );
            assert!(
                result.stderr.contains("控制组拒绝执行该命令"),
                "`{command}`: {}",
                result.stderr
            );
            // 拒绝单条命令不等于整轮作废。
            assert_eq!(policy.control_isolation_loss(), None, "`{command}`");
        }
    }

    /// 回归：`cmd` 在 `,` 和 `/` 处结束命令名，并吃掉命令名之前的 `@`（三者都在本机 cmd 上
    /// 逐字符实测确认，见 `CMD_NAME_TERMINATORS` / `CMD_NAME_PREFIXES` 的注释）。
    ///
    /// 曾经 `rayman_naming_token` 的切分集漏了 `,`，于是 `,rayman` 整体成为一个 token、
    /// basename 仍是 `,rayman`，命令被判 Allowed 并真的执行了当前目录里的 wrapper。创建—
    /// 调用—删除发生在同一条复合命令里：spawn 前扫描时文件还不存在，postflight 扫描时它已被
    /// 删除，因此这次逃逸从不 latch `control_isolation_loss`，该 trial 照常作为合法 control
    /// 观测计入 pass/fail 与 observed_delta。
    #[cfg(windows)]
    #[test]
    fn control_rejects_rayman_after_every_cmd_command_name_delimiter() {
        for command in [
            // 实测可逃逸的完整“创建—调用—删除”复合命令。
            "echo @echo off > rayman.cmd &,rayman & del rayman.cmd",
            "echo @echo off > rayman.cmd & @rayman & del rayman.cmd",
            "echo @echo off > rayman.cmd & rayman/x & del rayman.cmd",
            ",rayman",
            ";rayman",
            "=rayman",
            "@rayman",
            "@@rayman",
            "@,rayman",
            "@\"rayman\"",
            "rayman/?",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let policy = EnvPolicy::without_rayman();

            let result = run_shell(dir.path(), command, &policy, RUN_TIMEOUT);

            assert_eq!(
                result.outcome,
                GradeOutcome::InfrastructureError,
                "`{command}` 必须被拒绝"
            );
            assert!(
                result.stderr.contains("rayman 命令/包装器"),
                "`{command}`: {}",
                result.stderr
            );
            assert!(
                !dir.path().join("rayman.cmd").exists(),
                "`{command}`: 必须在 cmd 能创建 wrapper 之前就拒绝"
            );
            // 拒绝单条命令不等于整轮作废。
            assert_eq!(policy.control_isolation_loss(), None, "`{command}`");
        }
    }

    /// 切分语义曾在两处各写一遍，只有 `reject_uncontained_launch` 那份含 `,`。现在两处共用
    /// 同一份定义，因此调度器守卫必须认出与 rayman token 扫描完全相同的写法。
    #[cfg(windows)]
    #[test]
    fn uncontained_launch_guard_shares_the_delimiter_set_with_the_rayman_token_scan() {
        for command in [
            ",schtasks /create /tn x",
            "@schtasks /create /tn x",
            "echo x&,wmic process call create calc",
            "@sc start something",
            "echo x & at 10:00 calc",
        ] {
            assert!(
                reject_uncontained_launch(command).is_err(),
                "`{command}` 必须被拒绝"
            );
        }
        assert!(reject_uncontained_launch("echo scheduled backup").is_ok());
    }

    /// 回归：`!VAR!` 延迟展开在 `cmd /C` 下默认关闭，命令行上唯一能打开它的写法是对嵌套
    /// `cmd` 显式传 `/V` 或 `/V:ON`（`/v:on` 等价、`-V:ON` 无效，均已实测）。此前该守卫完全
    /// 没有建模延迟展开，`cmd /V:ON /C "set p=…\ray&set q=man.exe&!p!!q! --version"` 被判
    /// Allowed 并真的执行到了本机安装的 rayman。
    #[cfg(windows)]
    #[test]
    fn control_rejects_delayed_expansion_that_can_assemble_a_rayman_name() {
        let dir = tempfile::tempdir().unwrap();
        let policy = EnvPolicy::without_rayman();
        for command in [
            r#"cmd /V:ON /C "set p=ray&set q=man.exe&!p!!q! --version""#,
            r#"cmd /v:on /C "set p=ray&set q=man.exe&!p!!q!""#,
            r#"cmd /V /C "set p=ray&set q=man.exe&!p!!q!""#,
            r#"cmd/V:ON /C "set p=ray&set q=man.exe&!p!!q!""#,
            // 审计里实测打到本机真实 rayman 2.2.0 的原始 repro。
            r#"cmd /V:ON /C "set p=%LOCALAPPDATA%\Rayman\bin\ray&set q=man.exe&!p!!q! --version""#,
        ] {
            assert!(
                matches!(
                    classify_control_command(command, &policy, dir.path()),
                    ControlCommandVerdict::DelayedExpansion { .. }
                ),
                "`{command}` 必须被拒绝，实得 {:?}",
                classify_control_command(command, &policy, dir.path())
            );
        }
    }

    /// 延迟展开的拒绝必须窄：没有显式开关的 `!`、以及没有 `!` 的 `/V:ON`，都不是证据。
    /// 否则良性命令会被作废，正是上一轮修掉的那类测量偏差。
    #[cfg(windows)]
    #[test]
    fn control_allows_bangs_without_the_switch_and_the_switch_without_bangs() {
        let dir = tempfile::tempdir().unwrap();
        let policy = EnvPolicy::without_rayman();
        for command in [
            "echo Done!",
            "echo hello! & echo bye!",
            "echo !not_expanded! stays literal",
            r#"cmd /V:ON /C "echo no bang here""#,
            r#"cmd /V:OFF /C "echo literal !p!""#,
        ] {
            assert_eq!(
                classify_control_command(command, &policy, dir.path()),
                ControlCommandVerdict::Allowed,
                "`{command}` 是良性命令，不该被拒绝"
            );
        }
    }

    /// 变量取值（而不是命令文本）指向 rayman 时也必须拒绝：展开按子进程真实环境解析。
    #[cfg(windows)]
    #[test]
    fn control_rejects_expansion_whose_environment_value_names_rayman() {
        let dir = tempfile::tempdir().unwrap();
        let env = [(
            OsString::from("USERPROFILE"),
            OsString::from("C:\\tools\\rayman.cmd"),
        )];

        let expanded = expand_percent_references("%USERPROFILE% --help", &env, dir.path()).unwrap();

        assert_eq!(expanded, "C:\\tools\\rayman.cmd --help");
        assert_eq!(
            rayman_naming_token(&expanded),
            Some("C:\\tools\\rayman.cmd")
        );
    }

    /// 回归：常见 cmd 语法只是含 `%`/`^`，既不该被拒绝，更不该作废整轮 trial。
    #[cfg(windows)]
    #[test]
    fn control_allows_ordinary_cmd_percent_and_caret_syntax() {
        for command in [
            "echo %CD%",
            "echo building & if %ERRORLEVEL% neq 0 exit /b 1",
            "for /f %i in ('echo ok') do @echo %i",
            "echo 50%% done",
            "echo %UNDEFINED_EVAL_VARIABLE%",
            "echo a^&b",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let policy = EnvPolicy::without_rayman();
            assert_eq!(
                classify_control_command(command, &policy, dir.path()),
                ControlCommandVerdict::Allowed,
                "`{command}` 是普通 cmd 语法，不该被判为提及 rayman"
            );

            let result = run_shell(dir.path(), command, &policy, RUN_TIMEOUT);

            assert_ne!(
                result.outcome,
                GradeOutcome::InfrastructureError,
                "`{command}`: {}",
                result.stderr
            );
            assert_eq!(policy.control_isolation_loss(), None, "`{command}`");
        }
    }

    #[cfg(windows)]
    #[test]
    fn caret_escapes_are_removed_the_way_cmd_removes_them() {
        assert_eq!(strip_caret_escapes("^r^a^y^m^a^n"), "rayman");
        assert_eq!(strip_caret_escapes("echo a^&b"), "echo a&b");
        assert_eq!(strip_caret_escapes("trailing^"), "trailing");
    }

    #[test]
    fn output_capture_is_streamed_and_memory_bounded() {
        let mut input = vec![b'x'; OUTPUT_CAPTURE_BYTES * 32];
        input.extend_from_slice(b"tail-marker");

        let captured = read_bounded(std::io::Cursor::new(input)).unwrap();

        assert!(captured.truncated);
        assert_eq!(captured.tail.len(), OUTPUT_CAPTURE_BYTES);
        assert!(captured.into_string().ends_with("tail-marker"));
    }

    #[cfg(windows)]
    #[test]
    fn background_descendants_are_reaped_after_success_and_failure() {
        let dir = tempfile::tempdir().unwrap();
        for (exit, expected, marker) in [
            (0, GradeOutcome::Passed, "success-leak.txt"),
            (3, GradeOutcome::Failed, "failure-leak.txt"),
        ] {
            let script = format!("delayed-{exit}.cmd");
            std::fs::write(
                dir.path().join(&script),
                format!("@echo off\r\nping 127.0.0.1 -n 3 >NUL\r\necho leaked>{marker}\r\n"),
            )
            .unwrap();
            let command = format!("start /B {script} & exit {exit}");
            let result = run_shell(dir.path(), &command, &EnvPolicy::default(), RUN_TIMEOUT);
            assert_eq!(result.outcome, expected, "{}", result.stderr);
        }
        std::thread::sleep(Duration::from_secs(3));
        assert!(!dir.path().join("success-leak.txt").exists());
        assert!(!dir.path().join("failure-leak.txt").exists());
    }

    #[cfg(not(windows))]
    #[test]
    fn host_shell_is_refused_before_spawn_without_a_reliable_descendant_supervisor() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_shell(
            dir.path(),
            "echo leaked > must-not-exist.txt",
            &EnvPolicy::default(),
            RUN_TIMEOUT,
        );
        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert!(result.stderr.contains("descendant supervisor"));
        assert!(!shell_execution_supported());
        assert_eq!(
            shell_containment_mode(),
            "shell_refused_no_reliable_descendant_supervisor"
        );
        assert!(!dir.path().join("must-not-exist.txt").exists());
    }

    #[cfg(windows)]
    struct FailStderrCaptureFactory;

    #[cfg(windows)]
    impl CaptureFactory for FailStderrCaptureFactory {
        fn spawn(
            &mut self,
            reader: Box<dyn Read + Send>,
            stream: &'static str,
        ) -> io::Result<BoundedCapture> {
            if stream == "stderr" {
                drop(reader);
                Err(io::Error::other("forced stderr capture failure"))
            } else {
                BoundedCapture::spawn(reader, stream)
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn second_capture_thread_failure_drops_job_and_child_before_joining_first_reader() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("delayed-capture.cmd"),
            "@echo off\r\nping 127.0.0.1 -n 3 >NUL\r\necho leaked>capture-leak.txt\r\n",
        )
        .unwrap();
        let mut captures = FailStderrCaptureFactory;
        let start = Instant::now();

        let result = run_shell_with_capture_factory(
            dir.path(),
            "start /B delayed-capture.cmd & exit 0",
            &EnvPolicy::default(),
            RUN_TIMEOUT,
            &mut captures,
        );

        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert!(
            result.stderr.contains("forced stderr capture failure"),
            "{}",
            result.stderr
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "capture failure cleanup must not wait for inherited pipe EOF"
        );
        std::thread::sleep(Duration::from_secs(3));
        assert!(!dir.path().join("capture-leak.txt").exists());
    }

    #[cfg(windows)]
    #[test]
    fn scheduler_launcher_is_rejected_before_shell_start() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_shell(
            dir.path(),
            "schtasks /Create /TN rayman-eval-leak /TR calc.exe /SC ONCE /ST 00:00",
            &EnvPolicy::default(),
            RUN_TIMEOUT,
        );
        assert_eq!(result.outcome, GradeOutcome::InfrastructureError);
        assert!(result.stderr.contains("移出评测进程树"));
    }

    #[cfg(windows)]
    #[test]
    fn run_shell_times_out_and_reports() {
        let dir = tempfile::tempdir().unwrap();
        // 睡 30 秒但只给 1 秒超时 → 应被杀掉并标记失败。
        // The delayed background marker proves timeout cleanup covers descendants too.
        std::fs::write(
            dir.path().join("delayed-timeout.cmd"),
            "@echo off\r\nping 127.0.0.1 -n 3 >NUL\r\necho leaked>timeout-leak.txt\r\n",
        )
        .unwrap();
        let cmd = "start /B delayed-timeout.cmd & ping 127.0.0.1 -n 30 >NUL";
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
        std::thread::sleep(Duration::from_secs(3));
        assert!(!dir.path().join("timeout-leak.txt").exists());
    }

    #[cfg(windows)]
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
