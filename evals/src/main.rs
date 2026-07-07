//! RaymanCodingSkill 的 agent A/B outcome eval：真实 agent 在有/无技能两组下解同一批编码任务，
//! 用隐藏的客观命令评分，输出通过率差。

mod agent;
mod anthropic;
mod grade;
mod openai;
mod report;
mod task;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

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

fn rayman_exe() -> &'static str {
    if cfg!(windows) {
        "rayman.exe"
    } else {
        "rayman"
    }
}

/// 找到 `rayman` 所在目录，供 with_skill 组注入 `run` 工具的 PATH。
/// 顺序：安装位置 → 仓库 release → 仓库 debug。找不到返回 None（with_skill 退化为纯文本）。
fn find_rayman_bin() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Rayman").join("bin"));
    }
    if let Some(repo) = manifest_dir().parent() {
        candidates.push(repo.join("target").join("release"));
        candidates.push(repo.join("target").join("debug"));
    }
    candidates
        .into_iter()
        .find(|dir| dir.join(rayman_exe()).exists())
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

    let rayman_bin = find_rayman_bin();
    match &rayman_bin {
        Some(dir) => eprintln!("with_skill 组 rayman 可用: {}", dir.display()),
        None => eprintln!(
            "⚠ 未找到 rayman 可执行文件；with_skill 组只能用 SKILL.md 文本，无法真正调用 CLI。\
             \n  装好后再跑更公平：cargo build --release（或安装到 %LOCALAPPDATA%\\Rayman\\bin）。"
        ),
    }

    eprintln!(
        "后端={} 任务={} 每格重复={}",
        model_label,
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
                    // 只有 with_skill 组把 rayman 注入 run 工具的 PATH。
                    rayman_bin: if condition == WITH_SKILL {
                        rayman_bin.clone()
                    } else {
                        None
                    },
                };
                let log = run_agent(model.as_ref(), &workspace, &cfg);
                let graded =
                    grade::run_shell(&workspace, &task.grade_cmd, None, grade::GRADE_TIMEOUT);
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
