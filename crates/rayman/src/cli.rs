use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "rayman",
    version,
    about = "RaymanCodingSkill v2（精简）：上下文索引 / 目标 / 只读检查 / 资产 / 临时目录"
)]
pub struct Cli {
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

#[derive(Subcommand)]
pub enum Command {
    /// 工作区上下文索引（内容 hash 证明；map/check 会拒绝未验证内容）
    Context(ContextCmd),
    /// 最小目标契约与待完成项续接
    Goal(GoalCmd),
    /// 一次性工作区就绪检查（默认 standard；release 仅代表 strict-quality，不代表已安装发布）
    Check(CheckCmd),
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
    /// 保存当前工作树快照，然后清理旧快照
    Save {
        /// 保留最近 N 个快照（默认 3）
        #[arg(long, default_value_t = rayman::checkpoint::DEFAULT_KEEP)]
        keep: usize,
    },
    /// 列出已有快照
    List,
    /// 恢复快照到工作区（默认最近；会覆盖同名文件）
    Restore {
        /// 快照 id 或 "latest"（默认最近）
        id: Option<String>,
        /// 确认覆盖工作区文件（恢复是破坏性操作，必须显式确认）
        #[arg(long)]
        yes: bool,
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

#[derive(Subcommand)]
pub enum GoalAction {
    /// 新建目标
    Start {
        title: String,
        /// must 需求（可重复）
        #[arg(long = "must")]
        must: Vec<String>,
        /// should 需求（可重复）
        #[arg(long = "should")]
        should: Vec<String>,
    },
    /// 列出目标
    List,
    /// 查看单个目标
    Show { id: String },
    /// 记录某需求的证据并标记完成
    Evidence {
        id: String,
        #[arg(long)]
        req: String,
        #[arg(long = "message", short = 'm')]
        message: String,
        /// 本次证据涉及的变更文件；会记录 map impact 快照（可重复）
        #[arg(long = "changed")]
        changed: Vec<String>,
        /// 已实际运行且通过的验证命令；standard profile 会检查它是否覆盖变更类型（可重复）
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
        /// 由当前 shell 执行；非零退出不会写入 receipt
        #[arg(long)]
        command: String,
    },
    /// 关闭目标（success 要求所有 must 需求带证据）
    Close {
        id: String,
        #[arg(long, default_value = "success")]
        status: String,
    },
    /// 待完成项
    Pending(PendingCmd),
}

#[derive(Args)]
pub struct PendingCmd {
    #[command(subcommand)]
    pub action: PendingAction,
}

#[derive(Subcommand)]
pub enum PendingAction {
    Add {
        title: String,
        #[arg(long = "message", short = 'm', default_value = "")]
        message: String,
    },
    List,
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
                        should,
                    },
            }) => {
                assert_eq!(title, "add parser");
                assert_eq!(must, vec!["implement".to_string()]);
                assert_eq!(should, vec!["nice errors".to_string()]);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_check() {
        let cli = Cli::try_parse_from(["rayman", "check"]).unwrap();
        match cli.command {
            Command::Check(CheckCmd { profile }) => assert_eq!(profile, CheckProfile::Standard),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_standard_check_profile() {
        let cli = Cli::try_parse_from(["rayman", "check", "--profile", "standard"]).unwrap();
        match cli.command {
            Command::Check(CheckCmd { profile }) => assert_eq!(profile, CheckProfile::Standard),
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parses_release_check_profile() {
        let cli = Cli::try_parse_from(["rayman", "check", "--profile", "release"]).unwrap();
        match cli.command {
            Command::Check(CheckCmd { profile }) => assert_eq!(profile, CheckProfile::Release),
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
