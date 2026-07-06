use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "rayman-lean",
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
    /// 工作区上下文索引（指纹缓存，未变文件跳过重建）
    Context(ContextCmd),
    /// 最小目标契约与待完成项续接
    Goal(GoalCmd),
    /// 一次性只读就绪检查（上下文新鲜度 + 资产扫描 + 待完成）
    Check,
    /// 只读的过时资产与未完成标记扫描
    Assets,
    /// 托管临时目录
    Temp(TempCmd),
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
            "rayman-lean",
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
        let cli = Cli::try_parse_from(["rayman-lean", "check"]).unwrap();
        assert!(matches!(cli.command, Command::Check));
    }
}
