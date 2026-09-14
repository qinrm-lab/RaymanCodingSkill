use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};

use crate::i18n::Language;

#[derive(Parser)]
#[command(
    name = "rayman",
    version,
    about = "RaymanCodingSkill v2：多语言的上下文索引 / 目标 / 检查 / 恢复工作流\nMultilingual context / goal / check / recovery workflow"
)]
pub struct Cli {
    /// 界面语言：auto 按环境/系统区域选择；也可用 RAYMAN_LANG / UI language
    #[arg(long, visible_alias = "lang", value_enum, default_value_t = Language::Auto, global = true)]
    pub language: Language,

    /// 输出格式
    #[arg(long, value_enum, default_value_t = Format::Text, global = true)]
    pub format: Format,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Copy, Clone, ValueEnum)]
pub enum Format {
    Text,
    Json,
}

// clap owns construction of this short-lived parse tree. Keeping the nested
// argument structs inline preserves derive support and CLI diagnostics; boxing
// individual flag values would only trade one startup allocation for a less
// maintainable schema.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Command {
    /// 跨项目执行能力预检与固定操作协议检查
    GlobalExecution(GlobalExecutionCmd),
    /// Codex 生命周期钩子，防止 Owner Mode 过早交接
    CodexHook(CodexHookCmd),
    /// 激活、重绑、停用或检查工作区契约
    Workspace(WorkspaceCmd),
    /// 固定官方来源的版本通知与显式可信安装
    Update(UpdateCmd),
    /// 工作区上下文索引（内容 hash 证明；map/check 会拒绝未验证内容）
    Context(ContextCmd),
    /// 最小目标契约与待完成项续接
    Goal(GoalCmd),
    /// 一次性工作区就绪检查（默认 standard；release 仅代表 strict-quality，不代表已安装发布）
    Check(CheckCmd),
    /// 顺序刷新上下文并确认指定目标仍可继续实施
    Prepare(TaskWorkflowCmd),
    /// 顺序刷新上下文并执行绑定指定目标的完成门禁
    Finish(TaskFinishCmd),
    /// 项目地图与变更影响分析（依赖当前 context 索引）
    Map(MapCmd),
    /// 只读的过时资产与未完成标记扫描
    Assets,
    /// 托管临时目录
    Temp(TempCmd),
    /// 只读审计受管状态、退役状态与临时空间，不自动删除任何文件
    State(StateCmd),
    /// 工作树快照：整树本地拷贝，便于断电/切换 AI 工具后恢复
    Checkpoint(CheckpointCmd),
    #[command(name = "autosave", hide = true)]
    LegacyAutosave(LegacyCommandArgs),
    /// 检查已安装二进制、PATH 与工作区 skill 的身份契约；不证明源码新鲜度
    Doctor(DoctorCmd),
    #[command(name = "audit", hide = true)]
    LegacyAudit(LegacyCommandArgs),
    #[command(name = "workspace-skill", hide = true)]
    LegacyWorkspaceSkill(LegacyCommandArgs),
    #[command(name = "subagent", hide = true)]
    LegacySubagent(LegacyCommandArgs),
}

#[derive(Args)]
pub struct GlobalExecutionCmd {
    #[command(subcommand)]
    pub action: GlobalExecutionAction,
}

