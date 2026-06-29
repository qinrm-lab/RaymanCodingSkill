use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::governance::{EvalCommand, GateCommand, ReleaseCommand, SecurityCommand};

#[derive(Parser)]
#[command(name = "rayman", version, about = "RaymanCodingSkill Rust CLI")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    Generate(GenerateArgs),
    Review(ReviewArgs),
    Test(TestArgs),
    Refactor(RefactorArgs),
    Explain(ExplainArgs),
    UpdateModels(UpdateModelsArgs),
    CheckModels,
    InstallTools(InstallToolsArgs),
    ListModels,
    RouteModels(RouteModelsArgs),
    Config(ConfigArgs),
    #[command(subcommand)]
    Backup(BackupCommand),
    #[command(alias = "skill")]
    #[command(subcommand)]
    AgentSkill(AgentSkillCommand),
    #[command(subcommand)]
    WorkspaceSkill(WorkspaceSkillCommand),
    #[command(subcommand)]
    Session(SessionCommand),
    #[command(subcommand)]
    Context(ContextCommand),
    #[command(subcommand)]
    Project(ProjectCommand),
    #[command(subcommand)]
    Assets(AssetsCommand),
    Impact(ImpactArgs),
    #[command(subcommand)]
    Regression(RegressionCommand),
    #[command(subcommand)]
    Eval(EvalCommand),
    #[command(subcommand)]
    Security(SecurityCommand),
    #[command(subcommand)]
    Release(ReleaseCommand),
    #[command(subcommand)]
    Gate(GateCommand),
    #[command(subcommand)]
    Evidence(EvidenceCommand),
    #[command(name = "self")]
    #[command(subcommand)]
    SelfCommand(SelfCommand),
    #[command(subcommand)]
    Benchmark(BenchmarkCommand),
    #[command(subcommand)]
    Temp(TempCommand),
    #[command(subcommand)]
    Api(ApiCommand),
    #[command(subcommand)]
    Docs(DocsCommand),
    #[command(subcommand)]
    Instruction(InstructionCommand),
    #[command(subcommand)]
    Auxiliary(AuxiliaryCommand),
    #[command(subcommand)]
    Quality(QualityCommand),
    #[command(subcommand)]
    Goal(GoalCommand),
    #[command(subcommand)]
    Research(ResearchCommand),
    #[command(name = "subagent", alias = "host-subagent")]
    #[command(subcommand)]
    Subagent(SubagentCommand),
    #[command(subcommand)]
    CustomerDeploy(CustomerDeployCommand),
    #[command(subcommand)]
    Coverage(CoverageCommand),
    Stats,
    Audit,
}

#[derive(Args)]
pub(crate) struct ModelArgs {
    #[arg(short = 'm', long = "model-type")]
    pub(crate) model_type: Option<String>,
    #[arg(short = 'n', long = "model-name")]
    pub(crate) model_name: Option<String>,
    #[arg(long = "route-mode", value_parser = ["manual", "auto"])]
    pub(crate) route_mode: Option<String>,
    #[arg(long = "task")]
    pub(crate) task: Option<String>,
    #[arg(long = "no-fallback")]
    pub(crate) no_fallback: bool,
}

#[derive(Args)]
pub(crate) struct WorkflowArgs {
    #[arg(long)]
    pub(crate) workflow: Option<String>,
    #[arg(long = "goal-report")]
    pub(crate) goal_report: Option<PathBuf>,
    #[arg(long = "requirement")]
    pub(crate) requirement: Vec<String>,
    #[arg(long = "acceptance")]
    pub(crate) acceptance: Vec<String>,
}

#[derive(Args)]
pub(crate) struct GenerateArgs {
    pub(crate) prompt: String,
    #[arg(short = 'l', long = "language", default_value = "rust")]
    pub(crate) language: String,
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,
    #[arg(long = "no-auto-compile")]
    pub(crate) no_auto_compile: bool,
    #[command(flatten)]
    pub(crate) model: ModelArgs,
    #[command(flatten)]
    pub(crate) workflow: WorkflowArgs,
}

#[derive(Args)]
pub(crate) struct ReviewArgs {
    pub(crate) file: PathBuf,
    #[arg(short = 'l', long = "language", default_value = "rust")]
    pub(crate) language: String,
    #[arg(long = "apply-prune")]
    pub(crate) apply_prune: bool,
    #[arg(long = "backup-comment")]
    pub(crate) backup_comment: Option<String>,
    #[command(flatten)]
    pub(crate) model: ModelArgs,
    #[command(flatten)]
    pub(crate) workflow: WorkflowArgs,
}

#[derive(Args)]
pub(crate) struct TestArgs {
    pub(crate) file: PathBuf,
    #[arg(short = 'l', long = "language", default_value = "rust")]
    pub(crate) language: String,
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) model: ModelArgs,
    #[command(flatten)]
    pub(crate) workflow: WorkflowArgs,
}

#[derive(Args)]
pub(crate) struct RefactorArgs {
    pub(crate) file: PathBuf,
    pub(crate) goals: String,
    #[arg(short = 'l', long = "language", default_value = "rust")]
    pub(crate) language: String,
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,
    #[command(flatten)]
    pub(crate) model: ModelArgs,
}

