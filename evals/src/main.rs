//! RaymanCodingSkill 的 agent A/B outcome eval。
//!
//! 真实模型会产生宿主 shell 命令。本二进制没有把它们隔离进容器或 Windows Sandbox，
//! 因此真实后端必须由操作者显式确认 `--unsafe-host-exec`；确认后也只有 mandatory
//! Windows Job Object 路径允许 shell spawn。mock 仅对 hash 绑定的内置任务免确认，
//! 第三方 grade.txt 仍需独立的宿主执行确认。

mod agent;
mod anthropic;
mod grade;
mod openai;
mod report;
mod task;

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use agent::{AgentConfig, MockModel, Model, run_agent};
use grade::{EnvPolicy, GradeOutcome};
use report::{
    ArtifactHash, CONTROL, EvalReport, HostProvenance, InputIntegrity, Outcome,
    ReleaseContractProvenance, RunProvenance, TrialResult, WITH_SKILL,
};
use task::TaskProvenance;

const SYSTEM_BASE: &str = "You are an autonomous coding agent working inside a repository. \
Complete the user's task by reading and editing files and running commands with the provided tools. \
Verify your work by running the project's own tests/build. When the task is complete and verified, \
stop by replying with a short summary and no further tool calls. Be efficient.";
static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Parser)]
#[command(
    name = "rayman-evals",
    about = "RaymanCodingSkill agent A/B outcome eval"
)]
struct Cli {
    /// 任务目录（默认 evals/tasks）
    #[arg(long)]
    tasks: Option<PathBuf>,
    /// 只跑某个任务
    #[arg(long)]
    task: Option<String>,
    /// 每个 (任务×组) 的重复次数；至少 1，少于 2 会被报告为不可作因果解释
    #[arg(long, default_value_t = 1)]
    trials: usize,
    /// 后端：`mock`（免费）| `anthropic`（需 ANTHROPIC_API_KEY）| backends.json 里的命名后端
    #[arg(long, default_value = "mock")]
    backend: String,
    /// 覆盖模型 id（不填则用后端默认）
    #[arg(long)]
    model: Option<String>,
    /// OpenAI 兼容后端配置文件（默认 evals/backends.json）
    #[arg(long)]
    backends: Option<PathBuf>,
    /// 技能文件（默认仓库根 SKILL.md）
    #[arg(long)]
    skill: Option<PathBuf>,
    /// 每次尝试的最大步数
    #[arg(long, default_value_t = agent::MAX_STEPS)]
    max_steps: usize,
    /// 工作区与报告输出目录（默认 evals/.runs）；每次运行新建不可复用的 run-* 目录
    #[arg(long)]
    runs_dir: Option<PathBuf>,
    /// 可复现的顺序种子；未指定时生成并写入 report provenance
    #[arg(long)]
    seed: Option<u64>,
    /// 明确确认真实模型生成的 shell 命令将在当前用户的宿主机上直接执行（没有 sandbox）
    #[arg(long)]
    unsafe_host_exec: bool,
    /// 明确确认第三方/已修改任务的 grade.txt 将以当前用户身份在宿主机执行
    #[arg(long)]
    unsafe_custom_grade_exec: bool,
    /// 明确允许 backends.json 使用内联 api_key；默认只允许批准的 api_key_env
    #[arg(long)]
    unsafe_inline_api_key: bool,
}

#[derive(Clone, Copy, Debug)]
enum ExecutionMode {
    Mock,
    UnsandboxedHostExecution,
}

impl ExecutionMode {
    fn label(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::UnsandboxedHostExecution => "unsandboxed_host_execution",
        }
    }

    fn is_unsandboxed(self) -> bool {
        matches!(self, Self::UnsandboxedHostExecution)
    }
}

struct BuiltModel {
    model: Box<dyn Model>,
    mode: ExecutionMode,
}

struct RaymanBinary {
    path: PathBuf,
    dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct DoctorIdentity {
    contract: String,
    version: String,
    release_identity: DoctorReleaseIdentity,
    source_fresh: DoctorSourceFresh,
}

#[derive(Debug, Deserialize)]
struct DoctorReleaseIdentity {
    ready: bool,
}

#[derive(Debug, Deserialize)]
struct DoctorSourceFresh {
    verified: bool,
    status: String,
    required_verifier: String,
}

/// 启动时写入 provenance 的评测输入基线。真实后端的宿主 shell 可能在 trial 中修改
/// 这些源文件，因此不能只记录一次 hash 后默认它仍然有效。
#[derive(Debug, Clone)]
struct InputBaseline {
    skill_sha256: String,
    rayman_sha256: String,
    tasks: BTreeMap<String, TaskProvenance>,
}

impl InputBaseline {
    fn new(skill_sha256: String, rayman_sha256: String, tasks: &[TaskProvenance]) -> Self {
        Self {
            skill_sha256,
            rayman_sha256,
            tasks: tasks
                .iter()
                .cloned()
                .map(|task| (task.name.clone(), task))
                .collect(),
        }
    }

