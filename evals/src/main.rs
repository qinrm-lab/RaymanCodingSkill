//! RaymanCodingSkill 的 agent A/B outcome eval：真实 agent 在有/无技能两组下解同一批编码任务，
//! 用隐藏的客观命令评分，输出通过率差。

mod agent;
mod anthropic;
mod grade;
mod openai;
mod report;
mod task;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use agent::{AgentConfig, MockModel, Model, run_agent};
use grade::EnvPolicy;
use report::{CONTROL, EvalReport, Outcome, TrialResult, WITH_SKILL};

const SYSTEM_BASE: &str = "You are an autonomous coding agent working inside a repository. \
Complete the user's task by reading and editing files and running commands with the provided tools. \
Verify your work by running the project's own tests/build. When the task is complete and verified, \
stop by replying with a short summary and no further tool calls. Be efficient.";

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
    /// 每个 (任务×组) 的重复次数
    #[arg(long, default_value_t = 1)]
    trials: usize,
    /// 后端：`mock`（免费）| `anthropic`（需 ANTHROPIC_API_KEY）| backends.json 里的任意命名后端（DeepSeek/本地等）
    #[arg(long, default_value = "mock")]
    backend: String,
    /// 覆盖模型 id（不填则用后端默认：anthropic=claude-opus-4-8，命名后端=配置里的 model）
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
    /// 工作区与报告输出目录（默认 evals/.runs）
    #[arg(long)]
    runs_dir: Option<PathBuf>,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 找到 `rayman` 所在目录，供 with_skill 组注入 `run` 工具的 PATH。
/// 顺序：仓库 release → 安装位置 → 仓库 debug。优先本仓库最新构建，避免悄悄评测过期的安装版。
fn find_rayman_bin() -> Option<PathBuf> {
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
    candidates
        .into_iter()
        .find(|dir| dir.join(grade::rayman_exe()).exists())
}

/// 按 `--backend` 名字构建模型后端。mock/anthropic 内建，其余名字从配置文件查表（OpenAI 兼容）。
fn build_model(cli: &Cli) -> Result<Box<dyn Model>> {
    match cli.backend.as_str() {
        // mock：no-op agent，用来验证整套编排/评分/报告链路（两组都会 0 通过）。
        "mock" => Ok(Box::new(MockModel::new("mock(noop)", Vec::new()))),
        "anthropic" => {
            let model = cli.model.as_deref().unwrap_or(anthropic::DEFAULT_MODEL);
            Ok(Box::new(anthropic::AnthropicModel::from_env(model)?))
        }
        name => {
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
            Ok(Box::new(openai::OpenAiModel::new(
                name,
                cfg,
                cli.model.as_deref(),
            )?))
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
    rayman_bin: &'a Path,
    runs_dir: &'a Path,
    max_steps: usize,
}

/// 跑单个 trial。基础设施错误（工作区准备失败、后端故障、响应截断）收敛为 Error 结果，
/// 不打断整轮评测，让已完成的结果总能落进报告。
fn run_trial(ctx: &RunContext, task: &task::Task, condition: &str, trial: usize) -> TrialResult {
    let mut result = TrialResult {
        task: task.name.clone(),
        condition: condition.into(),
        trial,
        outcome: Outcome::Error,
        grade_exit: -1,
        steps: 0,
        tool_calls: 0,
        rayman_invocations: 0,
        finished: false,
        error: None,
    };
    let workspace = ctx
        .runs_dir
        .join(format!("{}__{}__t{}", task.name, condition, trial));
    if let Err(error) = task::setup_workspace(task, &workspace) {
        result.error = Some(format!("{error:#}"));
        return result;
    }
    let cfg = AgentConfig {
        system_base: SYSTEM_BASE.into(),
        skill_text: (condition == WITH_SKILL).then(|| ctx.skill_text.to_string()),
        task_prompt: task.prompt.clone(),
        max_steps: ctx.max_steps,
        // with_skill 组把 rayman 注入 run 工具 PATH；control 组反向从 PATH 剔除，确保调不到。
        env: if condition == WITH_SKILL {
            EnvPolicy::with_rayman(ctx.rayman_bin.to_path_buf())
        } else {
            EnvPolicy::without_rayman()
        },
    };
    let log = run_agent(ctx.model, &workspace, &cfg);
    let graded = grade::run_shell(
        &workspace,
        &task.grade_cmd,
        &EnvPolicy::default(),
        grade::GRADE_TIMEOUT,
    );
    // 评分通过就算 pass；未通过时若 agent 曾报基础设施错误（后端故障/截断），
    // 记 error 而非 fail，避免把环境问题算成模型失败。
    result.outcome = if graded.passed {
        Outcome::Pass
    } else if log.error.is_some() {
        Outcome::Error
    } else {
        Outcome::Fail
    };
    result.grade_exit = graded.exit;
    result.steps = log.steps;
    result.tool_calls = log.tool_calls;
    result.rayman_invocations = log.rayman_invocations;
    result.finished = log.finished;
    result.error = log.error;
    result
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    // 先构建后端（借用完整的 cli），再消费 cli 里的 Option 字段。
    let model = build_model(&cli)?;
    let model_label = model.label();

    let tasks_dir = cli.tasks.unwrap_or_else(|| manifest_dir().join("tasks"));
    let skill_path = cli
        .skill
        .unwrap_or_else(|| manifest_dir().parent().unwrap().join("SKILL.md"));
    let runs_dir = cli.runs_dir.unwrap_or_else(|| manifest_dir().join(".runs"));

    let skill_text = std::fs::read_to_string(&skill_path)
        .with_context(|| format!("无法读取技能文件: {}", skill_path.display()))?;
    let tasks = task::load_tasks(&tasks_dir, cli.task.as_deref())?;
    if tasks.is_empty() {
        anyhow::bail!("没有匹配的任务");
    }

    // with_skill 组的 system prompt 宣称 rayman 在 PATH 上，找不到二进制就不能开跑，
    // 否则会系统性压低处理组。
    let rayman_bin = find_rayman_bin().ok_or_else(|| {
        anyhow::anyhow!(
            "未找到 rayman 可执行文件，with_skill 组无法成立。先在仓库根运行: cargo build --release"
        )
    })?;
    eprintln!("rayman 二进制目录: {}", rayman_bin.display());

    eprintln!(
        "后端={} 任务={} 每格重复={}",
        model_label,
        tasks.len(),
        cli.trials
    );

    let ctx = RunContext {
        model: model.as_ref(),
        skill_text: &skill_text,
        rayman_bin: &rayman_bin,
        runs_dir: &runs_dir,
        max_steps: cli.max_steps,
    };

    let mut results = Vec::new();
    for task in &tasks {
        for condition in [WITH_SKILL, CONTROL] {
            for trial in 0..cli.trials {
                eprintln!("  [{}] {} trial {}", condition, task.name, trial);
                let result = run_trial(&ctx, task, condition, trial);
                if let Some(error) = &result.error {
                    eprintln!("    trial 错误: {error}");
                }
                results.push(result);
            }
        }
    }

    let report = EvalReport {
        model: model_label,
        trials_per_cell: cli.trials,
        results,
    };

    std::fs::create_dir_all(&runs_dir)?;
    let md = report.markdown();
    let summary = report.summary_json();
    std::fs::write(runs_dir.join("report.md"), &md)?;
    std::fs::write(
        runs_dir.join("report.json"),
        serde_json::to_string_pretty(&summary)?,
    )?;
    println!("{md}");
    eprintln!("报告写入 {}", runs_dir.display());
    Ok(())
}