#[derive(Subcommand)]
pub enum GlobalExecutionAction {
    /// Register fixed installation targets as their desktop owner
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
    /// Publish enrolled installation files; runtime validation remains separate
    Install {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        adapter_id: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        sources: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long, default_value_t = 180)]
        timeout_seconds: u64,
    },
    /// 核对后台身份、心跳和当前请求阶段
    Status {
        #[arg(long)]
        root: PathBuf,
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

#[derive(Args)]
pub struct CodexHookCmd {
    #[command(subcommand)]
    pub action: CodexHookAction,
}

#[derive(Subcommand)]
pub enum CodexHookAction {
    /// Read one Codex Stop event from stdin and emit the hook protocol response.
    Stop,
    /// Merge the Rayman Stop guard into a user Codex hooks.json.
    Install {
        /// Override the Codex home directory (defaults to CODEX_HOME or ~/.codex).
        #[arg(long)]
        codex_home: Option<PathBuf>,
        /// Confirm the hooks.json write.
        #[arg(long)]
        yes: bool,
    },
    /// Inspect whether the managed Rayman Stop guard is installed.
    Status {
        /// Override the Codex home directory (defaults to CODEX_HOME or ~/.codex).
        #[arg(long)]
        codex_home: Option<PathBuf>,
    },
    /// Remove only the managed Rayman Stop guard and preserve every other hook.
    Uninstall {
        /// Override the Codex home directory (defaults to CODEX_HOME or ~/.codex).
        #[arg(long)]
        codex_home: Option<PathBuf>,
        /// Confirm the hooks.json write.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
pub struct LegacyCommandArgs {
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    pub args: Vec<String>,
}

#[derive(Args)]
pub struct WorkspaceCmd {
    #[command(subcommand)]
    pub action: WorkspaceAction,
}

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Inspect activation only; use `workspace inspect` for Git/source state.
    Status,
    /// Inspect activation together with current Git/source state; write probes are opt-in.
    Inspect {
        /// Create/remove transient capability probes under managed state.
        #[arg(long)]
        probe_writes: bool,
    },
    /// Write a hash-bound workspace_skill.yaml activation contract.
    Activate {
        /// Canonical RaymanCodingSkill SKILL.md; defaults to root/SKILL.md.
        #[arg(long)]
        skill_file: Option<PathBuf>,
        /// Explicitly allow the activation contract write.
        #[arg(long)]
        yes: bool,
    },
    /// Refresh an eligible stale activation against the current CLI and SKILL identity.
    Rebind {
        /// Explicitly allow the activation contract rewrite.
        #[arg(long)]
        yes: bool,
    },
    /// Report current-workspace activation currency and optionally apply only an eligible identity rebind.
    EnsureCurrent {
        /// Rebind only this workspace's activation identity; never rewrite project automation or scan siblings.
        #[arg(long)]
        yes: bool,
    },
    /// Installer-only activation finalization after every other install check succeeds.
    #[command(name = "install-bind", hide = true)]
    InstallBind {
        /// Canonical RaymanCodingSkill SKILL.md for this installer workspace.
        #[arg(long)]
        skill_file: PathBuf,
        /// Explicitly allow the activation contract transaction.
        #[arg(long)]
        yes: bool,
    },
    /// Disable the skill while retaining runtime state for audit.
    Deactivate {
        /// Explicitly allow deactivation.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Args)]
pub struct UpdateCmd {
    #[command(subcommand)]
    pub action: UpdateAction,
}

#[derive(Subcommand)]
pub enum UpdateAction {
    /// 只读取本机更新偏好与上一次成功的发现结果
    Status,
    /// 立即检查固定官方 Releases 元数据；只报告候选，不下载或安装
    Check,
    /// 配置用户级、显式 opt-in 的周期性检查
    #[command(group(
        ArgGroup::new("update_preference")
            .required(true)
            .multiple(true)
            .args(["auto_check", "no_auto_check", "auto_install", "no_auto_install"])
    ))]
    Configure {
        /// 启用调用 skill 时的周期性检查
        #[arg(long, conflicts_with = "no_auto_check")]
        auto_check: bool,
        /// 停用调用 skill 时的周期性检查
        #[arg(long, conflicts_with = "auto_check")]
        no_auto_check: bool,
        /// 显式允许已验签 bundle 通过独立 worker 自动安装
        #[arg(long, conflicts_with_all = ["no_auto_install", "no_auto_check"])]
        auto_install: bool,
        /// 关闭自动安装但保留版本通知
        #[arg(long, conflicts_with = "auto_install")]
        no_auto_install: bool,
        /// 检查间隔（小时；仅配合 --auto-check）
        #[arg(long, requires = "auto_check")]
        interval_hours: Option<u16>,
        /// 确认写入用户级更新偏好
        #[arg(long)]
        yes: bool,
    },
    /// 仅在检查已启用且间隔到期时检查并写缓存；安装另需独立同意
    Poll,
}

#[derive(Args)]
pub struct DoctorCmd {
    /// 已安装身份不一致时以非零退出；源码新鲜度须用 verify-release-contract.ps1 -RequireSourceFresh
    #[arg(long)]
    pub check: bool,
    /// Create/remove transient state and activation-metadata capability probes.
    #[arg(long)]
    pub probe_writes: bool,
}

#[derive(Args)]
pub struct CheckpointCmd {
    #[command(subcommand)]
    pub action: CheckpointAction,
    /// 快照根目录（默认用户级：Windows 为 %LOCALAPPDATA%\Rayman\checkpoints）
    #[arg(long, global = true)]
    pub dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum CheckpointAction {
    /// 保存当前工作树快照；默认不删除任何旧恢复点
    Save {
        /// 显式确认保存后只保留最近 N 个完整快照；省略则不裁剪
        #[arg(long)]
        keep: Option<usize>,
    },
    /// 显式裁剪已验证的完整快照；不会把损坏快照当作可删除候选
    Prune {
        /// 保留最近 N 个完整快照（至少 1）
        #[arg(long, default_value_t = rayman::checkpoint::DEFAULT_KEEP)]
        keep: usize,
        /// 确认删除旧恢复点
        #[arg(long)]
        yes: bool,
    },
    /// 激活无效时仍保存 recovery-only 快照；不会成为默认 latest 或完成证据
    SalvageSave,
    /// 列出已有快照
    List,
    /// 恢复快照到工作区（默认最近；会覆盖同名文件）
    Restore {
        /// 快照 id 或 "latest"（默认最近）
        id: Option<String>,
        /// 确认覆盖工作区文件（恢复是破坏性操作，必须显式确认）
        #[arg(long)]
        yes: bool,
        /// 显式允许恢复 recovery-only 快照；当前激活仍必须已经修复
        #[arg(long)]
        allow_recovery_only: bool,
    },
    /// 验证指定或最近完整快照的 manifest、路径和逐文件 hash，不写入工作区
    Verify {
        /// 快照 id 或 "latest"（默认最近完整快照）
        id: Option<String>,
    },
    /// 显示最近一次快照的状态
    Status,
}

#[derive(Args)]
pub struct ContextCmd {
    #[command(subcommand)]
    pub action: ContextAction,
}

#[derive(Subcommand)]
pub enum ContextAction {
    /// stat-only 新鲜度检查（不重建）
    Status,
    /// 刷新索引（强哈希全部当前文件，并报告内容未变/变化）
    Refresh,
    /// Budgeted file inventory from the verified context index.
    Overview {
        /// 只返回这些 context kind，逗号分隔或重复提供
        #[arg(long = "kind", value_delimiter = ',')]
        kinds: Vec<String>,
        /// 只返回该 workspace-relative 路径或其后代
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        #[command(flatten)]
        projection: ContextProjectionArgs,
    },
    /// Budgeted search over indexed paths/symbols and optional verified content.
    Query {
        term: String,
        /// 搜索 workspace-relative path；若未指定任何模式，默认启用 path 和 symbols
        #[arg(long)]
        path: bool,
        /// 搜索索引中的 symbol 名称；若未指定任何模式，默认启用 path 和 symbols
        #[arg(long)]
        symbols: bool,
        /// 搜索已验证 UTF-8 文件内容
        #[arg(long)]
        content: bool,
        /// 只返回这些 context kind，逗号分隔或重复提供
        #[arg(long = "kind", value_delimiter = ',')]
        kinds: Vec<String>,
        /// 只返回该 workspace-relative 路径或其后代
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        #[command(flatten)]
        projection: ContextProjectionArgs,
    },
    /// Return a verified UTF-8 line range from one indexed file.
    Excerpt {
        path: String,
        /// One-based inclusive start line.
        #[arg(long)]
        start: usize,
        /// One-based inclusive end line.
        #[arg(long)]
        end: usize,
        #[command(flatten)]
        projection: ContextProjectionArgs,
    },
    /// Return complete verified UTF-8 text for indexed files.
    Pack {
        paths: Vec<String>,
        #[command(flatten)]
        projection: ContextProjectionArgs,
    },
    #[command(name = "os", hide = true)]
    LegacyOs {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
    #[command(name = "task", hide = true)]
    LegacyTask {
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(Args, Debug, Clone, Default)]
pub struct ContextProjectionArgs {
    /// 只保留这些可选 attributes 字段，逗号分隔或重复提供
    #[arg(long, value_delimiter = ',')]
    pub fields: Vec<String>,
    #[command(flatten)]
    pub page: ContextPageArgs,
}

#[derive(Args)]
pub struct MapCmd {
    #[command(subcommand)]
    pub action: MapAction,
}

#[derive(Args, Debug, Clone, Default)]
pub struct ContextPageArgs {
    /// 每页最多返回的完整记录数；必须大于零
    #[arg(long)]
    pub limit: Option<usize>,
    /// 上一页返回的绑定游标
    #[arg(long)]
    pub cursor: Option<String>,
    /// canonical compact JSON 的 UTF-8 字节上限；导航模式默认 32768
    #[arg(long = "budget-bytes")]
    pub budget_bytes: Option<usize>,
}

impl ContextPageArgs {
    pub fn requested(&self) -> bool {
        self.limit.is_some() || self.cursor.is_some() || self.budget_bytes.is_some()
    }
}

#[derive(Args, Debug, Clone, Default)]
pub struct MapProjectionArgs {
    /// 只保留这些可选 attributes 字段，逗号分隔或重复提供
    #[arg(long, value_delimiter = ',')]
    pub fields: Vec<String>,
    #[command(flatten)]
    pub page: ContextPageArgs,
}

impl MapProjectionArgs {
    pub fn requested(&self) -> bool {
        !self.fields.is_empty() || self.page.requested()
    }
}

#[derive(Subcommand)]
pub enum MapAction {
    /// 从当前 context 索引重建项目地图
    Refresh,
    /// 输出项目规模、模块、符号、依赖和风险摘要
    Summary,
    /// 查看单个文件的模块、符号、依赖、测试和风险
    File {
        path: String,
        /// 依赖和被依赖关系的最大遍历深度；导航模式默认 1
        #[arg(long = "max-depth")]
        max_depth: Option<usize>,
        #[command(flatten)]
        projection: MapProjectionArgs,
    },
    /// 按名称查找符号
    Symbol {
        name: String,
        /// 按大小写敏感的完整符号名匹配，而不是默认的不区分大小写子串匹配
        #[arg(long)]
        exact: bool,
        /// 只返回该 workspace-relative 路径或其后代
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        /// 只返回归属该 package 的符号
        #[arg(long)]
        package: Option<String>,
        #[command(flatten)]
        projection: MapProjectionArgs,
    },
    /// 查看 Cargo package / path-dependency 拓扑
    Topology {
        #[command(flatten)]
        projection: MapProjectionArgs,
    },
    /// 分析某个文件变更会影响的依赖方、测试和建议验证命令
    Impact {
        path: String,
        /// 只返回该 workspace-relative 路径或其后代
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        /// 只返回归属该 package 的路径记录
        #[arg(long)]
        package: Option<String>,
        /// 依赖和被依赖关系的最大遍历深度；导航模式默认 1
        #[arg(long = "max-depth")]
        max_depth: Option<usize>,
        #[command(flatten)]
        projection: MapProjectionArgs,
    },
    /// 聚合多个变更路径，生成大型变更的文件分组、风险和验证计划
    Plan {
        /// 计划触碰的文件路径（可重复）
        paths: Vec<String>,
        /// 计划存在阻塞项时退出 1
        #[arg(long)]
        check: bool,
        /// 只返回该 workspace-relative 路径或其后代
        #[arg(long = "path-prefix")]
        path_prefix: Option<String>,
        /// 只返回归属该 package 的路径记录
        #[arg(long)]
        package: Option<String>,
        /// 依赖和被依赖关系的最大遍历深度；导航模式默认 1
        #[arg(long = "max-depth")]
        max_depth: Option<usize>,
        #[command(flatten)]
        projection: MapProjectionArgs,
    },
    /// 汇总项目可维护性质量信号；--check 会在 error 级问题上非零退出
    Quality {
        /// 质量策略：standard 低误报；strict 会读取可选质量策略配置
        #[arg(long, value_enum, default_value_t = QualityProfile::Standard)]
        profile: QualityProfile,
        /// error 级质量问题存在时退出 1；warning 只报告不阻断
        #[arg(long)]
        check: bool,
    },
}

#[derive(Args)]
pub struct CheckCmd {
    /// 检查强度：默认 standard；quick 仅基础快照；release 为工作区 strict-quality，不是安装发布验证
    #[arg(long, value_enum, default_value_t = CheckProfile::Standard)]
    pub profile: CheckProfile,
    /// 将就绪结果绑定到一个精确目标 / Bind this result to one exact goal.
    #[arg(long)]
    pub goal: Option<String>,
    /// 未传 --goal 时要求恰好一个 current 目标 / Require exactly one current goal.
    #[arg(long)]
    pub require_current_goal: bool,
    /// 检查前在同一进程刷新上下文 / Refresh context immediately before checking.
    #[arg(long)]
    pub refresh_context: bool,
}

#[derive(Args)]
pub struct TaskWorkflowCmd {
    /// Exact current goal to prepare.
    #[arg(long)]
    pub goal: String,
}

#[derive(Args)]
pub struct TaskFinishCmd {
    /// Exact current goal whose completion must be proven.
    #[arg(long)]
    pub goal: String,
    /// Completion check strength.
    #[arg(long, value_enum, default_value_t = CheckProfile::Standard)]
    pub profile: CheckProfile,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum CheckProfile {
    Quick,
    Standard,
    Release,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum QualityProfile {
    Standard,
    Strict,
}

#[derive(Args)]
pub struct GoalCmd {
    #[command(subcommand)]
    pub action: GoalAction,
}

#[derive(Args)]
pub struct HandoffCmd {
    #[command(subcommand)]
    pub action: HandoffAction,
}

#[derive(Subcommand)]
pub enum HandoffAction {
    /// Start a release handoff bound to one completed implementation goal and exact Git commit.
    Start {
        #[arg(long = "from-goal")]
        from_goal: String,
        #[arg(long)]
        commit: String,
    },
}

#[derive(Subcommand)]
pub enum GoalAction {
    /// 新建目标
    Start {
        title: String,
        /// must 需求（可重复）
        #[arg(long = "must")]
        must: Vec<String>,
        /// Typed atomic must proof in KIND::TEXT form (repeatable).
        #[arg(long = "must-proof", value_name = "KIND::TEXT")]
        must_proof: Vec<String>,
        /// should 需求（可重复）
        #[arg(long = "should")]
        should: Vec<String>,
    },
    /// 列出目标；带任一过滤或分页参数时返回 budgeted context-delivery
    List {
        /// lifecycle 过滤，同字段多值为 OR，可重复
        #[arg(long)]
        lifecycle: Vec<String>,
        /// status 过滤，同字段多值为 OR，可重复
        #[arg(long)]
        status: Vec<String>,
        #[command(flatten)]
        page: ContextPageArgs,
    },
    /// 查看单个目标
    Show { id: String },
    /// 紧凑显示需求、计划、工作包和收据计数，不输出完整 baseline
    Summary { id: String },
    /// 预算化显示恢复工作所需的 Goal frontier，永不输出完整 baseline
    Brief {
        id: String,
        #[command(flatten)]
        page: ContextPageArgs,
    },
    /// Manage a commit-bound release handoff contract.
    Handoff(Box<HandoffCmd>),
    /// Persist a pre-mutation plan receipt bound to the goal baseline.
    Plan {
        id: String,
        /// Intended change paths.
        paths: Vec<String>,
        /// Accepted for symmetry with `map plan`; a blocked plan always exits nonzero.
        #[arg(long)]
        check: bool,
        /// Monotonically widen an existing plan before any new path changes.
        #[arg(long)]
        extend: bool,
    },
    /// Record a review receipt bound to the current source fingerprint.
    Review {
        id: String,
        #[arg(long)]
        reviewer: String,
        #[arg(long = "message", short = 'm')]
        message: String,
    },
    /// 管理分层 work package
    Package(Box<WorkPackageCmd>),
    /// 管理源码绑定的并发 lane 台账
    Lane(Box<LaneCmd>),
    /// 执行阶段检查并记录非权威 progress receipt
    Progress {
        id: String,
        #[arg(long)]
        package: String,
        #[arg(long = "message", short = 'm')]
        message: String,
        #[arg(long)]
        command: String,
    },
    /// 记录尚未被机器验证的进展说明并标记需求完成（evidence-only completion，不能支撑门禁主张）
    Evidence {
        id: String,
        #[arg(long)]
        req: String,
        #[arg(long = "message", short = 'm')]
        message: String,
        /// 本次证据涉及的变更文件；会记录 map impact 快照（可重复）
        #[arg(long = "changed")]
        changed: Vec<String>,
        /// 声称已运行并通过的验证命令；无 receipt，不能支撑 standard/release 主张（可重复）
        #[arg(long = "validated")]
        validated: Vec<String>,
    },
    /// 实际执行一条验证命令并把 exit code、输出摘要和工作区指纹写成 receipt
    Validate {
        id: String,
        #[arg(long)]
        req: String,
        #[arg(long = "message", short = 'm')]
        message: String,
        /// 本次验证覆盖的变更文件；会记录 map impact 快照（可重复）
        #[arg(long = "changed")]
        changed: Vec<String>,
        /// 明确声明这是非代码需求；与 --changed 互斥
        #[arg(long, conflicts_with = "changed")]
        non_code: bool,
        /// 对 goal baseline 零增量的完整工作区快照执行 authority gate；与 --changed/--non-code 互斥
        #[arg(
            long,
            conflicts_with_all = ["changed", "non_code"],
            requires = "authority"
        )]
        workspace_snapshot: bool,
        /// 作为单一程序 + argv 直接执行；拒绝 shell 控制符，非零退出不会写入 receipt
        #[arg(long)]
        command: String,
        /// Mark a recognized workspace-wide project gate as final authority; requires --repeat >= 2.
        #[arg(long)]
        authority: bool,
        /// Execute the exact command repeatedly on one unchanged workspace fingerprint.
        #[arg(long, default_value_t = 1)]
        repeat: u32,
    },
    /// 关闭目标（success 要求每个 must 需求带 `goal validate` 写入的当前 receipt；仅有证据只能关成 partial/blocked）
    Close {
        id: String,
        #[arg(long, default_value = "success")]
        status: String,
    },
    /// 将历史目标显式归档；保留 JSON，但不再参与 readiness
    Archive {
        id: String,
        #[arg(long)]
        reason: String,
        /// Migrate a pre-rollout schema-v2 success record that cannot carry the new receipt proof
        #[arg(long)]
        migrate_unreceipted: bool,
        /// Explicitly preserve a pre-policy-v2 goal whose real v1 receipts still pass v1 integrity
        #[arg(long, value_name = "POLICY", conflicts_with = "migrate_unreceipted")]
        migrate_receipt_policy: Option<String>,
        /// Preserve invalid completed history with no trusted archive path; current goals require no pending work or open lanes/packages.
        #[arg(
            long,
            conflicts_with_all = ["migrate_unreceipted", "migrate_receipt_policy"]
        )]
        quarantine_invalid_history: bool,
    },
    /// 以 archived authority 的同一 gate 在当前源码重跑，为精确 must 转移授权
    AuthorizeReplacement {
        id: String,
        /// 每个待替代的 current 非 success goal；可重复
        #[arg(long = "supersedes", required = true, num_args = 1..)]
        predecessors: Vec<String>,
        /// 同 workspace 上带 direct stable authority 的 archived success
        #[arg(long = "authority-from")]
        authority_goal: String,
        /// Re-run the exact trusted authority command on the current source.
        #[arg(long)]
        command: String,
        /// Rebind only the archived command's unique -MaintenanceOrchestrationCycle value.
        #[arg(long, value_name = "WORKSPACE_RELATIVE_CYCLE_JSON")]
        maintenance_cycle_rebind: Option<String>,
        /// Stable repetitions for the live lifecycle authority proof.
        #[arg(long, default_value_t = 2)]
        repeat: u32,
    },
    /// 标记旧目标已由另一个 current 目标取代
    Supersede {
        id: String,
        #[arg(long = "by")]
        replacement: String,
    },
    /// 不带 id 时列出 current 目标；带 id 时把该目标恢复为 current
    Current { id: Option<String> },
    /// Decide whether the agent must continue, may ask the user, waits externally, or is done.
    Frontier { id: String },
    /// 待完成项
    Pending(Box<PendingCmd>),
}

#[derive(Args)]
pub struct WorkPackageCmd {
    #[command(subcommand)]
    pub action: WorkPackageAction,
}

#[derive(Subcommand)]
pub enum WorkPackageAction {
    /// 预算化列出一个 Goal 的 package DAG
    List {
        goal: String,
        #[command(flatten)]
        page: ContextPageArgs,
    },
    /// 预算化显示一个 Goal 内的 package、需求和 progress
    Show {
        goal: String,
        id: String,
        #[command(flatten)]
        page: ContextPageArgs,
    },
    /// 新增一个 package；父节点必须已存在
    Add {
        goal: String,
        id: String,
        title: String,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long = "req")]
        requirements: Vec<String>,
        #[arg(long)]
        optional: bool,
    },
    /// 用同包且绑定当前源码快照的 progress receipt 完成 package
    Complete {
        goal: String,
        id: String,
        #[arg(long)]
        progress: String,
    },
}