    /// 返回所有已发现的漂移；读取失败本身也是输入身份失效，不能忽略。
    fn verify(
        &self,
        skill_path: &Path,
        rayman_binary_path: &Path,
        tasks: &[task::Task],
    ) -> Vec<String> {
        let mut events = Vec::new();
        match read_regular_text(skill_path, "技能文件") {
            Ok(text) => {
                let actual = sha256_bytes(text.as_bytes());
                if actual != self.skill_sha256 {
                    events.push(format!(
                        "技能文件 SHA-256 漂移（expected {}, actual {}）",
                        self.skill_sha256, actual
                    ));
                }
            }
            Err(error) => events.push(format!("无法复核技能文件身份: {error:#}")),
        }
        match sha256_regular_file(rayman_binary_path, "rayman 二进制") {
            Ok(actual) => {
                if actual != self.rayman_sha256 {
                    events.push(format!(
                        "rayman 二进制 SHA-256 漂移（expected {}, actual {}）",
                        self.rayman_sha256, actual
                    ));
                }
            }
            Err(error) => events.push(format!("无法复核 rayman 二进制身份: {error:#}")),
        }
        if self.tasks.len() != tasks.len() {
            events.push(format!(
                "任务集合大小漂移（expected {}, actual {}）",
                self.tasks.len(),
                tasks.len()
            ));
        }
        for task in tasks {
            let Some(expected) = self.tasks.get(&task.name) else {
                events.push(format!("出现未记录的任务输入: {}", task.name));
                continue;
            };
            match task.provenance() {
                Ok(actual) if actual != *expected => events.push(format!(
                    "任务 `{}` 输入 SHA-256 漂移（expected {}, actual {}）",
                    task.name, expected.task_sha256, actual.task_sha256
                )),
                Ok(_) => {}
                Err(error) => {
                    events.push(format!("无法复核任务 `{}` 输入身份: {error:#}", task.name))
                }
            }
        }
        events
    }
}

/// Real backends must not pair an arbitrary local binary with independently selected skill
/// text. The evaluator verifies a machine-readable doctor identity against the repository's
/// current contract/version source; a successful process exit alone is not a release binding.
/// Pinning PATH makes doctor's PATH identity check refer to this exact selected binary. Mock
/// remains deliberately exempt so offline orchestration tests do not depend on ignored metadata.
fn verify_real_release_binding(
    rayman_binary: &RaymanBinary,
    repo_root: &Path,
    selected_skill: &Path,
) -> Result<ReleaseContractProvenance> {
    let workspace_skill = repo_root.join("SKILL.md");
    let selected_hash = sha256_regular_file(selected_skill, "选定技能文件")?;
    let workspace_hash = sha256_regular_file(&workspace_skill, "工作区 SKILL.md")?;
    if selected_hash != workspace_hash {
        bail!(
            "真实评测拒绝使用与工作区 SKILL.md 不一致的 --skill；选定 SHA-256={selected_hash}，工作区 SHA-256={workspace_hash}"
        );
    }

    let expected_contract = expected_rayman_contract(repo_root)?;
    let expected_version = expected_rayman_version(repo_root)?;

    let mut path_entries = vec![rayman_binary.dir.clone()];
    if let Some(parent_path) = std::env::var_os("PATH") {
        path_entries.extend(std::env::split_paths(&parent_path));
    }
    let pinned_path =
        std::env::join_paths(path_entries).context("无法构造供 rayman doctor 使用的固定 PATH")?;
    let output = Command::new(&rayman_binary.path)
        .args(["--format", "json", "doctor", "--check"])
        .current_dir(repo_root)
        .env("PATH", pinned_path)
        .output()
        .with_context(|| format!("无法执行 {} doctor --check", rayman_binary.path.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    validate_doctor_identity(
        output.status.success(),
        &stdout,
        &expected_contract,
        &expected_version,
    )
    .with_context(|| {
        format!(
            "真实评测发布契约校验失败（selected rayman + workspace SKILL.md）；先同步安装再运行。stdout: {}; stderr: {}",
            brief_output(&stdout),
            brief_output(&stderr)
        )
    })
}

fn validate_doctor_identity(
    exit_success: bool,
    stdout: &str,
    expected_contract: &str,
    expected_version: &str,
) -> Result<ReleaseContractProvenance> {
    let doctor: DoctorIdentity = serde_json::from_str(stdout)
        .context("rayman doctor 必须输出包含 contract、version、ready 的 JSON 身份")?;
    if !exit_success {
        bail!("rayman doctor --check 以非零状态退出")
    }
    if !doctor.release_identity.ready {
        bail!("rayman doctor JSON 报告 release_identity.ready=false")
    }
    if doctor.contract != expected_contract {
        bail!(
            "rayman doctor contract 不匹配：reported={}, expected={expected_contract}",
            doctor.contract
        )
    }
    if doctor.version != expected_version {
        bail!(
            "rayman doctor version 不匹配：reported={}, expected={expected_version}",
            doctor.version
        )
    }
    if doctor.source_fresh.status != "not_checked_by_doctor" || doctor.source_fresh.verified {
        bail!(
            "rayman doctor source_fresh 语义不匹配：expected status=not_checked_by_doctor, verified=false；reported status={}, verified={}",
            doctor.source_fresh.status,
            doctor.source_fresh.verified
        )
    }
    if doctor.source_fresh.required_verifier.trim().is_empty() {
        bail!("rayman doctor source_fresh.required_verifier 不能为空")
    }
    Ok(ReleaseContractProvenance {
        required: true,
        verified: true,
        expected_contract: Some(expected_contract.into()),
        expected_version: Some(expected_version.into()),
        reported_contract: Some(doctor.contract),
        reported_version: Some(doctor.version),
        source_fresh_verified: Some(doctor.source_fresh.verified),
        source_fresh_status: Some(doctor.source_fresh.status),
        source_fresh_required_verifier: Some(doctor.source_fresh.required_verifier),
    })
}

/// The root release verifier and the shipped CLI use this source constant as the
/// contract identifier. Reading it avoids duplicating a brittle v4/v5 literal in
/// the standalone evaluator while still rejecting a stale binary's different JSON.
fn expected_rayman_contract(repo_root: &Path) -> Result<String> {
    let source = read_regular_text(
        &repo_root.join("crates/rayman/src/main.rs"),
        "rayman 发布契约源码",
    )?;
    source
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("const CLI_CONTRACT: &str = \"")
                .and_then(|rest| rest.strip_suffix("\";"))
                .map(str::to_string)
        })
        .filter(|contract| !contract.is_empty())
        .ok_or_else(|| anyhow::anyhow!("无法从 crates/rayman/src/main.rs 读取 CLI_CONTRACT"))
}

fn expected_rayman_version(repo_root: &Path) -> Result<String> {
    let manifest = read_regular_text(&repo_root.join("Cargo.toml"), "工作区 Cargo.toml")?;
    let mut in_workspace_package = false;
    for raw_line in manifest.lines() {
        let line = raw_line.split('#').next().unwrap_or_default().trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if in_workspace_package && let Some(version) = quoted_assignment(line, "version") {
            return Ok(version);
        }
    }
    bail!("无法从 Cargo.toml 的 [workspace.package] 读取 version")
}

fn quoted_assignment(line: &str, key: &str) -> Option<String> {
    let (actual_key, value) = line.split_once('=')?;
    if actual_key.trim() != key {
        return None;
    }
    value
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')
        .map(str::to_string)
}

fn brief_output(text: &str) -> String {
    const MAX_CHARS: usize = 1_000;
    let mut out = text.chars().take(MAX_CHARS).collect::<String>();
    if text.chars().nth(MAX_CHARS).is_some() {
        out.push_str("…(truncated)");
    }
    out.trim().to_string()
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 找到 `rayman` 二进制。报告记录并 hash 实际文件，而非仅记录一个可能漂移的目录。
fn find_rayman_binary() -> Option<RaymanBinary> {
    let mut candidates = Vec::new();
    let repo = manifest_dir().parent().map(Path::to_path_buf);
    if let Some(repo) = &repo {
        candidates.push(repo.join("target").join("release"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Rayman").join("bin"));
    }
    if let Some(repo) = &repo {
        candidates.push(repo.join("target").join("debug"));
    }
    candidates.into_iter().find_map(|dir| {
        let path = dir.join(grade::rayman_exe());
        let metadata = fs::symlink_metadata(&path).ok()?;
        (!is_link_or_reparse(&metadata) && metadata.is_file()).then_some(RaymanBinary { path, dir })
    })
}

fn require_host_execution_ack(acknowledged: bool, backend: &str) -> Result<()> {
    if acknowledged {
        return Ok(());
    }
    bail!(
        "后端 `{backend}` 会让真实模型生成的 `run` 命令直接在宿主机执行，当前没有 sandbox；仅在隔离环境中使用，并显式传 --unsafe-host-exec 确认"
    )
}

/// Keep the two independent real-backend gates ordered and observable. The explicit host-exec
/// acknowledgement is a pure CLI check and must win when absent. Only after it succeeds may a
/// platform-containment refusal run, still before backend config, credentials, or network use.
fn preflight_real_backend(
    backend: &str,
    unsafe_host_exec: bool,
    shell_refusal: Option<&str>,
) -> Result<()> {
    if backend == "mock" {
        return Ok(());
    }
    require_host_execution_ack(unsafe_host_exec, backend)?;
    if let Some(reason) = shell_refusal {
        bail!("真实后端在读取配置、密钥或请求模型前被拒绝：{reason}");
    }
    Ok(())
}

/// 按 `--backend` 名字构建模型后端。任何非 mock 后端在读取密钥/配置前就先要求宿主执行确认。
fn build_model(cli: &Cli) -> Result<BuiltModel> {
    match cli.backend.as_str() {
        "mock" => Ok(BuiltModel {
            model: Box::new(MockModel::new("mock(noop)", Vec::new())),
            mode: ExecutionMode::Mock,
        }),
        "anthropic" => {
            require_host_execution_ack(cli.unsafe_host_exec, "anthropic")?;
            let model = cli.model.as_deref().unwrap_or(anthropic::DEFAULT_MODEL);
            Ok(BuiltModel {
                model: Box::new(anthropic::AnthropicModel::from_env(model)?),
                mode: ExecutionMode::UnsandboxedHostExecution,
            })
        }
        name => {
            require_host_execution_ack(cli.unsafe_host_exec, name)?;
            let path = cli
                .backends
                .clone()
                .unwrap_or_else(|| manifest_dir().join("backends.json"));
            let config = openai::BackendsConfig::load(&path)?;
            let cfg = config.backends.get(name).with_context(|| {
                let known: Vec<&str> = config.backends.keys().map(String::as_str).collect();
                format!(
                    "配置 {} 里没有后端 `{name}`（已知: mock, anthropic, {}）",
                    path.display(),
                    known.join(", ")
                )
            })?;
            Ok(BuiltModel {
                model: Box::new(openai::OpenAiModel::new(
                    name,
                    cfg,
                    cli.model.as_deref(),
                    cli.unsafe_inline_api_key,
                )?),
                mode: ExecutionMode::UnsandboxedHostExecution,
            })
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}

/// 一次评测运行的共享上下文，供每个 trial 复用。
struct RunContext<'a> {
    model: &'a dyn Model,
    skill_text: &'a str,
    rayman_bin_dir: &'a Path,
    run_dir: &'a Path,
    max_steps: usize,
}

/// 跑单个 trial。基础设施错误（工作区准备失败、后端故障、响应截断）收敛为 Error 结果，
/// 不打断整轮评测，让已完成的结果总能落进报告。
fn run_trial(
    ctx: &RunContext,
    task: &task::Task,
    authorized_task: &TaskProvenance,
    condition: &str,
    trial: usize,
    condition_order: usize,
) -> TrialResult {
    let mut result = TrialResult {
        task: task.name.clone(),
        condition: condition.into(),
        trial,
        condition_order,
        outcome: Outcome::Error,
        grade_outcome: GradeOutcome::NotRun,
        grade_exit: -1,
        steps: 0,
        tool_calls: 0,
        rayman_invocations: 0,
        finished: false,
        error: None,
    };
    let workspace = ctx
        .run_dir
        .join("workspaces")
        .join(format!("{}__{}__t{}", task.name, condition, trial));
    if authorized_task.name != task.name {
        result.error = Some(format!(
            "trial 任务与已授权 manifest 不匹配: task={}, manifest={}",
            task.name, authorized_task.name
        ));
        return result;
    }
    if let Err(error) = task::setup_workspace(task, &workspace, &authorized_task.fixture_sha256) {
        result.error = Some(format!("{error:#}"));
        return result;
    }
    // Unsupported hosts may still exercise task-copy/report orchestration with the offline
    // mock, but no model or grade is started: both can lead to arbitrary shell requests and the
    // evaluator has no reliable descendant supervisor on those platforms.
    if let Some(reason) = grade::shell_execution_refusal_reason() {
        result.error = Some(reason.into());
        return result;
    }
    // Windows `cmd` resolves a bare command from the trial CWD before PATH. Reject a fixture
    // that would make control's `rayman` availability ambiguous before the model can run it.
    if condition == CONTROL
        && let Err(error) = grade::ensure_no_top_level_rayman_command(&workspace)
    {
        result.error = Some(format!("控制组当前目录命令冲突: {error}"));
        return result;
    }
    let cfg = AgentConfig {
        system_base: SYSTEM_BASE.into(),
        skill_text: (condition == WITH_SKILL).then(|| ctx.skill_text.to_string()),
        task_prompt: task.prompt.clone(),
        max_steps: ctx.max_steps,
        // with_skill 组把 rayman 注入 run 工具 PATH；control 组反向剔除可解析的
        // rayman PATH 目录。未隔离 host shell 仍由报告层 fail-closed 为 non-comparative。
        env: if condition == WITH_SKILL {
            EnvPolicy::with_rayman(ctx.rayman_bin_dir.to_path_buf())
        } else {
            EnvPolicy::without_rayman()
        },
    };
    let log = run_agent(ctx.model, &workspace, &cfg);
    let agent_had_error = log.error.is_some();
    result.steps = log.steps;
    result.tool_calls = log.tool_calls;
    result.rayman_invocations = log.rayman_invocations;
    result.finished = log.finished;
    result.error = log.error;
    // A wrapper can be created after the initial preflight. `run_shell` latches every rayman
    // command/wrapper it observes inside the trial workspace; the post-agent scan also catches a
    // wrapper written without a later shell call. Either way the control arm's availability is
    // ambiguous, so do not grade the trial.
    //
    // A shell command that was *refused* is deliberately not part of this condition: it never
    // reached `cmd`, so isolation still holds. Failing the whole trial on a refusal would make
    // errors accrue only to the control arm — which has this guard — and bias the comparison.
    if condition == CONTROL {
        let postflight = grade::ensure_no_top_level_rayman_command(&workspace);
        let latched = cfg.env.control_isolation_loss();
        if latched.is_some() || postflight.is_err() {
            let collision = match postflight {
                Err(error) => error,
                Ok(()) => latched.unwrap_or_else(|| {
                    "控制组 shell 启动前检测到当前目录 rayman 命令/包装器".into()
                }),
            };
            result.error = Some(match result.error.take() {
                Some(existing) => format!("{existing}\n控制组当前目录命令冲突: {collision}"),
                None => format!("控制组当前目录命令冲突: {collision}"),
            });
            return result;
        }
    }
    let graded = grade::run_shell(
        &workspace,
        &task.grade_cmd,
        &EnvPolicy::default(),
        grade::GRADE_TIMEOUT,
    );
    // 普通非零评分才是模型 Fail。评分命令本身未能形成可靠观察（超时、临时文件、
    // spawn/wait 失败）必须是 Error，避免把评测器故障计进 evaluable 分母。
    result.outcome = classify_trial_outcome(graded.outcome, agent_had_error);
    result.grade_outcome = graded.outcome;
    result.grade_exit = graded.exit;
    if graded.outcome.is_evaluation_error() {
        let grade_error = format!(
            "评分未形成可评估结果（{}，exit={}）: {}",
            graded.outcome.label(),
            graded.exit,
            graded.stderr.trim()
        );
        result.error = Some(match result.error.take() {
            Some(agent_error) => format!("{agent_error}\n{grade_error}"),
            None => grade_error,
        });
    }
    result
}

fn classify_trial_outcome(grade_outcome: GradeOutcome, agent_had_error: bool) -> Outcome {
    if grade_outcome.passed() {
        Outcome::Pass
    } else if agent_had_error || grade_outcome.is_evaluation_error() {
        Outcome::Error
    } else {
        Outcome::Fail
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.trials == 0 {
        bail!("--trials 必须大于 0");
    }
    if cli.max_steps == 0 {
        bail!("--max-steps 必须大于 0");
    }
    preflight_real_backend(
        &cli.backend,
        cli.unsafe_host_exec,
        grade::shell_execution_refusal_reason(),
    )?;

    // 先建后端，确保真实后端在读取密钥、请求网络前被 --unsafe-host-exec 拦住。
    let built = build_model(&cli)?;
    let model_label = built.model.label();

    let tasks_dir = cli.tasks.unwrap_or_else(|| manifest_dir().join("tasks"));
    let skill_path = cli
        .skill
        .unwrap_or_else(|| manifest_dir().parent().unwrap().join("SKILL.md"));
    let runs_dir = cli.runs_dir.unwrap_or_else(|| manifest_dir().join(".runs"));
    let seed = cli.seed.unwrap_or_else(default_seed);

    let skill_text = read_regular_text(&skill_path, "技能文件")?;
    let tasks = task::load_tasks(&tasks_dir, cli.task.as_deref())?;
    if tasks.is_empty() {
        bail!("没有匹配的任务");
    }
    let task_manifests = tasks
        .iter()
        .map(task::Task::provenance)
        .collect::<Result<Vec<_>>>()?;
    let grade_execution = task::authorize_grade_execution(
        &tasks_dir,
        cli.task.as_deref(),
        &task_manifests,
        cli.unsafe_custom_grade_exec,
    )?;

    // with_skill 组的 system prompt 宣称 rayman 在 PATH 上，找不到二进制就不能开跑，
    // 否则会系统性压低处理组。
    let rayman_binary = find_rayman_binary().ok_or_else(|| {
        anyhow::anyhow!(
            "未找到非 symlink 的 rayman 可执行文件，with_skill 组无法成立。先在仓库根运行: cargo build --release"
        )
    })?;
    let rayman_hash = sha256_regular_file(&rayman_binary.path, "rayman 二进制")?;
    let skill_hash = sha256_bytes(skill_text.as_bytes());
    let repo_root = manifest_dir().parent().unwrap().to_path_buf();
    let release_contract = if built.mode.is_unsandboxed() {
        verify_real_release_binding(&rayman_binary, &repo_root, &skill_path)?
    } else {
        ReleaseContractProvenance::not_required_for_mock()
    };
    let release_contract_verified = release_contract.verified;
    let input_baseline =
        InputBaseline::new(skill_hash.clone(), rayman_hash.clone(), &task_manifests);

    let runs_dir = ensure_or_create_real_directory(&runs_dir, "runs 目录")?;
    let (run_id, run_dir) = create_run_directory(&runs_dir)?;
    let provenance = RunProvenance {
        run_id: run_id.clone(),
        started_unix_seconds: unix_seconds(),
        evaluator_version: env!("CARGO_PKG_VERSION").into(),
        backend: model_label.clone(),
        execution_mode: built.mode.label().into(),
        unsandboxed_host_execution: built.mode.is_unsandboxed(),
        grade_execution,
        release_contract_verified,
        release_contract,
        git_head: git_head(&repo_root),
        skill: ArtifactHash {
            path: skill_path.display().to_string(),
            sha256: skill_hash,
        },
        rayman_binary: ArtifactHash {
            path: rayman_binary.path.display().to_string(),
            sha256: rayman_hash,
        },
        host: HostProvenance {
            os: std::env::consts::OS.into(),
            family: std::env::consts::FAMILY.into(),
            arch: std::env::consts::ARCH.into(),
            shell_execution_supported: grade::shell_execution_supported(),
            shell_containment: grade::shell_containment_mode().into(),
        },
        seed,
        order_strategy:
            "per-task deterministic alternating order; seed selects each task's first condition"
                .into(),
        tasks: task_manifests,
    };

    if built.mode.is_unsandboxed() {
        eprintln!(
            "WARNING: UNSANDBOXED HOST EXECUTION acknowledged; model-generated shell commands can access this user's host resources."
        );
    }
    eprintln!(
        "rayman 二进制: {} (sha256 recorded)",
        rayman_binary.path.display()
    );
    eprintln!(
        "后端={} 任务={} 每格重复={} seed={} run_id={}",
        model_label,
        tasks.len(),
        cli.trials,
        seed,
        run_id
    );

    let ctx = RunContext {
        model: built.model.as_ref(),
        skill_text: &skill_text,
        rayman_bin_dir: &rayman_binary.dir,
        run_dir: &run_dir,
        max_steps: cli.max_steps,
    };

    let mut results = Vec::new();
    let mut input_integrity = InputIntegrity::default();
    'evaluation: for task in &tasks {
        for trial in 0..cli.trials {
            let conditions = condition_order(seed, &task.name, trial);
            for (order, condition) in conditions.into_iter().enumerate() {
                let before = input_baseline.verify(&skill_path, &rayman_binary.path, &tasks);
                if !before.is_empty() {
                    eprintln!("评测输入完整性失败；停止后续 trial: {}", before.join("; "));
                    input_integrity.drift_detected = true;
                    input_integrity.events.extend(before);
                    break 'evaluation;
                }
                eprintln!("  [#{order} {condition}] {} trial {trial}", task.name);
                let Some(authorized_task) = input_baseline.tasks.get(&task.name) else {
                    input_integrity.drift_detected = true;
                    input_integrity
                        .events
                        .push(format!("任务 `{}` 缺少已授权 manifest", task.name));
                    break 'evaluation;
                };
                let mut result = run_trial(&ctx, task, authorized_task, condition, trial, order);
                let after = input_baseline.verify(&skill_path, &rayman_binary.path, &tasks);
                if !after.is_empty() {
                    let drift_error = format!("评测输入漂移: {}", after.join("; "));
                    result.outcome = Outcome::Error;
                    result.error = Some(match result.error.take() {
                        Some(existing) => format!("{existing}\n{drift_error}"),
                        None => drift_error,
                    });
                    input_integrity.drift_detected = true;
                    input_integrity.events.extend(after);
                }
                if let Some(error) = &result.error {
                    eprintln!("    trial 错误: {error}");
                }
                results.push(result);
                if input_integrity.drift_detected {
                    eprintln!("评测输入完整性失败；停止后续 trial");
                    break 'evaluation;
                }
            }
        }
    }

    let report = EvalReport {
        model: model_label,
        trials_per_cell: cli.trials,
        task_count: tasks.len(),
        provenance,
        input_integrity,
        results,
    };
    let markdown = report.markdown();
    let summary = report.summary_json();
    let summary_text = serde_json::to_string_pretty(&summary)?;
    write_new_file(&run_dir.join("report.md"), markdown.as_bytes())?;
    write_new_file(&run_dir.join("report.json"), summary_text.as_bytes())?;
    write_latest(&runs_dir, &run_id, &markdown, &summary)?;

    println!("{markdown}");
    eprintln!(
        "不可变报告写入 {}；最新指针为 {}",
        run_dir.display(),
        runs_dir.join("latest.json").display()
    );
    Ok(())
}

fn condition_order(seed: u64, task_name: &str, trial: usize) -> [&'static str; 2] {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for byte in task_name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // 每个 task 在相邻 trial 翻转一次；偶数重复严格 50/50，奇数重复的偏差至多一。
    let with_first = ((hash ^ trial as u64) & 1) == 0;
    if with_first {
        [WITH_SKILL, CONTROL]
    } else {
        [CONTROL, WITH_SKILL]
    }
}

fn create_run_directory(runs_dir: &Path) -> Result<(String, PathBuf)> {
    let runs_dir = ensure_or_create_real_directory(runs_dir, "runs 目录")?;
    for _ in 0..128 {
        let run_id = format!(
            "run-{}-p{}-{}",
            unix_nanos(),
            std::process::id(),
            RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let run_dir = runs_dir.join(&run_id);
        match fs::create_dir(&run_dir) {
            Ok(()) => {
                fs::create_dir(run_dir.join("workspaces"))
                    .with_context(|| format!("无法创建工作区目录: {}", run_dir.display()))?;
                return Ok((run_id, run_dir));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法创建 run 目录: {}", run_dir.display()));
            }
        }
    }
    bail!("无法分配唯一 run id（重复 128 次）")
}

/// Validate every existing ancestor before creating the first missing component. Calling
/// create_dir_all first would already traverse a symlink/junction and could create output in
/// an attacker-selected location before the final-directory check noticed anything.
fn ensure_or_create_real_directory(path: &Path, label: &str) -> Result<PathBuf> {
    use std::path::Component;

    if path.as_os_str().is_empty() {
        bail!("{label} 路径不能为空");
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("无法读取当前目录")?
            .join(path)
    };
    if absolute
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("{label} 路径不能包含 `..`: {}", path.display());
    }

    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => unreachable!("parent components rejected above"),
            Component::Prefix(_) => {
                current.push(component.as_os_str());
                continue;
            }
            Component::RootDir | Component::Normal(_) => {
                current.push(component.as_os_str());
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_link_or_reparse(&metadata) || !metadata.is_dir() {
                    bail!("{label} 祖先必须是非 symlink 目录: {}", current.display());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("无法创建 {label} 目录组件: {}", current.display())
                        });
                    }
                }
                // Re-check after create/AlreadyExists so a racing link never becomes an
                // accepted component merely because another actor won create_dir.
                ensure_real_directory(&current, label)?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("无法检查 {label} 祖先: {}", current.display()));
            }
        }
    }
    Ok(absolute)
}

