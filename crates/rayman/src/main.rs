mod cli;

use anyhow::{Result, bail};
use clap::Parser;
use serde_json::json;

use cli::{
    AutosaveAction, AutosaveCmd, CheckCmd, CheckProfile, CheckpointAction, CheckpointCmd, Cli,
    Command, ContextAction, ContextCmd, Format, GoalAction, GoalCmd, MapAction, MapCmd,
    PendingAction, PendingCmd, QualityProfile, TempAction, TempCmd,
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
                if json {
                    print(&json!({ "path": dir.display().to_string() }));
                } else {
                    println!("{}", dir.display());
                }
            }
            TempAction::Cleanup => {
                let removed = temp::cleanup(&root)?;
                if json {
                    print(&json!({ "removed": removed }));
                } else {
                    println!(
                        "{}",
                        if removed {
                            "已清理托管临时目录。"
                        } else {
                            "无托管临时目录可清理。"
                        }
                    );
                }
            }
        },

        Command::Check(cmd) => return run_check(&root, json, cmd),

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
                    "项目地图已刷新: modules={} symbols={} dependencies={} packages={} risks={}",
                    summary.modules,
                    summary.symbols,
                    summary.dependencies,
                    summary.packages,
                    summary.risks
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
        MapAction::Topology => {
            let report = map::topology_report(&project_map);
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_topology_report(&report);
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
        MapAction::Plan { paths, check } => {
            let report = map::change_plan(&project_map, &paths)?;
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_change_plan(&report);
            }
            if check && !report.ready {
                std::process::exit(1);
            }
        }
        MapAction::Quality { profile, check } => {
            let config = quality_config_for(root, profile)?;
            let report = map::quality_report_with_config(&project_map, &config);
            if json {
                print(&serde_json::to_value(&report)?);
            } else {
                print_quality_report(&report);
            }
            if check && !report.ready {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

fn quality_config_for(
    root: &std::path::Path,
    profile: QualityProfile,
) -> Result<map::QualityConfig> {
    match profile {
        QualityProfile::Standard => Ok(map::QualityConfig::standard()),
        QualityProfile::Strict => map::load_quality_config(root, "strict"),
    }
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
                    for validation in &req.validations {
                        println!("    validated: {}", validation.command);
                    }
                    for impact in &req.impacts {
                        println!(
                            "    impact: {} deps={} dependents={} candidate_tests={} recommended_checks={}",
                            impact.changed_path,
                            impact.direct_dependencies.len(),
                            impact.direct_dependents.len(),
                            impact.candidate_tests.len(),
                            impact.recommended_checks.len()
                        );
                    }
                }
            } else {
                println!("目标不存在: {id}");
            }
        }
        GoalAction::Evidence {
            id,
            req,
            message,
            changed,
            validated,
        } => {
            if message.trim().is_empty() {
                bail!("证据 `--message` 不能为空。");
            }
            if validated.iter().any(|command| command.trim().is_empty()) {
                bail!("`--validated <command>` 不能为空。");
            }
            if !changed.is_empty() && validated.is_empty() {
                bail!(
                    "`--changed` 证据必须同时提供至少一个 `--validated <command>`，避免把影响面建议误当作已验证事实。"
                );
            }
            let impacts = if changed.is_empty() {
                Vec::new()
            } else {
                let project_map = map::build_readonly(root)?;
                changed
                    .iter()
                    .map(|path| {
                        let report = map::impact_report(&project_map, path)?;
                        Ok(impact_evidence_from_report(&report))
                    })
                    .collect::<Result<Vec<_>>>()?
            };
            let impact_count = impacts.len();
            let validation_count = validated.len();
            let goal =
                store.record_evidence_with_context(&id, &req, &message, validated, impacts)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!(
                    "已记录 {req} 证据（目标 {}，impact={}，validated={}）",
                    goal.id, impact_count, validation_count
                );
            }
        }
        GoalAction::Close { id, status } => {
            let goal = store.close(&id, &status)?;
            if json {
                print(&serde_json::to_value(&goal)?);
            } else {
                println!("目标 {} 已关闭为 {}", goal.id, goal.status);
            }
        }
        GoalAction::Pending(PendingCmd { action }) => match action {
            PendingAction::Add { title, message } => {
                let item = pending.add(&title, &message)?;
                if json {
                    print(&serde_json::to_value(&item)?);
                } else {
                    println!("已记录待完成项 {}", item.id);
                }
            }
            PendingAction::List => {
                let items = pending.list()?;
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
                if json {
                    print(&json!({ "resolved": removed, "id": id }));
                } else {
                    println!(
                        "{}",
                        if removed {
                            "已解决待完成项。"
                        } else {
                            "未找到该待完成项。"
                        }
                    );
                }
            }
        },
    }
    Ok(())
}