#[derive(Args)]
pub(crate) struct ExplainArgs {
    pub(crate) file: PathBuf,
    #[arg(short = 'l', long = "language", default_value = "rust")]
    pub(crate) language: String,
    #[arg(long = "detail-level", default_value = "medium")]
    pub(crate) detail_level: String,
    #[command(flatten)]
    pub(crate) model: ModelArgs,
}

#[derive(Args)]
pub(crate) struct UpdateModelsArgs {
    #[arg(short = 'f', long)]
    pub(crate) force: bool,
}

#[derive(Args)]
pub(crate) struct InstallToolsArgs {
    pub(crate) tools: String,
}

#[derive(Args)]
pub(crate) struct RouteModelsArgs {
    #[command(flatten)]
    pub(crate) model: ModelArgs,
}

#[derive(Args)]
pub(crate) struct ConfigArgs {
    #[command(subcommand)]
    pub(crate) action: ConfigAction,
    #[arg(long = "config-path", default_value = "config/default_config.yaml")]
    pub(crate) config_path: PathBuf,
}

#[derive(Subcommand)]
pub(crate) enum ConfigAction {
    Show,
    Get { key: String },
    Set { key: String, value: String },
}

#[derive(Subcommand)]
pub(crate) enum BackupCommand {
    Create {
        paths: Vec<String>,
        #[arg(short = 'm', long = "message")]
        message: String,
    },
    List,
    Restore {
        backup_id: String,
    },
    Cleanup {
        #[arg(long)]
        stale: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AgentSkillCommand {
    #[command(alias = "install", alias = "update")]
    Sync {
        #[arg(long = "target")]
        target: Vec<String>,
        #[arg(long = "canonical-root")]
        canonical_root: Option<PathBuf>,
    },
    Status {
        #[arg(long = "target")]
        target: Vec<String>,
        #[arg(long = "canonical-root")]
        canonical_root: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub(crate) enum WorkspaceSkillCommand {
    Status,
    Enable {
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
    },
    Disable {
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
    },
    Stop {
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
    },
    MarkUsed {
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SessionCommand {
    Status,
    Recover,
    AddPending {
        title: String,
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        #[arg(long = "kind", value_enum, default_value = "task")]
        kind: PendingKind,
        #[arg(long = "priority", value_enum, default_value = "must")]
        priority: PendingPriority,
        #[arg(long = "source", default_value = "manual")]
        source: String,
    },
    Complete {
        item_id: String,
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
    },
    Close {
        #[arg(long = "status", value_enum, default_value = "success")]
        status: SessionStatus,
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        #[arg(long = "next-step")]
        next_step: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ContextCommand {
    Status {
        #[arg(long = "check")]
        check: bool,
    },
    List,
    Refresh,
    Os {
        #[arg(long = "write")]
        write: bool,
        #[arg(long = "check")]
        check: bool,
    },
    Task {
        query: String,
    },
    Explain,
}

#[derive(Subcommand)]
pub(crate) enum ProjectCommand {
    Detect,
    Index,
}

#[derive(Subcommand)]
pub(crate) enum AssetsCommand {
    Status,
    Scan,
    Cleanup(AssetCleanupArgs),
    Retire(AssetRetireArgs),
    Exempt(AssetExemptArgs),
}

#[derive(Args)]
pub(crate) struct AssetCleanupArgs {
    #[arg(long = "apply")]
    pub(crate) apply: bool,
}

#[derive(Args)]
pub(crate) struct AssetRetireArgs {
    #[arg(long = "path")]
    pub(crate) path: PathBuf,
    #[arg(long = "replacement")]
    pub(crate) replacement: String,
    #[arg(long = "reason")]
    pub(crate) reason: String,
    #[arg(long = "validation-command")]
    pub(crate) validation_command: String,
    #[arg(long = "apply-delete")]
    pub(crate) apply_delete: bool,
}

#[derive(Args)]
pub(crate) struct AssetExemptArgs {
    #[arg(long = "path")]
    pub(crate) path: PathBuf,
    #[arg(long = "reason")]
    pub(crate) reason: String,
    #[arg(long = "expires-at")]
    pub(crate) expires_at: String,
}

#[derive(Args)]
pub(crate) struct ImpactArgs {
    #[arg(long = "path")]
    pub(crate) path: Vec<PathBuf>,
}

#[derive(Subcommand)]
pub(crate) enum RegressionCommand {
    Plan {
        #[arg(long = "path")]
        path: Vec<PathBuf>,
    },
    Run {
        #[arg(long = "profile", value_enum, default_value = "full")]
        profile: RegressionRunProfile,
    },
    History {
        #[arg(long = "limit", default_value_t = 10)]
        limit: usize,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RegressionRunProfile {
    Auto,
    Quick,
    Full,
    SharedParallelFull,
    ParallelFull,
}

#[derive(Subcommand)]
pub(crate) enum EvidenceCommand {
    Check(EvidenceCheckArgs),
}

#[derive(Args)]
pub(crate) struct EvidenceCheckArgs {
    #[arg(long = "scope", value_enum, default_value = "workspace")]
    pub(crate) scope: EvidenceScope,
    #[arg(long = "goal-id")]
    pub(crate) goal_id: Option<String>,
    #[arg(long = "include-advisory")]
    pub(crate) include_advisory: bool,
    #[arg(long = "format", value_enum, default_value = "text")]
    pub(crate) format: EvidenceOutputFormat,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceScope {
    Workspace,
    Goal,
    Session,
    Research,
}

impl EvidenceScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Goal => "goal",
            Self::Session => "session",
            Self::Research => "research",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceOutputFormat {
    Text,
    Json,
}

impl RegressionRunProfile {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Quick => "quick",
            Self::Full => "full",
            Self::SharedParallelFull => "shared-parallel-full",
            Self::ParallelFull => "parallel-full",
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum SelfCommand {
    Status,
    Install,
}

#[derive(Subcommand)]
pub(crate) enum BenchmarkCommand {
    Run {
        #[arg(long = "smoke")]
        smoke: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum TempCommand {
    Status,
    Cleanup {
        #[arg(long = "completed")]
        completed: bool,
        #[arg(long = "stale")]
        stale: bool,
        #[arg(long = "all-failed")]
        all_failed: bool,
        #[arg(long = "cargo-targets")]
        cargo_targets: bool,
    },
    Doctor,
}

#[derive(ValueEnum, Clone)]
pub(crate) enum PendingKind {
    Task,
    Status,
    Code,
    Review,
    Workflow,
}

#[derive(ValueEnum, Clone)]
pub(crate) enum PendingPriority {
    Must,
    Should,
    Could,
}

#[derive(ValueEnum, Clone)]
pub(crate) enum SessionStatus {
    Success,
    Partial,
    Failed,
    InProgress,
    Skipped,
    Blocked,
}

#[derive(Subcommand)]
pub(crate) enum GoalCommand {
    Clarify {
        goal: String,
        #[arg(long = "requirement")]
        requirement: Vec<String>,
        #[arg(long = "acceptance")]
        acceptance: Vec<String>,
        #[arg(long = "verify")]
        verify: Vec<String>,
        #[arg(long = "assumption")]
        assumption: Vec<String>,
        #[arg(long = "format", value_enum, default_value = "text")]
        format: ClarificationOutputFormat,
    },
    Start {
        goal: String,
        #[arg(long = "workflow", default_value = "standard_development")]
        workflow: String,
        #[arg(long = "requirement")]
        requirement: Vec<String>,
        #[arg(long = "acceptance")]
        acceptance: Vec<String>,
        #[arg(long = "verify")]
        verify: Vec<String>,
        #[arg(long = "assumption")]
        assumption: Vec<String>,
    },
    Run {
        #[arg(long = "id")]
        id: Option<String>,
        #[arg(long = "validation", value_enum)]
        validation: Option<ValidationStatus>,
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        #[arg(long = "until", value_enum, default_value = "next-step")]
        until: GoalRunUntilArg,
        #[arg(long = "checkpoint-interval", default_value_t = 10)]
        checkpoint_interval: u64,
        #[arg(long = "max-repair-attempts", default_value_t = 3)]
        max_repair_attempts: u32,
    },
    Resume {
        #[arg(long = "id")]
        id: String,
        #[arg(long = "until", value_enum, default_value = "blocked")]
        until: GoalRunUntilArg,
        #[arg(long = "checkpoint-interval", default_value_t = 10)]
        checkpoint_interval: u64,
        #[arg(long = "max-repair-attempts", default_value_t = 3)]
        max_repair_attempts: u32,
    },
    Status {
        #[arg(long = "id")]
        id: Option<String>,
    },
    Close {
        #[arg(long = "id")]
        id: Option<String>,
        #[arg(long = "status", value_enum)]
        status: GoalCloseStatus,
        #[arg(short = 'm', long = "message")]
        message: Option<String>,
        #[arg(long = "next-step")]
        next_step: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum CustomerDeployCommand {
    Status,
    Set(Box<CustomerDeploySetArgs>),
    Unset { key: String },
    Validate,
}

#[derive(Subcommand)]
pub(crate) enum CoverageCommand {
    Status(CoverageStatusArgs),
}

#[derive(Args)]
pub(crate) struct CoverageStatusArgs {
    #[arg(long = "format", value_enum, default_value = "json")]
    pub(crate) format: CoverageOutputFormat,
    #[arg(long = "check")]
    pub(crate) check: bool,
    #[arg(long = "strict")]
    pub(crate) strict: bool,
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoverageOutputFormat {
    Json,
    Markdown,
}

#[derive(Args)]
pub(crate) struct CustomerDeploySetArgs {
    #[arg(long = "env")]
    pub(crate) environment: Option<String>,
    #[arg(long = "build")]
    pub(crate) build_command: Option<String>,
    #[arg(long = "test")]
    pub(crate) test_command: Vec<String>,
    #[arg(long = "deploy")]
    pub(crate) deploy_command: Option<String>,
    #[arg(long = "artifact")]
    pub(crate) artifact_path: Vec<String>,
    #[arg(long = "target")]
    pub(crate) target_alias: Option<String>,
    #[arg(long = "rollback")]
    pub(crate) rollback_command: Option<String>,
    #[arg(long = "credential-env")]
    pub(crate) credential_env: Vec<String>,
    #[arg(long = "credential-ref")]
    pub(crate) credential_ref: Vec<String>,
    #[arg(long = "notes")]
    pub(crate) notes: Option<String>,
}

#[derive(ValueEnum, Clone)]
pub(crate) enum ValidationStatus {
    Passed,
    Failed,
}

#[derive(ValueEnum, Clone)]
pub(crate) enum GoalCloseStatus {
    Success,
    Blocked,
    Failed,
    Partial,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoalRunUntilArg {
    NextStep,
    Blocked,
    Summary,
    Complete,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClarificationOutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
pub(crate) enum ApiCommand {
    Serve {
        #[arg(long = "host", default_value = "127.0.0.1")]
        host: String,
        #[arg(long = "port", default_value_t = 8000)]
        port: u16,
    },
}

#[derive(Subcommand)]
pub(crate) enum DocsCommand {
    Maintain(DocsMaintainArgs),
    #[command(hide = true)]
    Compress {
        file: PathBuf,
        #[arg(long = "budget-chars")]
        budget_chars: usize,
        #[arg(short = 'o', long = "output")]
        output: Option<PathBuf>,
    },
    CompactSkillRules {
        #[arg(long = "dry-run")]
        dry_run: bool,
        #[arg(long = "root")]
        root: Option<PathBuf>,
    },
}

#[derive(Args)]
pub(crate) struct DocsMaintainArgs {
    #[arg(long = "root")]
    pub(crate) root: Option<PathBuf>,
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,
    #[arg(long = "prompt")]
    pub(crate) prompt: Option<String>,
    #[arg(long = "prompt-file")]
    pub(crate) prompt_file: Option<PathBuf>,
    #[arg(long = "model-output")]
    pub(crate) model_output: Option<PathBuf>,
    #[arg(long = "dry-run")]
    pub(crate) dry_run: bool,
    #[arg(long = "check")]
    pub(crate) check: bool,
    #[arg(long = "apply-prune")]
    pub(crate) apply_prune: bool,
}

#[derive(Subcommand)]
pub(crate) enum InstructionCommand {
    Audit,
}

#[derive(Subcommand)]
pub(crate) enum AuxiliaryCommand {
    Advise(AuxiliaryAdviseArgs),
    Target,
    Status,
    Reconcile,
    #[command(hide = true)]
    Worker(AuxiliaryWorkerArgs),
}

#[derive(Subcommand)]
pub(crate) enum QualityCommand {
    #[command(subcommand)]
    Incident(QualityIncidentCommand),
    Patterns,
    Gate {
        #[arg(long = "goal-id")]
        goal_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum QualityIncidentCommand {
    Add(QualityIncidentAddArgs),
}

#[derive(Subcommand)]
pub(crate) enum ResearchCommand {
    Start {
        question: String,
        #[arg(long = "goal-id")]
        goal_id: Option<String>,
    },
    Run(ResearchRunArgs),
    Status {
        #[arg(long = "id")]
        id: Option<String>,
    },
    Reconcile {
        #[arg(long = "id")]
        id: Option<String>,
    },
    Report {
        #[arg(long = "id")]
        id: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum SubagentCommand {
    Plan(SubagentPlanArgs),
    #[command(name = "auto-start")]
    AutoStart(SubagentPlanArgs),
    Record(SubagentRecordArgs),
    Result(SubagentResultArgs),
    Review(SubagentReviewArgs),
    Status,
}

#[derive(Args)]
pub(crate) struct SubagentPlanArgs {
    #[arg(long = "task")]
    pub(crate) task: String,
    #[arg(long = "path")]
    pub(crate) path: Vec<PathBuf>,
    #[arg(long = "read-only")]
    pub(crate) read_only: bool,
    #[arg(long = "max-lanes", default_value_t = 4)]
    pub(crate) max_lanes: usize,
}

#[derive(Args)]
pub(crate) struct SubagentRecordArgs {
    #[arg(long = "agent-id")]
    pub(crate) agent_id: String,
    #[arg(long = "goal-id")]
    pub(crate) goal_id: Option<String>,
    #[arg(long = "dispatch-request-id")]
    pub(crate) dispatch_request_id: Option<String>,
    #[arg(long = "nickname")]
    pub(crate) nickname: Option<String>,
    #[arg(long = "task")]
    pub(crate) task: String,
    #[arg(long = "boundary")]
    pub(crate) boundary: String,
    #[arg(long = "read-only")]
    pub(crate) read_only: bool,
    #[arg(long = "write-path")]
    pub(crate) write_path: Vec<PathBuf>,
}

#[derive(Args)]
pub(crate) struct SubagentResultArgs {
    #[arg(long = "id")]
    pub(crate) id: String,
    #[arg(long = "status", value_enum, default_value = "completed")]
    pub(crate) status: SubagentResultStatus,
    #[arg(short = 'm', long = "message")]
    pub(crate) message: String,
    #[arg(long = "evidence")]
    pub(crate) evidence: Vec<String>,
    #[arg(long = "changed-path")]
    pub(crate) changed_path: Vec<PathBuf>,
}

#[derive(Args)]
pub(crate) struct SubagentReviewArgs {
    #[arg(long = "id")]
    pub(crate) id: String,
    #[arg(long = "verdict", value_enum, default_value = "accepted")]
    pub(crate) verdict: SubagentReviewVerdict,
    #[arg(short = 'm', long = "message")]
    pub(crate) message: String,
    #[arg(long = "overlap-resolution")]
    pub(crate) overlap_resolution: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentResultStatus {
    Completed,
    Failed,
    Conflict,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubagentReviewVerdict {
    Accepted,
    NotUsed,
    Conflict,
}

#[derive(Args)]
pub(crate) struct ResearchRunArgs {
    #[arg(long = "id")]
    pub(crate) id: Option<String>,
    #[arg(short = 'm', long = "model-type")]
    pub(crate) model_type: Option<String>,
    #[arg(short = 'n', long = "model-name")]
    pub(crate) model_name: Option<String>,
    #[arg(long = "route-mode", value_parser = ["manual", "auto"])]
    pub(crate) route_mode: Option<String>,
    #[arg(long = "no-fallback")]
    pub(crate) no_fallback: bool,
}

#[derive(Args)]
pub(crate) struct QualityIncidentAddArgs {
    #[arg(long = "source")]
    pub(crate) source: String,
    #[arg(long = "symptom")]
    pub(crate) symptom: String,
    #[arg(long = "root-cause")]
    pub(crate) root_cause: String,
    #[arg(long = "fix", default_value = "")]
    pub(crate) fix: String,
    #[arg(long = "generalized-behavior", default_value = "")]
    pub(crate) generalized_behavior: String,
    #[arg(long = "pattern")]
    pub(crate) pattern: Option<String>,
    #[arg(long = "tag")]
    pub(crate) tag: Vec<String>,
}

#[derive(Args)]
pub(crate) struct AuxiliaryAdviseArgs {
    #[arg(short = 't', long = "task", default_value = "planning")]
    pub(crate) task: String,
    #[arg(short = 'm', long = "message")]
    pub(crate) message: String,
}

#[derive(Args)]
pub(crate) struct AuxiliaryWorkerArgs {
    #[arg(long = "task-id")]
    pub(crate) task_id: String,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::EvalRunProfile;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_coding_workflow_commands() {
        let cli = Cli::try_parse_from([
            "rayman",
            "generate",
            "Create a parser",
            "-l",
            "rust",
            "-o",
            "parser.rs",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Generate(_)));

        let cli = Cli::try_parse_from(["rayman", "review", "parser.rs", "-l", "rust"]).unwrap();
        assert!(matches!(cli.command, Command::Review(_)));

        let cli = Cli::try_parse_from([
            "rayman",
            "test",
            "parser.rs",
            "-l",
            "rust",
            "-o",
            "tests.rs",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Test(_)));

        let cli = Cli::try_parse_from([
            "rayman",
            "refactor",
            "parser.rs",
            "reduce duplication",
            "-l",
            "rust",
            "-o",
            "parser_refactored.rs",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Refactor(_)));

        let cli = Cli::try_parse_from([
            "rayman",
            "explain",
            "parser.rs",
            "-l",
            "rust",
            "--detail-level",
            "high",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Explain(_)));
    }

    #[test]
    fn cli_parses_install_tools() {
        let cli = Cli::try_parse_from(["rayman", "install-tools", "rust,node"]).unwrap();
        assert!(matches!(cli.command, Command::InstallTools(_)));
    }

    #[test]
    fn cli_parses_workspace_skill_commands() {
        let cli = Cli::try_parse_from(["rayman", "workspace-skill", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::WorkspaceSkill(WorkspaceSkillCommand::Status)
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "workspace-skill",
            "enable",
            "-m",
            "enable this workspace",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::WorkspaceSkill(WorkspaceSkillCommand::Enable { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "workspace-skill",
            "disable",
            "-m",
            "disable this workspace",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::WorkspaceSkill(WorkspaceSkillCommand::Disable { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "workspace-skill", "stop"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::WorkspaceSkill(WorkspaceSkillCommand::Stop { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "workspace-skill",
            "mark-used",
            "-m",
            "explicit use",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::WorkspaceSkill(WorkspaceSkillCommand::MarkUsed { .. })
        ));
    }

    #[test]
    fn cli_parses_model_and_config_commands() {
        let cli = Cli::try_parse_from(["rayman", "list-models"]).unwrap();
        assert!(matches!(cli.command, Command::ListModels));

        let cli = Cli::try_parse_from(["rayman", "check-models"]).unwrap();
        assert!(matches!(cli.command, Command::CheckModels));

        let cli = Cli::try_parse_from(["rayman", "update-models", "--force"]).unwrap();
        assert!(matches!(cli.command, Command::UpdateModels(_)));

        let cli = Cli::try_parse_from(["rayman", "route-models", "--task", "code_review"]).unwrap();
        assert!(matches!(cli.command, Command::RouteModels(_)));

        let cli = Cli::try_parse_from(["rayman", "config", "show"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Config(ConfigArgs {
                action: ConfigAction::Show,
                ..
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "config", "get", "default_model.type"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Config(ConfigArgs {
                action: ConfigAction::Get { .. },
                ..
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "config", "set", "default_model.type", "openai"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Command::Config(ConfigArgs {
                action: ConfigAction::Set { .. },
                ..
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "stats"]).unwrap();
        assert!(matches!(cli.command, Command::Stats));
    }

    #[test]
    fn cli_parses_governance_commands() {
        let cli = Cli::try_parse_from(["rayman", "agent-skill", "sync"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Sync { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "agent-skill", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Status { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "agent-skill", "install"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Sync { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "agent-skill", "update"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Sync { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "skill", "install"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Sync { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "skill", "update"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Sync { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "skill", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AgentSkill(AgentSkillCommand::Status { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "backup",
            "create",
            "README.md",
            "-m",
            "before edit",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Backup(BackupCommand::Create { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "backup", "list"]).unwrap();
        assert!(matches!(cli.command, Command::Backup(BackupCommand::List)));

        let cli = Cli::try_parse_from(["rayman", "backup", "restore", "bkp_123"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Backup(BackupCommand::Restore { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "backup", "cleanup", "--stale"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Backup(BackupCommand::Cleanup { stale: true })
        ));

        let cli =
            Cli::try_parse_from(["rayman", "docs", "compact-skill-rules", "--dry-run"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Docs(DocsCommand::CompactSkillRules { dry_run: true, .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "instruction", "audit"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Instruction(InstructionCommand::Audit)
        ));

        let cli = Cli::try_parse_from(["rayman", "self", "install"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::SelfCommand(SelfCommand::Install)
        ));

        let cli = Cli::try_parse_from(["rayman", "audit"]).unwrap();
        assert!(matches!(cli.command, Command::Audit));

        let cli = Cli::try_parse_from(["rayman", "gate", "status", "--check"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Gate(GateCommand::Status { check: true, .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "evidence",
            "check",
            "--scope",
            "goal",
            "--goal-id",
            "goal_1",
            "--include-advisory",
            "--format",
            "json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Evidence(EvidenceCommand::Check(EvidenceCheckArgs {
                scope: EvidenceScope::Goal,
                format: EvidenceOutputFormat::Json,
                include_advisory: true,
                ..
            }))
        ));
    }

    #[test]
    fn cli_parses_session_status() {
        let cli = Cli::try_parse_from(["rayman", "session", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Session(SessionCommand::Status)
        ));

        let cli = Cli::try_parse_from(["rayman", "session", "recover"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Session(SessionCommand::Recover)
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "session",
            "close",
            "--status",
            "partial",
            "-m",
            "audit finding remains",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Session(SessionCommand::Close { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "session",
            "add-pending",
            "finish validation",
            "-m",
            "rerun gate",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Session(SessionCommand::AddPending { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "session",
            "complete",
            "todo_123",
            "-m",
            "validated",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Session(SessionCommand::Complete { .. })
        ));
    }

    #[test]
    fn cli_parses_context_status() {
        let cli = Cli::try_parse_from(["rayman", "context", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Context(ContextCommand::Status { check: false })
        ));

        let cli = Cli::try_parse_from(["rayman", "context", "status", "--check"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Context(ContextCommand::Status { check: true })
        ));

        let cli = Cli::try_parse_from(["rayman", "context", "refresh"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Context(ContextCommand::Refresh)
        ));

        let cli = Cli::try_parse_from(["rayman", "context", "task", "review context"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Context(ContextCommand::Task { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "context", "os", "--write", "--check"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Context(ContextCommand::Os {
                write: true,
                check: true
            })
        ));
    }

    #[test]
    fn cli_parses_project_impact_regression_self_and_benchmark() {
        let cli = Cli::try_parse_from(["rayman", "project", "detect"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Project(ProjectCommand::Detect)
        ));

        let cli = Cli::try_parse_from(["rayman", "impact", "--path", "src/lib.rs"]).unwrap();
        assert!(matches!(cli.command, Command::Impact(_)));

        let cli =
            Cli::try_parse_from(["rayman", "regression", "plan", "--path", "src/lib.rs"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Regression(RegressionCommand::Plan { .. })
        ));

        let cli =
            Cli::try_parse_from(["rayman", "regression", "run", "--profile", "parallel-full"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Command::Regression(RegressionCommand::Run {
                profile: RegressionRunProfile::ParallelFull
            })
        ));

        let cli =
            Cli::try_parse_from(["rayman", "regression", "run", "--profile", "auto"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Regression(RegressionCommand::Run {
                profile: RegressionRunProfile::Auto
            })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "regression",
            "run",
            "--profile",
            "shared-parallel-full",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Regression(RegressionCommand::Run {
                profile: RegressionRunProfile::SharedParallelFull
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "regression", "history", "--limit", "5"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Regression(RegressionCommand::History { limit: 5 })
        ));

        let cli = Cli::try_parse_from(["rayman", "eval", "run", "--profile", "full"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Eval(EvalCommand::Run {
                profile: EvalRunProfile::Full
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "security", "audit"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Security(SecurityCommand::Audit)
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "release",
            "evidence",
            "--label",
            "rc1",
            "--no-write",
            "--signed",
            "--sbom",
            "sbom.json",
            "--attestation",
            "attestation.json",
            "--require-provenance",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Release(ReleaseCommand::Evidence {
                no_write: true,
                signed: true,
                require_provenance: true,
                ..
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "self", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::SelfCommand(SelfCommand::Status)
        ));

        let cli = Cli::try_parse_from(["rayman", "benchmark", "run", "--smoke"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Benchmark(BenchmarkCommand::Run { smoke: true })
        ));

        let cli = Cli::try_parse_from(["rayman", "temp", "status"]).unwrap();
        assert!(matches!(cli.command, Command::Temp(TempCommand::Status)));

        let cli = Cli::try_parse_from([
            "rayman",
            "temp",
            "cleanup",
            "--completed",
            "--stale",
            "--all-failed",
            "--cargo-targets",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Temp(TempCommand::Cleanup {
                completed: true,
                stale: true,
                all_failed: true,
                cargo_targets: true
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "temp", "doctor"]).unwrap();
        assert!(matches!(cli.command, Command::Temp(TempCommand::Doctor)));
    }

    #[test]
    fn cli_parses_assets() {
        let cli = Cli::try_parse_from(["rayman", "assets", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Assets(AssetsCommand::Status)
        ));

        let cli = Cli::try_parse_from(["rayman", "assets", "scan"]).unwrap();
        assert!(matches!(cli.command, Command::Assets(AssetsCommand::Scan)));

        let cli = Cli::try_parse_from(["rayman", "assets", "cleanup", "--apply"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Assets(AssetsCommand::Cleanup(AssetCleanupArgs { apply: true }))
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "assets",
            "retire",
            "--path",
            "old.md",
            "--replacement",
            "new.md",
            "--reason",
            "replaced",
            "--validation-command",
            "cargo test",
            "--apply-delete",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Assets(AssetsCommand::Retire(AssetRetireArgs {
                apply_delete: true,
                ..
            }))
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "assets",
            "exempt",
            "--path",
            "old.md",
            "--reason",
            "audit",
            "--expires-at",
            "2999-01-01",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Assets(AssetsCommand::Exempt(_))
        ));
    }

    #[test]
    fn cli_parses_docs_maintain() {
        let cli = Cli::try_parse_from([
            "rayman",
            "docs",
            "maintain",
            "--root",
            ".",
            "--output",
            "docs/project-docs.html",
            "--prompt",
            "explain this project",
            "--model-output",
            "model.txt",
            "--dry-run",
            "--apply-prune",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Docs(DocsCommand::Maintain(DocsMaintainArgs {
                dry_run: true,
                apply_prune: true,
                ..
            }))
        ));
    }

    #[test]
    fn cli_parses_api_serve_defaults() {
        let cli = Cli::try_parse_from(["rayman", "api", "serve"]).unwrap();
        match cli.command {
            Command::Api(ApiCommand::Serve { host, port }) => {
                assert_eq!(host, "127.0.0.1");
                assert_eq!(port, 8000);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn cli_parses_auxiliary_advise() {
        let cli = Cli::try_parse_from([
            "rayman",
            "auxiliary",
            "advise",
            "--task",
            "planning",
            "-m",
            "check risk",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Auxiliary(AuxiliaryCommand::Advise(_))
        ));

        let cli = Cli::try_parse_from(["rayman", "auxiliary", "target"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auxiliary(AuxiliaryCommand::Target)
        ));

        let cli = Cli::try_parse_from(["rayman", "auxiliary", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auxiliary(AuxiliaryCommand::Status)
        ));

        let cli = Cli::try_parse_from(["rayman", "auxiliary", "reconcile"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auxiliary(AuxiliaryCommand::Reconcile)
        ));

        let cli =
            Cli::try_parse_from(["rayman", "auxiliary", "worker", "--task-id", "aux_123"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Auxiliary(AuxiliaryCommand::Worker(_))
        ));
    }

    #[test]
    fn cli_parses_quality_commands() {
        let cli = Cli::try_parse_from(["rayman", "quality", "patterns"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Quality(QualityCommand::Patterns)
        ));

        let cli =
            Cli::try_parse_from(["rayman", "quality", "gate", "--goal-id", "goal_abc"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Quality(QualityCommand::Gate { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "quality",
            "incident",
            "add",
            "--source",
            "codex://threads/example",
            "--symptom",
            "empty response",
            "--root-cause",
            "tool loop stopped",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Quality(QualityCommand::Incident(QualityIncidentCommand::Add(_)))
        ));
    }

    #[test]
    fn cli_parses_research_commands() {
        let cli = Cli::try_parse_from([
            "rayman",
            "research",
            "start",
            "why did validation fail?",
            "--goal-id",
            "goal_123",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Research(ResearchCommand::Start { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "research",
            "run",
            "--id",
            "research_123",
            "--model-type",
            "openai",
            "--model-name",
            "gpt-4o",
            "--route-mode",
            "auto",
            "--no-fallback",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Research(ResearchCommand::Run(_))
        ));

        let cli = Cli::try_parse_from(["rayman", "research", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Research(ResearchCommand::Status { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "research", "reconcile"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Research(ResearchCommand::Reconcile { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "research", "report"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Research(ResearchCommand::Report { .. })
        ));
    }

    #[test]
    fn cli_parses_subagent_commands() {
        let cli = Cli::try_parse_from([
            "rayman",
            "subagent",
            "plan",
            "--task",
            "全仓审计 subagent 性能",
            "--path",
            "crates/rayman-core/src/subagent.rs",
            "--read-only",
            "--max-lanes",
            "3",
        ])
        .unwrap();
        match cli.command {
            Command::Subagent(SubagentCommand::Plan(args)) => {
                assert!(args.read_only);
                assert_eq!(args.max_lanes, 3);
            }
            _ => panic!("expected subagent plan"),
        }

        let cli = Cli::try_parse_from([
            "rayman",
            "subagent",
            "auto-start",
            "--task",
            "全仓审计 subagent 性能",
            "--path",
            "crates/rayman-core/src/subagent.rs",
            "--read-only",
        ])
        .unwrap();
        match cli.command {
            Command::Subagent(SubagentCommand::AutoStart(args)) => {
                assert!(args.read_only);
            }
            _ => panic!("expected subagent auto-start"),
        }

        let cli = Cli::try_parse_from([
            "rayman",
            "host-subagent",
            "auto-start",
            "--task",
            "全仓审计 subagent 性能",
            "--read-only",
        ])
        .unwrap();
        match cli.command {
            Command::Subagent(SubagentCommand::AutoStart(args)) => {
                assert!(args.read_only);
            }
            _ => panic!("expected host-subagent auto-start"),
        }

        let cli = Cli::try_parse_from([
            "rayman",
            "subagent",
            "record",
            "--agent-id",
            "agent_123",
            "--goal-id",
            "goal_123",
            "--dispatch-request-id",
            "dispatch_123",
            "--task",
            "review API routes",
            "--boundary",
            "read-only review",
            "--read-only",
        ])
        .unwrap();
        match cli.command {
            Command::Subagent(SubagentCommand::Record(args)) => {
                assert_eq!(args.goal_id.as_deref(), Some("goal_123"));
                assert_eq!(args.dispatch_request_id.as_deref(), Some("dispatch_123"));
            }
            _ => panic!("expected subagent record"),
        }

        let cli = Cli::try_parse_from([
            "rayman",
            "host-subagent",
            "record",
            "--agent-id",
            "agent_456",
            "--task",
            "edit docs",
            "--boundary",
            "docs only",
            "--write-path",
            "docs",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Subagent(SubagentCommand::Record(_))
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "subagent",
            "result",
            "--id",
            "subagent_123",
            "--status",
            "completed",
            "-m",
            "changed docs",
            "--changed-path",
            "docs/CLI.md",
            "--evidence",
            "cargo test -p rayman-cli",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Subagent(SubagentCommand::Result(_))
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "subagent",
            "review",
            "--id",
            "subagent_123",
            "--verdict",
            "accepted",
            "-m",
            "primary reviewed",
            "--overlap-resolution",
            "no overlap",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Subagent(SubagentCommand::Review(_))
        ));

        let cli = Cli::try_parse_from(["rayman", "subagent", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Subagent(SubagentCommand::Status)
        ));
    }

    #[test]
    fn cli_parses_goal_commands() {
        let cli = Cli::try_parse_from([
            "rayman",
            "goal",
            "clarify",
            "支持导出客户订单",
            "--format",
            "text",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Goal(GoalCommand::Clarify {
                format: ClarificationOutputFormat::Text,
                ..
            })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "goal",
            "clarify",
            "export orders",
            "--format",
            "json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Goal(GoalCommand::Clarify {
                format: ClarificationOutputFormat::Json,
                ..
            })
        ));

        let cli = Cli::try_parse_from(["rayman", "goal", "start", "ship it"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Goal(GoalCommand::Start { .. })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "goal",
            "run",
            "--id",
            "goal_abc",
            "--until",
            "blocked",
            "--checkpoint-interval",
            "5",
            "--max-repair-attempts",
            "2",
            "--validation",
            "failed",
            "-m",
            "cargo test failed",
        ])
        .unwrap();
        match cli.command {
            Command::Goal(GoalCommand::Run {
                until,
                checkpoint_interval,
                max_repair_attempts,
                ..
            }) => {
                assert_eq!(until, GoalRunUntilArg::Blocked);
                assert_eq!(checkpoint_interval, 5);
                assert_eq!(max_repair_attempts, 2);
            }
            _ => panic!("unexpected command"),
        }

        let cli = Cli::try_parse_from([
            "rayman", "goal", "resume", "--id", "goal_abc", "--until", "summary",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Goal(GoalCommand::Resume {
                until: GoalRunUntilArg::Summary,
                ..
            })
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "goal",
            "close",
            "--status",
            "blocked",
            "-m",
            "missing credentials",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Goal(GoalCommand::Close { .. })
        ));
    }

    #[test]
    fn cli_parses_customer_deploy_commands() {
        let cli = Cli::try_parse_from(["rayman", "customer-deploy", "status"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::CustomerDeploy(CustomerDeployCommand::Status)
        ));

        let cli = Cli::try_parse_from([
            "rayman",
            "customer-deploy",
            "set",
            "--env",
            "prod",
            "--build",
            "cargo build --release",
            "--test",
            "cargo test",
            "--deploy",
            "scripts/deploy.ps1",
            "--credential-env",
            "PROD_TOKEN",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::CustomerDeploy(CustomerDeployCommand::Set(_))
        ));

        let cli = Cli::try_parse_from(["rayman", "customer-deploy", "unset", "build"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::CustomerDeploy(CustomerDeployCommand::Unset { .. })
        ));

        let cli = Cli::try_parse_from(["rayman", "customer-deploy", "validate"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::CustomerDeploy(CustomerDeployCommand::Validate)
        ));
    }

    #[test]
    fn cli_parses_coverage_status() {
        let cli = Cli::try_parse_from([
            "rayman",
            "coverage",
            "status",
            "--format",
            "markdown",
            "--check",
            "--strict",
            "--output",
            "docs/FEATURE_COVERAGE.md",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Coverage(CoverageCommand::Status(CoverageStatusArgs {
                format: CoverageOutputFormat::Markdown,
                check: true,
                strict: true,
                ..
            }))
        ));
    }

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
