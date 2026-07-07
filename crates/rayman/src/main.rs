mod cli;

use anyhow::Result;
use clap::Parser;
use serde_json::json;

use cli::{
    Cli, Command, ContextAction, ContextCmd, Format, GoalAction, GoalCmd, PendingAction,
    PendingCmd, TempAction, TempCmd,
};
use rayman::{assets, context, goal, temp, workspace_root};

fn main() {
    if let Err(error) = run() {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let json = matches!(cli.format, Format::Json);
    let root = workspace_root()?;

    match cli.command {
        Command::Context(ContextCmd { action }) => match action {
            ContextAction::Status => {
                let report = context::freshness(&root);
                if json {
                    print(&serde_json::to_value(&report)?);
                } else {
                    println!(
                        "上下文索引: {} (changed={}, added={}, removed={})",
                        report.status,
                        report.changed.len(),
                        report.added.len(),
                        report.removed.len()
                    );
                    if report.status != "ready" {
                        println!("  运行 `rayman context refresh` 更新索引。");
                    }
                }
            }
            ContextAction::Refresh => {
                let (_, report) = context::refresh(&root)?;
                if json {
                    print(&serde_json::to_value(&report)?);
                } else {
                    println!(
                        "索引已刷新: 共 {} 个文件（复用 {}，重算 {}，移除 {}）",
                        report.total, report.reused, report.rehashed, report.removed
                    );
                }
            }
        },

        Command::Goal(GoalCmd { action }) => run_goal(&root, json, action)?,

        Command::Assets => {
            let report = assets::scan(&root);
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_assets(&report);
            }
        }

        Command::Temp(TempCmd { action }) => match action {
            TempAction::Status => {
                let status = temp::status(&root);
                if json {
                    print(&json!({
                        "root": status.root,
                        "exists": status.exists,
                        "entry_count": status.entry_count
                    }));
                } else {
                    println!(
                        "托管临时目录: {} (exists={}, entries={})",
                        status.root, status.exists, status.entry_count
                    );
                }
            }
            TempAction::Scratch { label } => {
                let dir = temp::scratch_dir(&root, &label)?;
                println!("{}", dir.display());
            }
            TempAction::Cleanup => {
                let removed = temp::cleanup(&root)?;
                println!(
                    "{}",
                    if removed {
                        "已清理托管临时目录。"
                    } else {
                        "无托管临时目录可清理。"
                    }
                );
            }
        },

        Command::Check => return run_check(&root, json),
    }
    Ok(())
}

fn run_goal(root: &std::path::Path, json: bool, action: GoalAction) -> Result<()> {
    let store = goal::GoalStore::new(root);
    let pending = goal::PendingStore::new(root);
    match action {
        GoalAction::Start {
            title,
            must,
            should,
        } => {
            let mut requirements: Vec<(String, bool)> =
                must.into_iter().map(|text| (text, true)).collect();
            requirements.extend(should.into_iter().map(|text| (text, false)));
            let goal = store.start(&title, &requirements)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!(
                    "已创建目标 {} ({} 个需求)",
                    goal.id,
                    goal.requirements.len()
                );
            }
        }
        GoalAction::List => {
            let goals = store.list()?;
            if json {
                print(&serde_json::to_value(&goals)?);
            } else if goals.is_empty() {
                println!("暂无目标。");
            } else {
                for goal in goals {
                    println!("{}  [{}]  {}", goal.id, goal.status, goal.title);
                }
            }
        }
        GoalAction::Show { id } => {
            let goal = store.get(&id)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else if let Some(goal) = goal {
                println!("{} [{}] {}", goal.id, goal.status, goal.title);
                for req in goal.requirements {
                    println!(
                        "  {} [{}/{}] {}{}",
                        req.id,
                        req.kind,
                        req.status,
                        req.text,
                        req.evidence
                            .map(|evidence| format!("  证据: {evidence}"))
                            .unwrap_or_default()
                    );
                }
            } else {
                println!("目标不存在: {id}");
            }
        }
        GoalAction::Evidence { id, req, message } => {
            let goal = store.record_evidence(&id, &req, &message)?;
            println!("已记录 {req} 证据（目标 {}）", goal.id);
        }
        GoalAction::Close { id, status } => {
            let goal = store.close(&id, &status)?;
            println!("目标 {} 已关闭为 {}", goal.id, goal.status);
        }
        GoalAction::Pending(PendingCmd { action }) => match action {
            PendingAction::Add { title, message } => {
                let item = pending.add(&title, &message)?;
                println!("已记录待完成项 {}", item.id);
            }
            PendingAction::List => {
                let items = pending.list();
                if json {
                    print(&serde_json::to_value(&items)?);
                } else if items.is_empty() {
                    println!("无待完成项。");
                } else {
                    for item in items {
                        println!("{}  {}  {}", item.id, item.title, item.detail);
                    }
                }
            }
            PendingAction::Resolve { id } => {
                let removed = pending.resolve(&id)?;
                println!(
                    "{}",
                    if removed {
                        "已解决待完成项。"
                    } else {
                        "未找到该待完成项。"
                    }
                );
            }
        },
    }
    Ok(())
}

/// 一次性只读就绪检查：聚合上下文新鲜度、资产扫描、待完成项。
/// 有硬阻塞（上下文缺失/陈旧、存在待完成项）时以非零码退出，便于脚本/agent 门禁。
fn run_check(root: &std::path::Path, json: bool) -> Result<()> {
    let freshness = context::freshness(root);
    let asset_report = assets::scan(root);
    let pending = goal::PendingStore::new(root).list();

    let context_blocked = freshness.status != "ready";
    let blocked = context_blocked || !pending.is_empty();

    if json {
        print(&json!({
            "ready": !blocked,
            "context": serde_json::to_value(&freshness)?,
            "assets": {
                "obsolete": asset_report.obsolete.len(),
                "markers": asset_report.markers.len(),
            },
            "pending": pending.len(),
        }));
    } else {
        println!("就绪检查: {}", if blocked { "BLOCKED" } else { "READY" });
        println!(
            "  上下文: {}{}",
            freshness.status,
            if context_blocked {
                " → 运行 `rayman context refresh`"
            } else {
                ""
            }
        );
        println!(
            "  资产: 过时候选 {}，未完成标记 {}（提示，不阻塞）",
            asset_report.obsolete.len(),
            asset_report.markers.len()
        );
        println!("  待完成项: {}", pending.len());
    }

    if blocked {
        std::process::exit(1);
    }
    Ok(())
}

fn print_assets(report: &assets::AssetReport) {
    if report.is_clean() {
        println!("资产扫描: 干净（无过时候选、无未完成标记）。");
        return;
    }
    if !report.obsolete.is_empty() {
        println!("过时资产候选（提示，不自动删除）:");
        for finding in &report.obsolete {
            println!("  {} — {}", finding.path, finding.reason);
        }
    }
    if !report.markers.is_empty() {
        println!("未完成标记:");
        for finding in &report.markers {
            println!(
                "  {}:{} [{}] {}",
                finding.path, finding.line, finding.marker, finding.text
            );
        }
    }
}

fn print(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("错误: 无法序列化输出: {error}"),
    }
}