fn impact_evidence_from_report(report: &map::ImpactReport) -> goal::ImpactEvidence {
    goal::ImpactEvidence {
        changed_path: report.changed_path.clone(),
        direct_dependencies: report
            .direct_dependencies
            .iter()
            .map(|dependency| dependency.to_path.clone())
            .collect(),
        direct_dependents: report
            .direct_dependents
            .iter()
            .map(|dependency| dependency.from_path.clone())
            .collect(),
        candidate_tests: report
            .related_tests
            .iter()
            .map(|test| test.path.clone())
            .collect(),
        recommended_checks: report.recommended_checks.clone(),
        recommendation_basis: report.recommendation_basis.clone(),
        recorded_at: rayman::fsutil::now_iso(),
    }
}

/// 一次性只读就绪检查：聚合上下文新鲜度、资产扫描、待完成项。
/// 有硬阻塞（上下文缺失/陈旧、存在待完成项）时以非零码退出，便于脚本/agent 门禁。
fn run_check(root: &std::path::Path, json: bool, cmd: CheckCmd) -> Result<()> {
    let freshness = context::freshness(root);
    let asset_report = assets::scan(root);
    let goal_store = goal::GoalStore::new(root);
    // 损坏的 pending.json 是阻塞项而非"零待办"：静默放行会让门禁失效。
    let (pending, pending_error) = match goal::PendingStore::new(root).list() {
        Ok(items) => (items, None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    };

    let context_blocked = freshness.status != "ready";
    let mut standard_blockers = Vec::new();
    let mut standard_warnings = Vec::new();
    let mut map_summary = None;
    let mut map_quality = None;

    if matches!(cmd.profile, CheckProfile::Standard | CheckProfile::Release) {
        let (goals, goal_load_issues) = goal_store.list_with_issues()?;
        for issue in goal_load_issues {
            standard_blockers.push(format!(
                "goal 文件不可读取: {} ({})",
                issue.path, issue.error
            ));
        }
        if !context_blocked {
            match map::build_readonly(root) {
                Ok(project_map) => {
                    map_summary = Some(map::summary(&project_map));
                    let quality = if cmd.profile == CheckProfile::Release {
                        let config = map::load_quality_config(root, "strict")?;
                        map::quality_report_with_config(&project_map, &config)
                    } else {
                        map::quality_report(&project_map)
                    };
                    for finding in &quality.findings {
                        if finding.severity == "error" {
                            standard_blockers.push(format!(
                                "quality {}: {} — {}",
                                finding.kind, finding.path, finding.detail
                            ));
                        }
                    }
                    map_quality = Some(quality);
                }
                Err(error) => {
                    standard_blockers.push(format!("项目地图不可用: {error}"));
                }
            }
        }
        for checked_goal in &goals {
            match checked_goal.status.as_str() {
                "success" => {}
                "active" => {
                    standard_blockers.push(format!(
                        "goal {} 仍为 active；记录证据后必须 goal close 才能作为 standard READY",
                        checked_goal.id
                    ));
                }
                "partial" | "blocked" => {
                    standard_blockers.push(format!(
                        "goal {} 状态为 {}，不能作为 standard READY",
                        checked_goal.id, checked_goal.status
                    ));
                }
                other => {
                    standard_blockers.push(format!("goal {} 状态未知: {}", checked_goal.id, other));
                }
            }
            for req in &checked_goal.requirements {
                if checked_goal.status == "active" && req.kind == "must" && req.status != "done" {
                    standard_blockers.push(format!(
                        "active goal {} 的 must 需求 {} 仍未完成",
                        checked_goal.id, req.id
                    ));
                }
                if checked_goal.status == "success" && req.kind == "must" && req.status != "done" {
                    standard_blockers.push(format!(
                        "success goal {} 的 must 需求 {} 未处于 done 状态",
                        checked_goal.id, req.id
                    ));
                }
                if matches!(checked_goal.status.as_str(), "partial" | "blocked")
                    && req.kind == "must"
                    && req.status != "done"
                {
                    standard_blockers.push(format!(
                        "goal {} 的 must 需求 {} 仍未完成",
                        checked_goal.id, req.id
                    ));
                }
                if req.status == "done"
                    && req
                        .evidence
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default()
                        .is_empty()
                {
                    standard_blockers.push(format!(
                        "goal {} 需求 {} 缺少 evidence 文本",
                        checked_goal.id, req.id
                    ));
                }
                if req.status == "done" && req.validations.is_empty() {
                    let detail = if req.impacts.is_empty() {
                        "缺少结构化 validated 命令；代码变更还应记录 changed impact 快照"
                    } else {
                        "有 impact 快照但缺少 validated 命令"
                    };
                    standard_blockers
                        .push(format!("goal {} 需求 {} {detail}", checked_goal.id, req.id));
                }
                if req.status == "done" && !req.impacts.is_empty() && !req.validations.is_empty() {
                    for gap in validation_relevance_gaps(req) {
                        standard_blockers
                            .push(format!("goal {} 需求 {} {gap}", checked_goal.id, req.id));
                    }
                }
                if req.status == "done"
                    && req.impacts.is_empty()
                    && !req.validations.is_empty()
                    && !checked_goal.loaded_from_legacy
                {
                    standard_warnings.push(format!(
                        "goal {} 需求 {} 没有 impact 快照；非代码变更可忽略",
                        checked_goal.id, req.id
                    ));
                }
            }
        }
    }

    let blocked = context_blocked
        || !pending.is_empty()
        || pending_error.is_some()
        || !standard_blockers.is_empty();

    if json {
        print(&json!({
            "ready": !blocked,
            "profile": format!("{:?}", cmd.profile).to_ascii_lowercase(),
            "context": serde_json::to_value(&freshness)?,
            "assets": {
                "obsolete": asset_report.obsolete.len(),
                "markers": asset_report.markers.len(),
            },
            "pending": pending.len(),
            "pending_error": pending_error,
            "standard": {
                "blockers": standard_blockers,
                "warnings": standard_warnings,
                "project_map": map_summary,
                "quality": map_quality,
            },
        }));
    } else {
        println!(
            "就绪检查({:?}): {}",
            cmd.profile,
            if blocked { "BLOCKED" } else { "READY" }
        );
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
        if let Some(error) = &pending_error {
            println!("    BLOCKER: pending.json 不可读取: {error}");
        }
        if matches!(cmd.profile, CheckProfile::Standard | CheckProfile::Release) {
            if let Some(summary) = &map_summary {
                println!(
                    "  项目地图: modules={} symbols={} deps={} packages={} risks={}",
                    summary.modules,
                    summary.symbols,
                    summary.dependencies,
                    summary.packages,
                    summary.risks
                );
            }
            if let Some(quality) = &map_quality {
                println!(
                    "  质量: profile={} ready={} errors={} warnings={} covered_sources={}/{}",
                    quality.profile,
                    quality.ready,
                    quality.error_count,
                    quality.warning_count,
                    quality.candidate_test_covered_source_files,
                    quality.source_files
                );
            }
            println!("  standard blockers: {}", standard_blockers.len());
            for blocker in &standard_blockers {
                println!("    BLOCKER: {blocker}");
            }
            println!("  standard warnings: {}", standard_warnings.len());
            for warning in &standard_warnings {
                println!("    warning: {warning}");
            }
        }
    }

    if blocked {
        std::process::exit(1);
    }
    Ok(())
}

fn validation_relevance_gaps(req: &goal::Requirement) -> Vec<String> {
    let mut gaps = Vec::new();
    for impact in &req.impacts {
        let Some(expectation) = validation_expectation_for_impact(impact) else {
            continue;
        };
        if !req
            .validations
            .iter()
            .any(|validation| validation_matches_expectation(&validation.command, expectation))
        {
            gaps.push(format!(
                "validation 不覆盖 {}；需要 {}",
                impact.changed_path,
                validation_expectation_label(expectation)
            ));
        }
    }
    gaps
}

#[derive(Copy, Clone)]
enum ValidationExpectation {
    RustBuildOrTest,
    CargoManifestValidation,
}

fn validation_expectation_for_impact(
    impact: &goal::ImpactEvidence,
) -> Option<ValidationExpectation> {
    let path = impact.changed_path.to_ascii_lowercase();
    if path.ends_with(".rs") {
        return Some(ValidationExpectation::RustBuildOrTest);
    }
    if path.ends_with("cargo.toml") || path.ends_with("cargo.lock") {
        return Some(ValidationExpectation::CargoManifestValidation);
    }
    None
}

fn validation_expectation_label(expectation: ValidationExpectation) -> &'static str {
    match expectation {
        ValidationExpectation::RustBuildOrTest => {
            "Rust build/test validation such as `cargo test`, `cargo clippy`, `cargo check`, or `cargo build`"
        }
        ValidationExpectation::CargoManifestValidation => {
            "Cargo manifest validation such as `cargo test`, `cargo clippy`, `cargo check`, `cargo build`, `cargo deny check`, or `cargo audit`"
        }
    }
}

