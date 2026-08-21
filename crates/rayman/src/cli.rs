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
    /// 自动快照生命周期：开工注册 Windows 计划任务定时保存，完成/出错时存最后一次并停止
    Autosave(AutosaveCmd),
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
    /// Inspect activation together with current Git/source state.
    Inspect,
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
    /// Report activation currency and optionally apply an eligible identity-only rebind.
    EnsureCurrent {
        /// Apply the existing rebind transaction when identity drift is safely repairable.
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
}

#[derive(Args)]
pub struct AutosaveCmd {
    #[command(subcommand)]
    pub action: AutosaveAction,
}

#[derive(Subcommand)]
pub enum AutosaveAction {
    /// 开工：存一次初始快照并注册计划任务（幂等，每次开工跑一遍即可）
    Start {
        /// 自动保存间隔（分钟，默认 30）
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// 保留最近 N 个快照（默认 3）
        #[arg(long, default_value_t = rayman::checkpoint::DEFAULT_KEEP)]
        keep: usize,
        /// 关闭“完成后自动停止”（默认开启：所有目标关闭且无待完成项时自动收尾）
        #[arg(long)]
        no_auto_stop: bool,
        /// 快照根目录（默认用户级）
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// 计划任务触发时跑：存一次快照，必要时自动收尾（一般不手动调用）
    Tick {
        /// 目标工作区（计划任务会传绝对路径；缺省则从当前目录向上找）
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// 全部完成或出错时调用：存最后一次快照并注销计划任务
    Stop {
        /// 收尾状态（success / error / ...；默认 success）
        #[arg(long, default_value = "success")]
        status: String,
    },
    /// 显示自动保存状态
    Status,
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
    /// 刷新索引（只重算变更文件）
    Refresh,
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

#[derive(Args)]
pub struct MapCmd {
    #[command(subcommand)]
    pub action: MapAction,
}

#[derive(Subcommand)]
pub enum MapAction {
    /// 从当前 context 索引重建项目地图
    Refresh,
    /// 输出项目规模、模块、符号、依赖和风险摘要
    Summary,
    /// 查看单个文件的模块、符号、依赖、测试和风险
    File { path: String },
    /// 按名称查找符号
    Symbol { name: String },
    /// 查看 Cargo package / path-dependency 拓扑
    Topology,
    /// 分析某个文件变更会影响的依赖方、测试和建议验证命令
    Impact { path: String },
    /// 聚合多个变更路径，生成大型变更的文件分组、风险和验证计划
    Plan {
        /// 计划触碰的文件路径（可重复）
        paths: Vec<String>,
        /// 计划存在阻塞项时退出 1
        #[arg(long)]
        check: bool,
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
    /// 列出目标
    List,
    /// 查看单个目标
    Show { id: String },
    /// 紧凑显示需求、计划、工作包和收据计数，不输出完整 baseline
    Summary { id: String },
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
        /// Preserve an invalid archived success, or a complete current legacy success with no trusted archive path, as untrusted history.
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
                action: MapAction::Impact { path },
            }) => assert_eq!(path, "src/lib.rs"),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_map_topology() {
        let cli = Cli::try_parse_from(["rayman", "map", "topology"]).unwrap();
        match cli.command {
            Command::Map(MapCmd {
                action: MapAction::Topology,
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
                action: MapAction::Plan { paths, check },
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
            Command::Doctor(DoctorCmd { check: true })
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