#[derive(Args)]
pub struct LaneCmd {
    #[command(subcommand)]
    pub action: LaneAction,
}

#[derive(Subcommand)]
pub enum LaneAction {
    /// 在当前源码 baseline 上打开一个 lane
    Open {
        goal: String,
        id: String,
        #[arg(long)]
        mode: String,
        #[arg(long = "allow")]
        allowed_paths: Vec<String>,
    },
    /// 计算 lane 期间的源码差量并按 mode/allowlist 机械验收
    Close { goal: String, id: String },
}

#[derive(Args)]
pub struct PendingCmd {
    #[command(subcommand)]
    pub action: PendingAction,
}

// The solution package is intentionally one atomic CLI record. Splitting it
// across subcommands would permit incomplete human-boundary state; this enum
// exists only for argument parsing and is dropped immediately after dispatch.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum PendingAction {
    Add {
        title: String,
        /// Required: the store rejects an empty detail unconditionally, so a
        /// `[default: ""]` here rendered an optional flag whose advertised
        /// default invocation always failed.
        #[arg(long = "message", short = 'm')]
        message: String,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long, default_value = "agent")]
        owner: String,
        #[arg(long, default_value = "machine_actionable")]
        kind: String,
        #[arg(long = "attempt")]
        attempts: Vec<String>,
        #[arg(long = "evidence-path")]
        evidence_paths: Vec<String>,
        #[arg(long)]
        minimum_input: Option<String>,
        #[arg(long)]
        recommended: Option<String>,
        #[arg(long = "alternative")]
        alternatives: Vec<String>,
        #[arg(long)]
        risk: Option<String>,
        #[arg(long)]
        resume_command: Option<String>,
        #[arg(long)]
        auto_resume_condition: Option<String>,
        #[arg(long, default_value = "deferred")]
        consultation_timing: String,
        #[arg(long)]
        background_mechanism: Option<String>,
        #[arg(long)]
        background_authority_evidence: Option<String>,
        #[arg(long)]
        background_isolation_evidence: Option<String>,
        /// Stable semantic identity for this capability boundary. Public
        /// human/external blockers must provide it so a retry cannot mint a
        /// second copy of the same question.
        #[arg(long)]
        capability_key: Option<String>,
        /// Stable class of the authority/capability boundary (for example
        /// `execution_context` or `owner_decision`).
        #[arg(long)]
        boundary_class: Option<String>,
    },
    List,
    /// Render the exact aggregate human-boundary package for the complete
    /// current response. A host adapter may apply a stricter native boundary.
    Render {
        #[arg(long, conflicts_with = "current")]
        goal: Option<String>,
        /// Aggregate every currently askable current goal into one
        /// workspace-wide response.
        #[arg(long, conflicts_with = "goal")]
        current: bool,
    },
    /// Explicitly migrate one legacy non-agent package using its old digest
    /// and a stable goal-scoped capability identity.
    Migrate {
        id: String,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        legacy_package_sha256: String,
        #[arg(long)]
        capability_key: String,
        #[arg(long)]
        boundary_class: String,
    },
    /// Retired compatibility surface. Always fails; use `render`.
    #[command(hide = true)]
    Present {
        id: String,
        #[arg(long)]
        goal: String,
        #[arg(long)]
        package_sha256: String,
        #[arg(long)]
        channel: String,
        #[arg(long)]
        reference: Option<String>,
    },
    Resolve {
        id: String,
    },
}