fn validation_matches_expectation(command: &str, expectation: ValidationExpectation) -> bool {
    let command = command.to_ascii_lowercase();
    match expectation {
        ValidationExpectation::RustBuildOrTest => is_rust_build_or_test_command(&command),
        ValidationExpectation::CargoManifestValidation => {
            is_rust_build_or_test_command(&command) || is_dependency_audit_command(&command)
        }
    }
}

fn is_rust_build_or_test_command(command: &str) -> bool {
    command.contains("cargo test")
        || command.contains("cargo nextest")
        || command.contains("cargo clippy")
        || command.contains("cargo check")
        || command.contains("cargo build")
}

fn is_dependency_audit_command(command: &str) -> bool {
    command.contains("cargo deny") || command.contains("cargo audit")
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
        "项目地图: files={} source={} tests={} modules={} symbols={} deps={} packages={} package_deps={} entrypoints={} risks={}",
        summary.files,
        summary.source_files,
        summary.test_files,
        summary.modules,
        summary.symbols,
        summary.dependencies,
        summary.packages,
        summary.package_dependencies,
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

fn print_topology_report(report: &map::TopologyReport) {
    println!(
        "项目拓扑: packages={} package_dependencies={}",
        report.packages.len(),
        report.package_dependencies.len()
    );
    for package in &report.packages {
        println!(
            "  package {} root={} manifest={} source={} tests={}",
            package.name,
            package.root_path,
            package.manifest_path,
            package.source_files,
            package.test_files
        );
    }
    for dependency in &report.package_dependencies {
        println!(
            "  {} -> {} ({}, {}, {})",
            dependency.from_package,
            dependency.to_package,
            dependency.dependency_name,
            dependency.kind,
            dependency.evidence
        );
    }
}

fn print_impact_report(report: &map::ImpactReport) {
    println!("影响分析: {}", report.changed_path);
    if let Some(package) = &report.package {
        println!("  package: {package}");
    }
    println!("  直接依赖: {}", report.direct_dependencies.len());
    for dependency in &report.direct_dependencies {
        println!("    -> {} ({})", dependency.to_path, dependency.evidence);
    }
    println!("  直接依赖方: {}", report.direct_dependents.len());
    for dependency in &report.direct_dependents {
        println!("    <- {} ({})", dependency.from_path, dependency.evidence);
    }
    println!("  package 依赖: {}", report.package_dependencies.len());
    for dependency in &report.package_dependencies {
        println!(
            "    -> {} via {} ({})",
            dependency.to_package, dependency.dependency_name, dependency.evidence
        );
    }
    println!("  package 依赖方: {}", report.package_dependents.len());
    for dependency in &report.package_dependents {
        println!(
            "    <- {} via {} ({})",
            dependency.from_package, dependency.dependency_name, dependency.evidence
        );
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

fn print_change_plan(report: &map::ChangePlan) {
    println!(
        "变更计划: {}",
        if report.ready { "READY" } else { "BLOCKED" }
    );
    println!(
        "  changed_paths={} impacted_files={} related_tests={} priority={}",
        report.changed_paths.len(),
        report.impacted_files.len(),
        report.related_tests.len(),
        report.review_priority
    );
    if !report.blockers.is_empty() {
        println!("  blockers:");
        for blocker in &report.blockers {
            println!("    BLOCKER: {blocker}");
        }
    }
    if !report.warnings.is_empty() {
        println!("  warnings:");
        for warning in &report.warnings {
            println!("    warning: {warning}");
        }
    }
    println!("  文件分组:");
    for file in &report.impacted_files {
        println!("    [{}] {} — {}", file.role, file.path, file.reason);
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
    if !report.risks.is_empty() {
        println!("  风险:");
        for risk in &report.risks {
            println!(
                "    [{}] {} {} — {}",
                risk.severity, risk.kind, risk.path, risk.detail
            );
        }
    }
    println!("  建议依据: {}", report.recommendation_basis);
}

fn print_quality_report(report: &map::QualityReport) {
    println!(
        "项目质量({}): {}",
        report.profile,
        if report.ready { "READY" } else { "BLOCKED" }
    );
    println!(
        "  source={} tests={} candidate_test_covered_sources={} public_api_without_test_evidence={}",
        report.source_files,
        report.test_files,
        report.candidate_test_covered_source_files,
        report.public_api_files_without_test_evidence
    );
    println!(
        "  findings: errors={} warnings={} info={}",
        report.error_count, report.warning_count, report.info_count
    );
    for finding in &report.findings {
        println!(
            "    [{}] {} {} — {}",
            finding.severity, finding.kind, finding.path, finding.detail
        );
        println!("      建议: {}", finding.recommendation);
    }
}

fn print(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("错误: 无法序列化输出: {error}"),
    }
}
