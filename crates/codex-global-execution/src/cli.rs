use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rayman-global",
    version,
    about = "Rayman global fixed-capability execution"
)]
pub struct Cli {
    #[arg(long,value_enum,default_value_t=Format::Text,global=true)]
    pub format: Format,
    #[command(subcommand)]
    pub action: GlobalExecutionAction,
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    Text,
    Json,
}
#[derive(Args)]
pub struct GlobalExecutionCmd {
    #[command(subcommand)]
    pub action: GlobalExecutionAction,
}

#[derive(Subcommand)]
pub enum GlobalExecutionAction {
    /// Add the fixed SessionStart handler, preserving other global hooks
    InstallWorktreeHook {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Initialize a linked worktree under owner policy, without adopting parent history
    BootstrapWorktree {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Non-blocking SessionStart adapter; reads the event from stdin
    WorktreeHook {
        #[arg(long)]
        root: PathBuf,
    },
    /// Opt one registered repository into enrollment of linked worktrees beneath an exact root
    AuthorizeWorktrees {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        allowed_root: PathBuf,
        #[arg(long)]
        formal_state: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Request fixed-capability enrollment through the existing owner worker
    EnrollLinkedWorktree {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 120)]
        timeout_seconds: u64,
    },
    /// Recover one admitted commit; an execution binds the reviewed candidate bytes
    RecoverCommit {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        original_request_id: String,
        #[arg(long)]
        candidate_sha256: Option<String>,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
    /// Publish the compiled global entrypoint as the installation owner
    PublishSkill {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        expected_sha256: Option<String>,
    },
    /// Register fixed file destinations as the installation owner, before first use
    RegisterInstallAdapter {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        specification: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// Publish an enrolled file set; application/runtime validation remains separate
    Install {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        adapter_id: String,
        #[arg(long)]
        version: String,
        /// JSON map of registered roles to workspace-relative payload paths
        #[arg(long)]
        sources: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 180)]
        timeout_seconds: u64,
    },
    /// Withdraw one route, preserving the latest checkpoint rows and both backups
    RollbackCheckpointMigration {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        transaction_id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Owner-only publication of an exact prepared checkpoint migration
    PublishCheckpointMigration {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        transaction_id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Verify installation and report liveness separately from request progress
    Status {
        #[arg(long)]
        root: PathBuf,
    },
    /// Owner-only lossless backup and schema normalization; does not activate routing
    PrepareCheckpointMigration {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        runtime_sha256: String,
        #[arg(long)]
        yes: bool,
    },
    /// Copy an enrolled checkpoint database into a new sandbox working file
    CheckpointCopy {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        destination: PathBuf,
    },
    /// Apply the tenant-scoped delta between two sandbox SQLite working copies
    CheckpointApply {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
        #[arg(long)]
        lease_id: String,
        #[arg(long)]
        transaction_id: String,
    },
    /// Fixed application-state storage operations; write data comes from stdin
    AppState {
        #[arg(long)]
        no_wait: bool,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long, value_enum)]
        action: StateAction,
        #[arg(long)]
        object: Option<String>,
        #[arg(long)]
        lease_id: Option<String>,
        #[arg(long)]
        expected_sha256: Option<String>,
    },
    /// 由安装器初始化受保护的后台根目录
    Initialize {
        #[arg(long)]
        source_workspace: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// 由登记所有者明确启用当前项目的固定能力
    Enroll {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        git: Option<PathBuf>,
        #[arg(long, requires = "git")]
        author_name: Option<String>,
        #[arg(long, requires = "git")]
        author_email: Option<String>,
        #[arg(long)]
        formal_state: bool,
        #[arg(long)]
        preflight: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Owner-only publication of the Rayman state routing marker after restart
    ActivateRaymanState {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        yes: bool,
    },
    /// 查询原请求结果；超时后先查询，避免重复提交
    Result {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        request_id: String,
    },
    /// 预览或提交已登记项目的明确文件清单；提交需要 --yes
    Commit {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long = "path", required = true)]
        paths: Vec<String>,
        #[arg(long)]
        message: String,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
    /// 提交固定操作请求；源文件只作为协议数据读取
    Submit {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 300)]
        timeout_seconds: u64,
    },
    /// 运行已安装的固定操作后台；必须匹配安装身份及程序哈希
    Serve {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        once: bool,
    },
    /// 检查明确指定的项目；身份和可写性不代表后台已安装
    Inspect {
        #[arg(long, required = true)]
        workspace: Vec<PathBuf>,
        #[arg(long)]
        probe_writes: bool,
    },
    /// 只校验协议，不执行请求，也不把传入登记文件视为授权
    ValidateRequest {
        #[arg(long)]
        registration: PathBuf,
        #[arg(long)]
        request: PathBuf,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum StateAction {
    Acquire,
    Release,
    Renew,
    Write,
}
