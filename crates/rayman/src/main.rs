mod cli;

use anyhow::{Result, bail};
use clap::Parser;
use serde_json::json;

use cli::{
    AutosaveAction, AutosaveCmd, CheckpointAction, CheckpointCmd, Cli, Command, ContextAction,
    ContextCmd, Format, GoalAction, GoalCmd, MapAction, MapCmd, PendingAction, PendingCmd,
    TempAction, TempCmd,
};
use rayman::{assets, autosave, checkpoint, context, goal, map, temp, workspace_root};

fn main() {
    let cli = Cli::parse();
    let json = matches!(cli.format, Format::Json);
    if let Err(error) = run(cli) {
        if json {
            match serde_json::to_string_pretty(&json!({ "error": error.to_string() })) {
                Ok(text) => eprintln!("{text}"),
                Err(_) => eprintln!("{{\"error\":\"{}\"}}", error),
            }
        } else {
            eprintln!("错误: {error:#}");
        }
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
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

        Command::Map(cmd) => return run_map(&root, json, cmd),

        Command::Checkpoint(cmd) => return run_checkpoint(&root, json, cmd),

        Command::Autosave(cmd) => return run_autosave(&root, json, cmd),
    }
    Ok(())
}

fn run_map(root: &std::path::Path, json: bool, cmd: MapCmd) -> Result<()> {
    let project_map = map::build(root)?;
    match cmd.action {
        MapAction::Refresh => {
            let summary = map::summary(&project_map);
            if json {
                print(&json!({
                    "path": ".RaymanCodingSkill/context/project_map.json",
                    "summary": summary,
                }));
            } else {
                println!(
                    "项目地图已刷新: modules={} symbols={} dependencies={} risks={}",
                    summary.modules, summary.symbols, summary.dependencies, summary.risks
                );
                println!("  位置: .RaymanCodingSkill/context/project_map.json");
            }
        }
        MapAction::Summary => {
            let summary = map::summary(&project_map);
            if json {
                print(&serde_json::to_value(&summary)?);
            } else {
                print_map_summary(&summary);
            }
        }
        MapAction::File { path } => {
            let report = map::file_report(&project_map, &path)?;
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_file_report(&report);
            }
        }
        MapAction::Symbol { name } => {
            let report = map::symbol_report(&project_map, &name);
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_symbol_report(&report);
            }
        }
        MapAction::Impact { path } => {
            let report = map::impact_report(&project_map, &path)?;
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_impact_report(&report);
            }
        }
    }
    Ok(())
}

fn run_autosave(root: &std::path::Path, json: bool, cmd: AutosaveCmd) -> Result<()> {
    let outcome = match cmd.action {
        AutosaveAction::Start {
            interval,
            keep,
            no_auto_stop,
            dir,
        } => autosave::start(root, interval, keep, !no_auto_stop, dir.as_deref())?,
        AutosaveAction::Tick { workspace } => {
            let ws = autosave::resolve_workspace(workspace.as_deref())?;
            autosave::tick(&ws)?
        }
        AutosaveAction::Stop { status } => autosave::stop(root, &status)?,
        AutosaveAction::Status => autosave::status(root),
    };
    if json {
        print(&json!({
            "message": outcome.message,
            "state": outcome.state.as_ref().map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null)),
        }));
    } else {
        println!("{}", outcome.message);
    }
    Ok(())
}

