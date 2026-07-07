//! RaymanCodingSkill 的 agent A/B outcome eval：真实 agent 在有/无技能两组下解同一批编码任务，
//! 用隐藏的客观命令评分，输出通过率差。

mod agent;
mod anthropic;
mod grade;
mod report;
mod task;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

use agent::{AgentConfig, MockModel, Model, run_agent};
use report::{CONTROL, EvalReport, TrialResult, WITH_SKILL};

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
    /// 后端：mock（免费，验证编排）| anthropic（真实，需 ANTHROPIC_API_KEY）
    #[arg(long, value_enum, default_value_t = Backend::Mock)]
    backend: Backend,
    /// 模型 id（anthropic 后端）
    #[arg(long, default_value = anthropic::DEFAULT_MODEL)]
    model: String,
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

#[derive(Copy, Clone, ValueEnum)]
enum Backend {
    Mock,
    Anthropic,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
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

    let model: Box<dyn Model> = match cli.backend {
        // mock：no-op agent，用来验证整套编排/评分/报告链路（两组都会 0 通过）。
        Backend::Mock => Box::new(MockModel::new("mock(noop)", Vec::new())),
        Backend::Anthropic => Box::new(anthropic::AnthropicModel::from_env(&cli.model)?),
    };

    eprintln!(
        "后端={} 模型={} 任务={} 每格重复={}",
        model.label(),
        cli.model,
        tasks.len(),
        cli.trials
    );

    let mut results = Vec::new();
    for task in &tasks {
        for (condition, skill) in [(WITH_SKILL, Some(skill_text.clone())), (CONTROL, None)] {
            for trial in 0..cli.trials {
                eprintln!("  [{}] {} trial {}", condition, task.name, trial);
                let workspace = runs_dir.join(format!("{}__{}__t{}", task.name, condition, trial));
                task::setup_workspace(task, &workspace)?;
                let cfg = AgentConfig {
                    system_base: SYSTEM_BASE.into(),
                    skill_text: skill.clone(),
                    task_prompt: task.prompt.clone(),
                    max_steps: cli.max_steps,
                };
                let log = run_agent(model.as_ref(), &workspace, &cfg);
                let graded = grade::run_shell(&workspace, &task.grade_cmd);
                if let Some(error) = &log.error {
                    eprintln!("    agent 错误: {error}");
                }
                results.push(TrialResult {
                    task: task.name.clone(),
                    condition: condition.into(),
                    trial,
                    passed: graded.passed,
                    grade_exit: graded.exit,
                    steps: log.steps,
                    tool_calls: log.tool_calls,
                    rayman_invocations: log.rayman_invocations,
                    finished: log.finished,
                    error: log.error,
                });
            }
        }
    }

    let report = EvalReport {
        model: cli.model.clone(),
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