fn ensure_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 {label}: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_dir() {
        bail!("{label} 必须是非 symlink 目录: {}", path.display());
    }
    Ok(())
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn read_regular_text(path: &Path, label: &str) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 {label}: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!("{label} 必须是非 symlink 普通文件: {}", path.display());
    }
    fs::read_to_string(path).with_context(|| format!("无法读取 {label}: {}", path.display()))
}

fn sha256_regular_file(path: &Path, label: &str) -> Result<String> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("无法检查 {label}: {}", path.display()))?;
    if is_link_or_reparse(&metadata) || !metadata.is_file() {
        bail!("{label} 必须是非 symlink 普通文件: {}", path.display());
    }
    let bytes = fs::read(path).with_context(|| format!("无法读取 {label}: {}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("拒绝覆写既有 run 制品: {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("无法写入制品: {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("无法同步制品: {}", path.display()))?;
    Ok(())
}

/// latest 是可变索引；真正的报告只在 `<runs>/<run-id>/` 下写一次。使用 JSON 指针和 Markdown
/// 摘要而不是 symlink，避免 Windows 上的权限/语义差异。
fn write_latest(
    runs_dir: &Path,
    run_id: &str,
    markdown: &str,
    summary: &serde_json::Value,
) -> Result<()> {
    let pointer = json!({
        "run_id": run_id,
        "run_dir": run_id,
        "report_json": format!("{run_id}/report.json"),
        "report_markdown": format!("{run_id}/report.md"),
        "summary": summary,
    });
    replace_regular_file(
        &runs_dir.join("latest.json"),
        serde_json::to_string_pretty(&pointer)?.as_bytes(),
    )?;
    replace_regular_file(&runs_dir.join("latest.md"), markdown.as_bytes())
}

fn replace_regular_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (is_link_or_reparse(&metadata) || !metadata.is_file())
    {
        bail!("拒绝覆盖非普通 latest 文件: {}", path.display());
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("latest 文件没有父目录: {}", path.display()))?;
    let temp = parent.join(format!(
        ".rayman-eval-latest-{}-{}",
        std::process::id(),
        RUN_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    write_new_file(&temp, bytes)?;
    let replacement = match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(first_error) => {
            let metadata = fs::symlink_metadata(path)
                .with_context(|| format!("无法复查 latest 文件 {}", path.display()))?;
            if is_link_or_reparse(&metadata) || !metadata.is_file() {
                bail!("拒绝覆盖非普通 latest 文件: {}", path.display());
            }
            fs::remove_file(path)
                .with_context(|| format!("无法替换 latest 文件: {}", path.display()))?;
            fs::rename(&temp, path).with_context(|| {
                format!(
                    "无法替换 latest 文件 {}（初始错误: {first_error}）",
                    path.display()
                )
            })
        }
    };
    if replacement.is_err() {
        let _ = fs::remove_file(temp);
    }
    replacement
}

fn git_head(repo_root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!head.is_empty()).then_some(head)
}

fn default_seed() -> u64 {
    unix_nanos() as u64
        ^ (u64::from(std::process::id()) << 32)
        ^ RUN_COUNTER.load(Ordering::Relaxed)
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn sample_task(root: &Path) -> task::Task {
        let task_dir = root.join("tasks/sample");
        write(&task_dir.join("prompt.md"), "fix it\n");
        write(&task_dir.join("grade.txt"), "cargo test\n");
        write(
            &task_dir.join("fixture/src/lib.rs"),
            "pub fn original() {}\n",
        );
        task::load_tasks(&root.join("tasks"), None)
            .unwrap()
            .into_iter()
            .next()
            .unwrap()
    }

    #[test]
    fn real_backends_require_explicit_host_execution_ack() {
        assert!(require_host_execution_ack(false, "anthropic").is_err());
        assert!(require_host_execution_ack(true, "anthropic").is_ok());
    }

    #[test]
    fn missing_host_ack_is_reported_before_platform_supervisor_refusal() {
        let error = preflight_real_backend(
            "anthropic",
            false,
            Some("forced missing descendant supervisor"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--unsafe-host-exec"), "{error}");
        assert!(!error.contains("descendant supervisor"), "{error}");
    }

    #[test]
    fn acknowledged_real_backend_is_refused_before_config_on_unsupported_host() {
        let error = preflight_real_backend(
            "custom-provider",
            true,
            Some("forced missing descendant supervisor"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("descendant supervisor"), "{error}");
        assert!(error.contains("读取配置、密钥或请求模型前"), "{error}");
        assert!(!error.contains("--unsafe-host-exec"), "{error}");
    }

    #[test]
    fn real_release_binding_rejects_a_skill_that_differs_from_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let selected = temp.path().join("other-skill.md");
        write(&repo.join("SKILL.md"), "workspace skill\n");
        write(&selected, "different skill\n");
        let binary = RaymanBinary {
            path: temp.path().join("rayman"),
            dir: temp.path().to_path_buf(),
        };

        let error = verify_real_release_binding(&binary, &repo, &selected)
            .unwrap_err()
            .to_string();
        assert!(error.contains("不一致"), "{error}");
    }

    #[test]
    fn doctor_identity_rejects_an_old_contract_despite_zero_exit() {
        let old_doctor = r#"{
            "contract": "rayman-cli-contract-old",
            "version": "2.1.0",
            "release_identity": {"ready": true},
            "source_fresh": {
                "verified": false,
                "status": "not_checked_by_doctor",
                "required_verifier": "verify-release-contract"
            }
        }"#;

        let error =
            validate_doctor_identity(true, old_doctor, "rayman-cli-contract-current", "2.1.0")
                .unwrap_err()
                .to_string();

        assert!(error.contains("contract 不匹配"), "{error}");
    }

    #[test]
    fn doctor_identity_rejects_a_stale_version_despite_zero_exit() {
        let stale_doctor = r#"{
            "contract": "rayman-cli-contract-current",
            "version": "2.0.0",
            "release_identity": {"ready": true},
            "source_fresh": {
                "verified": false,
                "status": "not_checked_by_doctor",
                "required_verifier": "verify-release-contract"
            }
        }"#;

        let error =
            validate_doctor_identity(true, stale_doctor, "rayman-cli-contract-current", "2.1.0")
                .unwrap_err()
                .to_string();

        assert!(error.contains("version 不匹配"), "{error}");
    }

    #[test]
    fn doctor_identity_rejects_a_claimed_source_freshness_from_doctor() {
        let invalid_doctor = r#"{
            "contract": "rayman-cli-contract-current",
            "version": "2.1.0",
            "release_identity": {"ready": true},
            "source_fresh": {
                "verified": true,
                "status": "verified",
                "required_verifier": "verify-release-contract"
            }
        }"#;

        let error =
            validate_doctor_identity(true, invalid_doctor, "rayman-cli-contract-current", "2.1.0")
                .unwrap_err()
                .to_string();

        assert!(error.contains("source_fresh 语义不匹配"), "{error}");
    }

    #[test]
    fn doctor_identity_records_verified_expected_and_reported_values() {
        let doctor = r#"{
            "contract": "rayman-cli-contract-current",
            "version": "2.1.0",
            "release_identity": {"ready": true},
            "source_fresh": {
                "verified": false,
                "status": "not_checked_by_doctor",
                "required_verifier": "verify-release-contract"
            }
        }"#;

        let provenance =
            validate_doctor_identity(true, doctor, "rayman-cli-contract-current", "2.1.0").unwrap();

        assert!(provenance.required);
        assert!(provenance.verified);
        assert_eq!(
            provenance.expected_contract.as_deref(),
            Some("rayman-cli-contract-current")
        );
        assert_eq!(provenance.reported_version.as_deref(), Some("2.1.0"));
        assert_eq!(provenance.source_fresh_verified, Some(false));
        assert_eq!(
            provenance.source_fresh_status.as_deref(),
            Some("not_checked_by_doctor")
        );
    }

    #[test]
    fn expected_contract_and_version_follow_repository_sources() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path();
        write(
            &repo.join("crates/rayman/src/main.rs"),
            "const CLI_CONTRACT: &str = \"rayman-cli-contract-next\";\n",
        );
        write(
            &repo.join("Cargo.toml"),
            "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"9.8.7\"\n",
        );

        assert_eq!(
            expected_rayman_contract(repo).unwrap(),
            "rayman-cli-contract-next"
        );
        assert_eq!(expected_rayman_version(repo).unwrap(), "9.8.7");
    }

    #[test]
    fn grade_infrastructure_outcomes_are_not_model_failures() {
        assert_eq!(
            classify_trial_outcome(GradeOutcome::InfrastructureError, false),
            Outcome::Error
        );
        assert_eq!(
            classify_trial_outcome(GradeOutcome::TimedOut, false),
            Outcome::Error
        );
        assert_eq!(
            classify_trial_outcome(GradeOutcome::Failed, false),
            Outcome::Fail
        );
        assert_eq!(
            classify_trial_outcome(GradeOutcome::Passed, true),
            Outcome::Pass
        );
    }

    /// 回归：控制组模型跑普通 `cmd` 语法时，该命令曾被误判为“提及 rayman”，整轮 trial 被
    /// 记为 Error 并跳过评分。受测组没有这条拦截，Error 因此系统性偏向控制组，直接污染 A/B。
    /// 现在这类命令正常执行，trial 照常评分。
    #[cfg(windows)]
    #[test]
    fn control_trial_with_benign_percent_command_is_still_graded() {
        let temp = tempfile::tempdir().unwrap();
        let task = sample_task(temp.path());
        let run_dir = temp.path().join("run");
        fs::create_dir_all(run_dir.join("workspaces")).unwrap();
        let model = MockModel::new(
            "mock",
            vec![vec![(
                "run".into(),
                json!({"command": "echo %CD% & if %ERRORLEVEL% neq 0 exit /b 1"}),
            )]],
        );
        let ctx = RunContext {
            model: &model,
            skill_text: "",
            rayman_bin_dir: temp.path(),
            run_dir: &run_dir,
            max_steps: 3,
        };

        let manifest = task.provenance().unwrap();
        let result = run_trial(&ctx, &task, &manifest, CONTROL, 0, 0);

        assert_eq!(result.error, None, "{result:?}");
        assert_ne!(result.outcome, Outcome::Error, "{result:?}");
        assert_ne!(result.grade_outcome, GradeOutcome::NotRun, "{result:?}");
    }

    #[cfg(windows)]
    #[test]
    fn control_trial_fails_closed_when_agent_creates_cwd_rayman_wrapper() {
        let temp = tempfile::tempdir().unwrap();
        let task = sample_task(temp.path());
        let run_dir = temp.path().join("run");
        fs::create_dir_all(run_dir.join("workspaces")).unwrap();
        let model = MockModel::new(
            "mock",
            vec![vec![(
                "write_file".into(),
                json!({
                    "path": "RAYMAN.custom.extension",
                    "content": "@echo off"
                }),
            )]],
        );
        let ctx = RunContext {
            model: &model,
            skill_text: "",
            rayman_bin_dir: temp.path(),
            run_dir: &run_dir,
            max_steps: 2,
        };

        let manifest = task.provenance().unwrap();
        let result = run_trial(&ctx, &task, &manifest, CONTROL, 0, 0);

        assert_eq!(result.outcome, Outcome::Error);
        assert_eq!(result.grade_outcome, GradeOutcome::NotRun);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("当前目录命令冲突")),
            "{result:?}"
        );
    }

    #[test]
    fn trial_fixture_mismatch_blocks_the_model_and_grade_before_execution() {
        let temp = tempfile::tempdir().unwrap();
        let task = sample_task(temp.path());
        let run_dir = temp.path().join("run");
        fs::create_dir_all(run_dir.join("workspaces")).unwrap();
        let model = MockModel::new(
            "must-not-run",
            vec![vec![(
                "write_file".into(),
                json!({
                    "path": "agent-ran.txt",
                    "content": "model executed"
                }),
            )]],
        );
        let ctx = RunContext {
            model: &model,
            skill_text: "",
            rayman_bin_dir: temp.path(),
            run_dir: &run_dir,
            max_steps: 2,
        };
        let mut stale_manifest = task.provenance().unwrap();
        stale_manifest.fixture_sha256 = "0".repeat(64);

        let result = run_trial(&ctx, &task, &stale_manifest, WITH_SKILL, 0, 0);
        let workspace = run_dir.join("workspaces/sample__with_skill__t0");

        assert_eq!(result.outcome, Outcome::Error);
        assert_eq!(result.grade_outcome, GradeOutcome::NotRun);
        assert_eq!(result.steps, 0, "model must not be called");
        assert!(!workspace.join("agent-ran.txt").exists());
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("拒绝任何 agent/grade 执行")),
            "{result:?}"
        );
    }

    #[test]
    fn input_baseline_detects_fixture_skill_and_binary_drift() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("SKILL.md");
        let binary = temp.path().join("rayman");
        write(&skill, "skill version one\n");
        write(&binary, "binary version one\n");
        let task = sample_task(temp.path());
        let tasks = vec![task];
        let manifests = tasks
            .iter()
            .map(task::Task::provenance)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let baseline = InputBaseline::new(
            sha256_bytes(read_regular_text(&skill, "技能文件").unwrap().as_bytes()),
            sha256_regular_file(&binary, "rayman 二进制").unwrap(),
            &manifests,
        );
        assert!(baseline.verify(&skill, &binary, &tasks).is_empty());

        write(&skill, "skill version two\n");
        write(&binary, "binary version two\n");
        write(
            &temp.path().join("tasks/sample/fixture/src/lib.rs"),
            "pub fn mutated() {}\n",
        );
        let events = baseline.verify(&skill, &binary, &tasks);
        assert!(
            events
                .iter()
                .any(|event| event.contains("技能文件 SHA-256 漂移")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| event.contains("rayman 二进制 SHA-256 漂移")),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| event.contains("任务 `sample` 输入 SHA-256 漂移")),
            "{events:?}"
        );
    }

    #[test]
    fn condition_order_is_seeded_deterministic_and_counterbalanced() {
        let first = condition_order(42, "task", 0);
        assert_eq!(first, condition_order(42, "task", 0));
        assert_ne!(first[0], first[1]);
        let with_first = (0..4)
            .filter(|trial| condition_order(42, "task", *trial)[0] == WITH_SKILL)
            .count();
        assert_eq!(with_first, 2);
    }

    #[test]
    fn run_directories_are_unique_and_never_reused() {
        let temp = tempfile::tempdir().unwrap();
        let (first_id, first) = create_run_directory(temp.path()).unwrap();
        let (second_id, second) = create_run_directory(temp.path()).unwrap();
        assert_ne!(first_id, second_id);
        assert!(first.join("workspaces").is_dir());
        assert!(second.join("workspaces").is_dir());
        assert!(
            fs::create_dir(&first).is_err(),
            "existing run must not be reused"
        );
    }

    #[test]
    fn runs_directory_creation_rejects_a_non_directory_ancestor_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        write(&blocker, "not a directory\n");
        let requested = blocker.join("runs");

        let error = ensure_or_create_real_directory(&requested, "runs 目录")
            .unwrap_err()
            .to_string();

        assert!(error.contains("祖先"), "{error}");
        assert!(!requested.exists());
    }

    #[cfg(unix)]
    #[test]
    fn runs_directory_creation_rejects_a_symlink_ancestor_before_writing() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let link = temp.path().join("linked");
        symlink(&outside, &link).unwrap();

        let error = ensure_or_create_real_directory(&link.join("runs"), "runs 目录")
            .unwrap_err()
            .to_string();

        assert!(error.contains("symlink"), "{error}");
        assert!(!outside.join("runs").exists());
    }

    #[cfg(windows)]
    #[test]
    fn runs_directory_creation_rejects_a_junction_ancestor_before_writing() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let junction = temp.path().join("linked");
        let output = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let error = ensure_or_create_real_directory(&junction.join("runs"), "runs 目录")
            .unwrap_err()
            .to_string();

        assert!(error.contains("symlink"), "{error}");
        assert!(!outside.join("runs").exists());
        fs::remove_dir(&junction).unwrap();
    }

    #[test]
    fn latest_is_a_pointer_to_immutable_run_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let (run_id, run_dir) = create_run_directory(temp.path()).unwrap();
        write_new_file(&run_dir.join("report.json"), b"{}\n").unwrap();
        write_new_file(&run_dir.join("report.md"), b"# report\n").unwrap();
        write_latest(temp.path(), &run_id, "# report\n", &json!({"ok": true})).unwrap();
        let latest: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(temp.path().join("latest.json")).unwrap())
                .unwrap();
        assert_eq!(latest["run_id"], run_id);
        assert_eq!(latest["report_json"], format!("{run_id}/report.json"));
        assert_eq!(
            fs::read_to_string(run_dir.join("report.md")).unwrap(),
            "# report\n"
        );
    }
}