fn run_checkpoint(root: &std::path::Path, json: bool, cmd: CheckpointCmd) -> Result<()> {
    let dir = cmd.dir.as_deref();
    match cmd.action {
        CheckpointAction::Save { keep } => {
            let outcome = checkpoint::save(root, dir, keep)?;
            let mb = outcome.total_bytes as f64 / 1_048_576.0;
            if json {
                print(&json!({
                    "id": outcome.id,
                    "path": outcome.path.display().to_string(),
                    "file_count": outcome.file_count,
                    "skipped_count": outcome.skipped_count,
                    "total_bytes": outcome.total_bytes,
                    "pruned": outcome.pruned,
                }));
            } else {
                println!(
                    "已保存快照 {} — {} 个文件 ({:.1} MB){}，清理旧快照 {} 个",
                    outcome.id,
                    outcome.file_count,
                    mb,
                    if outcome.skipped_count > 0 {
                        format!("，跳过 {}（锁定/无权限）", outcome.skipped_count)
                    } else {
                        String::new()
                    },
                    outcome.pruned
                );
                println!("  位置: {}", rayman::fsutil::display_path(&outcome.path));
            }
        }
        CheckpointAction::List => {
            let checkpoints = checkpoint::list(root, dir)?;
            if json {
                let items: Vec<_> = checkpoints
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "created_at": c.manifest.as_ref().map(|m| m.created_at.clone()),
                            "file_count": c.manifest.as_ref().map(|m| m.file_count),
                            "total_bytes": c.manifest.as_ref().map(|m| m.total_bytes),
                        })
                    })
                    .collect();
                print(&serde_json::to_value(&items)?);
            } else if checkpoints.is_empty() {
                println!("当前工作区暂无快照。运行 `rayman checkpoint save` 创建一个。");
            } else {
                println!("快照（旧→新）:");
                for c in &checkpoints {
                    match &c.manifest {
                        Some(m) => println!(
                            "  {}  {} 个文件  {:.1} MB",
                            c.id,
                            m.file_count,
                            m.total_bytes as f64 / 1_048_576.0
                        ),
                        None => println!("  {}  (缺 manifest)", c.id),
                    }
                }
            }
        }
        CheckpointAction::Status => {
            let latest = checkpoint::latest(root, dir)?;
            if json {
                print(&json!({
                    "has_checkpoint": latest.is_some(),
                    "latest": latest.as_ref().map(|c| json!({
                        "id": c.id,
                        "created_at": c.manifest.as_ref().map(|m| m.created_at.clone()),
                        "file_count": c.manifest.as_ref().map(|m| m.file_count),
                    })),
                }));
            } else {
                match latest {
                    Some(c) => {
                        let created = c
                            .manifest
                            .as_ref()
                            .map(|m| m.created_at.clone())
                            .unwrap_or_else(|| "?".to_string());
                        println!("最近快照: {} (保存于 {created})", c.id);
                    }
                    None => println!("当前工作区暂无快照。"),
                }
            }
        }
        CheckpointAction::Restore { id, yes } => {
            if !yes {
                bail!(
                    "恢复会用快照覆盖工作区里的同名文件。确认请加 --yes：rayman checkpoint restore --yes"
                );
            }
            let outcome = checkpoint::restore(root, dir, id.as_deref())?;
            if json {
                print(&json!({
                    "id": outcome.id,
                    "restored": outcome.restored,
                    "failed": outcome.failed,
                }));
            } else {
                println!(
                    "已从快照 {} 恢复 {} 个文件{}。",
                    outcome.id,
                    outcome.restored,
                    if outcome.failed > 0 {
                        format!("，{} 个失败", outcome.failed)
                    } else {
                        String::new()
                    }
                );
            }
        }
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

fn print_map_summary(summary: &map::MapSummary) {
    println!(
        "项目地图: files={} source={} tests={} modules={} symbols={} deps={} entrypoints={} risks={}",
        summary.files,
        summary.source_files,
        summary.test_files,
        summary.modules,
        summary.symbols,
        summary.dependencies,
        summary.entrypoints,
        summary.risks
    );
    if summary.warnings > 0 {
        println!("  风险提示: warnings={}", summary.warnings);
    }
}

fn print_file_report(report: &map::FileReport) {
    println!("文件: {}", report.path);
    if let Some(module) = &report.module {
        println!(
            "  模块: {} kind={} lines={} symbols={} public={}",
            module.name, module.kind, module.lines, module.symbols, module.public_symbols
        );
    }
    println!("  符号: {}", report.symbols.len());
    for symbol in report.symbols.iter().take(20) {
        println!(
            "    {} {}:{} [{}]",
            symbol.name, symbol.path, symbol.line, symbol.visibility
        );
    }
    println!(
        "  依赖: outgoing={} incoming={}",
        report.outgoing_dependencies.len(),
        report.incoming_dependencies.len()
    );
    println!("  候选相关测试(启发式): {}", report.related_tests.len());
    for test in &report.related_tests {
        println!(
            "    {} ({}, basis={}, confidence={})",
            test.path, test.kind, test.basis, test.confidence
        );
    }
    println!("  风险: {}", report.risks.len());
    for risk in &report.risks {
        println!("    [{}] {} — {}", risk.severity, risk.kind, risk.detail);
    }
}

fn print_symbol_report(report: &map::SymbolReport) {
    if report.matches.is_empty() {
        println!("未找到符号: {}", report.query);
        return;
    }
    println!("符号匹配: {} ({} 个)", report.query, report.matches.len());
    for symbol in &report.matches {
        println!(
            "  {} {}:{} {} [{}]",
            symbol.kind, symbol.path, symbol.line, symbol.name, symbol.visibility
        );
    }
}

fn print_impact_report(report: &map::ImpactReport) {
    println!("影响分析: {}", report.changed_path);
    println!("  直接依赖: {}", report.direct_dependencies.len());
    for dependency in &report.direct_dependencies {
        println!("    -> {} ({})", dependency.to_path, dependency.evidence);
    }
    println!("  直接依赖方: {}", report.direct_dependents.len());
    for dependency in &report.direct_dependents {
        println!("    <- {} ({})", dependency.from_path, dependency.evidence);
    }
    println!("  候选相关测试(启发式): {}", report.related_tests.len());
    for test in &report.related_tests {
        println!(
            "    {} ({}, basis={}, confidence={})",
            test.path, test.kind, test.basis, test.confidence
        );
    }
    println!("  建议验证:");
    for check in &report.recommended_checks {
        println!("    {check}");
    }
    println!("  建议依据: {}", report.recommendation_basis);
    if !report.risks.is_empty() {
        println!("  风险:");
        for risk in &report.risks {
            println!("    [{}] {} — {}", risk.severity, risk.kind, risk.detail);
        }
    }
}

fn print(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("错误: 无法序列化输出: {error}"),
    }
}