#[derive(Args)]
pub struct TempCmd {
    #[command(subcommand)]
    pub action: TempAction,
}

#[derive(Args)]
pub struct StateCmd {
    #[command(subcommand)]
    pub action: StateAction,
}

#[derive(Subcommand)]
pub enum StateAction {
    /// 报告 v2 允许状态、退役目录和递归 temp 指标
    Audit {
        /// 发现退役状态或遍历错误时以非零退出
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
pub enum TempAction {
    Status,
    /// 在托管临时根下创建具名子目录
    Scratch {
        label: String,
    },
    /// 创建可探测、可归因且源码排除的 pytest 临时租约
    PytestLease {
        label: String,
    },
    /// 重新探测现有 pytest lease 的路径与读写能力
    PytestProbe {
        id: String,
    },
    /// 按 manifest 精确释放一个 pytest lease
    PytestRelease {
        id: String,
    },
    /// 清理整个托管临时根
    Cleanup,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_goal_start_with_requirements() {
        let cli = Cli::try_parse_from([
            "rayman",
            "goal",
            "start",
            "add parser",
            "--must",
            "implement",
            "--should",
            "nice errors",
        ])
        .unwrap();
        match cli.command {
            Command::Goal(GoalCmd {
                action:
                    GoalAction::Start {
                        title,
                        must,
                        must_proof,
                        should,
                    },
            }) => {
                assert_eq!(title, "add parser");
                assert_eq!(must, vec!["implement".to_string()]);
                assert!(must_proof.is_empty());
                assert_eq!(should, vec!["nice errors".to_string()]);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_check() {
        let cli = Cli::try_parse_from(["rayman", "check"]).unwrap();
        match cli.command {
            Command::Check(CheckCmd {
                profile,
                goal,
                require_current_goal,
                refresh_context,
            }) => {
                assert_eq!(profile, CheckProfile::Standard);
                assert!(goal.is_none());
                assert!(!require_current_goal);
                assert!(!refresh_context);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_global_language_with_long_name_and_alias() {
        let chinese = Cli::try_parse_from(["rayman", "--language", "zh-CN", "check"]).unwrap();
        assert_eq!(chinese.language, Language::ZhCn);

        let english = Cli::try_parse_from(["rayman", "check", "--lang", "en"]).unwrap();
        assert_eq!(english.language, Language::En);
    }

    #[test]
    fn parses_standard_check_profile() {
        let cli = Cli::try_parse_from(["rayman", "check", "--profile", "standard"]).unwrap();
        match cli.command {
            Command::Check(CheckCmd { profile, .. }) => assert_eq!(profile, CheckProfile::Standard),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_release_check_profile() {
        let cli = Cli::try_parse_from(["rayman", "check", "--profile", "release"]).unwrap();
        match cli.command {
            Command::Check(CheckCmd { profile, .. }) => assert_eq!(profile, CheckProfile::Release),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_map_impact() {
        let cli = Cli::try_parse_from(["rayman", "map", "impact", "src/lib.rs"]).unwrap();
        match cli.command {
            Command::Map(MapCmd {
                action: MapAction::Impact { path, .. },
            }) => assert_eq!(path, "src/lib.rs"),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_map_topology() {
        let cli = Cli::try_parse_from(["rayman", "map", "topology"]).unwrap();
        match cli.command {
            Command::Map(MapCmd {
                action: MapAction::Topology { .. },
            }) => {}
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_map_plan_check() {
        let cli = Cli::try_parse_from([
            "rayman",
            "map",
            "plan",
            "src/lib.rs",
            "src/map.rs",
            "--check",
        ])
        .unwrap();
        match cli.command {
            Command::Map(MapCmd {
                action: MapAction::Plan { paths, check, .. },
            }) => {
                assert_eq!(
                    paths,
                    vec!["src/lib.rs".to_string(), "src/map.rs".to_string()]
                );
                assert!(check);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_map_quality_check() {
        let cli =
            Cli::try_parse_from(["rayman", "map", "quality", "--profile", "strict", "--check"])
                .unwrap();
        match cli.command {
            Command::Map(MapCmd {
                action: MapAction::Quality { profile, check },
            }) => {
                assert_eq!(profile, QualityProfile::Strict);
                assert!(check);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_doctor_check() {
        let cli = Cli::try_parse_from(["rayman", "doctor", "--check"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Doctor(DoctorCmd { check: true, .. })
        ));
    }

    #[test]
    fn parses_goal_validate() {
        let cli = Cli::try_parse_from([
            "rayman",
            "goal",
            "validate",
            "goal_x",
            "--req",
            "req_1",
            "-m",
            "tests passed",
            "--command",
            "cargo test --all",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Goal(GoalCmd {
                action: GoalAction::Validate { .. }
            })
        ));
    }
}
